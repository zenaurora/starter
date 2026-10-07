use super::*;
use anyhow::{Context, ensure};
use objc2::{
    class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
};
use objc2_foundation::{NSArray, NSFileManager, NSURL};
use std::{fs, os::unix::fs::MetadataExt, path::Path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    path: PathBuf,
    device: u64,
    inode: u64,
}

fn validate(path: &Path, current: &Path, roots: &[PathBuf]) -> Result<Action> {
    let canonical = path.canonicalize().context("应用已不存在")?;
    ensure!(canonical == path, "不支持卸载符号链接或重定向路径");
    ensure!(
        roots
            .iter()
            .any(|root| canonical.starts_with(root) && canonical != *root),
        "仅支持 /Applications 和用户 Applications 中的应用"
    );
    ensure!(
        !canonical.starts_with("/System") && !canonical.starts_with("/Library/Apple"),
        "系统应用不可卸载"
    );
    ensure!(
        canonical
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app")),
        "目标不是应用包"
    );
    ensure!(
        !current.starts_with(&canonical),
        "不能卸载正在运行的 Starter"
    );
    ensure!(
        canonical
            .file_stem()
            .is_none_or(|name| !name.eq_ignore_ascii_case("Starter")),
        "不能从 Starter 内卸载自身"
    );
    ensure!(
        canonical.join("Contents/Info.plist").is_file(),
        "目标不是有效应用包"
    );
    let metadata = fs::symlink_metadata(&canonical)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "目标不是应用目录"
    );
    Ok(Action {
        path: canonical,
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
    }
    roots
}

pub fn discover(catalog: &Catalog) -> Vec<Target> {
    let Ok(current) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return vec![];
    };
    let roots = roots();
    catalog
        .apps
        .iter()
        .filter_map(|app| {
            let action = validate(&app.path, &current, &roots).ok()?;
            Some(Target {
                id: app.id.clone(),
                name: app.name.clone(),
                path: app.path.clone(),
                aliases: app.aliases.clone(),
                action,
            })
        })
        .collect()
}

pub fn description(target: &Target) -> String {
    format!(
        "将 {} 移到废纸篓，可在废纸篓中恢复。\n\n{}\n\n请先退出这个应用。应用的文稿和个人设置会保留。",
        target.name,
        target.path.display()
    )
}

pub fn execute(target: &Target) -> Result<String> {
    let current = std::env::current_exe()?.canonicalize()?;
    ensure!(
        validate(&target.path, &current, &roots())? == target.action,
        "应用路径已变化，请刷新列表后重试"
    );
    trash_bundle(&target.action.path, &target.name)?;
    Ok(format!("{} 已移到废纸篓", target.name))
}

fn trash_bundle(path: &Path, name: &str) -> Result<PathBuf> {
    autoreleasepool(|_| {
        let url = NSURL::from_file_path(path).context("应用路径无效")?;
        // Refuse to trash a running bundle; never force-quit another application.
        unsafe {
            let workspace: Retained<AnyObject> = msg_send![class!(NSWorkspace), sharedWorkspace];
            let running: Retained<NSArray<AnyObject>> = msg_send![&workspace, runningApplications];
            for app in running.iter() {
                let bundle: Option<Retained<NSURL>> = msg_send![&app, bundleURL];
                ensure!(
                    bundle.as_ref().is_none_or(|b| b.path() != url.path()),
                    "请先退出 {}，再重试卸载",
                    name
                );
            }
        }
        let mut destination = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut destination))
            .map_err(|error| {
                anyhow::anyhow!("移到废纸篓失败：{error}。若权限不足，请在 Finder 中操作。")
            })?;
        let destination = destination.context("废纸篓位置未返回")?;
        use std::{ffi::CStr, os::unix::ffi::OsStrExt};
        let path = unsafe { CStr::from_ptr(destination.fileSystemRepresentation().as_ptr()) };
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_identity_and_blocks_self_links_and_non_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let app = root.join("Example.app");
        fs::create_dir_all(app.join("Contents")).unwrap();
        fs::write(app.join("Contents/Info.plist"), "fixture").unwrap();
        let roots = vec![root.clone()];
        let current = root.join("Starter.app/Contents/MacOS/starter");
        let original = validate(&app, &current, &roots).unwrap();
        assert!(validate(&app, &app.join("Contents/MacOS/app"), &roots).is_err());
        assert!(validate(&app, &current, &[root.join("elsewhere")]).is_err());
        let link = root.join("Linked.app");
        std::os::unix::fs::symlink(&app, &link).unwrap();
        assert!(validate(&link, &current, &roots).is_err());
        fs::rename(&app, root.join("Previous.app")).unwrap();
        fs::create_dir_all(app.join("Contents")).unwrap();
        fs::write(app.join("Contents/Info.plist"), "replacement").unwrap();
        assert_ne!(original, validate(&app, &current, &roots).unwrap());
        fs::remove_file(app.join("Contents/Info.plist")).unwrap();
        assert!(validate(&app, &current, &roots).is_err());
    }

    #[test]
    fn native_trash_is_recoverable_and_preserves_bundle_contents() {
        // Only the app bundle created in this temporary fixture is touched.
        let dir = tempfile::tempdir().unwrap();
        let app = dir
            .path()
            .join(format!("Starter-fixture-{}.app", std::process::id()));
        fs::create_dir_all(app.join("Contents")).unwrap();
        fs::write(app.join("Contents/Info.plist"), b"fixture contents").unwrap();
        let destination = trash_bundle(&app, "Disposable test fixture").unwrap();
        assert!(!app.exists());
        assert_eq!(
            fs::read(destination.join("Contents/Info.plist")).unwrap(),
            b"fixture contents"
        );
        fs::rename(destination, &app).unwrap();
        assert!(app.exists(), "the trashed bundle must be restorable");
    }
}
