use crate::config::{Config, expand_home};
use anyhow::{Context, Result, ensure};
use std::path::Path;

#[cfg(not(target_os = "windows"))]
use std::{path::PathBuf, process::Command};

pub fn normalize_rule(value: &str) -> Result<String> {
    let value = value.trim().trim_start_matches('.').to_lowercase();
    ensure!(
        !value.is_empty() && !value.contains(['/', '\\', ' ', ',']),
        "请填写扩展名，例如 md、pdf；文件夹用 folder，所有文件用 *"
    );
    Ok(value)
}

pub fn application_for<'a>(path: &Path, directory: bool, config: &'a Config) -> Option<&'a str> {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let key = if directory { "folder" } else { &extension };
    config
        .open_with
        .get(key)
        .or_else(|| (!directory).then(|| config.open_with.get("*")).flatten())
        .map(String::as_str)
}

pub fn open(path: &Path, application: Option<&str>) -> Result<()> {
    let path = expand_home(path);
    if let Some(application) = application {
        #[cfg(target_os = "windows")]
        return shell_open(application, Some(&path));
        #[cfg(not(target_os = "windows"))]
        {
            let mut command = application_command(application);
            command.arg(&path);
            run(command).with_context(|| format!("无法使用 {application} 打开 {}", path.display()))
        }
    } else {
        open::that(&path).with_context(|| format!("无法打开 {}", path.display()))
    }
}

pub fn launch(application: &str) -> Result<()> {
    ensure!(!application.trim().is_empty(), "请选择应用");
    #[cfg(target_os = "windows")]
    return shell_open(application, None);
    #[cfg(not(target_os = "windows"))]
    run(application_command(application))
}

#[cfg(target_os = "windows")]
fn shell_open(application: &str, path: Option<&Path>) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let application = expand_home(Path::new(application));
    let application: Vec<u16> = application
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Windows filenames cannot contain quotes. Double terminal backslashes so
    // the closing quote is not consumed by command-line argument parsing.
    let parameters = path.map(|path| {
        let value = path.as_os_str().to_string_lossy();
        let trailing = value.chars().rev().take_while(|&c| c == '\\').count();
        format!("\"{value}{}\"", "\\".repeat(trailing))
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>()
    });
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            application.as_ptr(),
            parameters.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    ensure!(
        result as isize > 32,
        "无法打开应用（系统错误 {}），请检查路径",
        result as isize
    );
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn application_command(application: &str) -> Command {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("/usr/bin/open");
        command
            .arg("-a")
            .arg(expand_home(&PathBuf::from(application)));
        command
    }
    #[cfg(not(target_os = "macos"))]
    Command::new(expand_home(&PathBuf::from(application)))
}

#[cfg(not(target_os = "windows"))]
fn run(mut command: Command) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        ensure!(
            command.status()?.success(),
            "应用未能打开，请检查名称或路径"
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        command.spawn().context("无法运行所选应用")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn specific_file_rule_overrides_fallback_and_folder_is_independent() {
        let mut config = Config::default();
        config.open_with.insert("md".into(), "Editor".into());
        config.open_with.insert("*".into(), "Fallback".into());
        config.open_with.insert("folder".into(), "Finder".into());
        assert_eq!(
            application_for(Path::new("a.MD"), false, &config),
            Some("Editor")
        );
        assert_eq!(
            application_for(Path::new("no-extension"), false, &config),
            Some("Fallback")
        );
        assert_eq!(
            application_for(Path::new("dir.md"), true, &config),
            Some("Finder")
        );
        assert_eq!(normalize_rule(" .MD ").unwrap(), "md");
        assert!(normalize_rule("md,pdf").is_err());
    }
}
