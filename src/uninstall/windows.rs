use super::*;
use anyhow::{Context, ensure};
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::Path,
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    System::Environment::ExpandEnvironmentStringsW,
    UI::{
        Shell::{CommandLineToArgvW, ShellExecuteW},
        WindowsAndMessaging::SW_SHOWNORMAL,
    },
};
use winreg::{RegKey, enums::*};

const KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
#[derive(Clone, Debug)]
pub struct Action {
    hive: bool,
    view: u32,
    key: String,
    command: String,
}

fn record(action: &Action) -> Result<(String, String)> {
    let hive = RegKey::predef(if action.hive {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    });
    let key =
        hive.open_subkey_with_flags(format!(r"{KEY}\{}", action.key), KEY_READ | action.view)?;
    Ok((
        key.get_value("DisplayName")?,
        key.get_value("UninstallString")?,
    ))
}

pub fn discover(_: &Catalog) -> Vec<Target> {
    let mut targets = Vec::new();
    for hive in [false, true] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let base = RegKey::predef(if hive {
                HKEY_LOCAL_MACHINE
            } else {
                HKEY_CURRENT_USER
            });
            let Ok(base) = base.open_subkey_with_flags(KEY, KEY_READ | view) else {
                continue;
            };
            for key in base.enum_keys().flatten() {
                let Ok(program) = base.open_subkey(&key) else {
                    continue;
                };
                if program.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 {
                    continue;
                }
                let Ok(name) = program.get_value::<String, _>("DisplayName") else {
                    continue;
                };
                if name.trim().is_empty() || name.to_lowercase().starts_with("starter") {
                    continue;
                }
                let Ok(command) = program.get_value::<String, _>("UninstallString") else {
                    continue;
                };
                if parse(&command).is_err() {
                    continue;
                }
                let path = program
                    .get_value::<String, _>("InstallLocation")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_default();
                if !path.as_os_str().is_empty()
                    && std::env::current_exe()
                        .ok()
                        .is_some_and(|exe| exe.starts_with(&path))
                {
                    continue;
                }
                let id = format!("uninstall:{hive}:{view}:{key}");
                // Some HKCU registrations are reflected into both registry views.
                if targets.iter().any(|t: &Target| {
                    t.name == name && t.action.command == command && t.action.hive == hive
                }) {
                    continue;
                }
                targets.push(Target {
                    id,
                    name,
                    path,
                    aliases: vec![],
                    action: Action {
                        hive,
                        view,
                        key,
                        command,
                    },
                });
            }
        }
    }
    targets.sort_by_cached_key(|t| t.name.to_lowercase());
    targets
}

pub fn description(target: &Target) -> String {
    format!(
        "启动 {} 的官方卸载程序。随后按卸载向导操作；Windows 可能要求管理员确认。{}",
        target.name,
        if target.path.as_os_str().is_empty() {
            String::new()
        } else {
            format!("\n\n安装位置：{}", target.path.display())
        }
    )
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn parse(command: &str) -> Result<Vec<OsString>> {
    ensure!(
        !command.contains('\0') && !command.trim().is_empty(),
        "卸载命令无效"
    );
    let raw = wide(command.as_ref());
    let length = unsafe { ExpandEnvironmentStringsW(raw.as_ptr(), std::ptr::null_mut(), 0) };
    ensure!(length > 0 && length < 32768, "卸载命令过长");
    let mut expanded = vec![0u16; length as usize];
    ensure!(
        unsafe { ExpandEnvironmentStringsW(raw.as_ptr(), expanded.as_mut_ptr(), length) } == length,
        "环境变量展开失败"
    );
    let mut count = 0;
    let argv = unsafe { CommandLineToArgvW(expanded.as_ptr(), &mut count) };
    ensure!(!argv.is_null() && count > 0, "卸载命令解析失败");
    let mut args = Vec::new();
    for arg in unsafe { std::slice::from_raw_parts(argv, count as usize) } {
        let mut len = 0;
        while unsafe { *arg.add(len) } != 0 {
            len += 1;
        }
        args.push(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(*arg, len)
        }));
    }
    unsafe {
        LocalFree(argv.cast());
    }
    let filename = Path::new(&args[0])
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    ensure!(
        !matches!(
            filename.as_str(),
            "cmd.exe" | "powershell.exe" | "pwsh.exe" | "wscript.exe" | "cscript.exe"
        ),
        "不支持脚本形式的卸载程序，请使用 Windows 设置"
    );
    if matches!(filename.as_str(), "msiexec" | "msiexec.exe") {
        let root = std::env::var_os("SystemRoot").context("SystemRoot 缺失")?;
        args[0] = PathBuf::from(root)
            .join("System32/msiexec.exe")
            .into_os_string();
        for arg in &mut args[1..] {
            // ARP MSI strings commonly contain /I: turn maintenance into uninstall.
            let s = arg.to_string_lossy();
            if s.eq_ignore_ascii_case("/i") {
                *arg = "/x".into();
            } else if s.to_lowercase().starts_with("/i{") {
                *arg = format!("/x{}", &s[2..]).into();
            }
        }
    }
    ensure!(
        Path::new(&args[0]).is_absolute() && Path::new(&args[0]).is_file(),
        "卸载程序不可用，请使用 Windows 设置中的已安装应用"
    );
    Ok(args)
}

// ShellExecute takes a parameter string. Apply Windows argv quoting to each
// argument; never pass registry commands to a shell interpreter.
fn quote(arg: &std::ffi::OsStr) -> Vec<u16> {
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for c in arg.encode_wide() {
        if c == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            b'\\' as u16,
            if c == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        result.push(c);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    result
}

pub fn execute(target: &Target) -> Result<String> {
    let (name, command) = record(&target.action).context("应用登记已变化，请刷新后重试")?;
    ensure!(
        name == target.name && command == target.action.command,
        "卸载目标已变化，请刷新后重试"
    );
    let args = parse(&command)?;
    let exe = wide(&args[0]);
    let mut parameters = Vec::new();
    for arg in &args[1..] {
        if !parameters.is_empty() {
            parameters.push(32);
        }
        parameters.extend(quote(arg));
    }
    parameters.push(0);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            exe.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    ensure!(result > 32, "无法启动卸载程序（Windows 错误 {result}）");
    Ok(format!("已启动 {} 的卸载向导", target.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_argument_quoting_roundtrips_unicode_spaces_quotes_and_backslashes() {
        let exe = std::env::current_exe().unwrap().into_os_string();
        let arguments: Vec<OsString> =
            ["", "中文 file", "a\"b", r"C:\folder with spaces\", "& $()"]
                .map(OsString::from)
                .into();
        let mut command = quote(&exe);
        for arg in &arguments {
            command.push(32);
            command.extend(quote(arg));
        }
        let command = String::from_utf16(&command).unwrap();
        let parsed = parse(&command).unwrap();
        assert_eq!(&parsed[1..], &arguments);
        assert!(parse("").is_err());
        assert!(parse(r"cmd.exe /c del example").is_err());
    }
    #[test]
    fn msi_maintenance_commands_become_explicit_uninstall_commands() {
        let args = parse("MsiExec.exe /I{01234567-0123-0123-0123-0123456789AB}").unwrap();
        assert_eq!(args[1], "/x{01234567-0123-0123-0123-0123456789AB}");
        let args = parse("MsiExec.exe /I {01234567-0123-0123-0123-0123456789AB}").unwrap();
        assert_eq!(args[1], "/x");
    }
}
