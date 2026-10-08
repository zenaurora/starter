use anyhow::Result;
#[cfg(target_os = "macos")]
use anyhow::bail;
use async_channel::{Receiver, Sender};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use gpui_kit::{App, Window};
use starter::{
    config::Config,
    hotkeys::{self, Action},
    opening,
};
use std::{path::Path, process::Command};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

#[derive(Clone, Debug)]
pub enum Event {
    Toggle,
    Terminal,
    Application(String),
    OpenSettings,
    OpenReminders,
    OpenSystem,
    OpenConfig,
    Reload,
    Refresh,
    Quit,
}

/// Own OS resources on GPUI's main thread, including when the window is hidden.
pub struct Shell {
    manager: GlobalHotKeyManager,
    bindings: Vec<(HotKey, Action)>,
    _tray: TrayIcon,
}

pub struct ShellSetup {
    pub shell: Shell,
    pub warning: Option<String>,
    pub hotkey_events: Receiver<u32>,
}

impl Shell {
    pub fn start(config: &Config, sender: Sender<Event>) -> Result<ShellSetup> {
        let manager = GlobalHotKeyManager::new()?;
        let menu = Menu::new();
        let items = [
            (MenuItem::new("打开 Starter", true, None), Event::Toggle),
            (MenuItem::new("打开终端", true, None), Event::Terminal),
            (MenuItem::new("快速提醒…", true, None), Event::OpenReminders),
            (
                MenuItem::new("系统命令与设置…", true, None),
                Event::OpenSystem,
            ),
            (MenuItem::new("设置…", true, None), Event::OpenSettings),
            (MenuItem::new("打开配置文件", true, None), Event::OpenConfig),
            (MenuItem::new("重新加载配置", true, None), Event::Reload),
            (
                MenuItem::new("刷新应用与文件列表", true, None),
                Event::Refresh,
            ),
            (MenuItem::new("退出 Starter", true, None), Event::Quit),
        ];
        for (i, (item, _)) in items.iter().enumerate() {
            if i == 7 {
                menu.append(&PredefinedMenuItem::separator())?;
            }
            menu.append(item)?;
        }
        let mapping: Vec<_> = items
            .iter()
            .map(|(item, event)| (item.id().clone(), event.clone()))
            .collect();
        let menu_sender = sender.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, action)) = mapping.iter().find(|(id, _)| *id == event.id) {
                let _ = menu_sender.try_send(action.clone());
            }
        }));
        let tray = TrayIconBuilder::new()
            .with_tooltip("Starter")
            .with_menu(Box::new(menu));
        #[cfg(target_os = "macos")]
        let tray = tray.with_icon_templated(tray_icon()?);
        #[cfg(not(target_os = "macos"))]
        let tray = tray.with_icon(tray_icon()?);
        let tray = tray.build()?;
        // Route by the active hotkey IDs at receipt time, so reloading config works.
        let (key_sender, key_receiver) = async_channel::unbounded();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed {
                let _ = key_sender.try_send(event.id);
            }
        }));
        let mut shell = Self {
            manager,
            bindings: Vec::new(),
            _tray: tray,
        };
        let warning = shell.rebind(config).err().map(|e| {
            format!("全局快捷键绑定失败：{e:#}。请打开配置文件修改，或暂时停用占用快捷键的应用。")
        });
        Ok(ShellSetup {
            shell,
            warning,
            hotkey_events: key_receiver,
        })
    }

    pub fn event_for(&self, id: u32) -> Option<Event> {
        self.bindings
            .iter()
            .find(|(key, _)| key.id() == id)
            .map(|(_, action)| match action {
                Action::Toggle => Event::Toggle,
                Action::Terminal => Event::Terminal,
                Action::Application(app) => Event::Application(app.clone()),
            })
    }

    pub fn rebind(&mut self, config: &Config) -> Result<()> {
        let next = hotkeys::bindings(config)?;
        self.change_keys(
            &self.keys(),
            &next.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        )?;
        self.bindings = next;
        Ok(())
    }

    fn keys(&self) -> Vec<HotKey> {
        self.bindings.iter().map(|(key, _)| *key).collect()
    }

    fn change_keys(&self, old: &[HotKey], next: &[HotKey]) -> Result<()> {
        hotkeys::reconcile(
            old,
            next,
            |key| self.manager.register(key).map_err(Into::into),
            |key| self.manager.unregister(key).map_err(Into::into),
        )
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.manager.unregister_all(&self.keys());
    }
}

fn tray_icon() -> Result<Icon> {
    #[cfg(target_os = "macos")]
    let bytes = include_bytes!("../resources/icons/tray-template.png").as_slice();
    #[cfg(not(target_os = "macos"))]
    let bytes = include_bytes!("../resources/icons/starter-64.png").as_slice();
    // macOS displays this 72 px template at 18 pt, including on Retina screens.
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?.into_rgba8();
    let (width, height) = image.dimensions();
    Ok(Icon::from_rgba(image.into_raw(), width, height)?)
}

pub fn open_target(path: &Path) -> Result<()> {
    opening::open(path, None)
}

pub fn open_terminal(config: &Config) -> Result<()> {
    opening::launch(&config.terminal)
}

pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("/usr/bin/open").arg("-R").arg(path).status()?;
        if !status.success() {
            bail!("无法在 Finder 中定位文件");
        }
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
            .spawn()?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    open::that(path.parent().unwrap_or(path))?;
    Ok(())
}

pub fn hide(window: &mut Window, cx: &mut App) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            // GPUI owns this HWND for the lifetime of this callback. Only visibility changes.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                    handle.hwnd.get() as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
                );
            }
        }
        let _ = cx;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        cx.hide();
    }
}

pub fn show(window: &mut Window, cx: &mut App) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            // Restore visibility before asking GPUI to activate its owned HWND.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                    handle.hwnd.get() as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW,
                );
            }
        }
    }
    cx.activate(true);
    window.activate_window();
}
