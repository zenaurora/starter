use super::{Action, Software, Source, UpdateState, providers::Batch};
use crate::catalog::Application;
use std::path::Path;

pub(super) fn append(apps: &[Application], batch: &mut Batch) {
    for app in apps {
        // Reuse the launcher's catalog; never walk the application directories a second time.
        if batch.owns(&app.path) {
            continue;
        }
        let file = app.path.join("Contents/Info.plist");
        if !file.metadata().is_ok_and(|m| m.len() < 1024 * 1024) {
            continue;
        }
        let Ok(plist::Value::Dictionary(info)) = plist::Value::from_file(file) else {
            continue;
        };
        let value = |key| {
            info.get(key)
                .and_then(plist::Value::as_string)
                .unwrap_or_default()
        };
        let bundle = value("CFBundleIdentifier");
        // Starter already has a signed, dedicated updater in Settings.
        if bundle == "dev.starter.launcher" {
            continue;
        }
        let version = [
            value("CFBundleShortVersionString"),
            value("CFBundleVersion"),
        ]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("版本未知");
        let store = app.path.join("Contents/_MASReceipt/receipt").is_file();
        let system = bundle.starts_with("com.apple.") && app.path.starts_with("/System");
        let sparkle = !value("SUFeedURL").is_empty();
        let (source, action, description) = if store {
            (
                Source::AppStore,
                Action::OpenUrl("macappstore://showUpdatesPage".into()),
                "由 App Store 检查和安装，可能需要登录",
            )
        } else if system {
            (
                Source::System,
                Action::OpenUrl(
                    "x-apple.systempreferences:com.apple.Software-Update-Settings.extension".into(),
                ),
                "随 macOS 更新，由系统设置处理授权与重启",
            )
        } else {
            (
                Source::Application,
                Action::OpenApp(app.path.clone()),
                if sparkle {
                    "自动检测 Sparkle 版本 · 由应用自身安装"
                } else {
                    "自动匹配公开版本信息 · 由应用自身安装"
                },
            )
        };
        batch.items.push(Software {
            id: format!("app:{}", app.path.display()),
            name: app.name.clone(),
            source,
            installed: version.into(),
            latest: None,
            state: if system {
                UpdateState::Manual
            } else {
                UpdateState::Unchecked
            },
            pinned: false,
            description: description.into(),
            homepage: String::new(),
            path: Some(app.path.clone()),
            action,
            pin: None,
            search: String::new(),
        });
    }
    // OS upgrades require system authorization and may restart the computer. Always hand them off.
    batch.items.push(Software {
        id: "system:macos".into(),
        name: "macOS 系统更新".into(),
        source: Source::System,
        installed: std::env::consts::OS.into(),
        latest: None,
        state: UpdateState::Manual,
        pinned: false,
        description: "在系统设置中检查更新、确认授权与重启".into(),
        homepage: "https://support.apple.com/macos".into(),
        path: Some(Path::new("/System/Applications/System Settings.app").into()),
        action: Action::OpenUrl(
            "x-apple.systempreferences:com.apple.Software-Update-Settings.extension".into(),
        ),
        pin: None,
        search: String::new(),
    });
}
