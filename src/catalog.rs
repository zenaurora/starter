use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub aliases: Vec<String>,
}

#[derive(Default)]
pub struct Catalog {
    pub apps: Vec<Application>,
    pub warnings: Vec<String>,
}

/// Bound recursion and never descend into application bundles or symbolic links.
/// This runs off the UI thread; discovery never launches any application.
pub fn discover() -> Catalog {
    let mut catalog = Catalog::default();
    let mut seen = HashSet::new();
    for root in application_roots() {
        if root.exists() {
            visit(&root, 0, &mut seen, &mut catalog);
        }
    }
    catalog
        .apps
        .sort_by_cached_key(|app| app.name.to_lowercase());
    catalog
}

fn visit(dir: &Path, depth: usize, seen: &mut HashSet<PathBuf>, catalog: &mut Catalog) {
    if depth > 6 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            catalog.warnings.push(format!("{}: {error}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if is_application(&path) {
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(key) {
                continue;
            }
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            catalog.apps.push(Application {
                id: path.to_string_lossy().into_owned(),
                name,
                aliases: localized_names(&path),
                path,
            });
        } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            visit(&path, depth + 1, seen, catalog);
        }
    }
}

/// Keep the bundle filename as the stable display/config name, but also index
/// the app's own display names and Chinese localizations (independent of OS language).
#[cfg(target_os = "macos")]
fn localized_names(path: &Path) -> Vec<String> {
    objc2::rc::autoreleasepool(|_| {
        let contents = path.join("Contents");
        let mut names = plist_names(&contents.join("Info.plist"));
        if let Ok(entries) = fs::read_dir(contents.join("Resources")) {
            for entry in entries.flatten() {
                let path = entry.path();
                let locale = entry.file_name().to_string_lossy().to_lowercase();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "lproj")
                    && (locale == "zh.lproj"
                        || locale.starts_with("zh-")
                        || locale.starts_with("zh_"))
                {
                    names.extend(plist_names(&path.join("InfoPlist.strings")));
                }
            }
        }
        names.sort();
        names.dedup();
        names
    })
}

#[cfg(target_os = "macos")]
fn plist_names(path: &Path) -> Vec<String> {
    use objc2_foundation::{
        NSData, NSDictionary, NSPropertyListMutabilityOptions, NSPropertyListSerialization,
        NSString,
    };

    let Some(path) = path.to_str() else {
        return Vec::new();
    };
    let Some(data) = NSData::dataWithContentsOfFile(&NSString::from_str(path)) else {
        return Vec::new();
    };
    // Foundation handles XML/binary plists and UTF-8/UTF-16 .strings dictionaries.
    // The optional format out-pointer is null; malformed metadata is simply ignored.
    let Ok(plist) = (unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &data,
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    }) else {
        return Vec::new();
    };
    let Some(dictionary) = plist.downcast_ref::<NSDictionary>() else {
        return Vec::new();
    };
    ["CFBundleDisplayName", "CFBundleName"]
        .into_iter()
        .filter_map(|key| {
            dictionary
                .objectForKey(&NSString::from_str(key))?
                .downcast_ref::<NSString>()
                .map(|value| value.to_string())
        })
        .filter(|name| !name.trim().is_empty())
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn localized_names(_: &Path) -> Vec<String> {
    Vec::new()
}

fn is_application(path: &Path) -> bool {
    let extension = path.extension().and_then(|x| x.to_str()).unwrap_or("");
    if cfg!(target_os = "macos") {
        extension.eq_ignore_ascii_case("app") && path.is_dir()
    } else {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "lnk" | "exe" | "appref-ms"
        )
    }
}

fn application_roots() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        let mut roots = vec![
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
            PathBuf::from("/System/Library/CoreServices/Applications"),
        ];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        roots
    } else {
        ["APPDATA", "PROGRAMDATA"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|base| PathBuf::from(base).join("Microsoft/Windows/Start Menu/Programs"))
            .collect()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::{config::Config, search};
    use std::collections::BTreeMap;

    #[test]
    fn chinese_bundle_metadata_is_searchable_without_changing_identity() {
        let root = std::env::temp_dir().join(format!("starter-localized-{}", std::process::id()));
        let bundle = root.join("Example.app");
        let contents = bundle.join("Contents");
        let resources = contents.join("Resources");
        fs::create_dir_all(resources.join("zh-Hans.lproj")).unwrap();
        fs::create_dir_all(resources.join("zh-Hant.lproj")).unwrap();
        fs::create_dir_all(resources.join("zh_CN.lproj")).unwrap();
        fs::write(
            contents.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <plist version="1.0"><dict>
            <key>CFBundleDisplayName</key><string>Example Tool</string>
            <key>CFBundleName</key><integer>42</integer>
            </dict></plist>"#,
        )
        .unwrap();
        let strings = r#""CFBundleDisplayName" = "项目工具"; "CFBundleName" = "项目工具";"#;
        let utf16: Vec<_> = [0xff, 0xfe]
            .into_iter()
            .chain(strings.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        fs::write(resources.join("zh-Hans.lproj/InfoPlist.strings"), utf16).unwrap();
        fs::write(
            resources.join("zh-Hant.lproj/InfoPlist.strings"),
            r#""CFBundleDisplayName" = "專案工具";"#,
        )
        .unwrap();
        fs::write(resources.join("zh_CN.lproj/InfoPlist.strings"), "broken").unwrap();

        let mut catalog = Catalog::default();
        visit(&root, 0, &mut HashSet::new(), &mut catalog);
        assert_eq!(catalog.apps.len(), 1);
        let app = &catalog.apps[0];
        assert_eq!(app.name, "Example");
        assert_eq!(app.path, bundle);
        assert_eq!(app.id, bundle.to_string_lossy());
        assert_eq!(app.aliases, ["Example Tool", "專案工具", "项目工具"]);
        let candidates = search::app_candidates(&catalog.apps, &Config::default());
        for query in ["项目工具", "项目", "專案", "Example Tool", "Example"] {
            assert_eq!(
                search::rank(&candidates, query, &BTreeMap::new())[0].id,
                app.id
            );
        }
        // Corrupt or absent metadata must not make an installed application disappear.
        fs::write(contents.join("Info.plist"), "invalid").unwrap();
        fs::remove_dir_all(&resources).unwrap();
        assert!(localized_names(&bundle).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
