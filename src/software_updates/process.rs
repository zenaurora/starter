use super::Invocation;
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsString,
    fs::File,
    io::Read,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Environment {
    pub variables: Vec<(OsString, OsString)>,
    pub home: PathBuf,
    path: OsString,
}
impl Environment {
    pub fn discover() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let mut directories = vec![];
        // GUI launches do not have a shell's PATH. Do not execute startup scripts to obtain it.
        if let Some(path) = std::env::var_os("PATH") {
            directories.extend(std::env::split_paths(&path).filter(|p| p.is_absolute()));
        }
        for key in [
            "PNPM_HOME",
            "CARGO_HOME",
            "BUN_INSTALL",
            "VOLTA_HOME",
            "NVM_BIN",
        ] {
            if let Some(path) = std::env::var_os(key) {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    directories.push(if key == "PNPM_HOME" || key == "NVM_BIN" {
                        path
                    } else {
                        path.join("bin")
                    });
                }
            }
        }
        directories.extend(
            [
                ".local/bin",
                ".cargo/bin",
                ".bun/bin",
                ".deno/bin",
                ".kimi/bin",
                ".kimi-code/bin",
                ".opencode/bin",
                "Library/pnpm",
                ".local/share/pnpm",
                ".npm-global/bin",
                ".volta/bin",
                ".asdf/shims",
                ".local/share/mise/shims",
                ".local/share/fnm/aliases/default/bin",
                "Library/Application Support/fnm/aliases/default/bin",
            ]
            .map(|p| home.join(p)),
        );
        directories.extend(std::env::split_paths(&crate::task_terminal::task_path()));
        directories.push(PathBuf::from("/opt/local/bin"));
        directories.push(PathBuf::from("/Library/TeX/texbin"));
        let mut unique = std::collections::HashSet::new();
        directories.retain(|p| unique.insert(p.clone()));
        let path =
            std::env::join_paths(directories).unwrap_or_else(|_| crate::task_terminal::task_path());
        // Preserve the manager's registry, credentials, mirrors and installation settings.
        // Reconstructing a whitelist silently changes source configuration for some tools.
        let mut variables: Vec<_> = std::env::vars_os().collect();
        for (key, value) in [
            ("PATH".into(), path.clone()),
            ("HOME".into(), home.as_os_str().into()),
            ("LANG".into(), "en_US.UTF-8".into()),
            ("LC_ALL".into(), "en_US.UTF-8".into()),
            ("HOMEBREW_NO_AUTO_UPDATE".into(), "1".into()),
            ("HOMEBREW_NO_ANALYTICS".into(), "1".into()),
            ("HOMEBREW_NO_ENV_HINTS".into(), "1".into()),
            ("NO_COLOR".into(), "1".into()),
        ] {
            variables.retain(|(existing, _)| existing != &key);
            variables.push((key, value));
        }
        variables.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            variables,
            home,
            path,
        }
    }
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        self.find_all(name).into_iter().next()
    }
    pub fn find_all(&self, name: &str) -> Vec<PathBuf> {
        let mut seen = std::collections::HashSet::new();
        std::env::split_paths(&self.path)
            .map(|p| p.join(name))
            .filter(|p| {
                p.is_absolute()
                    && p.metadata()
                        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    && seen.insert(canonical(p))
            })
            .collect()
    }
    pub fn variables_for(&self, program: &Path) -> Vec<(OsString, OsString)> {
        let mut variables = self.variables.clone();
        if let Some(directory) = program.parent() {
            // Bind #!/usr/bin/env node/python and child tools to this manager's runtime first.
            if let Ok(path) = std::env::join_paths(
                std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&self.path)),
            ) && let Some(entry) = variables.iter_mut().find(|(key, _)| key == "PATH")
            {
                entry.1 = path;
            }
        }
        variables
    }
    pub fn path_setting(&self, key: &str, fallback: PathBuf) -> PathBuf {
        self.variables
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| PathBuf::from(value))
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    self.home.join(path)
                }
            })
            .unwrap_or(fallback)
    }
    #[cfg(test)]
    pub fn fixture(home: &Path, bin: &Path) -> Self {
        let path = std::env::join_paths([bin, Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
        Self {
            variables: vec![
                ("PATH".into(), path.clone()),
                ("HOME".into(), home.as_os_str().into()),
                ("LC_ALL".into(), "C".into()),
                ("HOMEBREW_NO_AUTO_UPDATE".into(), "1".into()),
            ],
            home: home.into(),
            path,
        }
    }
    pub fn run(&self, program: &Path, args: &[&str], cancelled: &AtomicBool) -> Result<String> {
        self.capture(
            &Invocation::new(program, args.iter().copied()),
            Duration::from_secs(25),
            cancelled,
            &[0],
        )
    }
    pub fn capture(
        &self,
        invocation: &Invocation,
        timeout: Duration,
        cancelled: &AtomicBool,
        accepted: &[i32],
    ) -> Result<String> {
        ensure!(!cancelled.load(Ordering::Relaxed), "检测已取消");
        ensure!(invocation.program.is_absolute(), "程序路径必须是绝对路径");
        let stdout = tempfile::tempfile()?;
        let stderr = tempfile::tempfile()?;
        let mut command = Command::new(&invocation.program);
        command
            .args(&invocation.args)
            .env_clear()
            .envs(self.variables_for(&invocation.program))
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .stdout(stdout.try_clone()?)
            .stderr(stderr.try_clone()?)
            .process_group(0);
        let child = command
            .spawn()
            .with_context(|| format!("无法启动 {}", invocation.program.display()))?;
        let mut guard = ChildGuard(child);
        let start = Instant::now();
        loop {
            if cancelled.load(Ordering::Relaxed) {
                bail!("检测已取消");
            }
            if start.elapsed() > timeout {
                bail!("检测超时（{} 秒）", timeout.as_secs());
            }
            // Cap while the process is alive as well as after exit. Disk-backed output keeps resident memory bounded.
            ensure!(
                stdout.metadata()?.len() <= OUTPUT_LIMIT
                    && stderr.metadata()?.len() <= OUTPUT_LIMIT,
                "检测输出超过限制"
            );
            if let Some(status) = guard.0.try_wait()? {
                ensure!(
                    stdout.metadata()?.len() <= OUTPUT_LIMIT
                        && stderr.metadata()?.len() <= OUTPUT_LIMIT,
                    "检测输出超过限制"
                );
                if !status.code().is_some_and(|code| accepted.contains(&code)) {
                    let error = read_limited(stderr, 2048)?;
                    bail!(
                        "{}：{}",
                        status,
                        error.trim().chars().take(320).collect::<String>()
                    );
                }
                return read_limited(stdout, OUTPUT_LIMIT);
            }
            thread::sleep(Duration::from_millis(30));
        }
    }
}
const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;
fn read_limited(mut file: File, limit: u64) -> Result<String> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![];
    file.take(limit).read_to_end(&mut bytes)?;
    String::from_utf8(bytes).context("检测输出不是 UTF-8")
}
struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Also reap descendants that inherited redirected output, including on success.
        unsafe {
            libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}
pub(super) fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.into())
}
pub(super) fn package_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@/._+-".contains(c))
}
