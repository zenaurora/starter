//! Mole owns preview and confirmation. Only a uniquely matched, revalidated app
//! is handed to it; never send raw search text or assume a cask token is a name.
use super::Target;
use crate::task_terminal::{Request, task_path};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    fs,
    io::Read,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Deserialize)]
struct Application {
    name: String,
    path: PathBuf,
    #[serde(default)]
    source: String,
    #[serde(default)]
    size: String,
}

pub struct Prepared {
    pub request: Request,
    pub detail: String,
}

pub fn find_program() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = ["/opt/homebrew/bin/mo", "/usr/local/bin/mo"]
        .map(PathBuf::from)
        .into();
    if let Some(home) = dirs::home_dir() {
        candidates.extend([home.join(".local/bin/mo"), home.join(".local/bin/mole")]);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path).filter(|dir| dir.is_absolute()) {
            candidates.extend([directory.join("mo"), directory.join("mole")]);
        }
    }
    candidates.into_iter().find_map(|path| {
        let canonical = path.canonicalize().ok()?;
        let metadata = fs::metadata(&canonical).ok()?;
        (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(canonical)
    })
}

pub fn prepare(target: &Target, cancelled: &AtomicBool) -> Result<Prepared> {
    super::macos::revalidate(target)?;
    let program = find_program()
        .context("尚未安装 Mole。可安装后重试：brew install mole；也可返回选择“仅移到废纸篓”。")?;
    let applications = inventory(&program, cancelled)?;
    let (argument, application) = argument_for(&target.path, &applications)?;
    super::macos::revalidate(target)?;
    ensure!(!cancelled.load(Ordering::Relaxed), "任务已取消");
    let detail = [application.source.as_str(), application.size.as_str()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    Ok(Prepared {
        request: Request {
            program,
            args: vec!["uninstall".into(), argument.into()],
            env: vec![],
        },
        detail,
    })
}

fn inventory(program: &Path, cancelled: &AtomicBool) -> Result<Vec<Application>> {
    let mut command = Command::new(program);
    command
        .args(["uninstall", "--list"])
        .env_clear()
        .env("PATH", task_path())
        .env("LANG", "en_US.UTF-8")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    for key in ["HOME", "USER", "LOGNAME", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.current_dir(dirs::home_dir().context("无法确定用户目录")?);
    let mut child = command.spawn().context("无法读取 Mole 应用清单")?;
    let stdout = child.stdout.take().context("Mole 未返回输出通道")?;
    let output = thread::spawn(move || {
        let mut bytes = vec![];
        stdout
            .take(4 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(40);
    let status = loop {
        if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
            break Err(anyhow::anyhow!("Mole 清单扫描已取消或超时，请重试"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => break Err(error.into()),
        }
        thread::sleep(Duration::from_millis(30));
    };
    // Clean up this private inventory group, including helpers holding stdout.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.wait();
    let bytes = output
        .join()
        .map_err(|_| anyhow::anyhow!("Mole 清单读取线程异常"))??;
    ensure!(
        status?.success(),
        "Mole 清单读取失败，请检查工具或更新 Mole：brew upgrade mole"
    );
    serde_json::from_slice(&bytes).context("Mole 未返回有效 JSON 应用清单，请更新 Mole 后重试")
}

fn argument_for<'a>(
    path: &Path,
    applications: &'a [Application],
) -> Result<(String, &'a Application)> {
    let rows: Vec<_> = applications.iter().filter(|app| app.path == path).collect();
    ensure!(rows.len() == 1, "Mole 无法唯一识别该应用路径，已停止交接");
    let application = rows[0];
    for name in [
        Some(application.name.as_str()),
        path.file_stem().and_then(|name| name.to_str()),
    ]
    .into_iter()
    .flatten()
    {
        if name.is_empty()
            || name.starts_with('-')
            || name.chars().any(char::is_control)
            || name.contains(['\\', '*', '?', '['])
        {
            continue;
        }
        let query = name.to_lowercase();
        let matches: Vec<_> = applications
            .iter()
            .filter(|app| {
                app.name.to_lowercase() == query
                    || app
                        .path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| stem.to_lowercase() == query)
            })
            .collect();
        if matches.len() == 1 && matches[0].path == path {
            return Ok((name.to_owned(), application));
        }
    }
    bail!(
        "应用名称重复或含 Mole 无法准确匹配的字符，已停止交接。请使用“仅移到废纸篓”或在 Mole 中手动选择。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, path: &str) -> Application {
        Application {
            name: name.into(),
            path: path.into(),
            source: "App".into(),
            size: "1MB".into(),
        }
    }

    #[test]
    fn path_mapping_uses_exact_names_not_cask_tokens_or_substrings() {
        let rows: Vec<Application> = serde_json::from_str(r#"[
            {"name":"CC Switch","path":"/Applications/CC Switch.app","source":"Homebrew","uninstall_name":"cc-switch"},
            {"name":"CC Switch Helper","path":"/Applications/CC Switch Helper.app"}
        ]"#).unwrap();
        assert_eq!(
            argument_for(Path::new("/Applications/CC Switch.app"), &rows)
                .unwrap()
                .0,
            "CC Switch"
        );
        assert!(argument_for(Path::new("/Applications/CC.app"), &rows).is_err());
    }

    #[test]
    fn duplicates_and_option_injection_never_fall_back_to_fuzzy_uninstall() {
        let rows = [
            app("Example", "/Applications/Example.app"),
            app("Example", "/Users/test/Applications/Example.app"),
        ];
        assert!(argument_for(Path::new("/Applications/Example.app"), &rows).is_err());
        let rows = [app("--permanent", "/Applications/--permanent.app")];
        assert!(argument_for(Path::new("/Applications/--permanent.app"), &rows).is_err());
        let rows = [app("[Example]", "/Applications/[Example].app")];
        assert!(argument_for(&rows[0].path, &rows).is_err());
        let rows = [app(
            "中文 'app' $(something)",
            "/Applications/中文 'app' $(something).app",
        )];
        assert_eq!(argument_for(&rows[0].path, &rows).unwrap().0, rows[0].name);
    }
}
