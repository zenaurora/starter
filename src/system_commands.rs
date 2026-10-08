//! A typed command catalogue: searchable names, action semantics and platform
//! destinations live together. User input is never interpreted as a shell command.
use crate::search::{Candidate, Kind};
use anyhow::Result;
use std::path::PathBuf;

mod native;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Lock,
    Sleep,
    Restart,
    Shutdown,
}

impl Power {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lock => "锁屏",
            Self::Sleep => "睡眠",
            Self::Restart => "重启",
            Self::Shutdown => "关机",
        }
    }
    pub fn requires_confirmation(self) -> bool {
        matches!(self, Self::Restart | Self::Shutdown)
    }
    pub fn execute(self) -> Result<()> {
        native::power(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Bluetooth,
    Display,
    Sound,
    Battery,
    Wifi,
    Network,
    Keyboard,
    Notifications,
    Privacy,
}
impl Setting {
    pub fn url(self, macos: bool) -> &'static str {
        if macos {
            // Legacy pane identifiers also redirect to the corresponding modern
            // System Settings pages, and keep macOS 12 support.
            match self {
                Self::Bluetooth => "x-apple.systempreferences:com.apple.preferences.Bluetooth",
                Self::Display => "x-apple.systempreferences:com.apple.preference.displays",
                Self::Sound => "x-apple.systempreferences:com.apple.preference.sound",
                Self::Battery => "x-apple.systempreferences:com.apple.preference.battery",
                Self::Wifi | Self::Network => {
                    "x-apple.systempreferences:com.apple.preference.network"
                }
                Self::Keyboard => "x-apple.systempreferences:com.apple.preference.keyboard",
                Self::Notifications => {
                    "x-apple.systempreferences:com.apple.preference.notifications"
                }
                Self::Privacy => "x-apple.systempreferences:com.apple.preference.security?Privacy",
            }
        } else {
            match self {
                Self::Bluetooth => "ms-settings:bluetooth",
                Self::Display => "ms-settings:display",
                Self::Sound => "ms-settings:sound",
                Self::Battery => "ms-settings:batterysaver",
                Self::Wifi => "ms-settings:network-wifi",
                Self::Network => "ms-settings:network-status",
                Self::Keyboard => "ms-settings:typing",
                Self::Notifications => "ms-settings:notifications",
                Self::Privacy => "ms-settings:privacy",
            }
        }
    }
    pub fn open(self) -> Result<()> {
        native::open_settings(self.url(cfg!(target_os = "macos")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Power(Power),
    Settings(Setting),
    Reminders,
}

struct Command {
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    aliases: &'static [&'static str],
    action: Action,
}

const COMMANDS: &[Command] = &[
    Command {
        id: "lock",
        title: "锁屏",
        detail: "立即锁定当前会话",
        aliases: &["lock", "lock screen", "锁定", "锁定屏幕"],
        action: Action::Power(Power::Lock),
    },
    Command {
        id: "sleep",
        title: "睡眠",
        detail: "让电脑进入睡眠，唤醒后继续工作",
        aliases: &["sleep", "suspend", "休眠", "睡眠模式"],
        action: Action::Power(Power::Sleep),
    },
    Command {
        id: "restart",
        title: "重启",
        detail: "重启电脑 · 执行前确认",
        aliases: &["restart", "reboot", "重新启动"],
        action: Action::Power(Power::Restart),
    },
    Command {
        id: "shutdown",
        title: "关机",
        detail: "关闭电脑 · 执行前确认",
        aliases: &["shutdown", "shut down", "power off", "关闭电脑"],
        action: Action::Power(Power::Shutdown),
    },
    Command {
        id: "bluetooth",
        title: "蓝牙设置",
        detail: "打开系统的蓝牙设置",
        aliases: &["bluetooth", "蓝牙", "蓝牙设置"],
        action: Action::Settings(Setting::Bluetooth),
    },
    Command {
        id: "display",
        title: "显示器设置",
        detail: "分辨率、亮度与外接显示器",
        aliases: &[
            "display",
            "displays",
            "screen",
            "显示",
            "显示器",
            "屏幕",
            "亮度",
            "分辨率",
        ],
        action: Action::Settings(Setting::Display),
    },
    Command {
        id: "sound",
        title: "声音设置",
        detail: "输出设备、输入设备与音量",
        aliases: &["sound", "audio", "volume", "声音", "音量", "麦克风"],
        action: Action::Settings(Setting::Sound),
    },
    Command {
        id: "battery",
        title: "电池设置",
        detail: "电池、电源与节能选项",
        aliases: &["battery", "power", "电池", "电源", "节能"],
        action: Action::Settings(Setting::Battery),
    },
    Command {
        id: "wifi",
        title: "Wi-Fi 设置",
        detail: "打开网络连接设置",
        aliases: &["wifi", "wi-fi", "无线", "无线网络"],
        action: Action::Settings(Setting::Wifi),
    },
    Command {
        id: "network",
        title: "网络设置",
        detail: "网络连接与配置",
        aliases: &["network", "网络", "网络设置"],
        action: Action::Settings(Setting::Network),
    },
    Command {
        id: "keyboard",
        title: "键盘设置",
        detail: "输入法与系统键盘选项",
        aliases: &["keyboard", "typing", "键盘", "输入法"],
        action: Action::Settings(Setting::Keyboard),
    },
    Command {
        id: "notifications",
        title: "通知设置",
        detail: "管理系统通知与 Starter 提醒通知",
        aliases: &["notifications", "notification", "通知", "通知设置"],
        action: Action::Settings(Setting::Notifications),
    },
    Command {
        id: "privacy",
        title: "隐私设置",
        detail: "打开系统隐私权限设置",
        aliases: &["privacy", "permissions", "隐私", "权限"],
        action: Action::Settings(Setting::Privacy),
    },
    Command {
        id: "reminder",
        title: "快速提醒",
        detail: "开会、休息或待办 · 设置时间后提醒",
        aliases: &["reminder", "remind", "提醒", "会议提醒", "开会提醒"],
        action: Action::Reminders,
    },
];

pub fn candidates() -> Vec<Candidate> {
    COMMANDS
        .iter()
        .map(|command| Candidate {
            id: format!("command:{}", command.id),
            title: command.title.into(),
            detail: command.detail.into(),
            aliases: command.aliases.iter().map(|s| (*s).into()).collect(),
            path: PathBuf::new(),
            kind: match command.action {
                Action::Settings(_) => Kind::SystemSetting,
                Action::Reminders => Kind::ReminderDraft,
                Action::Power(_) => Kind::SystemCommand,
            },
        })
        .collect()
}
pub fn resolve(id: &str) -> Option<Action> {
    let id = id.strip_prefix("command:")?;
    COMMANDS.iter().find(|c| c.id == id).map(|c| c.action)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn english_and_chinese_find_the_expected_action() {
        let candidates = candidates();
        for (query, action) in [
            ("bluetooth", Action::Settings(Setting::Bluetooth)),
            ("display", Action::Settings(Setting::Display)),
            ("sound", Action::Settings(Setting::Sound)),
            ("battery", Action::Settings(Setting::Battery)),
            ("重启", Action::Power(Power::Restart)),
            ("Lock", Action::Power(Power::Lock)),
            ("Reminder", Action::Reminders),
        ] {
            let results = crate::search::rank(&candidates, query, &Default::default());
            assert_eq!(resolve(&results[0].id), Some(action));
        }
        assert!(Power::Restart.requires_confirmation());
        assert!(Power::Shutdown.requires_confirmation());
        assert!(!Power::Lock.requires_confirmation());
        assert!(!Power::Sleep.requires_confirmation());
        assert_eq!(resolve("command:shutdown && arbitrary"), None);
    }
    #[test]
    fn settings_use_platform_specific_urls() {
        assert_eq!(Setting::Bluetooth.url(false), "ms-settings:bluetooth");
        assert!(
            Setting::Display
                .url(true)
                .starts_with("x-apple.systempreferences:")
        );
        assert_eq!(Setting::Sound.url(false), "ms-settings:sound");
        assert_eq!(Setting::Battery.url(false), "ms-settings:batterysaver");
    }
}
