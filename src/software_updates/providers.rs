use super::{
    Action, Invocation, ScanMode, Software, Source, UpdateState,
    process::{Environment, canonical, package_name},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

#[derive(Default)]
pub(super) struct Batch {
    pub items: Vec<Software>,
    pub warnings: Vec<String>,
    pub owned: Vec<PathBuf>,
}
impl Batch {
    fn merge(&mut self, other: Self) {
        self.items.extend(other.items);
        self.warnings.extend(other.warnings);
        self.owned.extend(other.owned);
    }
    fn attempt(&mut self, name: &str, result: Result<Self>) {
        match result {
            Ok(batch) => self.merge(batch),
            Err(error) => self.warnings.push(format!("{name}：{error:#}")),
        }
    }
    pub fn owns(&self, path: &Path) -> bool {
        let path = canonical(path);
        self.owned.iter().any(|root| path.starts_with(root))
    }
}
pub(super) fn scan(env: &Environment, mode: ScanMode, cancel: &AtomicBool) -> Batch {
    let check = matches!(mode, ScanMode::Check | ScanMode::Background);
    // A fixed, bounded set of workers. Providers query once per inventory, never once per row.
    thread::scope(|scope| {
        let brew = scope.spawn(|| {
            let mut b = Batch::default();
            for p in env.find_all("brew") {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                b.attempt("Homebrew", brew(env, &p, mode == ScanMode::Check, cancel));
            }
            b
        });
        let node = scope.spawn(|| {
            let mut b = Batch::default();
            for (name, source) in [("npm", Source::Npm), ("pnpm", Source::Pnpm)] {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                for p in env.find_all(name) {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    b.attempt(name, node(env, &p, source, check, cancel));
                }
            }
            if !cancel.load(Ordering::Relaxed)
                && let Some(p) = env.find("bun")
            {
                b.attempt("Bun 全局包", bun(env, &p, cancel));
            }
            b
        });
        let tools = scope.spawn(|| {
            let mut b = Batch::default();
            if let Some(p) = env.find("uv") {
                b.attempt("uv tool", uv(env, &p, cancel));
            }
            if !cancel.load(Ordering::Relaxed)
                && let Some(p) = env.find("pipx")
            {
                b.attempt("pipx", pipx(env, &p, cancel));
            }
            if !cancel.load(Ordering::Relaxed)
                && let Some(p) = env.find("cargo")
            {
                b.attempt("Cargo", cargo(env, &p));
            }
            if !cancel.load(Ordering::Relaxed)
                && let Some(p) = env.find("rustup")
            {
                b.attempt("rustup", rustup(env, &p, check, cancel));
            }
            b
        });
        let mut batch = Batch::default();
        for worker in [brew, node, tools] {
            match worker.join() {
                Ok(b) => batch.merge(b),
                Err(_) => batch
                    .warnings
                    .push("某个来源的扫描意外中止，其余结果已保留".into()),
            }
        }
        batch
    })
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}
fn array<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}
fn item(source: Source, program: &Path, name: &str, installed: &str, args: &[&str]) -> Software {
    Software {
        id: format!("{}:{}:{name}", source.label(), canonical(program).display()),
        name: name.into(),
        source,
        installed: installed.into(),
        latest: None,
        state: UpdateState::Unchecked,
        pinned: false,
        description: String::new(),
        homepage: String::new(),
        path: None,
        action: Action::Command(Invocation::new(program, args.iter().copied())),
        pin: None,
        search: String::new(),
    }
}

fn brew(env: &Environment, program: &Path, check: bool, cancel: &AtomicBool) -> Result<Batch> {
    let mut warnings = vec![];
    if check
        && let Err(error) = env.capture(
            &Invocation::new(program, ["update"]),
            Duration::from_secs(120),
            cancel,
            &[0],
        )
    {
        warnings.push(format!("Homebrew 软件源未刷新，保留本地缓存：{error:#}"));
    }
    let output = env.run(program, &["info", "--json=v2", "--installed"], cancel)?;
    let mut batch = parse_brew(&output, program)?;
    batch.warnings.extend(warnings);
    Ok(batch)
}
pub(super) fn parse_brew(output: &str, program: &Path) -> Result<Batch> {
    let json: Value = serde_json::from_str(output).context("无效的 Homebrew JSON")?;
    ensure!(
        json["formulae"].is_array() && json["casks"].is_array(),
        "Homebrew 清单缺少软件数组"
    );
    let prefix = program
        .parent()
        .and_then(Path::parent)
        .context("Homebrew 前缀无效")?;
    let mut batch = Batch::default();
    batch.owned.extend([
        canonical(&prefix.join("Cellar")),
        canonical(&prefix.join("Caskroom")),
    ]);
    for formula in array(&json, "formulae") {
        let name = text(formula, "full_name");
        if !package_name(name) {
            continue;
        }
        let installed = if !text(formula, "linked_keg").is_empty() {
            text(formula, "linked_keg")
        } else {
            formula["installed"]
                .as_array()
                .and_then(|v| v.last())
                .map(|v| text(v, "version"))
                .unwrap_or_default()
        };
        if installed.is_empty() {
            continue;
        }
        let mut entry = item(
            Source::Formula,
            program,
            name,
            installed,
            &["upgrade", "--formula", "--", name],
        );
        entry.id = format!("brew:{}:formula:{name}", prefix.display());
        entry.name = text(formula, "name").into();
        entry.pinned = formula["pinned"].as_bool().unwrap_or(false);
        entry.state = if formula["outdated"].as_bool().unwrap_or(false) {
            UpdateState::Available
        } else {
            UpdateState::Managed
        };
        if entry.state == UpdateState::Available {
            let version = text(&formula["versions"], "stable");
            if !version.is_empty() {
                let revision = formula["revision"].as_u64().unwrap_or(0);
                entry.latest = Some(if revision > 0 {
                    format!("{version}_{revision}")
                } else {
                    version.into()
                });
            }
        }
        entry.path = Some(prefix.join("Cellar").join(&entry.name).join(installed));
        entry.homepage = text(formula, "homepage").into();
        entry.description = format!(
            "Homebrew · {} · 管理器可能同时调整依赖",
            text(formula, "desc")
        );
        entry.pin = Some(Invocation::new(
            program,
            [if entry.pinned { "unpin" } else { "pin" }, "--", name],
        ));
        batch.items.push(entry);
    }
    for cask in array(&json, "casks") {
        let name = text(cask, "full_token");
        let installed = text(cask, "installed");
        if !package_name(name) || installed.is_empty() {
            continue;
        }
        let mut entry = item(
            Source::Cask,
            program,
            name,
            installed,
            &["upgrade", "--cask", "--", name],
        );
        entry.id = format!("brew:{}:cask:{name}", prefix.display());
        entry.name = text(cask, "token").into();
        entry.homepage = text(cask, "homepage").into();
        entry.path = Some(prefix.join("Caskroom").join(&entry.name));
        for artifact in array(cask, "artifacts") {
            if let Some(app) = artifact.get("app").and_then(Value::as_array) {
                let Some(name) = app.first().and_then(Value::as_str) else {
                    continue;
                };
                let target = artifact.get("target").and_then(Value::as_str).or_else(|| {
                    app.get(1)
                        .and_then(|v| v.get("target"))
                        .and_then(Value::as_str)
                });
                let path = target
                    .map(PathBuf::from)
                    .unwrap_or_else(|| Path::new("/Applications").join(name));
                if path.is_absolute() && path.exists() {
                    batch.owned.push(canonical(&path));
                    entry.path = Some(path);
                    break;
                }
            }
        }
        let candidate = text(cask, "version");
        let automatic = cask["auto_updates"].as_bool().unwrap_or(false) || candidate == "latest";
        // Self-updating apps can be ahead of the Brew receipt. Compare the live bundle,
        // and include concrete versions Brew normally skips because of auto_updates.
        let live = entry
            .path
            .as_deref()
            .and_then(super::versions::bundle_info)
            .and_then(|info| {
                info.get("CFBundleShortVersionString")?
                    .as_string()
                    .map(str::to_owned)
            });
        if let Some(version) = &live {
            entry.installed = version.clone();
        }
        let concrete = !candidate.is_empty() && candidate != "latest";
        if automatic && concrete {
            entry.action = Action::Command(Invocation::new(
                program,
                ["upgrade", "--cask", "--greedy", "--", name],
            ));
        }
        let newer = super::versions::newer(
            candidate.split(',').next().unwrap_or_default(),
            entry.installed.split(',').next().unwrap_or_default(),
        );
        entry.state = if concrete
            && (newer
                || (live.is_none() && !automatic && cask["outdated"].as_bool().unwrap_or(false)))
        {
            UpdateState::Available
        } else if !concrete {
            UpdateState::Manual
        } else {
            UpdateState::Managed
        };
        if entry.state == UpdateState::Available {
            entry.latest = Some(candidate.into());
        }
        entry.description = if automatic {
            "已纳入自更新应用的版本检查 · 由 Homebrew 安装".into()
        } else {
            format!("Homebrew 应用 · {}", text(cask, "desc"))
        };
        if !concrete {
            entry.description = "Homebrew 未提供固定版本号 · 由应用自身检查更新".into();
            entry.action = entry
                .path
                .as_ref()
                .filter(|p| p.extension().is_some_and(|e| e == "app"))
                .map(|p| Action::OpenApp(p.clone()))
                .unwrap_or(Action::Manual);
        }
        batch.items.push(entry);
    }
    Ok(batch)
}

fn node(
    env: &Environment,
    program: &Path,
    source: Source,
    check: bool,
    cancel: &AtomicBool,
) -> Result<Batch> {
    let args: &[&str] = if source == Source::Npm {
        &["list", "--global", "--depth=0", "--json", "--long"]
    } else {
        &["list", "--global", "--depth=0", "--json"]
    };
    let output = env.capture(
        &Invocation::new(program, args.iter().copied()),
        Duration::from_secs(30),
        cancel,
        &[0, 1],
    )?;
    let mut batch = parse_node(&output, program, source)?;
    if check {
        match env
            .capture(
                &Invocation::new(program, ["outdated", "--global", "--json"]),
                Duration::from_secs(40),
                cancel,
                &[0, 1],
            )
            .and_then(|s| serde_json::from_str::<Value>(&s).context("无效的候选版本 JSON"))
        {
            Ok(outdated) => {
                if let Some(records) = outdated.as_object() {
                    for item in &mut batch.items {
                        let Some(record) = records.get(&item.name) else {
                            continue;
                        };
                        // An outdated response can contain multiple installations with the same name.
                        let candidates: Vec<_> = if let Some(records) = record.as_array() {
                            records
                                .iter()
                                .filter(|r| node_candidate_matches(r, item))
                                .collect()
                        } else {
                            std::iter::once(record)
                                .filter(|r| node_candidate_matches(r, item))
                                .collect()
                        };
                        let [record] = candidates.as_slice() else {
                            continue;
                        };
                        let wanted = text(record, "wanted");
                        if wanted.is_empty() || wanted == item.installed || !package_name(wanted) {
                            continue;
                        }
                        if matches!(item.action, Action::Command(_)) {
                            item.latest = Some(wanted.into());
                            item.state = UpdateState::Available;
                        }
                    }
                } else {
                    batch
                        .warnings
                        .push(format!("{} 候选格式不兼容，保留未知状态", source.label()));
                }
            }
            Err(error) => batch
                .warnings
                .push(format!("{} 检查失败：{error:#}", source.label())),
        }
    }
    Ok(batch)
}
pub(super) fn parse_node(output: &str, program: &Path, source: Source) -> Result<Batch> {
    let json: Value = serde_json::from_str(output).context("无效的全局包 JSON")?;
    ensure!(json.get("error").is_none(), "管理器未能完整读取全局包");
    let roots: Vec<&Value> = if let Some(array) = json.as_array() {
        array.iter().collect()
    } else {
        vec![&json]
    };
    let mut batch = Batch::default();
    for root in roots {
        let Some(deps) = root.get("dependencies").and_then(Value::as_object) else {
            continue;
        };
        for (name, metadata) in deps {
            if !package_name(name) {
                continue;
            }
            let mut entry = item(
                source,
                program,
                name,
                text(metadata, "version"),
                &["update", "--global", name],
            );
            let install_root = text(root, "path");
            entry.id = format!(
                "{}:{}:{}:{name}",
                source.label(),
                canonical(program).display(),
                install_root
            );
            let path = text(metadata, "path");
            entry.path = if !path.is_empty() {
                Some(PathBuf::from(path))
            } else if !install_root.is_empty() {
                Some(Path::new(install_root).join("node_modules").join(name))
            } else {
                None
            };
            if let Some(path) = &entry.path {
                batch.owned.push(canonical(path));
            }
            let resolved = text(metadata, "resolved");
            let version = text(metadata, "version");
            let custom = !Path::new(install_root).is_absolute()
                || !registry_spec(version)
                || metadata["link"].as_bool().unwrap_or(false)
                || metadata["missing"].as_bool().unwrap_or(false)
                || (!text(metadata, "name").is_empty() && text(metadata, "name") != name)
                || (!resolved.is_empty() && !registry_tarball(resolved))
                || ["from", "_from"].iter().any(|field| {
                    let from = text(metadata, field);
                    !from.is_empty()
                        && from != name
                        && !from
                            .strip_prefix(&format!("{name}@"))
                            .is_some_and(registry_spec)
                });
            entry.description = if source == Source::Pnpm {
                "pnpm 全局包 · 保留原范围；管理器可能调整同组依赖"
            } else {
                "npm 全局包 · 按原管理器范围更新，不强制切换 latest"
            }
            .into();
            if custom {
                entry.action = Action::Manual;
                entry.state = UpdateState::Manual;
                entry.description =
                    "链接、别名、自定义来源或安装记录不完整；请在原安装环境更新".into();
            }
            batch.items.push(entry);
        }
    }
    Ok(batch)
}
fn registry_spec(spec: &str) -> bool {
    !spec.is_empty()
        && spec
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".*^~<>=| +-".contains(c))
}
fn registry_tarball(resolved: &str) -> bool {
    (resolved.starts_with("https://") || resolved.starts_with("http://"))
        && resolved.contains("/-/")
        && resolved.ends_with(".tgz")
}
fn node_candidate_matches(record: &Value, item: &Software) -> bool {
    if text(record, "current") != item.installed {
        return false;
    }
    let location = text(record, "location");
    if location.is_empty() {
        return true;
    }
    let Some(path) = &item.path else { return false };
    let location = Path::new(location);
    if location.is_absolute() {
        canonical(path) == canonical(location)
    } else {
        path.ends_with(location)
    }
}

fn bun(env: &Environment, program: &Path, cancel: &AtomicBool) -> Result<Batch> {
    let output = env.run(program, &["pm", "ls", "--global"], cancel)?;
    let mut batch = Batch::default();
    for line in output.lines() {
        let line = line.trim_start_matches(|c: char| c.is_whitespace() || "├└─│".contains(c));
        let Some((name, version)) = line.rsplit_once('@') else {
            continue;
        };
        if !package_name(name) || !package_name(version) {
            continue;
        }
        let mut entry = item(
            Source::Bun,
            program,
            name,
            version,
            &["update", "--global", name],
        );
        entry.description = "Bun 全局包 · 更新时检查，保留原版本范围".into();
        let root = env
            .path_setting("BUN_INSTALL", env.home.join(".bun"))
            .join("install/global/node_modules")
            .join(name);
        batch.owned.push(canonical(&root));
        entry.path = Some(root);
        batch.items.push(entry);
    }
    ensure!(
        output.trim().is_empty()
            || !batch.items.is_empty()
            || output
                .lines()
                .any(|line| line.trim_end().ends_with(" node_modules (0 installed)"))
            || output.contains("(0)")
            || output.contains("no packages"),
        "Bun 返回的全局包清单格式暂不支持，本次未能读取"
    );
    Ok(batch)
}

fn uv(env: &Environment, program: &Path, cancel: &AtomicBool) -> Result<Batch> {
    let output = env.run(program, &["tool", "list", "--show-paths"], cancel)?;
    let root = env.path_setting(
        "UV_TOOL_DIR",
        env.path_setting("XDG_DATA_HOME", env.home.join(".local/share"))
            .join("uv/tools"),
    );
    let mut batch = Batch::default();
    for line in output.lines().filter(|l| !l.starts_with([' ', '\t', '-'])) {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let Some(version) = parts.next().and_then(|v| v.strip_prefix('v')) else {
            continue;
        };
        if !package_name(name) {
            continue;
        }
        let mut entry = item(
            Source::Uv,
            program,
            name,
            version,
            &["tool", "upgrade", name],
        );
        let path = line
            .split_once(" (")
            .and_then(|(_, p)| p.strip_suffix(')'))
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| root.join(name));
        entry.id = format!("uv:{}:{name}", root.display());
        entry.path = Some(path.clone());
        batch.owned.push(canonical(&path));
        entry.description = "uv tool · 运行时检查，保留原安装约束、索引和 Python 环境".into();
        batch.items.push(entry);
    }
    ensure!(
        output.trim().is_empty() || !batch.items.is_empty(),
        "无法识别 uv tool 清单"
    );
    Ok(batch)
}
fn pipx(env: &Environment, program: &Path, cancel: &AtomicBool) -> Result<Batch> {
    let help = env.run(program, &["list", "--help"], cancel)?;
    ensure!(
        help.contains("--skip-maintenance"),
        "当前版本的只读清单无法保证不维护共享库；请通过 pipx 管理"
    );
    let output = env.run(program, &["list", "--skip-maintenance", "--json"], cancel)?;
    let json: Value = serde_json::from_str(&output)?;
    let venvs = json
        .get("venvs")
        .and_then(Value::as_object)
        .context("不支持此版本的 pipx 清单格式")?;
    let mut batch = Batch::default();
    for (name, venv) in venvs {
        if !package_name(name) {
            continue;
        }
        let package = &venv["metadata"]["main_package"];
        let mut entry = item(
            Source::Pipx,
            program,
            name,
            text(package, "package_version"),
            &["upgrade", name],
        );
        entry.description = "pipx 环境 · 保留原来源，不升级其他环境或注入包".into();
        for app in array(package, "app_paths") {
            if let Some(path) = app
                .as_str()
                .or_else(|| app.get("__Path__").and_then(Value::as_str))
            {
                batch.owned.push(canonical(Path::new(path)));
            }
        }
        batch.items.push(entry);
    }
    Ok(batch)
}
fn cargo(env: &Environment, program: &Path) -> Result<Batch> {
    let root = env.path_setting(
        "CARGO_INSTALL_ROOT",
        env.path_setting("CARGO_HOME", env.home.join(".cargo")),
    );
    let path = root.join(".crates2.json");
    let mut batch = Batch::default();
    if !path.is_file() {
        return Ok(batch);
    }
    ensure!(path.metadata()?.len() < 4 * 1024 * 1024, "Cargo 元数据过大");
    let json: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let installs = json
        .get("installs")
        .and_then(Value::as_object)
        .context("不支持的 Cargo 安装记录")?;
    for (key, metadata) in installs {
        let mut parts = key.splitn(3, ' ');
        let (Some(name), Some(version), Some(origin)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if !package_name(name) {
            continue;
        }
        let mut entry = item(Source::Cargo, program, name, version, &[]);
        entry.id = format!("cargo:{}:{name}", root.display());
        entry.path = Some(root.join("bin"));
        for bin in array(metadata, "bins").filter_map(Value::as_str) {
            if package_name(bin) {
                batch.owned.push(canonical(&root.join("bin").join(bin)));
            }
        }
        let req = if metadata.get("version_req").is_some_and(Value::is_null) {
            "*"
        } else {
            text(metadata, "version_req")
        };
        // Reinstalling from just a name loses source, features and constraints. Only recorded registry installs can be reconstructed.
        let complete = origin == "(registry+https://github.com/rust-lang/crates.io-index)"
            && !req.is_empty()
            && metadata["features"].is_array()
            && metadata["all_features"].is_boolean()
            && metadata["no_default_features"].is_boolean()
            && !text(metadata, "profile").is_empty()
            && !text(metadata, "target").is_empty()
            && metadata["bins"].as_array().is_some_and(|bins| {
                !bins.is_empty()
                    && bins
                        .iter()
                        .all(|bin| bin.as_str().is_some_and(package_name))
            });
        if complete {
            let mut args = vec![
                "install".into(),
                "--locked".into(),
                "--root".into(),
                root.display().to_string(),
                "--version".into(),
                req.into(),
                "--profile".into(),
                text(metadata, "profile").into(),
            ];
            if metadata["all_features"] == true {
                args.push("--all-features".into());
            }
            if metadata["no_default_features"] == true {
                args.push("--no-default-features".into());
            }
            let features: Vec<_> = array(metadata, "features")
                .filter_map(Value::as_str)
                .collect();
            if !features.is_empty() {
                args.extend(["--features".into(), features.join(",")]);
            }
            if !text(metadata, "target").is_empty() {
                args.extend(["--target".into(), text(metadata, "target").into()]);
            }
            for bin in array(metadata, "bins").filter_map(Value::as_str) {
                args.extend(["--bin".into(), bin.into()]);
            }
            args.push(name.into());
            entry.action = Action::Command(Invocation::new(program, args));
            entry.pinned = req.starts_with('=');
            entry.description = format!(
                "Cargo · 保留记录的版本范围 {req}、features、target 和 profile；编译可能较久"
            );
        } else {
            entry.action = Action::Manual;
            entry.state = UpdateState::Manual;
            entry.description =
                "Cargo · Git / 路径 / 自定义源或安装参数不完整，请从原来源更新".into();
        }
        batch.items.push(entry);
    }
    Ok(batch)
}
fn rustup(env: &Environment, program: &Path, check: bool, cancel: &AtomicBool) -> Result<Batch> {
    let output = env.run(program, &["toolchain", "list"], cancel)?;
    let mut batch = Batch::default();
    let mut updates = HashMap::new();
    if check {
        match env.capture(
            &Invocation::new(program, ["check"]),
            Duration::from_secs(45),
            cancel,
            &[0, 100],
        ) {
            Ok(result) => {
                for line in result.lines() {
                    if let Some((name, detail)) = line.split_once(" - ")
                        && detail.contains("Update available")
                    {
                        updates.insert(name.trim().to_string(), detail.to_string());
                    }
                }
            }
            Err(error) => batch.warnings.push(format!("rustup 检查失败：{error:#}")),
        }
    }
    for line in output.lines() {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        if !package_name(name) || line == "no installed toolchains" {
            continue;
        }
        let rolling = ["stable", "beta", "nightly"].iter().any(|channel| {
            name == *channel
                || name
                    .strip_prefix(&format!("{channel}-"))
                    .is_some_and(|tail| !tail.starts_with(|c: char| c.is_ascii_digit()))
        });
        let mut entry = item(
            Source::Rustup,
            program,
            name,
            "已安装",
            &["update", name, "--no-self-update"],
        );
        entry.description = "仅更新此工具链，不更新 rustup 本体".into();
        if updates.contains_key(name) {
            entry.state = UpdateState::Available;
        }
        if !rolling {
            entry.pinned = true;
            entry.state = UpdateState::Manual;
            entry.action = Action::Manual;
            entry.description = "精确版本、带日期或自定义工具链，保留原版本".into();
        }
        batch.items.push(entry);
    }
    Ok(batch)
}

pub(super) fn append_self_updaters(env: &Environment, batch: &mut Batch) {
    // Only official standalone installer locations. A matching command name alone is not proof of ownership.
    for (name, directory, args) in [
        ("bun", ".bun/bin", vec!["upgrade"]),
        ("opencode", ".opencode/bin", vec!["upgrade"]),
    ] {
        let Some(program) = env.find(name) else {
            continue;
        };
        if batch.owns(&program)
            || !canonical(&program).starts_with(canonical(&env.home.join(directory)))
        {
            continue;
        }
        let mut entry = item(Source::SelfUpdate, &program, name, "独立安装", &args);
        entry.path = Some(program);
        entry.description = "官方独立安装位置 · 更新时由自身检查版本，保留默认频道".into();
        batch.items.push(entry);
    }
    if let Some(program) = env.find("claude")
        && !batch.owns(&program)
        && canonical(&program).starts_with(env.home.join(".local/share/claude/versions"))
    {
        let mut entry = item(
            Source::SelfUpdate,
            &program,
            "Claude Code",
            "原生安装",
            &["update"],
        );
        entry.path = Some(program);
        entry.description = "Claude 原生安装 · 保留现有自动更新频道和版本策略".into();
        batch.items.push(entry);
    }
    for (name, homepage) in [
        (
            "conda",
            "https://docs.conda.io/projects/conda/en/stable/commands/update.html",
        ),
        ("port", "https://guide.macports.org/"),
        (
            "nix",
            "https://nix.dev/manual/nix/stable/command-ref/new-cli/nix3-profile-upgrade",
        ),
        ("tlmgr", "https://www.tug.org/texlive/tlmgr.html"),
        ("gem", "https://guides.rubygems.org/command-reference/"),
        ("yarn", "https://classic.yarnpkg.com/en/docs/cli/global"),
    ] {
        if let Some(program) = env.find(name) {
            let mut entry = item(Source::SelfUpdate, &program, name, "检测到管理器", &[]);
            entry.action = Action::OpenUrl(homepage.into());
            entry.state = UpdateState::Manual;
            entry.homepage = homepage.into();
            entry.path = Some(program);
            entry.description = "此来源尚未提供逐项适配；查看原管理器说明，不执行全量更新".into();
            batch.items.push(entry);
        }
    }
    // uv's standalone installer leaves a receipt. Do not enable self update for pip/Cargo/Brew uv.
    if let Some(program) = env.find("uv")
        && !batch.owns(&program)
        && let Some(receipt) = uv_receipt(env, &program)
    {
        let mut entry = item(
            Source::SelfUpdate,
            &program,
            "uv",
            text(&receipt, "version"),
            &["self", "update"],
        );
        entry.path = Some(program);
        entry.description = "uv 独立安装收据已确认 · 只更新 uv 本体".into();
        batch.items.push(entry);
    }
    for (name, homepage) in [
        (
            "kimi",
            "https://moonshotai.github.io/kimi-code/en/reference/kimi-command",
        ),
        ("claude", "https://code.claude.com/docs/en/setup"),
        ("bun", "https://bun.com/docs/installation"),
        (
            "deno",
            "https://docs.deno.com/runtime/reference/cli/upgrade/",
        ),
        ("opencode", "https://opencode.ai/docs/cli/#upgrade"),
        (
            "uv",
            "https://docs.astral.sh/uv/getting-started/installation/#upgrading-uv",
        ),
    ] {
        if let Some(program) = env.find(name)
            && !batch.owns(&program)
            && !batch.items.iter().any(|i| {
                i.path
                    .as_ref()
                    .is_some_and(|p| canonical(p) == canonical(&program))
            })
        {
            let mut entry = item(Source::SelfUpdate, &program, name, "来源待确认", &[]);
            entry.action = Action::OpenUrl(homepage.into());
            entry.state = UpdateState::Manual;
            entry.homepage = homepage.into();
            entry.path = Some(program);
            entry.description = if name == "deno" {
                "Deno · 请按原 stable / LTS / RC / canary 频道更新，避免默认切换至 stable"
            } else {
                "安装来源或产品版本未确认；按官方说明处理，避免改用其他安装器"
            }
            .into();
            batch.items.push(entry);
        }
    }
}
fn uv_receipt(env: &Environment, program: &Path) -> Option<Value> {
    let has = |key| env.variables.iter().any(|(name, _)| name == key);
    let prefixes = if has("AXOUPDATER_CONFIG_WORKING_DIR") {
        vec![env.home.clone()]
    } else if has("AXOUPDATER_CONFIG_PATH") {
        vec![env.path_setting("AXOUPDATER_CONFIG_PATH", env.home.clone())]
    } else {
        vec![
            env.path_setting("XDG_CONFIG_HOME", env.home.join(".config"))
                .join("uv"),
            env.home.join(".config/uv"),
        ]
    };
    let path = prefixes
        .into_iter()
        .map(|p| p.join("uv-receipt.json"))
        .find(|p| p.is_file())?;
    if std::fs::metadata(&path).ok()?.len() >= 64 * 1024 {
        return None;
    }
    let receipt: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let prefix = Path::new(text(&receipt, "install_prefix"));
    if !prefix.is_absolute()
        || text(&receipt, "version").is_empty()
        || !array(&receipt, "binaries").any(|bin| bin == "uv")
    {
        return None;
    }
    let prefix = canonical(prefix);
    let program = canonical(program);
    let mut root = program.parent()?;
    if root.file_name().is_some_and(|p| p == "bin") && prefix.file_name().is_none_or(|p| p != "bin")
    {
        root = root.parent()?;
    }
    (root == prefix).then_some(receipt)
}

/// Re-read selected providers before the first write, using the same process environment.
/// Self updaters require one full ownership check, shared by every step in the plan.
pub(super) fn validate(
    env: &Environment,
    expected: &[std::sync::Arc<Software>],
    pin: bool,
    cancel: &AtomicBool,
) -> Result<()> {
    let ownership_check = expected
        .iter()
        .any(|item| item.source == Source::SelfUpdate);
    let mut current = if ownership_check {
        let mut batch = scan(env, ScanMode::Local, cancel);
        append_self_updaters(env, &mut batch);
        batch
    } else {
        Batch::default()
    };
    let mut read = std::collections::HashSet::new();
    for item in expected {
        ensure!(!cancel.load(Ordering::Relaxed), "任务已取消");
        let program = if pin {
            &item.pin.as_ref().context("固定命令已失效")?.program
        } else {
            let Action::Command(command) = &item.action else {
                anyhow::bail!("不是可执行项目");
            };
            &command.program
        };
        ensure!(program.is_file(), "{} 的管理器已不存在", item.name);
        if ownership_check {
            continue;
        }
        let group = match item.source {
            Source::Cask => Source::Formula,
            source => source,
        };
        if !read.insert((group.label(), program.clone())) {
            continue;
        }
        let result = match group {
            Source::Formula => brew(env, program, false, cancel)?,
            Source::Npm | Source::Pnpm => node(env, program, group, false, cancel)?,
            Source::Bun => bun(env, program, cancel)?,
            Source::Uv => uv(env, program, cancel)?,
            Source::Pipx => pipx(env, program, cancel)?,
            Source::Cargo => cargo(env, program)?,
            Source::Rustup => rustup(env, program, false, cancel)?,
            _ => anyhow::bail!("此来源需要手动处理"),
        };
        current.merge(result);
    }
    for old in expected {
        let new = current
            .items
            .iter()
            .find(|item| item.id == old.id)
            .with_context(|| format!("{} 的安装来源已变化，请重新读取清单", old.name))?;
        let same_command = if pin {
            new.pin == old.pin
        } else {
            matches!((&new.action,&old.action), (Action::Command(a), Action::Command(b)) if a == b)
                && !new.pinned
        };
        ensure!(
            same_command && old.installed == new.installed,
            "{} 的版本、固定状态或安装参数已变化，请重新确认计划",
            old.name
        );
    }
    Ok(())
}
