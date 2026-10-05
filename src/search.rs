use crate::{catalog::Application, config::Config, history::Usage};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{SearcherBuilder, sinks::UTF8};
use ignore::WalkBuilder;
use nucleo_matcher::{
    Config as MatcherConfig, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub const RESULT_LIMIT: usize = 100;
const FILE_LIMIT: usize = 100_000;
const CONTENT_FILE_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Apps,
    Files,
    Content,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Apps => "应用",
            Self::Files => "文件与文件夹",
            Self::Content => "文本内容",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub mode: Mode,
    pub text: String,
}

impl Query {
    pub fn parse(input: &str) -> Self {
        let input = input.trim_start();
        for (prefix, mode) in [("/f", Mode::Files), ("/c", Mode::Content)] {
            if let Some(rest) = input.strip_prefix(prefix)
                && (rest.is_empty() || rest.starts_with(char::is_whitespace))
            {
                return Self {
                    mode,
                    text: rest.trim().to_owned(),
                };
            }
        }
        Self {
            mode: Mode::Apps,
            text: input.trim().to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub aliases: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    File,
    Folder,
    Content,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::App => "应用",
            Self::File => "文件",
            Self::Folder => "目录",
            Self::Content => "内容",
        }
    }
}

pub fn app_candidates(apps: &[Application], config: &Config) -> Vec<Candidate> {
    apps.iter()
        .map(|app| Candidate {
            id: app.id.clone(),
            title: app.name.clone(),
            detail: app.path.display().to_string(),
            path: app.path.clone(),
            kind: Kind::App,
            aliases: app_aliases(app, config),
        })
        .collect()
}

fn app_aliases(app: &Application, config: &Config) -> Vec<String> {
    let mut aliases = app.aliases.clone();
    // Some Windows shortcuts and apps without localized metadata use English
    // names only. These exact-name aliases also work for existing configurations.
    for name in std::iter::once(&app.name).chain(&app.aliases) {
        let chinese: &[&str] = match name.to_lowercase().as_str() {
            "wechat" | "weixin" => &["微信"],
            "wecom" => &["企业微信"],
            "tencentmeeting" | "tencent meeting" | "voov meeting" => &["腾讯会议"],
            "dingtalk" => &["钉钉"],
            "feishu" => &["飞书"],
            "qq" => &["腾讯QQ"],
            "neteasemusic" | "netease cloud music" => &["网易云音乐"],
            "terminal" => &["终端"],
            "system settings" | "system preferences" => &["系统设置", "系统偏好设置"],
            "activity monitor" => &["活动监视器"],
            "calculator" => &["计算器"],
            "calendar" => &["日历"],
            "notes" => &["备忘录"],
            "reminders" => &["提醒事项"],
            "preview" => &["预览"],
            "photos" => &["照片"],
            _ => &[],
        };
        aliases.extend(chinese.iter().map(|name| (*name).to_owned()));
    }
    aliases.extend(config.aliases.get(&app.name).into_iter().flatten().cloned());
    aliases.sort();
    aliases.dedup();
    aliases
}

/// Relevance first; favorites and frequency order the default list and break ties.
pub fn rank(
    candidates: &[Candidate],
    query: &str,
    usage: &BTreeMap<String, Usage>,
) -> Vec<Candidate> {
    rank_apps(candidates, query, usage, &BTreeSet::new())
}

pub fn rank_apps(
    candidates: &[Candidate],
    query: &str,
    usage: &BTreeMap<String, Usage>,
    favorites: &BTreeSet<String>,
) -> Vec<Candidate> {
    rank_with_preferences(candidates, query, usage, favorites, &AtomicBool::new(false))
        .unwrap_or_default()
}

pub fn rank_cancellable(
    candidates: &[Candidate],
    query: &str,
    usage: &BTreeMap<String, Usage>,
    cancelled: &AtomicBool,
) -> Option<Vec<Candidate>> {
    rank_with_preferences(candidates, query, usage, &BTreeSet::new(), cancelled)
}

fn rank_with_preferences(
    candidates: &[Candidate],
    query: &str,
    usage: &BTreeMap<String, Usage>,
    favorites: &BTreeSet<String>,
    cancelled: &AtomicBool,
) -> Option<Vec<Candidate>> {
    let query = query.to_lowercase();
    let pattern = Pattern::new(
        &query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let mut buffer = Vec::new();
    let mut scored = Vec::new();
    for candidate in candidates {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let score = std::iter::once(candidate.title.as_str())
            .chain(candidate.aliases.iter().map(String::as_str))
            .filter_map(|name| score_name(name, &query, &pattern, &mut matcher, &mut buffer))
            .max();
        if let Some(score) = score {
            let usage = usage.get(&candidate.id).cloned().unwrap_or_default();
            scored.push((
                score,
                favorites.contains(&candidate.id),
                usage.count,
                usage.last_opened,
                candidate.title.to_lowercase(),
                candidate,
            ));
        }
    }
    type Scored<'a> = (u32, bool, u64, u64, String, &'a Candidate);
    let compare = |a: &Scored<'_>, b: &Scored<'_>| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| b.3.cmp(&a.3))
            .then_with(|| a.4.cmp(&b.4))
            .then_with(|| a.5.id.cmp(&b.5.id))
    };
    if scored.len() > RESULT_LIMIT {
        scored.select_nth_unstable_by(RESULT_LIMIT, compare);
        scored.truncate(RESULT_LIMIT);
    }
    scored.sort_unstable_by(compare);
    Some(
        scored
            .into_iter()
            .map(|(_, _, _, _, _, c)| c.clone())
            .collect(),
    )
}

/// Recent visits are independent of favorites/frequency, and exclude absent apps.
pub fn recent_apps(
    candidates: &[Candidate],
    usage: &BTreeMap<String, Usage>,
    limit: usize,
) -> Vec<Candidate> {
    let mut recent: Vec<_> = candidates
        .iter()
        .filter_map(|candidate| {
            let usage = usage.get(&candidate.id)?;
            (candidate.kind == Kind::App && usage.last_opened > 0)
                .then_some((usage.last_opened, candidate))
        })
        .collect();
    recent.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    recent
        .into_iter()
        .take(limit)
        .map(|(_, candidate)| candidate.clone())
        .collect()
}

fn score_name(
    name: &str,
    query: &str,
    pattern: &Pattern,
    matcher: &mut Matcher,
    buffer: &mut Vec<char>,
) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let lower = name.to_lowercase();
    if lower == query {
        return Some(300_000);
    }
    if lower.starts_with(query) {
        return Some(200_000 - lower.chars().count().min(10_000) as u32);
    }
    if let Some(score) = pattern.score(Utf32Str::new(name, buffer), matcher) {
        return Some(100_000 + score.min(65_000));
    }
    let length = query.chars().count();
    if !(3..=64).contains(&length) {
        return None;
    }
    let tolerance = if length >= 7 { 2 } else { 1 };
    let distance = std::iter::once(lower.as_str())
        .chain(lower.split(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .map(|word| {
            let prefix: String = word.chars().take(length).collect();
            let prefix_distance = strsim::damerau_levenshtein(query, &prefix);
            if word.chars().count().abs_diff(length) <= tolerance {
                prefix_distance.min(strsim::damerau_levenshtein(query, word))
            } else {
                prefix_distance
            }
        })
        .min()?;
    (distance <= tolerance).then(|| 10_000 - distance as u32 * 1_000)
}

#[derive(Default)]
pub struct FileIndex {
    pub candidates: Vec<Candidate>,
    pub skipped: usize,
    pub truncated: bool,
}

pub fn file_index(roots: &[PathBuf], cancelled: &AtomicBool) -> FileIndex {
    let mut index = FileIndex::default();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        for entry in walker(root) {
            if cancelled.load(Ordering::Relaxed) {
                return index;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    index.skipped += 1;
                    continue;
                }
            };
            if entry.depth() == 0 || !seen.insert(entry.path().to_path_buf()) {
                continue;
            }
            let Some(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_file() && !kind.is_dir() {
                continue;
            }
            index.candidates.push(Candidate {
                id: entry.path().display().to_string(),
                title: entry.file_name().to_string_lossy().into_owned(),
                detail: entry.path().parent().unwrap_or(root).display().to_string(),
                path: entry.path().to_path_buf(),
                kind: if kind.is_dir() {
                    Kind::Folder
                } else {
                    Kind::File
                },
                aliases: Vec::new(),
            });
            if index.candidates.len() >= FILE_LIMIT {
                index.truncated = true;
                return index;
            }
        }
    }
    index
}

fn walker(root: &Path) -> ignore::Walk {
    WalkBuilder::new(root)
        .follow_links(false)
        .require_git(false)
        .filter_entry(|entry| {
            entry.depth() == 0
                || !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | "target" | ".cache")
                )
        })
        .build()
}

#[derive(Default)]
pub struct ContentSummary {
    pub skipped: usize,
    pub limited: bool,
}

/// Literal, case-insensitive matching. Stream batches; never build a content index.
pub fn content_search(
    roots: &[PathBuf],
    query: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(Vec<Candidate>) -> bool,
) -> anyhow::Result<ContentSummary> {
    let matcher = RegexMatcherBuilder::new()
        .fixed_strings(true)
        .case_insensitive(true)
        .build(query)?;
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let mut summary = ContentSummary::default();
    let mut batch = Vec::new();
    let mut count = 0;
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        for entry in walker(root) {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(summary);
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    summary.skipped += 1;
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file())
                || !seen.insert(entry.path().to_path_buf())
            {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    summary.skipped += 1;
                    continue;
                }
            };
            if metadata.len() > CONTENT_FILE_LIMIT {
                summary.skipped += 1;
                continue;
            }
            // Bound the read too: a file can grow after the metadata check.
            let text = match read_text(entry.path()) {
                Ok(text) => text,
                _ => {
                    summary.skipped += 1;
                    continue;
                }
            };
            let path = entry.path();
            let mut keep_going = true;
            searcher.search_slice(
                &matcher,
                text.as_bytes(),
                UTF8(|line, text| {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(false);
                    }
                    batch.push(Candidate {
                        id: format!("{}:{line}", path.display()),
                        title: text.trim().chars().take(240).collect(),
                        detail: format!("{}:{line}", path.display()),
                        path: path.to_path_buf(),
                        kind: Kind::Content,
                        aliases: Vec::new(),
                    });
                    count += 1;
                    if batch.len() >= 16 {
                        keep_going = emit(std::mem::take(&mut batch));
                    }
                    Ok(keep_going && count < RESULT_LIMIT)
                }),
            )?;
            if !keep_going {
                return Ok(summary);
            }
            if count >= RESULT_LIMIT {
                summary.limited = true;
                break;
            }
        }
        if summary.limited {
            break;
        }
    }
    if !batch.is_empty() {
        emit(batch);
    }
    Ok(summary)
}

fn read_text(path: &Path) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(CONTENT_FILE_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= CONTENT_FILE_LIMIT, "文件过大");
    anyhow::ensure!(!bytes.contains(&0), "二进制文件");
    Ok(String::from_utf8(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> Candidate {
        Candidate {
            id: name.into(),
            title: name.into(),
            detail: String::new(),
            path: PathBuf::from(name),
            kind: Kind::App,
            aliases: Vec::new(),
        }
    }

    #[test]
    fn modes_do_not_consume_paths_or_urls() {
        assert_eq!(Query::parse("/f hello").mode, Mode::Files);
        assert_eq!(Query::parse("/c\tneedle").text, "needle");
        assert_eq!(Query::parse("/f").mode, Mode::Files);
        assert_eq!(Query::parse("/foo/bar").mode, Mode::Apps);
        assert_eq!(Query::parse("https://a/c").mode, Mode::Apps);
    }

    #[test]
    fn spelling_and_aliases_outrank_typo_rescue() {
        let mut vscode = app("Visual Studio Code");
        vscode.aliases.push("vsc".into());
        let apps = vec![app("Terminal"), app("Terminator"), vscode, app("Safari")];
        assert_eq!(
            rank(&apps, "termianl", &BTreeMap::new())[0].title,
            "Terminal"
        );
        assert_eq!(rank(&apps, "safrai", &BTreeMap::new())[0].title, "Safari");
        assert_eq!(
            rank(&apps, "vsc", &BTreeMap::new())[0].title,
            "Visual Studio Code"
        );
        assert_eq!(
            rank(&apps, "Terminal", &BTreeMap::new())[0].title,
            "Terminal"
        );
        assert!(rank(&apps, "zzzzzzzzzz", &BTreeMap::new()).is_empty());
        assert!(rank(&apps, "这是一个完全不存在的应用名称", &BTreeMap::new()).is_empty());
        assert_eq!(rank(&apps, "!Safari", &BTreeMap::new())[0].title, "Safari");
        assert!(
            rank_cancellable(&apps, "term", &BTreeMap::new(), &AtomicBool::new(true)).is_none()
        );
    }

    #[test]
    fn chinese_app_names_work_without_user_aliases() {
        let apps =
            ["WeChat", "WeCom", "TencentMeeting", "微信读书", "Terminal"].map(|name| Application {
                id: name.into(),
                name: name.into(),
                path: PathBuf::from(format!("{name}.app")),
                aliases: Vec::new(),
            });
        let mut config = Config::default();
        config.aliases.insert("WeChat".into(), vec!["聊天".into()]);
        let candidates = app_candidates(&apps, &config);
        for (query, expected) in [
            ("微信", "WeChat"),
            ("企业微信", "WeCom"),
            ("腾讯会议", "TencentMeeting"),
            ("微信读", "微信读书"),
            ("聊天", "WeChat"),
            ("WeChat", "WeChat"),
        ] {
            let hits = rank(&candidates, query, &BTreeMap::new());
            assert_eq!(
                hits.first().map(|hit| hit.title.as_str()),
                Some(expected),
                "{query}"
            );
        }
    }

    #[test]
    fn favorites_frequency_and_recent_are_independent() {
        let apps = vec![
            app("Safari"),
            app("Terminal"),
            app("Code"),
            app("Calculator"),
        ];
        let usage = BTreeMap::from([
            (
                "Safari".into(),
                Usage {
                    count: 8,
                    last_opened: 30,
                },
            ),
            (
                "Terminal".into(),
                Usage {
                    count: 30,
                    last_opened: 10,
                },
            ),
            (
                "Code".into(),
                Usage {
                    count: 2,
                    last_opened: 40,
                },
            ),
            (
                "Removed app".into(),
                Usage {
                    count: 100,
                    last_opened: 100,
                },
            ),
        ]);
        let favorites = BTreeSet::from(["Calculator".into()]);
        let titles = |results: Vec<Candidate>| {
            results
                .into_iter()
                .map(|candidate| candidate.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles(rank_apps(&apps, "", &usage, &favorites)),
            ["Calculator", "Terminal", "Safari", "Code"]
        );
        assert_eq!(titles(recent_apps(&apps, &usage, 2)), ["Code", "Safari"]);
        assert_eq!(
            rank_apps(&apps, "Safari", &usage, &favorites)[0].title,
            "Safari"
        );
        assert_eq!(
            rank_apps(&apps, "", &usage, &BTreeSet::new())[0].title,
            "Terminal"
        );
        // A favorite beyond the 100-result cut must still reach the first position.
        let many: Vec<_> = (0..130).map(|i| app(&format!("App {i:03}"))).collect();
        let favorite = BTreeSet::from(["App 129".into()]);
        let ranked = rank_apps(&many, "", &BTreeMap::new(), &favorite);
        assert_eq!(ranked.len(), RESULT_LIMIT);
        assert_eq!(ranked[0].title, "App 129");
    }

    #[test]
    fn content_is_literal_and_ignores_binary_and_gitignore() {
        let root = std::env::temp_dir().join(format!("starter-search-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("notes.txt"), "Needle.* is literal\n研发计划\n").unwrap();
        std::fs::write(root.join("项目计划.txt"), "中文搜索\n").unwrap();
        std::fs::write(root.join("binary"), b"Needle.*\0binary").unwrap();
        std::fs::write(root.join("ignored.txt"), "Needle.*").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        let mut hits = Vec::new();
        content_search(
            std::slice::from_ref(&root),
            "needle.*",
            &AtomicBool::new(false),
            |batch| {
                hits.extend(batch);
                true
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].detail.ends_with("notes.txt:1"));
        let files = file_index(std::slice::from_ref(&root), &AtomicBool::new(false));
        assert!(!files.candidates.iter().any(|c| c.title == "ignored.txt"));
        assert_eq!(
            rank(&files.candidates, "项目", &BTreeMap::new())[0].title,
            "项目计划.txt"
        );
        let mut chinese_hits = Vec::new();
        content_search(
            std::slice::from_ref(&root),
            "研发",
            &AtomicBool::new(false),
            |batch| {
                chinese_hits.extend(batch);
                true
            },
        )
        .unwrap();
        assert_eq!(chinese_hits.len(), 1);
        assert!(chinese_hits[0].detail.ends_with("notes.txt:2"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
