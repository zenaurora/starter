use crate::{catalog::Application, config::Config, history::Usage};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{SearcherBuilder, sinks::UTF8};
use ignore::WalkBuilder;
use nucleo_matcher::{
    Config as MatcherConfig, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub const RESULT_LIMIT: usize = 100;
const RANK_BATCH: usize = 4096;
const EXACT_SCORE: u32 = 300_000;
const FILE_LIMIT: usize = 100_000;
const CONTENT_FILE_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Apps,
    Files,
    Content,
    Uninstall,
    Clipboard,
    System,
    Reminders,
    Monitor,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Apps => "应用",
            Self::Files => "文件与文件夹",
            Self::Content => "文本内容",
            Self::Uninstall => "卸载应用",
            Self::Clipboard => "剪贴板历史",
            Self::System => "系统命令与设置",
            Self::Reminders => "提醒",
            Self::Monitor => "系统监控",
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            Self::Apps => "",
            Self::Files => "/f ",
            Self::Content => "/c ",
            Self::Uninstall => "/uninstall ",
            Self::Clipboard => "/clip ",
            Self::System => "/system ",
            Self::Reminders => "/remind ",
            Self::Monitor => "/monitor ",
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
        for (prefix, mode) in [
            ("/f", Mode::Files),
            ("/c", Mode::Content),
            ("/uninstall", Mode::Uninstall),
            ("/clip", Mode::Clipboard),
            ("/clipboard", Mode::Clipboard),
            ("/system", Mode::System),
            ("/settings", Mode::System),
            ("/remind", Mode::Reminders),
            ("/reminder", Mode::Reminders),
            ("/monitor", Mode::Monitor),
        ] {
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
    Uninstall,
    ClipboardText,
    ClipboardLink,
    ClipboardImage,
    ClipboardFiles,
    SystemCommand,
    SystemSetting,
    Reminder,
    ReminderDraft,
    Monitor,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::App => "应用",
            Self::File => "文件",
            Self::Folder => "目录",
            Self::Content => "内容",
            Self::Uninstall => "卸载",
            Self::ClipboardText => "文本",
            Self::ClipboardLink => "链接",
            Self::ClipboardImage => "图片",
            Self::ClipboardFiles => "文件",
            Self::SystemCommand => "系统命令",
            Self::SystemSetting => "系统设置",
            Self::Reminder => "提醒",
            Self::ReminderDraft => "新建提醒",
            Self::Monitor => "系统监控",
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

pub(crate) fn app_aliases(app: &Application, config: &Config) -> Vec<String> {
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

pub fn rank_apps<'a>(
    candidates: impl IntoIterator<Item = &'a Candidate>,
    query: &str,
    usage: &BTreeMap<String, Usage>,
    favorites: &BTreeSet<String>,
) -> Vec<Candidate> {
    rank_with_preferences(candidates, query, usage, favorites, &AtomicBool::new(false))
        .unwrap_or_default()
}

fn rank_with_preferences<'a, T: SearchEntry + 'a>(
    candidates: impl IntoIterator<Item = &'a T>,
    query: &str,
    usage: &BTreeMap<String, Usage>,
    favorites: &BTreeSet<String>,
    cancelled: &AtomicBool,
) -> Option<Vec<Candidate>> {
    if query.is_empty() {
        // 无需计算匹配分数，只需要计算偏好
        return select_top_candidates(candidates, usage, favorites, cancelled, |_, _| Some(0));
    }

    let query = query.to_lowercase();
    let pattern = Pattern::new(
        &query,
        CaseMatching::Ignore, // 不区分大小写
        Normalization::Smart,
        AtomKind::Fuzzy, // 模糊匹配模式
    );
    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let mut buffer = Vec::new();

    select_top_candidates(
        candidates,
        usage,
        favorites,
        cancelled,
        |candidate, lower| {
            let title = candidate.title();
            let mut best = score_name(&title, lower, &query, &pattern, &mut matcher, &mut buffer);
            for alias in candidate.aliases() {
                // Exact matches are the score ceiling; later aliases cannot improve it.
                if best == Some(EXACT_SCORE) {
                    break;
                }
                let lower = alias.to_lowercase();
                best = best.max(score_name(
                    alias,
                    &lower,
                    &query,
                    &pattern,
                    &mut matcher,
                    &mut buffer,
                ));
            }
            best
        },
    )
}

struct Scored<'a, T> {
    score: u32,
    favorite: bool,
    count: u64,
    last_opened: u64,
    lower_title: Cow<'a, str>,
    id: Cow<'a, str>, // 对于 file 来说 id 是 path
    entry: &'a T,
}

fn select_top_candidates<'a, T: SearchEntry + 'a>(
    candidates: impl IntoIterator<Item = &'a T>,
    usage: &BTreeMap<String, Usage>,
    favorites: &BTreeSet<String>,
    cancelled: &AtomicBool,
    mut score_candidate: impl FnMut(&T, &str) -> Option<u32>,
) -> Option<Vec<Candidate>> {
    let candidates = candidates.into_iter();
    let mut scored = Vec::with_capacity(RANK_BATCH.min(candidates.size_hint().0));
    let compare = |a: &Scored<'_, T>, b: &Scored<'_, T>| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.favorite.cmp(&a.favorite))
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| b.last_opened.cmp(&a.last_opened))
            .then_with(|| a.lower_title.cmp(&b.lower_title))
            .then_with(|| a.id.cmp(&b.id))
    };
    for candidate in candidates {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let lower = candidate.lower_title();
        if let Some(score) = score_candidate(candidate, &lower) {
            let id = candidate.id();
            let (count, last_opened) = usage
                .get(id.as_ref())
                .map_or((0, 0), |usage| (usage.count, usage.last_opened));
            scored.push(Scored {
                score,
                favorite: favorites.contains(id.as_ref()),
                count,
                last_opened,
                lower_title: lower,
                id,
                entry: candidate,
            });
            // Discard losers in bounded batches instead of retaining one score
            // and lowercase String for every match in a 100,000-entry file index.
            if scored.len() == RANK_BATCH {
                scored.select_nth_unstable_by(RESULT_LIMIT, compare);
                scored.truncate(RESULT_LIMIT);
            }
        }
    }

    // 只展示前 100 条
    if scored.len() > RESULT_LIMIT {
        scored.select_nth_unstable_by(RESULT_LIMIT, compare);
        scored.truncate(RESULT_LIMIT);
    }
    scored.sort_unstable_by(compare);
    Some(
        scored
            .into_iter()
            .map(|item| item.entry.candidate())
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
    name: &str,  // 原始名字
    lower: &str, // name 小写
    query: &str, // 用户输入
    pattern: &Pattern,
    matcher: &mut Matcher,
    buffer: &mut Vec<char>,
) -> Option<u32> {
    // 精确匹配
    if lower == query {
        return Some(EXACT_SCORE);
    }
    // 前缀匹配
    if lower.starts_with(query) {
        return Some(200_000 - lower.chars().count().min(10_000) as u32);
    }
    // nucleo 模糊匹配
    // buffer 是传进来用来复用的，减少重复内存分配
    if let Some(score) = pattern.score(Utf32Str::new(name, buffer), matcher) {
        return Some(100_000 + score.min(65_000));
    }

    // Damerau-Levenshtein 容错匹配
    // 容错机制会导致复杂度更高，nucleo 本身没有支持。fzf 有 --typo 的选项，但是默认也是不开启的
    let length = query.chars().count();
    if length <= 3 || length >= 32 {
        // 对应长度 3 个以内的没必要做容错
        // 长度太长也是没必要，计算成本太高
        return None;
    }

    let tolerance = if length >= 7 { 2 } else { 1 };
    std::iter::once(lower)
        .chain(
            lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| *word != lower && !word.is_empty()),
        )
        // example: "docker-compose-cli" → ["docker-compose-cli", "docker", "compose", "cli"]
        // update: 避免不必要的 min，因为我只要能匹配上就行了，不用非要找到一个最小的 distance
        .find_map(|word| {
            if word.starts_with(query) {
                return Some(10_000);
            }

            let target = if word.chars().count() > length + tolerance {
                let end = word
                    .char_indices()
                    .nth(length)
                    .map_or(word.len(), |(i, _)| i);
                &word[..end]
            } else {
                word
            };

            let distance = strsim::damerau_levenshtein(query, target);
            (distance <= tolerance).then(|| 10_000 - distance as u32 * 1_000)
        })
}

#[derive(Default)]
pub struct FileIndex {
    files: Vec<FileEntry>,
    pub skipped: usize,
    pub truncated: bool,
}

struct FileEntry {
    path: PathBuf,
    // Walking Path components for every keystroke costs more than keeping the
    // short filename. The full identity/parent path is still stored only once.
    name: String,
    lower: String,
    kind: Kind,
}

// Ranking reads names/identity from either source. Only the final 100 file hits
// need the UI's full Candidate representation (id, title, parent and path).
trait SearchEntry {
    fn title(&self) -> Cow<'_, str>;
    fn lower_title(&self) -> Cow<'_, str> {
        Cow::Owned(self.title().to_lowercase())
    }
    fn id(&self) -> Cow<'_, str>;
    fn aliases(&self) -> &[String] {
        &[]
    }
    fn candidate(&self) -> Candidate;
}

impl SearchEntry for Candidate {
    fn title(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.title)
    }
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.id)
    }
    fn aliases(&self) -> &[String] {
        &self.aliases
    }
    fn candidate(&self) -> Candidate {
        self.clone()
    }
}

impl SearchEntry for FileEntry {
    fn title(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }
    fn lower_title(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.lower)
    }
    fn id(&self) -> Cow<'_, str> {
        self.path.to_string_lossy()
    }
    fn candidate(&self) -> Candidate {
        Candidate {
            id: self.id().into_owned(),
            title: self.title().into_owned(),
            detail: self
                .path
                .parent()
                .unwrap_or(&self.path)
                .display()
                .to_string(),
            path: self.path.clone(),
            kind: self.kind,
            aliases: Vec::new(),
        }
    }
}

impl FileIndex {
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn search(&self, query: &str, cancelled: &AtomicBool) -> Option<Vec<Candidate>> {
        rank_with_preferences(
            &self.files,
            query,
            &BTreeMap::new(),
            &BTreeSet::new(),
            cancelled,
        )
    }
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
            let name = entry.file_name().to_string_lossy().into_owned();
            index.files.push(FileEntry {
                path: entry.path().to_path_buf(),
                lower: name.to_lowercase(),
                name,
                kind: if kind.is_dir() {
                    Kind::Folder
                } else {
                    Kind::File
                },
            });
            if index.len() >= FILE_LIMIT {
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
        assert_eq!(Query::parse(" /uninstall 微信").mode, Mode::Uninstall);
        assert_eq!(Query::parse("/uninstall\t微信").text, "微信");
        assert_eq!(Query::parse("/clip 图片 2026-10").mode, Mode::Clipboard);
        assert_eq!(Query::parse("/clipboard 链接").text, "链接");
        assert_eq!(Query::parse("/uninstaller/foo").mode, Mode::Apps);
        assert_eq!(Query::parse("/clipboard/path").mode, Mode::Apps);
        assert_eq!(Query::parse("/system display").mode, Mode::System);
        assert_eq!(Query::parse("/settings sound").mode, Mode::System);
        assert_eq!(Query::parse("/remind 10m 开会").text, "10m 开会");
        assert_eq!(Query::parse("/reminder").mode, Mode::Reminders);
        assert_eq!(Query::parse("/reminder/file").mode, Mode::Apps);
        assert_eq!(Query::parse("/monitor").mode, Mode::Monitor);
        assert_eq!(Query::parse("/monitor Chrome").text, "Chrome");
        assert_eq!(Query::parse("/monitor/file").mode, Mode::Apps);
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
    fn bounded_ranking_keeps_the_global_top_results_and_identity_ties() {
        let apps: Vec<_> = (0..12_500)
            .rev()
            .map(|index| {
                let mut candidate = app(if index % 2 == 0 { "Tool" } else { "tool" });
                candidate.id = format!("app:{index:05}");
                candidate
            })
            .collect();
        let usage: BTreeMap<_, _> = apps
            .iter()
            .enumerate()
            .map(|(index, app)| {
                (
                    app.id.clone(),
                    Usage {
                        count: (index % 17) as u64,
                        last_opened: (index % 23) as u64,
                    },
                )
            })
            .collect();
        let favorites: BTreeSet<_> = apps.iter().step_by(151).map(|app| app.id.clone()).collect();
        let mut expected: Vec<_> = apps.iter().collect();
        expected.sort_unstable_by(|a, b| {
            favorites
                .contains(&b.id)
                .cmp(&favorites.contains(&a.id))
                .then_with(|| usage[&b.id].count.cmp(&usage[&a.id].count))
                .then_with(|| usage[&b.id].last_opened.cmp(&usage[&a.id].last_opened))
                .then_with(|| a.id.cmp(&b.id))
        });
        let expected: Vec<_> = expected
            .into_iter()
            .take(RESULT_LIMIT)
            .map(|app| &app.id)
            .collect();
        for query in ["", "tool"] {
            let results = rank_apps(&apps, query, &usage, &favorites);
            assert_eq!(
                results.iter().map(|app| &app.id).collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn file_hits_preserve_paths_types_and_duplicate_root_handling() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("项目");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("项目计划.txt");
        std::fs::write(&file, "text").unwrap();
        let cancelled = AtomicBool::new(false);
        let index = file_index(&[root.path().to_owned(), folder.clone()], &cancelled);
        assert_eq!(index.len(), 2);
        let hits = index.search("项目", &cancelled).unwrap();
        assert_eq!(hits[0].kind, Kind::Folder);
        let hit = hits.iter().find(|hit| hit.path == file).unwrap();
        assert_eq!(hit.id, file.display().to_string());
        assert_eq!(hit.title, "项目计划.txt");
        assert_eq!(hit.detail, folder.display().to_string());
        assert_eq!(hit.kind, Kind::File);
        cancelled.store(true, Ordering::Relaxed);
        assert!(index.search("项目", &cancelled).is_none());
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
        assert!(!files.files.iter().any(|c| c.title() == "ignored.txt"));
        assert_eq!(
            files.search("项目", &AtomicBool::new(false)).unwrap()[0].title,
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
