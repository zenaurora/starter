use crate::config::Config;
use anyhow::{Context, Result, ensure};
use global_hotkey::hotkey::{HotKey, Modifiers};
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Terminal,
    Application(String),
}

/// Canonical syntax also accepts familiar labels typed in settings.
pub fn parse(value: &str) -> Result<HotKey> {
    let canonical = value
        .split('+')
        .map(|part| match part.trim().to_lowercase().as_str() {
            "cmd" | "command" | "win" | "windows" => "Super".to_string(),
            "option" => "Alt".to_string(),
            _ => part.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join("+");
    HotKey::from_str(&canonical).with_context(|| format!("无效的快捷键：{value}"))
}

/// Split existing configuration into canonical choices for the visual editor.
pub fn parts(value: &str) -> Result<Vec<String>> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    let key = parse(value)?;
    let mut parts = Vec::new();
    for (modifier, name) in [
        (Modifiers::SUPER, "Super"),
        (Modifiers::CONTROL, "Ctrl"),
        (Modifiers::ALT, "Alt"),
        (Modifiers::SHIFT, "Shift"),
    ] {
        if key.mods.contains(modifier) {
            parts.push(name.to_string());
        }
    }
    parts.push(key.key.to_string());
    Ok(parts)
}

/// Choices can appear in any order; an OS hotkey has one key and modifiers.
pub fn compose(parts: &[String]) -> Result<String> {
    if parts.is_empty() {
        return Ok(String::new());
    }
    let mut modifiers = HashSet::new();
    let mut key = None;
    for part in parts {
        let modifier = match part.to_lowercase().as_str() {
            "cmd" | "command" | "super" | "win" | "windows" => Some("Super"),
            "ctrl" | "control" => Some("Ctrl"),
            "alt" | "option" => Some("Alt"),
            "shift" => Some("Shift"),
            _ => None,
        };
        if let Some(modifier) = modifier {
            ensure!(modifiers.insert(modifier), "修饰键不能重复选择：{modifier}");
        } else {
            ensure!(
                key.is_none(),
                "每组快捷键只能选择一个普通按键，其余格子请选择修饰键"
            );
            key = Some(parse(part)?.key.to_string());
        }
    }
    ensure!(
        !modifiers.is_empty(),
        "请至少选择一个 Cmd/Win、Ctrl、Option/Alt 或 Shift"
    );
    let key = key.context("请选择一个普通按键，例如 K、Space 或 F1")?;
    let mut result = ["Super", "Ctrl", "Alt", "Shift"]
        .into_iter()
        .filter(|modifier| modifiers.contains(modifier))
        .map(str::to_string)
        .collect::<Vec<_>>();
    result.push(key);
    Ok(result.join("+"))
}

pub fn bindings(config: &Config) -> Result<Vec<(HotKey, Action)>> {
    let mut bindings = vec![(parse(&config.launcher_hotkey)?, Action::Toggle)];
    if !config.terminal_hotkey.trim().is_empty() {
        bindings.push((parse(&config.terminal_hotkey)?, Action::Terminal));
    }
    for shortcut in config.shortcuts.iter().filter(|shortcut| shortcut.enabled) {
        ensure!(
            !shortcut.application.trim().is_empty(),
            "请选择快捷键要打开的应用"
        );
        bindings.push((
            parse(&shortcut.hotkey)?,
            Action::Application(shortcut.application.clone()),
        ));
    }
    let mut seen = HashMap::new();
    for (key, action) in &bindings {
        ensure!(
            !key.mods.is_empty(),
            "全局快捷键必须包含 Cmd/Win、Ctrl、Alt 或 Shift：{key}"
        );
        ensure!(seen.insert(key.id(), action).is_none(), "快捷键重复：{key}");
    }
    Ok(bindings)
}

/// Register additions before removing old keys. Any failure restores the old set.
pub fn reconcile(
    old: &[HotKey],
    next: &[HotKey],
    mut register: impl FnMut(HotKey) -> Result<()>,
    mut unregister: impl FnMut(HotKey) -> Result<()>,
) -> Result<()> {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let result = (|| {
        for &key in next.iter().filter(|key| !old.contains(key)) {
            register(key).with_context(|| format!("快捷键 {key} 已被占用或系统拒绝注册"))?;
            added.push(key);
        }
        for &key in old.iter().filter(|key| !next.contains(key)) {
            unregister(key)?;
            removed.push(key);
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        for key in added {
            if let Err(error) = unregister(key) {
                failures.push(error.to_string());
            }
        }
        for key in removed {
            if let Err(error) = register(key) {
                failures.push(error.to_string());
            }
        }
        ensure!(
            failures.is_empty(),
            "{error:#}；恢复快捷键失败：{}",
            failures.join("；")
        );
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppShortcut;

    #[test]
    fn visual_choices_roundtrip_aliases_and_validate_combinations() {
        for value in [
            "Option+Space",
            "Cmd+Shift+K",
            "Win+Ctrl+Alt+Shift+F12",
            "Ctrl+Numpad1",
            "Cmd+Equal",
        ] {
            let value_parts = parts(value).unwrap();
            assert_eq!(
                parse(&compose(&value_parts).unwrap()).unwrap(),
                parse(value).unwrap()
            );
            let reversed = value_parts.into_iter().rev().collect::<Vec<_>>();
            assert_eq!(
                parse(&compose(&reversed).unwrap()).unwrap(),
                parse(value).unwrap()
            );
        }
        for invalid in [
            vec!["Cmd", "Super", "K"],
            vec!["Ctrl", "K", "L"],
            vec!["Cmd", "Shift"],
            vec!["K"],
            vec!["Cmd", "unknown"],
        ] {
            assert!(compose(&invalid.into_iter().map(str::to_string).collect::<Vec<_>>()).is_err());
        }
        assert_eq!(compose(&[]).unwrap(), "");
    }

    #[test]
    fn failed_registration_or_removal_restores_previous_keys() {
        use std::{cell::RefCell, collections::HashSet};
        let a = parse("Ctrl+A").unwrap();
        let b = parse("Ctrl+B").unwrap();
        let c = parse("Ctrl+C").unwrap();
        let d = parse("Ctrl+D").unwrap();
        for removal_failure in [false, true] {
            let registered = RefCell::new(HashSet::from([a, b]));
            let result = reconcile(
                &[a, b],
                &[c, d],
                |key| {
                    if !removal_failure && key == d {
                        anyhow::bail!("occupied");
                    }
                    registered.borrow_mut().insert(key);
                    Ok(())
                },
                |key| {
                    if removal_failure && key == b {
                        anyhow::bail!("removal failed");
                    }
                    registered.borrow_mut().remove(&key);
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert_eq!(*registered.borrow(), HashSet::from([a, b]));
        }
        let registered = RefCell::new(HashSet::from([a, b]));
        reconcile(
            &[a, b],
            &[b, c],
            |key| {
                assert!(registered.borrow_mut().insert(key));
                Ok(())
            },
            |key| {
                assert!(registered.borrow_mut().remove(&key));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*registered.borrow(), HashSet::from([b, c]));
    }

    #[test]
    fn aliases_and_duplicate_bindings() {
        assert_eq!(parse("cmd+K").unwrap(), parse("Super+KeyK").unwrap());
        assert_eq!(parse("Option+Space").unwrap(), parse("Alt+Space").unwrap());
        let mut config = Config::default();
        config.shortcuts.push(AppShortcut {
            hotkey: config.launcher_hotkey.clone(),
            application: "Editor".into(),
            enabled: true,
        });
        assert!(bindings(&config).is_err());
        config.shortcuts[0].enabled = false;
        assert_eq!(bindings(&config).unwrap().len(), 2);
        config.shortcuts[0].enabled = true;
        config.shortcuts[0].hotkey = "Cmd+K".into();
        assert_eq!(
            bindings(&config).unwrap()[2].1,
            Action::Application("Editor".into())
        );
        config.shortcuts[0].hotkey = "K".into();
        assert!(bindings(&config).is_err());
    }
}
