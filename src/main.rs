#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod appearance;
mod icons;
mod platform;
mod settings;
mod ui;
mod worker;

use gpui_kit::*;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        AppWindow,
        FolderSearch,
        FolderPlus,
        Keyboard,
        Tag,
        Command,
        CornerDownLeft,
        StarFill,
        Clock
    ]
);

struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        Ok(paths)
    }
}

fn main() {
    if starter::updates::run_helper() {
        return;
    }
    gpui_kit::application().with_assets(AppAssets).run(|cx| {
        gpui_kit::init(cx);
        // 配置启动器窗口：固定尺寸、无标题栏的悬浮弹出窗口
        let options = WindowOptions {
            // 窗口大小 720x560，并在屏幕上居中显示
            window_bounds: Some(WindowBounds::centered(size(px(720.), px(560.)), cx)),
            // 不使用系统标题栏（采用自定义标题栏）
            titlebar: None,
            // macOS 菜单级弹窗会遮住输入法候选框，使用可激活的浮动窗口。
            kind: if cfg!(target_os = "macos") {
                WindowKind::Floating
            } else {
                WindowKind::PopUp
            },
            // 禁止用户调整窗口大小
            is_resizable: false,
            // 禁止最小化窗口
            is_minimizable: false,
            // 应用标识符，用于平台层关联窗口与任务栏
            app_id: Some("dev.starter.launcher".into()),
            // 其余字段使用默认值
            ..Default::default()
        };
        match gpui_kit::open_window(options, cx, |window, cx| {
            platform::configure_launcher_window(window);
            cx.new(|cx| ui::Launcher::new(window, cx))
        }) {
            Ok(_) => {
                starter::updates::acknowledge_startup();
                cx.set_activation_policy(ActivationPolicy::Accessory);
                cx.activate(true);
            }
            Err(error) => {
                eprintln!("Cannot open Starter: {error:#}");
                cx.quit();
            }
        }
    });
}
