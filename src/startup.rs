use anyhow::{Context, Result};

pub const BACKGROUND_ARG: &str = "--background";

/// Apply the login setting before saving, restoring it if saving fails.
pub fn apply(enabled: bool, previous: bool, save: impl FnOnce() -> Result<()>) -> Result<()> {
    apply_with(enabled, previous, set_enabled, save)
}

fn apply_with(
    enabled: bool,
    previous: bool,
    mut set: impl FnMut(bool) -> Result<()>,
    save: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if enabled != previous {
        set(enabled).context("无法修改开机自启动")?;
    }
    if let Err(error) = save() {
        if enabled != previous {
            set(previous).map_err(|rollback| {
                anyhow::anyhow!("{error:#}；开机自启动恢复失败：{rollback:#}")
            })?;
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn set_enabled(enabled: bool) -> Result<()> {
    let path = dirs::home_dir()
        .context("无法定位用户目录")?
        .join("Library/LaunchAgents/dev.starter.launcher.plist");
    let executable = std::env::current_exe()?;
    if enabled {
        let bundle = executable
            .parent()
            .and_then(std::path::Path::parent)
            .and_then(std::path::Path::parent)
            .filter(|path| path.extension().is_some_and(|ext| ext == "app"));
        anyhow::ensure!(bundle.is_some(), "请从安装的 Starter.app 中启用开机自启动");
    }
    write_launch_agent(&path, &executable, enabled)
}

#[cfg(target_os = "macos")]
fn write_launch_agent(
    path: &std::path::Path,
    executable: &std::path::Path,
    enabled: bool,
) -> Result<()> {
    if !enabled {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let executable = executable.to_str().context("应用路径不是有效的 UTF-8")?;
    let mut agent = plist::Dictionary::new();
    agent.insert("Label".into(), "dev.starter.launcher".into());
    agent.insert(
        "AssociatedBundleIdentifiers".into(),
        plist::Value::Array(vec!["dev.starter.launcher".into()]),
    );
    agent.insert(
        "ProgramArguments".into(),
        plist::Value::Array(vec![executable.into(), BACKGROUND_ARG.into()]),
    );
    agent.insert("RunAtLoad".into(), true.into());
    agent.insert("LimitLoadToSessionType".into(), "Aqua".into());
    let mut bytes = Vec::new();
    plist::Value::Dictionary(agent).to_writer_xml(&mut bytes)?;
    crate::config::atomic_write(path, &bytes)
}

#[cfg(target_os = "windows")]
fn set_enabled(enabled: bool) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = user.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
    if enabled {
        let executable = std::env::current_exe()?;
        let command = format!("\"{}\" {BACKGROUND_ARG}", executable.display());
        run.set_value("Starter", &command)?;
    } else {
        match run.delete_value("Starter") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn set_enabled(_: bool) -> Result<()> {
    anyhow::bail!("当前系统暂不支持开机自启动")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn registration_failure_does_not_save_and_save_failure_restores_registration() {
        let saved = Cell::new(false);
        assert!(
            apply_with(
                true,
                false,
                |_| anyhow::bail!("registration failed"),
                || {
                    saved.set(true);
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!saved.get());

        let enabled = Cell::new(false);
        assert!(
            apply_with(
                true,
                false,
                |value| {
                    enabled.set(value);
                    Ok(())
                },
                || anyhow::bail!("disk full")
            )
            .is_err()
        );
        assert!(!enabled.get());
        apply_with(
            true,
            false,
            |value| {
                enabled.set(value);
                Ok(())
            },
            || Ok(()),
        )
        .unwrap();
        assert!(enabled.get());
        assert!(
            apply_with(
                false,
                true,
                |value| {
                    enabled.set(value);
                    Ok(())
                },
                || anyhow::bail!("disk full")
            )
            .is_err()
        );
        assert!(enabled.get());
        apply_with(
            false,
            true,
            |value| {
                enabled.set(value);
                Ok(())
            },
            || Ok(()),
        )
        .unwrap();
        assert!(!enabled.get());
        apply_with(
            false,
            false,
            |_| panic!("unchanged settings must not alter login items"),
            || Ok(()),
        )
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_roundtrips_paths_and_removes_only_its_own_registration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("LaunchAgents/dev.starter.launcher.plist");
        let executable =
            std::path::Path::new("/Applications/工具 & <Starter>.app/Contents/MacOS/starter");
        write_launch_agent(&path, executable, true).unwrap();
        let agent = plist::Value::from_file(&path).unwrap();
        let agent = agent.as_dictionary().unwrap();
        assert_eq!(
            agent["ProgramArguments"].as_array().unwrap(),
            &vec![executable.to_str().unwrap().into(), BACKGROUND_ARG.into()]
        );
        assert_eq!(agent["RunAtLoad"].as_boolean(), Some(true));
        let other = path.with_file_name("another-app.plist");
        std::fs::write(&other, "other login item").unwrap();
        write_launch_agent(&path, executable, false).unwrap();
        write_launch_agent(&path, executable, false).unwrap();
        assert!(!path.exists());
        assert!(other.exists());
    }
}
