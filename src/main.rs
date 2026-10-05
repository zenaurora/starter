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
    gpui_kit::application().with_assets(AppAssets).run(|cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(720.), px(560.)), cx)),
            titlebar: None,
            kind: WindowKind::PopUp,
            is_resizable: false,
            is_minimizable: false,
            app_id: Some("dev.starter.launcher".into()),
            ..Default::default()
        };
        match gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| ui::Launcher::new(window, cx))
        }) {
            Ok(_) => {
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
