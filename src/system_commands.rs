//! A typed command catalogue: searchable names, action semantics and platform
//! destinations live together. User input is never interpreted as a shell command.
use crate::search::{Candidate, Kind, Mode};
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
    Monitor,
    SoftwareUpdates,
    Saved,
    Todos,
    Ports,
    Search(Mode),
    Preferences,
}

struct Command {
    id: &'static str,
    shortcut: &'static str,
    title: &'static str,
    detail: &'static str,
    aliases: &'static [&'static str],
    action: Action,
}

const COMMANDS: &[Command] = &[
    Command {
        id: "saved",
        shortcut: "/save",
        title: "常用收藏",
        detail: "直接输入或一键收藏剪贴板，随时搜索和复制",
        aliases: &[
            "saved",
            "snippets",
            "收藏",
            "常用文本",
            "发票",
            "账号",
            "好友码",
            "论文",
        ],
        action: Action::Saved,
    },
    Command {
        id: "todos",
        shortcut: "/todo",
        title: "待办",
        detail: "快速记一件事，勾选完成；收藏也能加入待办",
        aliases: &["todos", "待办事项", "任务", "阅读清单"],
        action: Action::Todos,
    },
    Command {
        id: "ports",
        shortcut: "/ports",
        title: "端口排查",
        detail: "查找 TCP / UDP 端口占用，查看进程与程序路径",
        aliases: &[
            "/port",
            "ports",
            "port",
            "端口",
            "占用",
            "address already in use",
            "EADDRINUSE",
        ],
        action: Action::Ports,
    },
    #[cfg(target_os = "macos")]
    Command {
        id: "software-updates",
        shortcut: "/update",
        title: "软件更新",
        detail: "查看安装来源和软件版本，选择项目并确认更新计划",
        aliases: &[
            "update",
            "updates",
            "upgrade",
            "brew",
            "软件更新",
            "本机更新",
            "更新软件",
        ],
        action: Action::SoftwareUpdates,
    },
    Command {
        id: "clipboard",
        shortcut: "/clip",
        title: "剪贴板历史",
        detail: "搜索和复制之前复制过的文本、图片、链接与文件",
        aliases: &["/clipboard", "clipboard", "剪贴板", "复制历史"],
        action: Action::Search(Mode::Clipboard),
    },
    Command {
        id: "files",
        shortcut: "/f",
        title: "文件搜索",
        detail: "按名称查找已配置目录中的文件和文件夹",
        aliases: &["files", "file", "文件", "文件夹"],
        action: Action::Search(Mode::Files),
    },
    Command {
        id: "content",
        shortcut: "/c",
        title: "文件内容搜索",
        detail: "在已配置目录中查找文件内的文本",
        aliases: &["content", "文本内容", "全文搜索"],
        action: Action::Search(Mode::Content),
    },
    Command {
        id: "uninstall",
        shortcut: "/uninstall",
        title: "卸载应用",
        detail: "选择应用，查看卸载方式并确认",
        aliases: &["卸载", "移除应用"],
        action: Action::Search(Mode::Uninstall),
    },
    Command {
        id: "system",
        shortcut: "/system",
        title: "系统命令与设置",
        detail: "查找系统操作和蓝牙、显示、网络等设置入口",
        aliases: &["/settings", "settings", "系统", "系统设置"],
        action: Action::Search(Mode::System),
    },
    Command {
        id: "reminder",
        shortcut: "/remind",
        title: "快速提醒",
        detail: "开会、休息或待办 · 设置时间后提醒",
        aliases: &["/reminder", "reminder", "提醒", "会议提醒", "开会提醒"],
        action: Action::Reminders,
    },
    Command {
        id: "monitor",
        shortcut: "/monitor",
        title: "系统监控",
        detail: "查看 CPU、内存、磁盘和应用资源占用",
        aliases: &[
            "monitor",
            "activity monitor",
            "system monitor",
            "监控",
            "资源",
            "活动监视器",
        ],
        action: Action::Monitor,
    },
    Command {
        id: "preferences",
        shortcut: "/preferences",
        title: "Starter 设置",
        detail: "管理快捷键、搜索目录、外观和剪贴板记录",
        aliases: &["preferences", "starter settings", "偏好设置", "软件设置"],
        action: Action::Preferences,
    },
    Command {
        id: "lock",
        shortcut: "/lock",
        title: "锁屏",
        detail: "立即锁定当前会话",
        aliases: &["lock", "lock screen", "锁定", "锁定屏幕"],
        action: Action::Power(Power::Lock),
    },
    Command {
        id: "sleep",
        shortcut: "/sleep",
        title: "睡眠",
        detail: "让电脑进入睡眠，唤醒后继续工作",
        aliases: &["sleep", "suspend", "休眠", "睡眠模式"],
        action: Action::Power(Power::Sleep),
    },
    Command {
        id: "restart",
        shortcut: "/restart",
        title: "重启",
        detail: "重启电脑 · 执行前确认",
        aliases: &["restart", "reboot", "重新启动"],
        action: Action::Power(Power::Restart),
    },
    Command {
        id: "shutdown",
        shortcut: "/shutdown",
        title: "关机",
        detail: "关闭电脑 · 执行前确认",
        aliases: &["shutdown", "shut down", "power off", "关闭电脑"],
        action: Action::Power(Power::Shutdown),
    },
    Command {
        id: "bluetooth",
        shortcut: "/bluetooth",
        title: "蓝牙设置",
        detail: "打开系统的蓝牙设置",
        aliases: &["bluetooth", "蓝牙", "蓝牙设置"],
        action: Action::Settings(Setting::Bluetooth),
    },
    Command {
        id: "display",
        shortcut: "/display",
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
        shortcut: "/sound",
        title: "声音设置",
        detail: "输出设备、输入设备与音量",
        aliases: &["sound", "audio", "volume", "声音", "音量", "麦克风"],
        action: Action::Settings(Setting::Sound),
    },
    Command {
        id: "battery",
        shortcut: "/battery",
        title: "电池设置",
        detail: "电池、电源与节能选项",
        aliases: &["battery", "power", "电池", "电源", "节能"],
        action: Action::Settings(Setting::Battery),
    },
    Command {
        id: "wifi",
        shortcut: "/wifi",
        title: "Wi-Fi 设置",
        detail: "打开网络连接设置",
        aliases: &["wifi", "wi-fi", "无线", "无线网络"],
        action: Action::Settings(Setting::Wifi),
    },
    Command {
        id: "network",
        shortcut: "/network",
        title: "网络设置",
        detail: "网络连接与配置",
        aliases: &["network", "网络", "网络设置"],
        action: Action::Settings(Setting::Network),
    },
    Command {
        id: "keyboard",
        shortcut: "/keyboard",
        title: "键盘设置",
        detail: "输入法与系统键盘选项",
        aliases: &["keyboard", "typing", "键盘", "输入法"],
        action: Action::Settings(Setting::Keyboard),
    },
    Command {
        id: "notifications",
        shortcut: "/notifications",
        title: "通知设置",
        detail: "管理系统通知与 Starter 提醒通知",
        aliases: &["notifications", "notification", "通知", "通知设置"],
        action: Action::Settings(Setting::Notifications),
    },
    Command {
        id: "privacy",
        shortcut: "/privacy",
        title: "隐私设置",
        detail: "打开系统隐私权限设置",
        aliases: &["privacy", "permissions", "隐私", "权限"],
        action: Action::Settings(Setting::Privacy),
    },
];

pub fn candidates() -> &'static [Candidate] {
    // Builtins never change during a process lifetime. Build their owned search
    // text once instead of allocating every name/alias on each input event.
    static CANDIDATES: std::sync::LazyLock<Vec<Candidate>> = std::sync::LazyLock::new(|| {
        COMMANDS
            .iter()
            .map(|command| {
                let mut aliases: Vec<_> = command
                    .aliases
                    .iter()
                    .copied()
                    .chain([command.shortcut, &command.shortcut[1..]])
                    .map(str::to_owned)
                    .collect();
                aliases.sort_unstable();
                aliases.dedup();
                Candidate {
                    id: format!("command:{}", command.id),
                    title: command.title.into(),
                    detail: command.detail.into(),
                    aliases,
                    path: PathBuf::new(),
                    kind: match command.action {
                        Action::Settings(_) | Action::Preferences => Kind::SystemSetting,
                        Action::Reminders => Kind::ReminderDraft,
                        Action::Monitor => Kind::Monitor,
                        Action::Search(Mode::Files) => Kind::Folder,
                        Action::Search(Mode::Content) => Kind::Content,
                        Action::Search(Mode::Clipboard) => Kind::ClipboardText,
                        Action::Search(Mode::Uninstall) => Kind::Uninstall,
                        Action::Search(_) => Kind::SystemCommand,
                        Action::Power(_)
                        | Action::SoftwareUpdates
                        | Action::Saved
                        | Action::Todos
                        | Action::Ports => Kind::SystemCommand,
                    },
                }
            })
            .collect()
    });
    &CANDIDATES
}
pub fn resolve(id: &str) -> Option<Action> {
    command(id).map(|c| c.action)
}

fn command(id: &str) -> Option<&'static Command> {
    let id = id.strip_prefix("command:")?;
    COMMANDS.iter().find(|c| c.id == id)
}

pub fn shortcut(id: &str) -> Option<&'static str> {
    command(id).map(|c| c.shortcut)
}

pub fn candidate(action: Action) -> Option<&'static Candidate> {
    COMMANDS
        .iter()
        .position(|c| c.action == action)
        .map(|index| &candidates()[index])
}

/// Exact command token only; arguments retain their case and internal whitespace.
pub fn parse(input: &str) -> Option<(Action, &str)> {
    let input = input.trim_start();
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    let token = &input[..end];
    if !token.starts_with('/') {
        return None;
    }
    COMMANDS
        .iter()
        .find(|c| {
            token.eq_ignore_ascii_case(c.shortcut)
                || c.aliases
                    .iter()
                    .any(|alias| alias.starts_with('/') && token.eq_ignore_ascii_case(alias))
        })
        .map(|c| (c.action, input[end..].trim()))
}

/// Slash input searches only builtins, without ranking the installed app catalogue.
/// A second slash or backslash in the first token identifies a path, not a command.
pub fn palette(input: &str) -> Option<Vec<Candidate>> {
    let query = input.trim_start().strip_prefix('/')?.trim();
    let token = query.split_whitespace().next().unwrap_or_default();
    if token.contains(['/', '\\']) {
        return None;
    }
    Some(if query.is_empty() {
        candidates().to_vec()
    } else {
        crate::search::rank(candidates(), query, &Default::default())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogue_drives_slash_discovery_and_exact_parsing() {
        let all = palette(" / ").unwrap();
        assert_eq!(all.len(), COMMANDS.len());
        let mut ids = std::collections::BTreeSet::new();
        let mut shortcuts = std::collections::BTreeSet::new();
        for (command, candidate) in COMMANDS.iter().zip(&all) {
            assert!(ids.insert(&candidate.id));
            assert!(shortcuts.insert(command.shortcut));
            assert_eq!(resolve(&candidate.id), Some(command.action));
            assert_eq!(shortcut(&candidate.id), Some(command.shortcut));
            assert_eq!(parse(command.shortcut), Some((command.action, "")));
            for alias in command.aliases.iter().filter(|s| s.starts_with('/')) {
                assert_eq!(parse(alias), Some((command.action, "")));
            }
            if let Action::Search(mode) = command.action {
                assert_eq!(crate::search::Query::parse(mode.prefix()).mode, mode);
            }
        }
        for (query, action) in [
            ("/mon", Action::Monitor),
            ("/待办", Action::Todos),
            ("/cli", Action::Search(Mode::Clipboard)),
            ("/发票", Action::Saved),
        ] {
            assert_eq!(resolve(&palette(query).unwrap()[0].id), Some(action));
        }
        assert!(palette("/不存在的功能xyz").unwrap().is_empty());
        assert!(palette("/Users/example/file").is_none());
        assert!(palette("https://example.com/").is_none());
        assert!(palette("ordinary search").is_none());
    }

    #[test]
    fn command_arguments_preserve_text_and_require_a_token_boundary() {
        assert_eq!(
            parse(" /PoRt\t3000  node "),
            Some((Action::Ports, "3000  node"))
        );
        assert_eq!(parse("/TODO 买牛奶"), Some((Action::Todos, "买牛奶")));
        assert_eq!(
            parse("/reminder\t10m 开会"),
            Some((Action::Reminders, "10m 开会"))
        );
        for query in ["/todo-list", "/monitor/file", "/save/example", "todo", ""] {
            assert_eq!(parse(query), None, "{query}");
        }
    }

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
            ("monitor", Action::Monitor),
            ("监控", Action::Monitor),
        ] {
            let results = crate::search::rank(candidates, query, &Default::default());
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
