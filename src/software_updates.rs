//! Provider-owned software inventory. Discovery never installs or upgrades packages.
mod apps;
mod process;
mod providers;
mod versions;

use crate::{catalog::Application, task_terminal::Request};
use anyhow::{Result, ensure};
use process::Environment;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Formula,
    Cask,
    Npm,
    Pnpm,
    Bun,
    Uv,
    Pipx,
    Cargo,
    Rustup,
    AppStore,
    Application,
    SelfUpdate,
    System,
}
impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Formula | Self::Cask => "Homebrew",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "Bun",
            Self::Uv => "uv tool",
            Self::Pipx => "pipx",
            Self::Cargo => "Cargo",
            Self::Rustup => "rustup",
            Self::AppStore => "App Store",
            Self::Application => "应用自身",
            Self::SelfUpdate => "独立更新器",
            Self::System => "macOS",
        }
    }
    pub fn is_app(self) -> bool {
        matches!(
            self,
            Self::Cask | Self::Application | Self::AppStore | Self::System
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Available,
    Unchecked,
    Managed,
    Manual,
    CheckFailed,
}
impl UpdateState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Available => "可更新",
            Self::Unchecked => "未检查",
            Self::Managed => "未发现更新",
            Self::Manual => "更新源待确认",
            Self::CheckFailed => "检查失败",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
}
impl Invocation {
    pub(super) fn new(
        program: &std::path::Path,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
    pub fn display(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    Command(Invocation),
    OpenApp(PathBuf),
    OpenUrl(String),
    Manual,
}
#[derive(Clone, Debug)]
pub struct Software {
    pub id: String,
    pub name: String,
    pub source: Source,
    pub installed: String,
    pub latest: Option<String>,
    pub state: UpdateState,
    pub pinned: bool,
    pub description: String,
    pub homepage: String,
    pub path: Option<PathBuf>,
    pub action: Action,
    pub pin: Option<Invocation>,
    pub search: String,
}
impl Software {
    pub fn can_update(&self) -> bool {
        !self.pinned
            && matches!(self.action, Action::Command(_))
            && matches!(self.state, UpdateState::Available | UpdateState::Unchecked)
    }
    pub fn status(&self) -> &str {
        if self.pinned {
            "已固定"
        } else {
            self.latest.as_deref().unwrap_or(self.state.label())
        }
    }
    fn index(&mut self) {
        self.search =
            format!("{} {} {}", self.name, self.description, self.source.label()).to_lowercase();
    }
}

pub struct Snapshot {
    pub items: Vec<Arc<Software>>,
    pub warnings: Vec<String>,
    pub checked_online: bool,
    pub created: Instant,
    environment: Environment,
    apps: Vec<PathBuf>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            items: vec![],
            warnings: vec![],
            checked_online: false,
            created: Instant::now(),
            environment: Environment::discover(),
            apps: vec![],
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    Cached,
    Local,
    Background,
    Check,
}

/// Retained by the launcher, with no idle thread. Only one scan per service runs at a time.
#[derive(Default)]
pub struct Inventory {
    cache: Mutex<Option<Arc<Snapshot>>>,
    scan_lock: Mutex<()>,
}
impl Inventory {
    pub fn cached(&self) -> Option<Arc<Snapshot>> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn invalidate(&self) {
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    pub fn scan(
        &self,
        mode: ScanMode,
        apps: &[Application],
        cancelled: &AtomicBool,
    ) -> Arc<Snapshot> {
        self.scan_using(mode, apps, cancelled, Environment::discover())
    }
    fn scan_using(
        &self,
        mode: ScanMode,
        apps: &[Application],
        cancelled: &AtomicBool,
        environment: Environment,
    ) -> Arc<Snapshot> {
        let app_paths: Vec<_> = apps.iter().map(|app| app.path.clone()).collect();
        let _guard = loop {
            if cancelled.load(Ordering::Relaxed) {
                return self.cached().unwrap_or_default();
            }
            match self.scan_lock.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            thread::sleep(Duration::from_millis(20));
        };
        if mode == ScanMode::Cached
            && let Some(snapshot) = self.cached()
            && snapshot.created.elapsed()
                < Duration::from_secs(if snapshot.checked_online { 600 } else { 120 })
            && snapshot.apps == app_paths
            && snapshot.environment == environment
        {
            return snapshot;
        }
        let check = matches!(mode, ScanMode::Check | ScanMode::Background);
        let mut batch = providers::scan(&environment, mode, cancelled);
        if !cancelled.load(Ordering::Relaxed) {
            apps::append(apps, &mut batch);
            providers::append_self_updaters(&environment, &mut batch);
            if check {
                versions::check(&mut batch, cancelled);
            }
        }
        let mut seen = HashSet::new();
        batch.items.retain(|item| seen.insert(item.id.clone()));
        batch.items.sort_by_cached_key(|item| {
            (
                item.state != UpdateState::Available,
                item.name.to_lowercase(),
                item.id.clone(),
            )
        });
        let items = batch
            .items
            .into_iter()
            .map(|mut item| {
                item.index();
                Arc::new(item)
            })
            .collect();
        let snapshot = Arc::new(Snapshot {
            items,
            warnings: batch.warnings,
            checked_online: check,
            created: Instant::now(),
            environment,
            apps: app_paths,
        });
        if !cancelled.load(Ordering::Relaxed) {
            *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot.clone());
        }
        snapshot
    }
}

#[derive(Clone)]
pub struct Step {
    pub id: String,
    pub name: String,
    pub command: Invocation,
}
#[derive(Clone)]
pub struct Plan {
    pub steps: Vec<Step>,
    environment: Environment,
    expected: Vec<Arc<Software>>,
    pin_operation: bool,
}
impl Plan {
    /// Resolve only explicit IDs against one immutable inventory. Nothing gets implicitly selected.
    pub fn selected(snapshot: &Snapshot, selected: &HashSet<String>) -> Result<Self> {
        let mut steps = vec![];
        for item in &snapshot.items {
            if !selected.contains(&item.id) {
                continue;
            }
            ensure!(item.can_update(), "{} 当前不能自动更新", item.name);
            let Action::Command(command) = &item.action else {
                unreachable!()
            };
            steps.push(Step {
                id: item.id.clone(),
                name: item.name.clone(),
                command: command.clone(),
            });
        }
        ensure!(!steps.is_empty(), "请先选择软件");
        ensure!(steps.len() == selected.len(), "软件清单已变化，请重新选择");
        Ok(Self {
            steps,
            environment: snapshot.environment.clone(),
            expected: snapshot
                .items
                .iter()
                .filter(|i| selected.contains(&i.id))
                .cloned()
                .collect(),
            pin_operation: false,
        })
    }
    pub fn pin(snapshot: &Snapshot, item: &Software) -> Option<Self> {
        Some(Self {
            steps: vec![Step {
                id: item.id.clone(),
                name: format!(
                    "{} · {}固定版本",
                    item.name,
                    if item.pinned { "取消" } else { "" }
                ),
                command: item.pin.clone()?,
            }],
            environment: snapshot.environment.clone(),
            expected: vec![Arc::new(item.clone())],
            pin_operation: true,
        })
    }
    pub fn validate(&self, cancelled: &AtomicBool) -> Result<()> {
        providers::validate(
            &self.environment,
            &self.expected,
            self.pin_operation,
            cancelled,
        )
    }
    pub fn request(&self, index: usize) -> Request {
        let command = &self.steps[index].command;
        Request {
            program: command.program.clone(),
            args: command.args.iter().map(Into::into).collect(),
            env: self.environment.variables_for(&command.program),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Running,
    Succeeded,
    Failed(u32),
    Stopped,
    Skipped,
}
impl Outcome {
    pub fn label(&self) -> String {
        match self {
            Self::Pending => "等待".into(),
            Self::Running => "进行中".into(),
            Self::Succeeded => "完成".into(),
            Self::Failed(code) => format!("失败 ({code})"),
            Self::Stopped => "已中断".into(),
            Self::Skipped => "未执行".into(),
        }
    }
}
/// Failed or interrupted steps end the queue. Never resume a partially applied operation implicitly.
pub fn finish_step(outcomes: &mut [Outcome], index: usize, code: u32, stopped: bool) -> bool {
    outcomes[index] = if stopped {
        Outcome::Stopped
    } else if code == 0 {
        Outcome::Succeeded
    } else {
        Outcome::Failed(code)
    };
    if stopped || code != 0 {
        for outcome in &mut outcomes[index + 1..] {
            *outcome = Outcome::Skipped;
        }
        false
    } else {
        index + 1 < outcomes.len()
    }
}

#[cfg(test)]
mod tests;
