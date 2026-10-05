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
                path,
            });
        } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            visit(&path, depth + 1, seen, catalog);
        }
    }
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
