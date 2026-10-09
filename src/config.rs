use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: ThemeName,
    pub monospace_font: String,
    pub launcher_hotkey: String,
    pub auto_check_updates: bool,
    pub clipboard_history: bool,
    pub shortcuts: Vec<AppShortcut>,
    /// Starter-only file associations: extensions without '.', '*' or 'folder'.
    pub open_with: BTreeMap<String, String>,
    pub search_roots: Vec<PathBuf>,
    pub aliases: BTreeMap<String, Vec<String>>,
    pub favorites: BTreeSet<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeName::default(),
            monospace_font: if cfg!(target_os = "macos") {
                "Menlo"
            } else {
                "Consolas"
            }
            .into(),
            launcher_hotkey: if cfg!(target_os = "macos") {
                "Alt+Space"
            } else {
                "Ctrl+Space"
            }
            .into(),
            auto_check_updates: true,
            clipboard_history: true,
            shortcuts: Vec::new(),
            open_with: BTreeMap::new(),
            search_roots: Vec::new(),
            aliases: BTreeMap::new(),
            favorites: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct AppShortcut {
    pub hotkey: String,
    /// Accept the single-app field used by older configurations.
    #[serde(alias = "application", deserialize_with = "shortcut_applications")]
    pub applications: Vec<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn shortcut_applications<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Applications {
        Single(String),
        Multiple(Vec<String>),
    }
    Ok(match Applications::deserialize(deserializer)? {
        Applications::Single(application) => vec![application],
        Applications::Multiple(applications) => applications,
    })
}

fn enabled() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    #[default]
    Catppuccin,
    Everforest,
    Gruvbox,
    CatppuccinLatte,
}

impl ThemeName {
    pub const ALL: [Self; 4] = [
        Self::Catppuccin,
        Self::Everforest,
        Self::Gruvbox,
        Self::CatppuccinLatte,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Catppuccin => "Catppuccin Mocha",
            Self::Everforest => "Everforest",
            Self::Gruvbox => "Gruvbox",
            Self::CatppuccinLatte => "Catppuccin Latte",
        }
    }
}

pub fn save(path: &Path, config: &Config) -> Result<()> {
    atomic_write(path, toml::to_string_pretty(config)?.as_bytes())
}

pub fn config_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("Cannot locate the system configuration directory")?
        .join("starter/config.toml"))
}

pub fn load_or_create() -> Result<(Config, PathBuf)> {
    let path = config_path()?;
    if !path.exists() {
        let config = Config::default();
        let header = "# Starter configuration. Save, then choose Reload configuration in the tray.\n\
            # search_roots = [\"~/Documents\", \"~/code\"]\n\
            # [aliases]\n\
            # \"Visual Studio Code\" = [\"code\", \"vsc\"]\n\n";
        atomic_write(
            &path,
            format!("{header}{}", toml::to_string_pretty(&config)?).as_bytes(),
        )?;
        return Ok((config, path));
    }
    let config = read(&path)?;
    Ok((config, path))
}

pub fn read(path: &Path) -> Result<Config> {
    let text =
        fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let config = toml::from_str::<toml::Table>(&text).and_then(|mut table| {
        // Old terminal settings are retired; terminals use user-created app shortcuts.
        table.remove("terminal");
        table.remove("terminal_hotkey");
        table.try_into()
    });
    config.with_context(|| format!("Invalid configuration in {}", path.display()))
}

pub fn expand_home(path: &Path) -> PathBuf {
    if let Ok(relative) = path.strip_prefix("~")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(relative);
    }
    path.to_path_buf()
}

pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().context("Storage path has no parent")?;
    fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp, data)?;
    fs::rename(&temp, path).with_context(|| format!("Cannot save {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_app_shortcuts_migrate_to_application_lists() {
        let legacy: Config = toml::from_str(
            "[[shortcuts]]\nhotkey = 'Cmd+Enter'\napplication = 'kitty'\nenabled = false",
        )
        .unwrap();
        assert_eq!(legacy.shortcuts[0].applications, vec!["kitty"]);
        assert!(!legacy.shortcuts[0].enabled);
        let saved = toml::to_string_pretty(&legacy).unwrap();
        assert!(saved.contains("applications = ["));
        assert!(!saved.contains("application ="));
        let migrated: Config = toml::from_str(&saved).unwrap();
        assert_eq!(migrated.shortcuts, legacy.shortcuts);

        let group: Config = toml::from_str(
            "[[shortcuts]]\nhotkey = 'Cmd+Enter'\napplications = ['kitty', 'ChatGPT']",
        )
        .unwrap();
        assert_eq!(group.shortcuts[0].applications, vec!["kitty", "ChatGPT"]);
        assert!(group.shortcuts[0].enabled);
        assert!(toml::from_str::<Config>(
            "[[shortcuts]]\nhotkey = 'Cmd+Enter'\napplication = 'kitty'\napplications = ['ChatGPT']"
        ).is_err());
    }

    #[test]
    fn settings_save_roundtrip_and_legacy_defaults() {
        let path = std::env::temp_dir()
            .join(format!("starter-settings-{}", std::process::id()))
            .join("config.toml");
        let mut config = Config {
            theme: ThemeName::Everforest,
            auto_check_updates: false,
            clipboard_history: false,
            search_roots: vec![PathBuf::from("~/code")],
            ..Config::default()
        };
        config
            .aliases
            .insert("Terminal".into(), vec!["终端".into(), "term".into()]);
        config
            .favorites
            .insert("app:/Applications/Terminal.app".into());
        config.open_with.insert("md".into(), "Editor".into());
        config.shortcuts.push(AppShortcut {
            hotkey: "Cmd+K".into(),
            applications: vec!["Editor".into(), "Browser".into()],
            enabled: false,
        });
        save(&path, &config).unwrap();
        let saved = read(&path).unwrap();
        assert_eq!(saved.open_with, config.open_with);
        assert_eq!(saved.shortcuts, config.shortcuts);
        assert_eq!(saved.theme, ThemeName::Everforest);
        assert!(!saved.auto_check_updates);
        assert!(!saved.clipboard_history);
        assert_eq!(saved.search_roots, config.search_roots);
        assert_eq!(saved.aliases, config.aliases);
        assert_eq!(saved.favorites, config.favorites);
        config.theme = ThemeName::Gruvbox;
        save(&path, &config).unwrap();
        assert_eq!(read(&path).unwrap().theme, ThemeName::Gruvbox);
        fs::write(&path, "terminal = 'kitty'\nterminal_hotkey = 'Alt+Enter'").unwrap();
        let old = read(&path).unwrap();
        assert_eq!(old.theme, ThemeName::Catppuccin);
        assert!(old.auto_check_updates);
        assert!(old.clipboard_history);
        assert!(old.open_with.is_empty());
        assert!(old.shortcuts.is_empty());
        assert_eq!(crate::hotkeys::bindings(&old).unwrap().len(), 1);
        save(&path, &old).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("terminal"));
        fs::write(&path, "unknown_setting = true").unwrap();
        assert!(
            read(&path).is_err(),
            "other unknown fields must still be rejected"
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
