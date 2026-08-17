//! 公共规则：跳过规则、slug、会话 id 解析、哈希、desktop-projects.json 注册。
//!
//! 逻辑与 Python 版 `reasonix_migrate_common.py` 逐一对齐（本机逻辑是远端标准）。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

pub const FORMAT_VERSION: u32 = 1;

/// 顶层跳过（相对 home 的第一段）：运行态目录 / 应用状态，跨机器不应迁移。
pub const SKIP_TOP_LEVEL_DIRS: &[&str] = &[
    "cache",
    "logs",
    "stats",
    "repair",
    "crash-fatal",
    "cli-telemetry-pending",
    "state", // 桌面端状态（topic 标题缓存、legacy 会话、运行时警告）
];

pub const SKIP_TOP_LEVEL_FILES: &[&str] = &[
    "machine-id.key",                    // 机器标识
    "machine-id.key.lock",
    "install-id",                        // 安装标识
    "cli-telemetry-install-id",
    "metrics-pending.json",              // 待上报遥测
    "desktop-projects-legacy-recovered", // 应用内部迁移标记
];

/// 已知 sidecar 后缀（从文件名剥离会话 id；未知 sidecar 靠「id 前缀归属」吸收）。
/// 顺序无关，匹配时取最左 `.` 位置（与 Python 正则 `\.(?:...)$` 语义一致）。
const SIDECAR_SUFFIXES: &[&str] = &[
    "jsonl.meta",
    "jsonl.telemetry.json",
    "events.jsonl",
    "event-index.json",
    "goal-state.json",
    "recovery.json",
    "context.json",
    "conflicts.jsonl",
    "jsonl",
    "ckpt",
    "jobs",
];

// ---------------------------------------------------------------------------
// 会话 id
// ---------------------------------------------------------------------------

/// 会话 id 判定前缀：`^\d{8}-\d{6}\.\d{6,}`（如 `20260808-155503.898846400`）。
pub fn is_session_id_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 22 {
        return false;
    }
    b[0..8].iter().all(|c| c.is_ascii_digit())
        && b[8] == b'-'
        && b[9..15].iter().all(|c| c.is_ascii_digit())
        && b[15] == b'.'
        && b[16..22].iter().all(|c| c.is_ascii_digit())
}

/// recovery 分支后缀：`-recovery-<hex>`（旧）或 `-recovery-<hex>-<hex>`（v1.24 起
/// fork 分支带 writer 后缀，双 hex），大小写不敏感；返回 `-recovery-` 起始下标。
pub fn recovery_suffix_start(s: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    let needle = "-recovery-";
    let idx = lower.rfind(needle)?;
    let rest = &s[idx + needle.len()..];
    if rest.is_empty() {
        return None;
    }
    // 一个或多个 hex 组（`hex` 或 `hex-hex-...`），组间单个 `-` 分隔
    if rest
        .split('-')
        .all(|g| !g.is_empty() && g.chars().all(|c| c.is_ascii_hexdigit()))
    {
        Some(idx)
    } else {
        None
    }
}

/// recovery 分支 id 归并到主会话 id：`<主id>-recovery-<hash>` -> `<主id>`。
pub fn base_session_id(session_id: &str) -> &str {
    match recovery_suffix_start(session_id) {
        Some(idx) => &session_id[..idx],
        None => session_id,
    }
}

pub fn is_recovery_branch(session_id: &str) -> bool {
    recovery_suffix_start(session_id).is_some()
}

/// 从条目名剥离已知 sidecar 得到会话 id；非会话条目返回 None。
/// 例：`<id>.jsonl` / `<id>.jsonl.meta` / `<id>.events.jsonl` / `<id>.ckpt`(目录) -> `<id>`。
/// 语义与 Python 正则 `\.(?:jsonl\.meta|...)$` 一致：`.` 之后到结尾**恰好等于**某个
/// sidecar 模式（因此 `20260808-155503.898846400.events.jsonl` 必须跳过时间戳里的 `.`，
/// 在第二个 `.` 处才匹配）。
pub fn session_id_of(name: &str) -> Option<&str> {
    for (i, ch) in name.char_indices() {
        if ch != '.' {
            continue;
        }
        let rest = &name[i + 1..];
        if SIDECAR_SUFFIXES.iter().any(|s| rest == *s) {
            let base = &name[..i];
            return if is_session_id_prefix(base) { Some(base) } else { None };
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 跳过规则
// ---------------------------------------------------------------------------

/// 文件名/目录名级跳过规则（任意层级通用）。
pub fn name_skipped(name: &str) -> bool {
    if name == ".display.json" || name == ".planner-display.json" {
        return true;
    }
    if name == ".trash" {
        // 会话回收站（删除的会话残留），不迁移
        return true;
    }
    if name.starts_with(".bak") || name.contains(".bak-") {
        return true;
    }
    // 锁/租约文件：按后缀精确匹配，避免误伤 unlock.json / block.json 等
    if name.ends_with(".lock") || name.ends_with(".lease.json") || name.ends_with(".lease.lock") {
        return true;
    }
    false
}

/// 凭据文件（默认不打包，--include-secrets 才带）。
pub fn is_secret_file(name: &str) -> bool {
    name == ".env" || name.starts_with(".env.")
}

/// 相对 home 第一段是否跳过（顶层运行态目录 / 应用状态文件）。
pub fn top_level_skipped(top: &str) -> bool {
    if SKIP_TOP_LEVEL_DIRS.contains(&top) || SKIP_TOP_LEVEL_FILES.contains(&top) {
        return true;
    }
    // desktop-tabs/window/workspace/projects.json 等，应用自管
    top.starts_with("desktop-")
}

// ---------------------------------------------------------------------------
// slug
// ---------------------------------------------------------------------------

/// Windows 风格路径规范化（解析 `.`/`..`，绝对化）。
fn abs_norm(path: &str) -> String {
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(p)
    } else {
        p.to_path_buf()
    };
    let s = joined.to_string_lossy().replace('/', "\\");
    let mut out: Vec<String> = Vec::new();
    for part in s.split('\\') {
        match part {
            "" => {
                if out.is_empty() {
                    out.push(String::new()); // 保留根（C:\ 或 \\server\share 的前导空段）
                }
            }
            "." => {}
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            p => out.push(p.to_string()),
        }
    }
    out.join("\\")
}

/// 与 Reasonix 一致的 workspace-slug：绝对路径小写，分隔符(: \ /)换成 -。
/// 例：`C:\Users\Ameng\Desktop\claude_woker` -> `c--users-ameng-desktop-claude_woker`
pub fn slug_of(workspace_root: &str) -> String {
    let s = workspace_root.trim();
    if s.is_empty() {
        return String::new();
    }
    let s = abs_norm(s);
    s.replace([':', '\\', '/'], "-").to_lowercase()
}

// ---------------------------------------------------------------------------
// 哈希与 meta
// ---------------------------------------------------------------------------

pub fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 读取会话 .jsonl.meta（UTF-8），失败返回 None。
pub fn read_meta(path: &Path) -> Option<Value> {
    let raw = fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// meta 里的 turns：可能为 int 或 {count: int}，统一成 Option<i64>。
pub fn turns_of(meta: &Value) -> Option<i64> {
    let t = meta.get("turns")?;
    if let Some(n) = t.as_i64() {
        return Some(n);
    }
    t.get("count").and_then(|c| c.as_i64())
}

// ---------------------------------------------------------------------------
// 会话枚举（单会话迁移用）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct Session {
    pub id: String,
    pub slug: Option<String>,
    pub title: String,
    pub topic_id: Option<String>,
    pub workspace_root: Option<String>,
    pub meta_path: Option<String>,
    pub turns: Option<i64>,
    /// reasonix 实时轮数（<stem>.display-index.json 的 authored_turns），比 turns 可靠
    pub authored_turns: Option<i64>,
    /// 是否在 desktop-projects.json 注册（Reasonix 左侧会话列表可见）
    pub registered: bool,
    /// 会话 scope："global" 或 None
    pub scope: Option<String>,
}

impl Session {
    fn from_meta_dir(sessions_dir: &Path, slug: Option<&str>, skip_authored: bool) -> Vec<Session> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(sessions_dir) else {
            return out;
        };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(sid) = name.strip_suffix(".jsonl.meta") {
                let meta = read_meta(&entry.path()).unwrap_or_else(|| json!({}));
                let title = meta
                    .get("topic_title")
                    .or_else(|| meta.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // 轮数：reasonix 的实时计数在 <stem>.display-index.json 的 authored_turns，
                // meta.turns 只是滞后的消息计数，不可靠
                // skip_authored：home 扫描时不读 display-index（可能几十 MB），等过滤出
                // 可见会话后由 fill_authored_turns 补读——避免白读大量未注册/已删除会话
                let authored_turns = if skip_authored {
                    None
                } else {
                    let di_path = sessions_dir.join(format!("{}.display-index.json", sid));
                    read_meta(&di_path)
                        .and_then(|d| d.get("authored_turns").and_then(|v| v.as_i64()))
                };
                out.push(Session {
                    id: sid.to_string(),
                    slug: slug.map(|s| s.to_string()),
                    title,
                    topic_id: meta.get("topic_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    workspace_root: meta
                        .get("workspace_root")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    meta_path: Some(entry.path().to_string_lossy().to_string()),
                    turns: turns_of(&meta),
                    authored_turns,
                    registered: false,
                    scope: meta.get("scope").and_then(|v| v.as_str()).map(|s| s.to_string()),
                });
            }
        }
        out.sort_by(|a, b| b.id.cmp(&a.id)); // id 倒序（新的在前）
        out
    }
}

/// 扫描一个 sessions 目录下所有会话（含 recovery 分支），按 id 倒序。
pub fn list_sessions_dir(sessions_dir: &Path, slug: Option<&str>) -> Vec<Session> {
    Session::from_meta_dir(sessions_dir, slug, false)
}

/// 为已输出的会话补读 display-index 的 authored_turns（只读列表里的会话，IO 最小）。
/// 配合 from_meta_dir(skip_authored=true)：home 扫描先只读 meta 过滤出可见会话，
/// 再在这里补 display-index——避免读大量未注册/已删除会话的 display-index（可能几十 MB）。
fn fill_authored_turns(s: &mut Session) {
    if s.authored_turns.is_some() {
        return;
    }
    let Some(mp) = s.meta_path.as_deref() else {
        return;
    };
    let p = Path::new(mp);
    let Some(parent) = p.parent() else { return };
    let Some(stem) = p
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".jsonl.meta"))
    else {
        return;
    };
    let di = parent.join(format!("{}.display-index.json", stem));
    if let Some(d) = read_meta(&di) {
        s.authored_turns = d.get("authored_turns").and_then(|v| v.as_i64());
    }
}

/// 会话扫描结果缓存：避免频繁点「列出会话」每次都全量扫盘（30 秒内复用）。
static SESSIONS_HOME_CACHE: Mutex<Option<(String, SystemTime, Vec<Session>)>> = Mutex::new(None);

/// 扫描 `<home>/projects/*/sessions/` 下会话，只保留 Reasonix 左侧可见的，并**按
/// Reasonix 的显示顺序输出**（desktop-projects.json 的 projects[] 数组顺序 →
/// 每个项目 topics[] 数组顺序），保证与桌面端左侧一致。
///
/// 可见判定：会话 meta 的 workspace_root 必须是已注册项目（projects[].root），
/// 且 topic 存在、未被删除（deletedTopics）。slug 按归属项目算，而不是文件所在目录。
/// `force=true` 时忽略缓存强制重新扫描（点「列出会话」按钮）。
pub fn list_sessions_home(home: &Path, force: bool, project: Option<&str>) -> Vec<Session> {
    // 缓存 key 含项目（全部项目 / 单项目分开缓存）
    let key = format!("{}|{}", home.to_string_lossy(), project.unwrap_or("*"));
    if !force {
        if let Ok(g) = SESSIONS_HOME_CACHE.lock() {
            if let Some((k, t, v)) = g.as_ref() {
                if *k == key && t.elapsed().unwrap_or_default().as_secs() < 30 {
                    return v.clone();
                }
            }
        }
    }
    let v = list_sessions_home_scan(home, project);
    if let Ok(mut g) = SESSIONS_HOME_CACHE.lock() {
        *g = Some((key, SystemTime::now(), v.clone()));
    }
    v
}

fn list_sessions_home_scan(home: &Path, project: Option<&str>) -> Vec<Session> {
    let projects = home.join("projects");
    let Ok(rd) = fs::read_dir(&projects) else {
        return Vec::new();
    };
    let dp = load_desktop_projects(home);
    // 已知 workspace：规范化小写 root → 归属项目 slug
    let known: HashMap<String, String> = dp
        .projects
        .iter()
        .map(|p| (abs_norm(&p.root).to_lowercase(), slug_of(&p.root)))
        .collect();
    // 1) 目录扫描收集全部会话（reasonix 可见判定 + slug 归属）
    let mut all: Vec<Session> = Vec::new();
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for slug_dir in dirs {
        // 指定项目时只扫该项目目录
        if let Some(p) = project {
            let name = slug_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name != p {
                continue;
            }
        }
        let sdir = slug_dir.join("sessions");
        if sdir.is_dir() {
            // skip_authored：阶段 1 只读 meta 过滤，display-index 在过滤出可见会话后补读
            let mut sessions = Session::from_meta_dir(&sdir, None, true);
            // Global 会话：scope=global，无 workspace_root，用 globalTopics 判定可见
            // 普通会话：workspace_root 必须属于已注册项目
            sessions.retain(|s| {
                let is_global = s.scope.as_deref() == Some("global");
                if is_global {
                    // Global 会话：topic 必须在 globalTopics 且未删除
                    match s.topic_id.as_deref() {
                        Some(t) => dp.visible_topics.contains(t) && !dp.deleted_topics.contains(t),
                        // 无 topic_id 的旧 Global 会话：也视为可见（legacy 会话无 topic_id）
                        None => true,
                    }
                } else {
                    // 普通会话：workspace_root 必须属于已注册项目
                    let ws_ok = s
                        .workspace_root
                        .as_deref()
                        .map(|w| known.contains_key(&abs_norm(w).to_lowercase()))
                        .unwrap_or(false);
                    if !ws_ok {
                        return false;
                    }
                    match s.topic_id.as_deref() {
                        Some(t) => !dp.deleted_topics.contains(t),
                        None => false,
                    }
                }
            });
            // Reasonix 列表规则：turns=0 的空会话不显示（reasonix 侧同样过滤）
            sessions.retain(|s| s.turns.map(|t| t > 0).unwrap_or(true));
            for s in &mut sessions {
                if s.scope.as_deref() == Some("global") {
                    // Global 会话：slug = 物理目录名（如 e--reasonixdata-global-workspace）
                    s.slug = Some(
                        slug_dir
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                    );
                    s.registered = true;
                } else if let Some(ws) = s.workspace_root.as_deref() {
                    if let Some(sl) = known.get(&abs_norm(ws).to_lowercase()) {
                        s.slug = Some(sl.clone());
                        s.registered = true;
                    }
                }
            }
            all.extend(sessions);
        }
    }
    // 2) 按会话 id 倒序输出（新会话在前，与 Reasonix 左侧一致）
    //    同一 topic 在多个项目注册时（双注册），每个项目各输出一次（与 reasonix 一致）
    let mut out = Vec::new();
    // 构建 root → ProjectEntry 的映射，供按 root 查找
    let proj_by_root: HashMap<String, &ProjectEntry> = dp
        .projects
        .iter()
        .map(|p| (abs_norm(&p.root).to_lowercase(), p))
        .collect();
    // 1) Global 会话：按 desktop-project-tree-organization.json 的 global.topicOrder 输出
    //    兜底 dp.global_topics（tree 文件不存在时）
    let global_order = if dp.global_topic_order.is_empty() {
        dp.global_topics.clone()
    } else {
        dp.global_topic_order.clone()
    };
    let mut project_topic_set: HashSet<String> = HashSet::new();
    for p in &dp.projects {
        for t in &p.topics {
            project_topic_set.insert(t.clone());
        }
    }
    for gt in &global_order {
        if project_topic_set.contains(gt.as_str()) {
            continue;
        }
        let mut best: Option<(Session, String, u64)> = None;
        for s in &all {
            if s.topic_id.as_deref() != Some(gt.as_str()) {
                continue;
            }
            let meta_val = s.meta_path.as_deref().and_then(|mp| read_meta(Path::new(mp)));
            let upd = meta_val
                .as_ref()
                .and_then(|m| m.get("updated_at").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_default();
            let rev = meta_val
                .as_ref()
                .and_then(|m| m.get("revision").and_then(|v| v.as_u64()))
                .unwrap_or(0);
            let replace = match &best {
                None => true,
                Some((_, b_upd, b_rev)) => {
                    if upd != *b_upd {
                        upd > *b_upd
                    } else {
                        rev > *b_rev
                    }
                }
            };
            if replace {
                best = Some((s.clone(), upd, rev));
            }
        }
        if let Some((mut s, _, _)) = best {
            out.push(s);
        }
    }
    // 2) 项目会话：按 tree-organization 的 project_order 排列项目，
    //    项目内按 project_topic_orders 排列 topic；兜底 dp.projects 顺序
    let project_roots: Vec<String> = if dp.project_order.is_empty() {
        dp.projects.iter().map(|p| abs_norm(&p.root).to_lowercase()).collect()
    } else {
        dp.project_order.clone()
    };
    for root_norm in &project_roots {
        let Some(p) = proj_by_root.get(root_norm.as_str()) else {
            continue;
        };
        let p_slug = slug_of(&p.root);
        let p_norm = abs_norm(&p.root).to_lowercase();
        // topic 顺序：tree organization 优先，兜底 dp.projects 里的 topics
        let topic_order: Vec<String> = dp
            .project_topic_orders
            .get(root_norm)
            .cloned()
            .unwrap_or_else(|| p.topics.clone());
        for t in &topic_order {
            let mut best: Option<(Session, bool, String, u64)> = None;
            for s in &all {
                if s.topic_id.as_deref() != Some(t.as_str()) {
                    continue;
                }
                let ws_match = s
                    .workspace_root
                    .as_deref()
                    .map(|w| abs_norm(w).to_lowercase() == p_norm)
                    .unwrap_or(false);
                let meta_val = s.meta_path.as_deref().and_then(|mp| read_meta(Path::new(mp)));
                let upd = meta_val
                    .as_ref()
                    .and_then(|m| m.get("updated_at").and_then(|v| v.as_str()).map(String::from))
                    .unwrap_or_default();
                let rev = meta_val
                    .as_ref()
                    .and_then(|m| m.get("revision").and_then(|v| v.as_u64()))
                    .unwrap_or(0);
                let replace = match &best {
                    None => true,
                    Some((b, b_ws, b_upd, b_rev)) => {
                        if ws_match != *b_ws {
                            ws_match
                        } else if upd != *b_upd {
                            upd > *b_upd
                        } else {
                            rev > *b_rev
                        }
                    }
                };
                if replace {
                    best = Some((s.clone(), ws_match, upd, rev));
                }
            }
            if let Some((mut s, _, _, _)) = best {
                s.slug = Some(p_slug.clone());
                out.push(s);
            }
        }
    }
    // 只给最终可见会话补读 display-index（避免白读大量未注册会话的，可能几十 MB）
    for s in &mut out {
        fill_authored_turns(s);
    }
    // 顺序 = desktop-project-tree-organization.json 的排列顺序，与 Reasonix 左侧一致
    out
}

/// 从迁移工具导出的备份 zip（manifest.json）列出会话，按 id 倒序。
pub fn list_sessions_zip(zip_path: &Path) -> Vec<Session> {
    let Ok(f) = fs::File::open(zip_path) else {
        return Vec::new();
    };
    let Ok(mut zf) = zip::ZipArchive::new(f) else {
        return Vec::new();
    };
    let Ok(manifest) = zf.by_name("manifest.json") else {
        return Vec::new();
    };
    let Ok(v): Result<Value, _> = serde_json::from_reader(manifest) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for s in v.get("sessions").and_then(|x| x.as_array()).into_iter().flatten() {
        out.push(Session {
            id: s.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            slug: s.get("slug").and_then(|v| v.as_str()).map(|s| s.to_string()),
            title: s.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            workspace_root: s
                .get("workspace_root")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            topic_id: None,
            meta_path: None,
            turns: None,
            authored_turns: None,
            registered: false,
            scope: None,
        });
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// 过滤 recovery 分支，并按 topic_id 去重（同 topic 多物理副本优先保留已注册项目里的）。
pub fn dedupe_main_sessions(
    candidates: Vec<Session>,
    registered_slugs: Option<&HashSet<String>>,
) -> Vec<Session> {
    let mains: Vec<Session> = candidates
        .into_iter()
        .filter(|s| !is_recovery_branch(&s.id))
        .collect();
    let mut seen: Vec<Session> = Vec::new();
    for s in mains {
        let key = s.topic_id.clone().unwrap_or_else(|| s.id.clone());
        if let Some(pos) = seen.iter().position(|x| {
            x.topic_id.clone().unwrap_or_else(|| x.id.clone()) == key
        }) {
            if let Some(reg) = registered_slugs {
                let cur_reg = s.slug.as_deref().map(|x| reg.contains(x)).unwrap_or(false);
                let prev_reg = seen[pos]
                    .slug
                    .as_deref()
                    .map(|x| reg.contains(x))
                    .unwrap_or(false);
                if cur_reg && !prev_reg {
                    seen[pos] = s;
                }
            }
        } else {
            seen.push(s);
        }
    }
    seen.sort_by(|a, b| b.id.cmp(&a.id));
    seen
}

// ---------------------------------------------------------------------------
// 项目注册（desktop-projects.json，单会话迁移用）
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct DesktopProjects {
    pub projects: Vec<ProjectEntry>,
    pub visible_topics: HashSet<String>,
    pub deleted_topics: HashSet<String>,
    pub global_topics: Vec<String>,
    /// desktop-project-tree-organization.json 的 Global topic 顺序
    pub global_topic_order: Vec<String>,
    /// desktop-project-tree-organization.json 的项目 topic 顺序（root → topicOrder）
    pub project_topic_orders: HashMap<String, Vec<String>>,
    /// desktop-project-tree-organization.json 的项目排列顺序（root 列表）
    pub project_order: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub root: String,
    pub topics: Vec<String>,
}

/// 读 home 的 desktop-projects.json；文件不存在或损坏时返回空结构。
pub fn load_desktop_projects(home: &Path) -> DesktopProjects {
    let mut dp = DesktopProjects::default();
    let path = home.join("desktop-projects.json");
    let Ok(raw) = fs::read(&path) else {
        return dp;
    };
    let Ok(data): Result<Value, _> = serde_json::from_slice(&raw) else {
        return dp;
    };
    if let Some(projects) = data.get("projects").and_then(|v| v.as_array()) {
        for p in projects {
            dp.projects.push(ProjectEntry {
                root: p.get("root").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                topics: p
                    .get("topics")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
            });
        }
    }
    for p in &dp.projects {
        dp.visible_topics.extend(p.topics.iter().cloned());
    }
    if let Some(gt) = data.get("globalTopics").and_then(|v| v.as_array()) {
        let topics: Vec<String> = gt.iter().filter_map(|t| t.as_str().map(String::from)).collect();
        dp.visible_topics.extend(topics.iter().cloned());
        dp.global_topics = topics;
    }
    if let Some(dt) = data.get("deletedTopics").and_then(|v| v.as_array()) {
        dp.deleted_topics
            .extend(dt.iter().filter_map(|t| t.as_str().map(String::from)));
    }
    // 加载 desktop-project-tree-organization.json（侧边栏真实顺序来源）
    let tree_path = home.join("desktop-project-tree-organization.json");
    if let Ok(tree_raw) = fs::read(&tree_path) {
        if let Ok(tree) = serde_json::from_slice::<Value>(&tree_raw) {
            // Global topic 顺序
            if let Some(gt) = tree.get("global").and_then(|g| g.get("topicOrder")).and_then(|v| v.as_array()) {
                dp.global_topic_order = gt.iter().filter_map(|t| t.as_str().map(String::from)).collect();
            }
            // 项目排列顺序 + 每个项目的 topic 顺序
            if let Some(projs) = tree.get("projects").and_then(|v| v.as_array()) {
                for p in projs {
                    if let Some(root) = p.get("root").and_then(|v| v.as_str()) {
                        let root_norm = abs_norm(root).to_lowercase();
                        dp.project_order.push(root_norm.clone());
                        if let Some(to) = p.get("topicOrder").and_then(|v| v.as_array()) {
                            dp.project_topic_orders.insert(
                                root_norm,
                                to.iter().filter_map(|t| t.as_str().map(String::from)).collect(),
                            );
                        }
                    }
                }
            }
        }
    }
    dp
}

/// 项目列表条目：slug + 已注册会话 topic 数（= Reasonix 桌面端左侧可见会话数）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectInfo {
    pub slug: String,
    pub count: usize,
}

/// 只读 desktop-projects.json + desktop-project-tree-organization.json 的项目 slug + 会话数。
/// 顺序与 Reasonix 左侧栏一致：Global 在最上面，项目按 tree-organization 排列。
pub fn list_projects(home: &Path) -> Vec<ProjectInfo> {
    let dp = load_desktop_projects(home);
    let mut out: Vec<ProjectInfo> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // 1) Global 项目（最上面）
    if !dp.global_topics.is_empty() {
        let projects_dir = home.join("projects");
        if let Ok(rd) = fs::read_dir(&projects_dir) {
            for entry in rd.flatten() {
                let dir_name = entry.file_name().to_string_lossy().to_string();
                if seen.contains(&dir_name) {
                    continue;
                }
                let sdir = entry.path().join("sessions");
                if sdir.is_dir() {
                    let has_global = fs::read_dir(&sdir)
                        .map(|rd| {
                            rd.flatten().any(|e| {
                                let n = e.file_name().to_string_lossy().to_string();
                                if !n.ends_with(".jsonl.meta") {
                                    return false;
                                }
                                read_meta(&e.path())
                                    .and_then(|m| m.get("scope").and_then(|v| v.as_str()).map(|s| s == "global"))
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false);
                    if has_global {
                        let count = dp
                            .global_topics
                            .iter()
                            .filter(|t| !dp.deleted_topics.contains(t.as_str()))
                            .count();
                        out.push(ProjectInfo { slug: dir_name.clone(), count });
                        seen.insert(dir_name);
                        break;
                    }
                }
            }
        }
    }
    // 2) 项目：按 tree-organization 的 project_order 排列，兜底 dp.projects 顺序
    let proj_by_root: HashMap<String, &ProjectEntry> = dp
        .projects
        .iter()
        .map(|p| (abs_norm(&p.root).to_lowercase(), p))
        .collect();
    let project_roots: Vec<String> = if dp.project_order.is_empty() {
        dp.projects.iter().map(|p| abs_norm(&p.root).to_lowercase()).collect()
    } else {
        dp.project_order.clone()
    };
    for root_norm in &project_roots {
        let Some(p) = proj_by_root.get(root_norm.as_str()) else {
            continue;
        };
        let s = slug_of(&p.root);
        if s.is_empty() || seen.contains(&s) {
            continue;
        }
        seen.insert(s.clone());
        let count = p
            .topics
            .iter()
            .filter(|t| !dp.deleted_topics.contains(t.as_str()))
            .collect::<HashSet<_>>()
            .len();
        out.push(ProjectInfo { slug: s, count });
    }
    out
}

/// 目标 home 的 desktop-projects.json 是否已注册 workspace_root 为项目。
pub fn project_is_registered(home: &Path, workspace_root: &str) -> bool {
    let path = home.join("desktop-projects.json");
    let Ok(raw) = fs::read(&path) else {
        return false;
    };
    let Ok(data): Result<Value, _> = serde_json::from_slice(&raw) else {
        return false;
    };
    let ws = abs_norm(workspace_root).to_lowercase();
    data.get("projects")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|p| {
                abs_norm(p.get("root").and_then(|v| v.as_str()).unwrap_or("")).to_lowercase() == ws
            })
        })
        .unwrap_or(false)
}

/// 确保目标 home 的 desktop-projects.json 已注册 workspace_root 为项目。
/// 返回 true 表示本次新增了注册；false=已注册或注册表不存在（全新 home 由应用自建）。
pub fn ensure_project_registered(home: &Path, workspace_root: &str) -> bool {
    let path = home.join("desktop-projects.json");
    let Ok(raw) = fs::read(&path) else {
        return false;
    };
    let Ok(mut data): Result<Value, _> = serde_json::from_slice(&raw) else {
        return false;
    };
    let ws = abs_norm(workspace_root);
    let ws_cmp = ws.to_lowercase();
    let already = data
        .get("projects")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|p| {
                abs_norm(p.get("root").and_then(|v| v.as_str()).unwrap_or("")).to_lowercase()
                    == ws_cmp
            })
        })
        .unwrap_or(false);
    if already {
        return false;
    }
    let entry = json!({"root": ws, "topics": []});
    match data.get_mut("projects") {
        Some(Value::Array(arr)) => arr.push(entry),
        _ => {
            data["projects"] = json!([entry]);
        }
    }
    // 原子替换（.tmp + rename），避免半写
    let tmp = path.with_file_name("desktop-projects.json.tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(&data).unwrap_or_default()).is_err() {
        return false;
    }
    if fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_prefix() {
        assert!(is_session_id_prefix("20260808-155503.898846400"));
        assert!(is_session_id_prefix("20260808-155503.898846400-model-x"));
        assert!(!is_session_id_prefix("2026088-155503.898846400")); // 少一位
        assert!(!is_session_id_prefix("a0260808-155503.898846400"));
        assert!(!is_session_id_prefix("20260808-15550.898846400")); // 时间少一位
    }

    #[test]
    fn recovery_branch() {
        assert!(is_recovery_branch("20260808-155503.898846400-9974-recovery-abc123"));
        assert!(!is_recovery_branch("20260808-155503.898846400"));
        // 直接后缀：剥掉 -recovery-hex 即主 id
        assert_eq!(
            base_session_id("20260809-100000.123456789-model-a-recovery-abc123"),
            "20260809-100000.123456789-model-a"
        );
        // 带中间节点（fork 后代）：Python 只剥 -recovery-hex 尾巴，保留中间节点
        assert_eq!(
            base_session_id("20260808-155503.898846400-9974-recovery-abc123"),
            "20260808-155503.898846400-9974"
        );
        assert_eq!(base_session_id("20260808-155503.898846400"), "20260808-155503.898846400");
        // v1.24 起 fork 分支 recovery 带 writer 后缀：`-recovery-<hex>-<hex>`（双 hex）
        assert!(is_recovery_branch(
            "20260807-145055.936123800-deepseek-deepseek-v4-flash-21c719480427-535086b70a5b-8-7da16dd807da-recovery-ed225b5df26e78a1-4c319b5c22a0"
        ));
        assert_eq!(
            base_session_id("20260807-145055.936123800-deepseek-deepseek-v4-flash-21c719480427-535086b70a5b-8-7da16dd807da-recovery-ed225b5df26e78a1-4c319b5c22a0"),
            "20260807-145055.936123800-deepseek-deepseek-v4-flash-21c719480427-535086b70a5b-8-7da16dd807da"
        );
        // `-recovery-` 后带非 hex 尾巴不算 recovery
        assert!(!is_recovery_branch("20260808-155503.898846400-recovery-notes"));
    }

    #[test]
    fn sidecar_stripping() {
        assert_eq!(session_id_of("20260808-155503.898846400.jsonl"), Some("20260808-155503.898846400"));
        assert_eq!(
            session_id_of("20260808-155503.898846400.jsonl.meta"),
            Some("20260808-155503.898846400")
        );
        assert_eq!(
            session_id_of("20260808-155503.898846400.events.jsonl"),
            Some("20260808-155503.898846400")
        );
        assert_eq!(
            session_id_of("20260808-155503.898846400.ckpt"),
            Some("20260808-155503.898846400")
        );
        // 事件文件优先匹配最长后缀，不能把 id 截成 `...898846400.events`
        assert_eq!(session_id_of("20260808-155503.898846400.events.jsonl"), Some("20260808-155503.898846400"));
        // fork recovery 分支的 jsonl
        assert_eq!(
            session_id_of("20260808-155503.898846400-9974-recovery-abc123.jsonl"),
            Some("20260808-155503.898846400-9974-recovery-abc123")
        );
        // v1.24 双 hex recovery 分支的 jsonl 同样剥出完整 id
        assert_eq!(
            session_id_of("20260807-145055.936123800-deepseek-deepseek-v4-flash-21c719480427-535086b70a5b-8-7da16dd807da-recovery-ed225b5df26e78a1-4c319b5c22a0.jsonl"),
            Some("20260807-145055.936123800-deepseek-deepseek-v4-flash-21c719480427-535086b70a5b-8-7da16dd807da-recovery-ed225b5df26e78a1-4c319b5c22a0")
        );
        assert_eq!(session_id_of("notes.txt"), None);
        assert_eq!(session_id_of("desktop-workspace"), None);
    }

    #[test]
    fn skip_rules() {
        assert!(name_skipped(".display.json"));
        assert!(name_skipped(".planner-display.json"));
        assert!(name_skipped(".trash"));
        assert!(name_skipped("x.jsonl.lock"));
        assert!(name_skipped("x.jsonl.lease.json"));
        assert!(name_skipped("desktop-projects.json.bak-2026-08-09"));
        assert!(!name_skipped("unlock.json"));
        assert!(!name_skipped("block.json"));
        assert!(!name_skipped("20260808-155503.898846400.jsonl"));
        assert!(is_secret_file(".env"));
        assert!(is_secret_file(".env.local"));
        assert!(!is_secret_file("config.toml"));
        assert!(top_level_skipped("cache"));
        assert!(top_level_skipped("machine-id.key"));
        assert!(top_level_skipped("desktop-tabs.json"));
        assert!(!top_level_skipped("projects"));
        assert!(!top_level_skipped("memory"));
    }

    #[test]
    fn slug() {
        // Windows 风格
        assert_eq!(slug_of(r"C:\Users\Ameng\Desktop\claude_woker"), "c--users-ameng-desktop-claude_woker");
        assert_eq!(slug_of(r"E:\ReasonixData"), "e--reasonixdata");
        // 去尾分隔符
        assert_eq!(slug_of(r"E:\ReasonixData\"), "e--reasonixdata");
        // 小写化
        assert_eq!(slug_of(r"D:\PROJECTS\Web"), "d--projects-web");
        assert_eq!(slug_of(""), "");
    }

    #[test]
    fn sha256() {
        assert_eq!(
            sha256_bytes(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn dedupe_by_topic() {
        let cands = vec![
            Session { id: "20260809-100000.111111111-model-a".into(), slug: Some("s1".into()), title: "A".into(), topic_id: Some("t1".into()), workspace_root: None, meta_path: None, turns: None, authored_turns: None, registered: false, scope: None },
            Session { id: "20260809-100000.111111111-model-a-recovery-abc".into(), slug: Some("s1".into()), title: "A".into(), topic_id: None, workspace_root: None, meta_path: None, turns: None, authored_turns: None, registered: false, scope: None },
            Session { id: "20260809-090000.111111111-model-b".into(), slug: Some("s2".into()), title: "B".into(), topic_id: Some("t2".into()), workspace_root: None, meta_path: None, turns: None, authored_turns: None, registered: false, scope: None },
        ];
        let out = dedupe_main_sessions(cands, None);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|s| !is_recovery_branch(&s.id)));
    }
}

// ---------------------------------------------------------------------------
// 会话分组视图（同事建议：所见即所得——一组 = Reasonix 左侧一个会话）
// ---------------------------------------------------------------------------

/// 一组会话 = 一个 topic（或同名 stem 前缀）下的活跃分支 + 全部历史副本。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionGroupView {
    pub topic_id: String,
    pub title: String,
    /// 活跃分支（updated_at 最大 → revision 最大；可能带 -recovery- 文件名）
    pub active: Session,
    /// 组内全部分支（含活跃），按 id 倒序
    pub branches: Vec<Session>,
    pub branch_count: usize,
    pub total_bytes: u64,
}

/// 按项目分组列出会话（含全部 recovery 分支），与 Reasonix ListSessions 一致：
/// 组内活跃分支由 meta.updated_at + revision 判定，绝不凭文件名是否带 -recovery-。
pub fn list_sessions_groups(home: &Path) -> Vec<SessionGroupView> {
    let projects = home.join("projects");
    let Ok(rd) = fs::read_dir(&projects) else {
        return Vec::new();
    };
    let dp = load_desktop_projects(home);
    let known: HashMap<String, String> = dp
        .projects
        .iter()
        .map(|p| (abs_norm(&p.root).to_lowercase(), slug_of(&p.root)))
        .collect();
    let mut all: Vec<Session> = Vec::new();
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for slug_dir in dirs {
        let sdir = slug_dir.join("sessions");
        if sdir.is_dir() {
            // skip_authored：先只读 meta 过滤，display-index 在分组输出后补读
            let mut sessions = Session::from_meta_dir(&sdir, None, true);
            // Global 会话：scope=global，用 globalTopics 判定可见
            // 普通会话：workspace_root 必须属于已注册项目
            sessions.retain(|s| {
                let is_global = s.scope.as_deref() == Some("global");
                if is_global {
                    match s.topic_id.as_deref() {
                        Some(t) => dp.visible_topics.contains(t) && !dp.deleted_topics.contains(t),
                        None => true,
                    }
                } else {
                    let ws_ok = s
                        .workspace_root
                        .as_deref()
                        .map(|w| known.contains_key(&abs_norm(w).to_lowercase()))
                        .unwrap_or(false);
                    if !ws_ok {
                        return false;
                    }
                    match s.topic_id.as_deref() {
                        Some(t) => !dp.deleted_topics.contains(t),
                        None => false,
                    }
                }
            });
            // 空会话（turns=0）reasonix 不显示
            sessions.retain(|s| s.turns.map(|t| t > 0).unwrap_or(true));
            for s in &mut sessions {
                if s.scope.as_deref() == Some("global") {
                    s.slug = Some(
                        slug_dir
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                    );
                    s.registered = true;
                } else if let Some(ws) = s.workspace_root.as_deref() {
                    if let Some(sl) = known.get(&abs_norm(ws).to_lowercase()) {
                        s.slug = Some(sl.clone());
                        s.registered = true;
                    }
                }
            }
            all.extend(sessions);
        }
    }
    // 分组键：(项目 slug, topic_id 或 stem 前缀)
    let mut grouped: HashMap<(String, String), Vec<Session>> = HashMap::new();
    for s in &all {
        let key = s
            .topic_id
            .clone()
            .unwrap_or_else(|| base_session_id(&s.id).to_string());
        grouped
            .entry((s.slug.clone().unwrap_or_default(), key))
            .or_default()
            .push(s.clone());
    }
    let mut views: Vec<SessionGroupView> = Vec::new();
    // 用 desktop-project-tree-organization.json 的排列顺序（兜底 dp.projects 顺序）
    let proj_by_root: HashMap<String, &ProjectEntry> = dp
        .projects
        .iter()
        .map(|p| (abs_norm(&p.root).to_lowercase(), p))
        .collect();
    // 1) Global 会话：按 tree-organization 的 global.topicOrder 输出
    let global_order = if dp.global_topic_order.is_empty() {
        dp.global_topics.clone()
    } else {
        dp.global_topic_order.clone()
    };
    let mut project_topic_set: HashSet<String> = HashSet::new();
    for p in &dp.projects {
        for t in &p.topics {
            project_topic_set.insert(t.clone());
        }
    }
    for gt in &global_order {
        if project_topic_set.contains(gt.as_str()) {
            continue;
        }
        let mut branches: Vec<Session> = all
            .iter()
            .filter(|s| s.topic_id.as_deref() == Some(gt.as_str()))
            .cloned()
            .collect();
        if branches.is_empty() {
            continue;
        }
        branches.sort_by(|a, b| b.id.cmp(&a.id));
        let rank = |s: &Session| {
            let m = s.meta_path.as_deref().and_then(|mp| read_meta(Path::new(mp)));
            (
                m.as_ref()
                    .and_then(|v| v.get("updated_at"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                m.as_ref()
                    .and_then(|v| v.get("revision"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            )
        };
        let active = branches
            .iter()
            .cloned()
            .max_by(|a, b| rank(a).cmp(&rank(b)))
            .unwrap_or_else(|| branches[0].clone());
        let title = if !active.title.is_empty() {
            active.title.clone()
        } else {
            branches
                .iter()
                .find(|b| !b.title.is_empty())
                .map(|b| b.title.clone())
                .unwrap_or_default()
        };
        let mut total_bytes = 0u64;
        for b in &branches {
            total_bytes += stem_total_bytes(b.meta_path.as_deref());
        }
        views.push(SessionGroupView {
            topic_id: gt.clone(),
            title,
            active,
            branch_count: branches.len(),
            branches,
            total_bytes,
        });
    }
    // 2) 项目会话：按 tree-organization 排列
    let project_roots: Vec<String> = if dp.project_order.is_empty() {
        dp.projects.iter().map(|p| abs_norm(&p.root).to_lowercase()).collect()
    } else {
        dp.project_order.clone()
    };
    for root_norm in &project_roots {
        let Some(p) = proj_by_root.get(root_norm.as_str()) else {
            continue;
        };
        let p_slug = slug_of(&p.root);
        let topic_order: Vec<String> = dp
            .project_topic_orders
            .get(root_norm)
            .cloned()
            .unwrap_or_else(|| p.topics.clone());
        let mut used: HashSet<String> = HashSet::new();
        for t in &topic_order {
            if used.contains(t) {
                continue;
            }
            let Some(mut branches) = grouped.get(&(p_slug.clone(), t.clone())).cloned() else {
                continue;
            };
            used.insert(t.clone());
            branches.sort_by(|a, b| b.id.cmp(&a.id));
            let rank = |s: &Session| {
                let m = s.meta_path.as_deref().and_then(|mp| read_meta(Path::new(mp)));
                (
                    m.as_ref()
                        .and_then(|v| v.get("updated_at"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    m.as_ref()
                        .and_then(|v| v.get("revision"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                )
            };
            let active = branches
                .iter()
                .cloned()
                .max_by(|a, b| rank(a).cmp(&rank(b)))
                .unwrap_or_else(|| branches[0].clone());
            let title = if !active.title.is_empty() {
                active.title.clone()
            } else {
                branches
                    .iter()
                    .find(|b| !b.title.is_empty())
                    .map(|b| b.title.clone())
                    .unwrap_or_default()
            };
            let mut total_bytes = 0u64;
            for b in &branches {
                total_bytes += stem_total_bytes(b.meta_path.as_deref());
            }
            views.push(SessionGroupView {
                topic_id: t.clone(),
                title,
                active,
                branch_count: branches.len(),
                branches,
                total_bytes,
            });
        }
    }
    // 只给输出的分支/活跃会话补读 display-index（避免白读大量未注册会话的）
    for v in &mut views {
        for b in &mut v.branches {
            fill_authored_turns(b);
        }
        fill_authored_turns(&mut v.active);
    }
    views
}

/// 统计一个会话 stem 的所有同名文件/目录大小之和（不含子目录递归，近似即可）。
fn stem_total_bytes(meta_path: Option<&str>) -> u64 {
    let Some(mp) = meta_path else { return 0 };
    let p = Path::new(mp);
    let Some(dir) = p.parent() else { return 0 };
    let Some(fname) = p.file_name().map(|s| s.to_string_lossy().to_string()) else {
        return 0;
    };
    let Some(stem) = fname.strip_suffix(".jsonl.meta") else { return 0 };
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    let mut total = 0u64;
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n == stem || n.starts_with(&format!("{}.", stem)) || n.starts_with(&format!("{}-", stem)) {
            if let Ok(md) = e.metadata() {
                if md.is_file() {
                    total += md.len();
                }
            }
        }
    }
    total
}
