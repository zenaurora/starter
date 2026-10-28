//! Read-only version discovery. An external catalog never takes ownership of an installation.
use super::{Action, Software, Source, UpdateState, providers::Batch};
use anyhow::{Context, Result, bail, ensure};
use serde::{
    Deserialize,
    de::{self, SeqAccess, Visitor},
};
use serde_json::Value;
use std::{
    fmt,
    io::{self, Read},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

const SPARKLE: &str = "http://www.andymatuschak.org/xml-namespaces/sparkle";

pub(super) fn newer(candidate: &str, installed: &str) -> bool {
    let valid = |v: &str| v.chars().any(|c| c.is_ascii_digit()) && v.len() < 128;
    valid(candidate)
        && valid(installed)
        && version_compare::compare_to(candidate, installed, version_compare::Cmp::Gt)
            .unwrap_or(false)
}

pub(super) fn bundle_info(path: &Path) -> Option<plist::Dictionary> {
    let path = path.join("Contents/Info.plist");
    if !path.metadata().is_ok_and(|m| m.len() < 1024 * 1024) {
        return None;
    }
    plist::Value::from_file(path).ok()?.into_dictionary()
}
fn field<'a>(info: &'a plist::Dictionary, key: &str) -> &'a str {
    info.get(key)
        .and_then(plist::Value::as_string)
        .unwrap_or_default()
}
#[derive(Clone)]
struct Application {
    index: usize,
    bundle: String,
    filename: String,
    version: String,
    build: String,
    feed: String,
    store: bool,
}
struct Checked {
    latest: Option<String>,
    description: String,
}
impl Checked {
    fn apply(self, item: &mut Software) {
        item.state = if self.latest.is_some() {
            UpdateState::Available
        } else {
            UpdateState::Managed
        };
        item.latest = self.latest;
        item.description = self.description;
    }
}

pub(super) fn check(batch: &mut Batch, cancel: &AtomicBool) {
    let apps: Vec<_> = batch
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            if !matches!(item.source, Source::Application | Source::AppStore)
                && !(item.source == Source::Cask && matches!(item.action, Action::OpenApp(_)))
            {
                return None;
            }
            let path = item.path.as_ref()?;
            let info = bundle_info(path)?;
            let bundle = field(&info, "CFBundleIdentifier");
            if bundle.is_empty() {
                return None;
            }
            Some(Application {
                index,
                bundle: bundle.into(),
                filename: path.file_name()?.to_str()?.into(),
                version: item.installed.clone(),
                build: field(&info, "CFBundleVersion").into(),
                feed: field(&info, "SUFeedURL").into(),
                store: item.source == Source::AppStore,
            })
        })
        .collect();
    if apps.is_empty() || cancel.load(Ordering::Relaxed) {
        return;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .timeout_connect(Some(Duration::from_secs(5)))
        .build()
        .into();
    let system = plist::Value::from_file("/System/Library/CoreServices/SystemVersion.plist")
        .ok()
        .and_then(|p| {
            p.as_dictionary()?
                .get("ProductVersion")?
                .as_string()
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let online: Vec<_> = apps
        .iter()
        .filter(|app| app.store || !app.feed.is_empty())
        .collect();
    let unknown: Vec<_> = apps
        .iter()
        .filter(|app| !app.store && app.feed.is_empty())
        .cloned()
        .collect();
    let results = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..online.len().min(4) {
            let agent = &agent;
            let system = &system;
            let online = &online;
            let next = &next;
            let results = &results;
            scope.spawn(move || {
                while !cancel.load(Ordering::Relaxed) {
                    let Some(app) = online.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let result = if app.store {
                        app_store(agent, app, cancel)
                    } else {
                        fetch(agent, &app.feed, 2 * 1024 * 1024, cancel)
                            .and_then(|xml| sparkle(&xml, app, system, std::env::consts::ARCH))
                    };
                    results.lock().unwrap().push((app.index, result));
                }
            });
        }
        // Stream the catalog once; retain only exact matches, never the full catalog in memory.
        if !unknown.is_empty() {
            let agent = &agent;
            let results = &results;
            let system = &system;
            scope.spawn(move || {
                let result = catalog(agent, &unknown, system, cancel);
                let mut results = results.lock().unwrap();
                match result {
                    Ok(matches) => {
                        for (app, candidates) in unknown.iter().zip(matches) {
                            let result = match candidates.as_slice() {
                                [candidate] if app.version.chars().any(|c| c.is_ascii_digit()) => {
                                    Ok(Checked {
                                        latest: newer(&candidate.version, &app.version)
                                            .then(|| candidate.version.clone()),
                                        description:
                                            "版本信息来自 Homebrew Cask · 由原应用更新器或官网安装"
                                                .into(),
                                    })
                                }
                                [] => {
                                    results.push((app.index, Ok(None)));
                                    continue;
                                }
                                _ => {
                                    results.push((app.index, Ok(None)));
                                    continue;
                                }
                            };
                            results.push((app.index, result.map(Some)));
                        }
                    }
                    Err(error) => {
                        for app in unknown {
                            results
                                .push((app.index, Err(anyhow::anyhow!("公开版本目录：{error}"))));
                        }
                    }
                }
            });
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    for (index, result) in results.into_inner().unwrap() {
        let item = &mut batch.items[index];
        match result {
            Ok(Some(checked)) => checked.apply(item),
            Ok(None) => {
                item.state = UpdateState::Manual;
                item.description = "未找到能准确匹配的公开版本源 · 可打开应用自身的更新器".into();
            }
            Err(error) => {
                item.state = UpdateState::CheckFailed;
                item.description = format!("版本检查失败：{error} · 可重新检查或打开应用");
            }
        }
    }
}

struct Cancellable<'a, R> {
    inner: R,
    cancel: &'a AtomicBool,
}
impl<R: Read> Read for Cancellable<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "检查已取消",
            ));
        }
        self.inner.read(buf)
    }
}
fn fetch(agent: &ureq::Agent, url: &str, limit: u64, cancel: &AtomicBool) -> Result<String> {
    ensure!(url.starts_with("https://"), "更新源未提供 HTTPS");
    let mut response = agent
        .get(url)
        .header("User-Agent", "Starter/version-check")
        .call()
        .context("更新源请求失败")?;
    let reader = response.body_mut().with_config().limit(limit).reader();
    let mut text = String::new();
    Cancellable {
        inner: reader,
        cancel,
    }
    .read_to_string(&mut text)
    .context("无法读取版本信息")?;
    Ok(text)
}
fn sparkle(xml: &str, app: &Application, system: &str, arch: &str) -> Result<Option<Checked>> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            nodes_limit: 20_000,
            ..Default::default()
        },
    )
    .context("无效的 Sparkle 源")?;
    ensure!(doc.root_element().has_tag_name("rss"), "更新源不是 appcast");
    let mut candidate: Option<(String, String)> = None;
    for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
        let value = |name| {
            item.children()
                .find(|n| n.tag_name().namespace() == Some(SPARKLE) && n.tag_name().name() == name)
                .and_then(|n| n.text())
                .unwrap_or("")
        };
        if !value("channel").is_empty() {
            continue;
        }
        let min = value("minimumSystemVersion");
        let max = value("maximumSystemVersion");
        if (!min.is_empty() && (system.is_empty() || newer(min, system)))
            || (!max.is_empty() && (system.is_empty() || newer(system, max)))
        {
            continue;
        }
        let minimum = value("minimumUpdateVersion");
        if !minimum.is_empty() && (app.build.is_empty() || newer(minimum, &app.build)) {
            continue;
        }
        let hardware = value("hardwareRequirements");
        if !hardware.is_empty()
            && !(hardware == "arm64" && arch == "aarch64")
            && !(hardware == "x86_64" && arch == "x86_64")
        {
            continue;
        }
        for enclosure in item.children().filter(|n| n.has_tag_name("enclosure")) {
            let attribute = |name| enclosure.attribute((SPARKLE, name)).unwrap_or("");
            let os = attribute("os");
            if (!os.is_empty() && os != "macos") || !attribute("deltaFrom").is_empty() {
                continue;
            }
            let build = if attribute("version").is_empty() {
                value("version")
            } else {
                attribute("version")
            };
            if build.is_empty() {
                continue;
            }
            let short = if attribute("shortVersionString").is_empty() {
                value("shortVersionString")
            } else {
                attribute("shortVersionString")
            };
            if candidate
                .as_ref()
                .is_none_or(|(previous, _)| newer(build, previous))
            {
                candidate = Some((
                    build.into(),
                    if short.is_empty() { build } else { short }.into(),
                ));
            }
        }
    }
    let Some((build, short)) = candidate else {
        bail!("更新源没有适用于本机的稳定版本");
    };
    ensure!(!app.build.is_empty(), "应用未提供可比较的内部版本");
    Ok(Some(Checked {
        latest: newer(&build, &app.build).then_some(short),
        description: "已检查 Sparkle 更新源 · 由应用自身验证签名并安装".into(),
    }))
}
fn app_store(
    agent: &ureq::Agent,
    app: &Application,
    cancel: &AtomicBool,
) -> Result<Option<Checked>> {
    // Bundle IDs are restricted to the characters valid in Apple's identifiers before URL use.
    ensure!(
        app.bundle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b)),
        "无效的应用标识"
    );
    let url = format!(
        "https://itunes.apple.com/lookup?bundleId={}&entity=desktopSoftware",
        app.bundle
    );
    let data: Value = serde_json::from_str(&fetch(agent, &url, 1024 * 1024, cancel)?)?;
    let result = data["results"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["bundleId"].as_str() == Some(&app.bundle) && v["kind"] == "mac-software")
        .context("商店未返回此应用的版本（可能受地区限制）")?;
    let version = result["version"]
        .as_str()
        .filter(|v| !v.is_empty())
        .context("商店未提供版本号")?;
    ensure!(
        app.version.chars().any(|c| c.is_ascii_digit()),
        "应用未提供可比较的当前版本"
    );
    Ok(Some(Checked {
        latest: newer(version, &app.version).then(|| version.into()),
        description: "已检查 App Store 公开版本 · 在商店中安装更新".into(),
    }))
}

#[derive(Deserialize)]
struct CatalogApp {
    version: String,
    #[serde(default)]
    artifacts: Vec<Value>,
    #[serde(default)]
    depends_on: Value,
}
struct Candidate {
    version: String,
}
impl CatalogApp {
    fn compatible(&self, system: &str) -> bool {
        let Some(requirements) = self.depends_on["macos"].as_object() else {
            return true;
        };
        !system.is_empty()
            && requirements.iter().all(|(operator, versions)| {
                versions.as_array().is_some_and(|versions| {
                    versions.iter().filter_map(Value::as_str).any(|version| {
                        match operator.as_str() {
                            ">=" => !newer(version, system),
                            ">" => newer(system, version),
                            "<=" => !newer(system, version),
                            "<" => newer(version, system),
                            "==" | "=" => version_compare::compare_to(
                                system,
                                version,
                                version_compare::Cmp::Eq,
                            )
                            .unwrap_or(false),
                            _ => false,
                        }
                    })
                })
            })
    }
    fn matches(&self, app: &Application) -> bool {
        let filename = self.artifacts.iter().any(|a| {
            a["app"]
                .as_array()
                .and_then(|v| v.first())
                .and_then(Value::as_str)
                .is_some_and(|v| {
                    Path::new(v).file_name().and_then(|s| s.to_str()) == Some(&app.filename)
                })
        });
        // Both bundle identity and artifact name must match. No token/name guessing.
        filename
            && self.artifacts.iter().any(|a| {
                ["uninstall", "zap"].into_iter().any(|key| {
                    a[key].as_array().into_iter().flatten().any(|v| {
                        let exact = |v: &Value| {
                            v.as_str() == Some(&app.bundle)
                                || v.as_array().is_some_and(|a| {
                                    a.iter().any(|v| v.as_str() == Some(&app.bundle))
                                })
                        };
                        exact(&v["quit"])
                            || v["trash"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .any(|p| {
                                    Path::new(p).file_name().and_then(|s| s.to_str())
                                        == Some(&format!("{}.plist", app.bundle))
                                })
                    })
                })
            })
    }
}
struct Catalog<'a> {
    apps: &'a [Application],
    system: &'a str,
}
impl<'de> Visitor<'de> for Catalog<'_> {
    type Value = Vec<Vec<Candidate>>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a Homebrew cask array")
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut matches: Vec<Vec<Candidate>> = (0..self.apps.len()).map(|_| vec![]).collect();
        while let Some(cask) = seq.next_element::<CatalogApp>()? {
            let version = cask.version.split(',').next().unwrap_or("");
            if version == "latest" || version.is_empty() || !cask.compatible(self.system) {
                continue;
            }
            for (app, candidates) in self.apps.iter().zip(&mut matches) {
                if cask.matches(app) && candidates.len() < 2 {
                    candidates.push(Candidate {
                        version: version.into(),
                    });
                }
            }
        }
        Ok(matches)
    }
}
fn catalog(
    agent: &ureq::Agent,
    apps: &[Application],
    system: &str,
    cancel: &AtomicBool,
) -> Result<Vec<Vec<Candidate>>> {
    let mut response = agent
        .get("https://formulae.brew.sh/api/cask.json")
        .header("User-Agent", "Starter/version-check")
        .call()?;
    let reader = response
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .reader();
    let mut deserializer = serde_json::Deserializer::from_reader(io::BufReader::new(Cancellable {
        inner: reader,
        cancel,
    }));
    let result = de::Deserializer::deserialize_seq(&mut deserializer, Catalog { apps, system })?;
    deserializer.end()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> Application {
        Application {
            index: 0,
            bundle: "org.example.App".into(),
            filename: "Example.app".into(),
            version: "2.0".into(),
            build: "200".into(),
            feed: String::new(),
            store: false,
        }
    }
    #[test]
    fn sparkle_compares_builds_and_filters_channels_deltas_and_platforms() {
        let xml = r#"<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel>
          <item><enclosure sparkle:version="201" sparkle:shortVersionString="2.0.1"/></item>
          <item><sparkle:channel>beta</sparkle:channel><enclosure sparkle:version="900"/></item>
          <item><sparkle:minimumSystemVersion>99</sparkle:minimumSystemVersion><enclosure sparkle:version="800"/></item>
          <item><sparkle:minimumUpdateVersion>300</sparkle:minimumUpdateVersion><enclosure sparkle:version="700"/></item>
          <item><sparkle:hardwareRequirements>x86_64</sparkle:hardwareRequirements><enclosure sparkle:version="600"/></item>
          <item><enclosure sparkle:version="500" sparkle:deltaFrom="199"/></item>
          <item><enclosure sparkle:version="400" sparkle:os="windows"/></item>
        </channel></rss>"#;
        assert_eq!(
            sparkle(xml, &app(), "15.0", "aarch64")
                .unwrap()
                .unwrap()
                .latest
                .as_deref(),
            Some("2.0.1")
        );
        let mut installed = app();
        installed.build = "999".into();
        assert!(
            sparkle(xml, &installed, "15.0", "aarch64")
                .unwrap()
                .unwrap()
                .latest
                .is_none()
        );
        assert!(sparkle("<html/>", &app(), "15", "aarch64").is_err());
    }
    #[test]
    fn catalog_requires_exact_identity_and_filename_and_retains_ambiguity() {
        let data = r#"[{"version":"2.1,123","artifacts":[{"app":["Example.app"]},{"uninstall":[{"quit":"org.example.App"}]}]},
          {"version":"9","artifacts":[{"app":["Other.app"]},{"uninstall":[{"quit":"org.example.App"}]}]},
          {"version":"3","artifacts":[{"app":["Example.app"]},{"zap":[{"trash":["~/Library/Preferences/org.example.App.plist"]}]}]}]"#;
        let apps = [app()];
        let mut de = serde_json::Deserializer::from_str(data);
        let result = de::Deserializer::deserialize_seq(
            &mut de,
            Catalog {
                apps: &apps,
                system: "15.0",
            },
        )
        .unwrap();
        assert_eq!(result[0].len(), 2);
        assert_eq!(result[0][0].version, "2.1");
        let unrelated = CatalogApp {
            version: "9".into(),
            artifacts: vec![serde_json::json!({"app":["Example.app"]})],
            depends_on: Value::Null,
        };
        assert!(!unrelated.matches(&app()));
    }
    #[test]
    fn cancellation_ends_reads_and_catalog_respects_system_requirements() {
        let cancel = AtomicBool::new(true);
        let mut reader = Cancellable {
            inner: io::Cursor::new(b"data"),
            cancel: &cancel,
        };
        assert_eq!(
            reader
                .read_to_string(&mut String::new())
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
        let cask: CatalogApp = serde_json::from_value(
            serde_json::json!({"version":"3.0","depends_on":{"macos":{">=":["15.0"]}}}),
        )
        .unwrap();
        assert!(!cask.compatible("14.0"));
        assert!(cask.compatible("15.1"));
    }
    #[test]
    fn versions_never_offer_downgrades_or_latest_as_a_number() {
        assert!(newer("2.1", "2.0"));
        assert!(!newer("2.0", "2.1"));
        assert!(!newer("latest", "2.0"));
    }
}
