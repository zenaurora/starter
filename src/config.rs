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
    pub terminal_hotkey: String,
    /// macOS: application name; Windows: executable path/name. Never a shell string.
    pub terminal: String,
    pub auto_check_updates: bool,
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
            terminal_hotkey: "Alt+Enter".into(),
            terminal: if cfg!(target_os = "macos") {
                "Terminal"
            } else {
                "wt.exe"
            }
            .into(),
            auto_check_updates: true,
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
    pub application: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
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
            # macOS terminal: app name (e.g. Ghostty). Windows terminal: executable path.\n\
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
    toml::from_str(&text).with_context(|| format!("Invalid configuration in {}", path.display()))
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
    fn settings_save_roundtrip_and_legacy_defaults() {
        let path = std::env::temp_dir()
            .join(format!("starter-settings-{}", std::process::id()))
            .join("config.toml");
        let mut config = Config {
            theme: ThemeName::Everforest,
            auto_check_updates: false,
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
            application: "Editor".into(),
            enabled: false,
        });
        save(&path, &config).unwrap();
        let saved = read(&path).unwrap();
        assert_eq!(saved.open_with, config.open_with);
        assert_eq!(saved.shortcuts, config.shortcuts);
        assert_eq!(saved.theme, ThemeName::Everforest);
        assert!(!saved.auto_check_updates);
        assert_eq!(saved.search_roots, config.search_roots);
        assert_eq!(saved.aliases, config.aliases);
        assert_eq!(saved.favorites, config.favorites);
        config.theme = ThemeName::Gruvbox;
        save(&path, &config).unwrap();
        assert_eq!(read(&path).unwrap().theme, ThemeName::Gruvbox);
        let old: Config = toml::from_str("terminal = 'kitty'").unwrap();
        assert_eq!(old.theme, ThemeName::Catppuccin);
        assert_eq!(old.terminal, "kitty");
        assert!(old.auto_check_updates);
        assert!(old.open_with.is_empty());
        assert!(old.shortcuts.is_empty());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
