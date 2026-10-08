// Claude Code のセッション記録（~/.claude 以下の JSONL）の読み取り。書き込みは一切しない
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

const SNIPPET: usize = 300;
const MAX_ITEMS: usize = 400;
const MAX_TEXT: usize = 6000;
const SCAN_THREADS: usize = 6;

// ---------- JSONL の解析 ----------

fn nonempty<'a>(r: &'a Value, key: &str) -> Option<&'a str> {
    r.get(key)?.as_str().filter(|s| !s.is_empty())
}

fn flag(r: &Value, key: &str) -> bool {
    r.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ツール結果・メタ・スラッシュコマンド等を除いた「人が打ったプロンプト」だけを返す
fn human_prompt(r: &Value) -> Option<String> {
    if r["type"] != "user" || flag(r, "isMeta") || flag(r, "isSidechain") || flag(r, "isCompactSummary") {
        return None;
    }
    let content = r["message"].get("content");
    if let Some(Value::Array(a)) = content {
        if a.iter().any(|c| c["type"] == "tool_result") {
            return None;
        }
    }
    let text = text_of(content);
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') || text.starts_with("[Request interrupted") {
        return None;
    }
    Some(text.to_string())
}

// 先頭 n 文字（文字境界で切る）
fn head(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn clip(text: &str, n: usize) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let h = head(&t, n);
    if h.len() < t.len() {
        format!("{h}…")
    } else {
        t
    }
}

fn each_record(file: &Path, mut f: impl FnMut(&Value)) -> io::Result<()> {
    let mut reader = BufReader::with_capacity(1 << 16, File::open(file)?);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        // 書き込み途中の行などは読み飛ばす
        if let Ok(r) = serde_json::from_slice::<Value>(&line) {
            f(&r);
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    pub pid: i32,
    status: String,
    name: Option<String>,
    kind: Option<String>,
    started_at: Value,
    status_updated_at: Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: String,
    project_dir: String,
    title: Option<String>,
    first_prompt: Option<String>,
    last_prompt: Option<String>,
    pub cwd: Option<String>,
    branch: Option<String>,
    started_at: f64,
    updated_at: f64,
    prompts: u32,
    turns: u32,
    model: Option<String>,
    context_tokens: Option<u64>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    lines_added: Option<i64>,
    lines_removed: Option<i64>,
    version: Option<String>,
    entrypoint: Option<String>,
    pr_url: Option<String>,
    size: u64,
    pub live: Option<Live>,
}

#[derive(Clone, Copy, PartialEq)]
struct Stamp {
    mtime: SystemTime,
    size: u64,
}

impl Stamp {
    fn of(file: &Path) -> io::Result<Self> {
        let meta = fs::metadata(file)?;
        Ok(Self { mtime: meta.modified()?, size: meta.len() })
    }

    fn mtime_ms(&self) -> f64 {
        self.mtime.duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    }
}

fn dir_name(p: Option<&Path>) -> String {
    p.and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn summarize(file: &Path, stamp: Stamp) -> io::Result<Summary> {
    let mut s = Summary {
        id: file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        project_dir: dir_name(file.parent()),
        title: None,
        first_prompt: None,
        last_prompt: None,
        cwd: None,
        branch: None,
        started_at: 0.0,
        updated_at: 0.0,
        prompts: 0,
        turns: 0,
        model: None,
        context_tokens: None,
        cost_usd: None,
        lines_added: None,
        lines_removed: None,
        version: None,
        entrypoint: None,
        pr_url: None,
        size: stamp.size,
        live: None,
    };
    let mut ai_title: Option<String> = None;
    let mut custom_title: Option<String> = None;
    let mut last_human: Option<String> = None;
    let mut last_prompt_record: Option<String> = None;
    let mut started_at: Option<f64> = None;
    let mut updated_at: Option<f64> = None;

    each_record(file, |r| {
        let set = |slot: &mut Option<String>, key: &str| {
            if let Some(v) = nonempty(r, key) {
                *slot = Some(v.to_string());
            }
        };
        match r["type"].as_str().unwrap_or("") {
            "ai-title" => return set(&mut ai_title, "aiTitle"),
            "custom-title" => {
                if let Some(v) = nonempty(r, "customTitle").or_else(|| nonempty(r, "title")) {
                    custom_title = Some(v.to_string());
                }
                return;
            }
            "last-prompt" => return set(&mut last_prompt_record, "lastPrompt"),
            "cost-state" => {
                s.cost_usd = r["totalCostUSD"].as_f64().or(s.cost_usd);
                s.lines_added = r["totalLinesAdded"].as_i64().or(s.lines_added);
                s.lines_removed = r["totalLinesRemoved"].as_i64().or(s.lines_removed);
                return;
            }
            "pr-link" => {
                if let Some(v) = nonempty(r, "prUrl").or_else(|| nonempty(r, "url")) {
                    s.pr_url = Some(v.to_string());
                }
                return;
            }
            _ => {}
        }
        if flag(r, "isSidechain") {
            return;
        }
        if let Some(t) = nonempty(r, "timestamp").and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok()) {
            let t = t.timestamp_millis() as f64;
            started_at.get_or_insert(t);
            updated_at = Some(t);
        }
        // 再開には開始時の cwd が必要
        if s.cwd.is_none() {
            set(&mut s.cwd, "cwd");
        }
        // git 管理外は "HEAD" になる
        if let Some(b) = nonempty(r, "gitBranch") {
            s.branch = (b != "HEAD").then(|| b.to_string());
        }
        set(&mut s.version, "version");
        set(&mut s.entrypoint, "entrypoint");

        match r["type"].as_str() {
            Some("user") => {
                if let Some(text) = human_prompt(r) {
                    s.prompts += 1;
                    if s.first_prompt.is_none() {
                        s.first_prompt = Some(clip(&text, SNIPPET));
                    }
                    last_human = Some(text);
                }
            }
            Some("assistant") => {
                s.turns += 1;
                let m = &r["message"];
                if let Some(model) = nonempty(m, "model").filter(|&v| v != "<synthetic>") {
                    s.model = Some(model.to_string());
                }
                let u = &m["usage"];
                let ctx = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"]
                    .iter()
                    .map(|k| u[*k].as_u64().unwrap_or(0))
                    .sum::<u64>();
                if ctx > 0 {
                    s.context_tokens = Some(ctx);
                }
            }
            _ => {}
        }
    })?;

    s.last_prompt = last_prompt_record.or(last_human).map(|t| clip(&t, SNIPPET)).filter(|t| !t.is_empty());
    s.title = custom_title.or(ai_title);
    s.updated_at = updated_at.unwrap_or_else(|| stamp.mtime_ms());
    s.started_at = started_at.unwrap_or_else(|| stamp.mtime_ms());
    Ok(s)
}

// ---------- セッション一覧（mtime+size でキャッシュ） ----------

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => matches!(b, b'0'..=b'9' | b'a'..=b'f'),
        })
}

fn list_session_files(projects_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(dirs) = fs::read_dir(projects_dir) else { return files };
    for d in dirs.flatten() {
        if !d.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(d.path()) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let is_session = path.extension().is_some_and(|x| x == "jsonl")
                && path.file_stem().and_then(|n| n.to_str()).is_some_and(is_uuid);
            if is_session {
                files.push(path);
            }
        }
    }
    files
}

fn pid_alive(pid: i32) -> bool {
    // シグナル 0 は存在確認だけ。EPERM は「他ユーザーのプロセスとして存在する」
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn live_sessions(live_dir: &Path) -> HashMap<String, Live> {
    let mut live = HashMap::new();
    let Ok(entries) = fs::read_dir(live_dir) else { return live };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        // 壊れた/書き込み途中のファイルは無視
        let Some(j) = fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        let Some(id) = nonempty(&j, "sessionId") else { continue };
        let Some(pid) = j["pid"].as_i64().and_then(|p| i32::try_from(p).ok()).filter(|&p| p > 0) else { continue };
        if !pid_alive(pid) {
            continue;
        }
        let updated = if j["statusUpdatedAt"].is_null() { &j["updatedAt"] } else { &j["statusUpdatedAt"] };
        live.insert(
            id.to_string(),
            Live {
                pid,
                status: nonempty(&j, "status").unwrap_or("idle").to_string(),
                name: nonempty(&j, "name").map(str::to_string),
                kind: nonempty(&j, "kind").map(str::to_string),
                started_at: j["startedAt"].clone(),
                status_updated_at: updated.clone(),
            },
        );
    }
    live
}

#[derive(Serialize)]
pub struct Snapshot {
    now: f64,
    home: String,
    pub sessions: Vec<Summary>,
}

struct Cached {
    stamp: Stamp,
    summary: Summary,
}

pub struct Sessions {
    claude_dir: PathBuf,
    home: String,
    cache: Mutex<HashMap<PathBuf, Cached>>,
}

impl Sessions {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(&home).join(".claude"));
        Self { claude_dir, home, cache: Mutex::new(HashMap::new()) }
    }

    fn projects_dir(&self) -> PathBuf {
        self.claude_dir.join("projects")
    }

    pub fn scan(&self) -> Snapshot {
        // ポーリングが重なっても走査は 1 本ずつ。待たされた側はキャッシュですぐ返る
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let files = list_session_files(&self.projects_dir());
        let seen: HashSet<&PathBuf> = files.iter().collect();
        cache.retain(|k, _| seen.contains(k));

        let stale: Vec<(&PathBuf, Stamp)> = files
            .iter()
            .filter_map(|f| Stamp::of(f).ok().map(|st| (f, st)))
            .filter(|(f, st)| cache.get(*f).is_none_or(|hit| hit.stamp != *st))
            .collect();
        let next = AtomicUsize::new(0);
        let fresh: Vec<(&PathBuf, Stamp, Summary)> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..SCAN_THREADS.min(stale.len()))
                .map(|_| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        while let Some(&(file, stamp)) = stale.get(next.fetch_add(1, Ordering::Relaxed)) {
                            if let Ok(summary) = summarize(file, stamp) {
                                out.push((file, stamp, summary));
                            }
                        }
                        out
                    })
                })
                .collect();
            workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
        });
        // 読めなかったファイルは古い要約を出さない
        for (file, _) in &stale {
            cache.remove(*file);
        }
        for (file, stamp, summary) in fresh {
            cache.insert(file.clone(), Cached { stamp, summary });
        }

        let live = live_sessions(&self.claude_dir.join("sessions"));
        let mut sessions: Vec<Summary> = cache
            .values()
            .map(|c| &c.summary)
            .filter(|s| s.prompts > 0 || s.turns > 0 || live.contains_key(&s.id))
            .map(|s| Summary { live: live.get(&s.id).cloned(), ..s.clone() })
            .collect();
        sessions.sort_by(|a, b| b.updated_at.total_cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));

        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
        Snapshot { now, home: self.home.clone(), sessions }
    }

    pub fn find(&self, id: &str) -> Option<(Summary, PathBuf)> {
        if !is_uuid(id) {
            return None;
        }
        let summary = self.scan().sessions.into_iter().find(|s| s.id == id)?;
        let file = self.projects_dir().join(&summary.project_dir).join(format!("{id}.jsonl"));
        Some((summary, file))
    }
}

// ---------- 詳細（会話の抜粋） ----------

#[derive(Serialize)]
struct Item {
    role: &'static str,
    ts: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    text: String,
}

#[derive(Serialize)]
pub struct Transcript {
    total: usize,
    items: Vec<Item>,
}

fn tool_brief(input: &Value) -> String {
    const KEYS: [&str; 8] = ["command", "file_path", "pattern", "description", "query", "url", "skill", "prompt"];
    let first = KEYS.iter().map(|k| &input[*k]).find(|v| !v.is_null());
    first.and_then(Value::as_str).map(|v| clip(v, 160)).unwrap_or_default()
}

pub fn transcript(file: &Path) -> io::Result<Transcript> {
    let mut items = Vec::new();
    each_record(file, |r| {
        if flag(r, "isSidechain") {
            return;
        }
        let ts = || r["timestamp"].as_str().map(str::to_string);
        match r["type"].as_str() {
            Some("user") => {
                if let Some(text) = human_prompt(r) {
                    items.push(Item { role: "user", ts: ts(), name: None, text: head(&text, MAX_TEXT).to_string() });
                }
            }
            Some("assistant") => {
                for c in r["message"]["content"].as_array().into_iter().flatten() {
                    match c["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = c["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                items.push(Item { role: "assistant", ts: ts(), name: None, text: head(text, MAX_TEXT).to_string() });
                            }
                        }
                        Some("tool_use") => items.push(Item {
                            role: "tool",
                            ts: ts(),
                            name: c["name"].as_str().map(str::to_string),
                            text: tool_brief(&c["input"]),
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    })?;
    let total = items.len();
    let items = items.split_off(total.saturating_sub(MAX_ITEMS));
    Ok(Transcript { total, items })
}
