//! 导出：扫描 home → zip 打包 + manifest；以及备份完整性校验（verify）。
//!
//! 逻辑与 Python 版 `reasonix-migrate-export.py` / `reasonix-migrate-verify.py` 对齐。

use chrono::{DateTime, Local, TimeZone};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::common::{
    base_session_id, is_recovery_branch, is_secret_file, name_skipped, read_meta, session_id_of,
    sha256_bytes, top_level_skipped, FORMAT_VERSION,
};

/// 项目会话目录下的非会话内容（整体打包，不做 id 解析）。
pub const SESSION_DIR_NON_SESSION: &str = "subagents";

#[derive(Debug, Clone)]
pub struct SessionGroup {
    /// 相对 sessions 目录的 posix 路径（文件或目录条目）
    pub files: Vec<String>,
    /// 该族内文件的最大修改时间
    pub max_mtime: SystemTime,
    /// recovery 分支 id 列表
    pub recovery: Vec<String>,
}

impl Default for SessionGroup {
    fn default() -> Self {
        SessionGroup {
            files: Vec::new(),
            max_mtime: UNIX_EPOCH,
            recovery: Vec::new(),
        }
    }
}

pub struct ScanResult {
    /// {主会话id: group}
    pub groups: HashMap<String, SessionGroup>,
    /// 非会话内容（subagents 等）相对 sessions 目录的 posix 路径
    pub non_session: Vec<String>,
}

fn fname(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// 扫描 sessions 目录（两阶段 id 归属，与 Python 一致）：
///   阶段 1 用已知 sidecar 识别会话 id 集合；
///   阶段 2 任何以 `<id>.` 开头的条目（含未知 sidecar / ckpt / jobs 目录）归入该族。
pub fn scan_sessions(sessions_dir: &Path) -> ScanResult {
    let mut groups: HashMap<String, SessionGroup> = HashMap::new();
    let mut non_session: Vec<String> = Vec::new();
    if !sessions_dir.is_dir() {
        return ScanResult { groups, non_session };
    }
    let entries: Vec<PathBuf> = sorted_entries(sessions_dir)
        .into_iter()
        .filter(|c| !name_skipped(&fname(c)))
        .collect();

    // 阶段 1：已知 sidecar 识别 id
    let mut known_ids: HashSet<String> = HashSet::new();
    for c in &entries {
        if c.is_dir() && fname(c) == SESSION_DIR_NON_SESSION {
            continue;
        }
        if let Some(sid) = session_id_of(&fname(c)) {
            known_ids.insert(sid.to_string());
        }
    }

    // 阶段 2：归属
    for c in &entries {
        let name = fname(c);
        if c.is_dir() && name == SESSION_DIR_NON_SESSION {
            // subagents 整体打包
            collect_tree(c, sessions_dir, &mut non_session);
            continue;
        }
        let sid: Option<String> = if c.is_dir() {
            // `<id>.ckpt` / `<id>.jobs` 目录
            known_ids
                .iter()
                .find(|known| name.starts_with(&format!("{}.", known)))
                .cloned()
        } else {
            match session_id_of(&name) {
                Some(s) => Some(s.to_string()),
                None => known_ids
                    .iter()
                    .find(|known| name.starts_with(&format!("{}.", known)))
                    .cloned(),
            }
        };
        let Some(sid) = sid else { continue };
        let base = base_session_id(&sid).to_string();
        let g = groups.entry(base.clone()).or_default();
        g.files.push(c.strip_prefix(sessions_dir).unwrap_or(c).to_string_lossy().replace('\\', "/"));
        if c.is_file() {
            if let Ok(meta) = c.metadata() {
                if let Ok(mt) = meta.modified() {
                    if mt > g.max_mtime {
                        g.max_mtime = mt;
                    }
                }
            }
        }
        if is_recovery_branch(&sid) && !g.recovery.contains(&sid) {
            g.recovery.push(sid);
        }
    }
    ScanResult { groups, non_session }
}

fn collect_tree(dir: &Path, sessions_dir: &Path, out: &mut Vec<String>) {
    for ch in sorted_entries(dir) {
        let name = fname(&ch);
        if name_skipped(&name) {
            continue;
        }
        let rel = ch.strip_prefix(sessions_dir).unwrap_or(&ch).to_string_lossy().replace('\\', "/");
        out.push(rel);
        if ch.is_dir() {
            collect_tree(&ch, sessions_dir, out);
        }
    }
}

/// --project 过滤：slug 精确 / 小写精确 / 小写以 `-f` 结尾。
fn match_project(slug: &str, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let low = slug.to_lowercase();
    filters.iter().any(|f| {
        let fl = f.to_lowercase();
        slug == f || low == fl || low.ends_with(&format!("-{}", fl))
    })
}

/// --since 过滤：解析 ISO 时间（与 Python fromisoformat 常见用法对齐），
/// 无时区时按本地时区解释。
fn parse_since(s: &str) -> Option<SystemTime> {
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
        });
    if let Some(n) = naive {
        return Local
            .from_local_datetime(&n)
            .single()
            .map(|dt| dt.into());
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Local).into())
}

fn iso_seconds(t: SystemTime) -> String {
    let dt: DateTime<Local> = t.into();
    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// 顶层条目（应用跳过规则），返回 (abs_path, rel, is_dir)。
fn collect_top_level(source: &Path, include_secrets: bool) -> Vec<(PathBuf, String, bool)> {
    let mut out = Vec::new();
    for child in sorted_entries(source) {
        let name = fname(&child);
        if name_skipped(&name) {
            continue;
        }
        let is_dir = child.is_dir();
        if top_level_skipped(&name) {
            continue;
        }
        if !is_dir && is_secret_file(&name) && !include_secrets {
            continue;
        }
        out.push((child, name, is_dir));
    }
    out
}

/// 目录级 include 展开（递归，应用 name_skipped / .env 规则）。
fn add_tree(abs_dir: &Path, rel: &str, inc: &mut HashSet<String>, include_secrets: bool) {
    for child in sorted_entries(abs_dir) {
        let name = fname(&child);
        if name_skipped(&name) {
            continue;
        }
        if is_secret_file(&name) && !include_secrets {
            continue;
        }
        let child_rel = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
        inc.insert(child_rel.clone());
        if child.is_dir() {
            add_tree(&child, &child_rel, inc, include_secrets);
        }
    }
}

/// 目录展开（剪枝跳过项）：产出 (rel, abs_path, is_dir)。
fn walk_skip(abs_dir: &Path, source: &Path, include_secrets: bool) -> Vec<(String, PathBuf, bool)> {
    let mut out = Vec::new();
    for child in sorted_entries(abs_dir) {
        let name = fname(&child);
        if name_skipped(&name) {
            continue;
        }
        if is_secret_file(&name) && !include_secrets {
            continue;
        }
        let rel = child.strip_prefix(source).unwrap_or(&child).to_string_lossy().replace('\\', "/");
        let is_dir = child.is_dir();
        out.push((rel.clone(), child.clone(), is_dir));
        if is_dir {
            out.extend(walk_skip(&child, source, include_secrets));
        }
    }
    out
}

pub struct ExportOptions<'a> {
    pub source: &'a Path,
    pub output: &'a Path,
    pub project_filters: &'a [String],
    pub session_filters: &'a [String],
    pub since: Option<&'a str>,
    pub include_secrets: bool,
}

#[derive(Serialize)]
pub struct ExportSummary {
    pub session_count: usize,
    pub file_count: usize,
    pub dir_count: usize,
    pub total_bytes: u64,
    pub warnings: Vec<String>,
    pub output: PathBuf,
    pub source: PathBuf,
}
/// 导出：结构探测 → 会话分组（过滤）→ include 集合 → 展开 → 打包 + manifest。
pub fn export(
    opts: &ExportOptions,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<ExportSummary, String> {
    let source = opts.source;
    if !source.is_dir() {
        return Err(format!("源 home 不存在或不是目录: {}", source.display()));
    }
    let since_dt: Option<SystemTime> = match opts.since {
        Some(s) => Some(
            parse_since(s).ok_or_else(|| format!("--since 不是合法 ISO 时间: {}", s))?,
        ),
        None => None,
    };

    let top_entries = collect_top_level(source, opts.include_secrets);
    let top_names: Vec<String> = top_entries.iter().map(|e| e.1.clone()).collect();

    // 结构探测
    let projects_dir = source.join("projects");
    let mut all_slugs: Vec<String> = Vec::new();
    if projects_dir.is_dir() {
        for slug_dir in sorted_entries(&projects_dir) {
            if slug_dir.is_dir() {
                all_slugs.push(fname(&slug_dir));
            }
        }
    }

    // 会话分组（含 recovery 分支 / non_session）
    let mut session_groups: HashMap<String, HashMap<String, SessionGroup>> = HashMap::new();
    let mut non_session_by_slug: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_sidecars: HashSet<String> = HashSet::new();
    for slug in &all_slugs {
        let sdir = projects_dir.join(slug).join("sessions");
        let scan = scan_sessions(&sdir);
        if !scan.groups.is_empty() || !scan.non_session.is_empty() {
            session_groups.insert(slug.clone(), scan.groups);
            non_session_by_slug.insert(slug.clone(), scan.non_session);
            if let Some(gs) = session_groups.get(slug) {
                for g in gs.values() {
                    for f in &g.files {
                        if let Some((_, ext)) = f.rsplit_once('.') {
                            all_sidecars.insert(ext.to_string());
                        }
                    }
                }
            }
        }
    }

    // 过滤
    let mut filtered_groups: HashMap<String, HashMap<String, SessionGroup>> = HashMap::new();
    let mut filtered_non_session: HashMap<String, Vec<String>> = HashMap::new();
    for slug in &all_slugs {
        if !match_project(slug, opts.project_filters) {
            continue;
        }
        let fg = session_allowed(
            session_groups.get(slug),
            opts.session_filters,
            since_dt,
        );
        if !fg.is_empty() {
            filtered_groups.insert(slug.clone(), fg);
        }
        if let Some(ns) = non_session_by_slug.get(slug) {
            if !ns.is_empty() {
                filtered_non_session.insert(slug.clone(), ns.clone());
            }
        }
    }

    // include 集合
    let mut inc: HashSet<String> = HashSet::new();
    for (abs_path, rel, is_dir) in &top_entries {
        if rel == "projects" {
            inc.insert("projects".to_string());
            for (slug, filtered) in &filtered_groups {
                let slug_abs = projects_dir.join(slug);
                inc.insert(format!("projects/{}", slug));
                for g in filtered.values() {
                    for f in &g.files {
                        inc.insert(format!("projects/{}/sessions/{}", slug, f));
                    }
                }
                if let Some(ns) = filtered_non_session.get(slug) {
                    for n in ns {
                        inc.insert(format!("projects/{}/sessions/{}", slug, n));
                    }
                }
                // 项目目录下除 sessions 外的内容（memory/ 等）
                for child in sorted_entries(&slug_abs) {
                    let name = fname(&child);
                    if name == "sessions" || name_skipped(&name) {
                        continue;
                    }
                    inc.insert(format!("projects/{}/{}", slug, name));
                    if child.is_dir() {
                        add_tree(&child, &format!("projects/{}/{}", slug, name), &mut inc, opts.include_secrets);
                    }
                }
            }
        } else {
            inc.insert(rel.clone());
            if *is_dir {
                add_tree(abs_path, rel, &mut inc, opts.include_secrets);
            }
        }
    }

    // 展开目录项 → 文件/目录清单
    let mut warnings: Vec<String> = Vec::new();
    let mut file_entries: Vec<(String, PathBuf)> = Vec::new();
    let mut dir_entries: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut sorted_inc: Vec<&String> = inc.iter().collect();
    sorted_inc.sort();
    for rel in sorted_inc {
        if !seen.insert(rel.clone()) {
            continue;
        }
        let p = rel.split('/').fold(source.to_path_buf(), |acc, seg| acc.join(seg));
        if p.is_dir() {
            dir_entries.push(rel.clone());
            for (srel, sub, is_dir) in walk_skip(&p, source, opts.include_secrets) {
                if !seen.insert(srel.clone()) {
                    continue;
                }
                if is_dir {
                    dir_entries.push(srel);
                } else {
                    file_entries.push((srel, sub));
                }
            }
        } else if p.is_file() {
            file_entries.push((rel.clone(), p));
        } else {
            warnings.push(format!("{}（include 集合中的路径在导出期间消失，已跳过）", rel));
        }
    }

    // 会话记录
    let mut session_records: Vec<Value> = Vec::new();
    let mut slugs_sorted: Vec<&String> = filtered_groups.keys().collect();
    slugs_sorted.sort();
    for slug in slugs_sorted {
        let gs = &filtered_groups[slug];
        let mut metas: HashMap<String, Value> = HashMap::new();
        for base in gs.keys() {
            if let Some(m) = read_meta(&projects_dir.join(slug).join("sessions").join(format!("{}.jsonl.meta", base))) {
                metas.insert(base.clone(), m);
            }
        }
        let slug_ws = metas
            .values()
            .find_map(|m| m.get("workspace_root").and_then(|v| v.as_str()))
            .map(String::from);
        let mut bases_sorted: Vec<&String> = gs.keys().collect();
        bases_sorted.sort();
        for base in bases_sorted {
            let meta = metas.get(base).cloned().unwrap_or_else(|| json!({}));
            let ws = meta
                .get("workspace_root")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| slug_ws.clone());
            let g = &gs[base];
            let mtime = iso_seconds(if g.max_mtime > UNIX_EPOCH { g.max_mtime } else { UNIX_EPOCH });
            session_records.push(json!({
                "id": base,
                "slug": slug,
                "workspace_root": ws,
                "scope": meta.get("scope"),
                "title": meta.get("topic_title")
                    .or_else(|| meta.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(base),
                "last_updated": mtime,
                "recovery_branch_count": g.recovery.len(),
            }));
        }
    }

    // 打包
    if let Some(parent) = opts.output.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建输出目录: {}", e))?;
    }
    let file = fs::File::create(opts.output).map_err(|e| format!("无法创建输出文件: {}", e))?;
    let mut zf = ZipWriter::new(file);
    let mut manifest_files: Vec<Value> = Vec::new();
    let mut dir_count = 0usize;
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;

    let dir_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let total_entries = dir_entries.len() + file_entries.len();
    let mut done = 0usize;
    for rel in &dir_entries {
        let _ = zf.add_directory(format!("{}/", rel), dir_options);
        dir_count += 1;
        done += 1;
    }
    let file_options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (rel, abs_p) in &file_entries {
        // 应用并发写导致文件短暂消失，重试容忍（对齐 Python 的 4 次 × 0.3s）
        let mut data: Option<Vec<u8>> = None;
        for _ in 0..4 {
            match fs::read(abs_p) {
                Ok(d) => {
                    data = Some(d);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    std::thread::sleep(Duration::from_millis(300));
                }
                Err(_) => break,
            }
        }
        let Some(data) = data else {
            warnings.push(format!("{}（源文件在导出期间被应用移除/移动，已跳过）", rel));
            continue;
        };
        let _ = zf.start_file(rel.clone(), file_options);
        let _ = zf.write_all(&data);
        manifest_files.push(json!({
            "path": rel,
            "size": data.len(),
            "sha256": sha256_bytes(&data),
        }));
        file_count += 1;
        total_bytes += data.len() as u64;
        done += 1;
        // 进度回调（节流：每 50 个或最后一批）
        if done % 50 == 0 || done == total_entries {
            progress(done, total_entries);
        }
    }

    let mut sidecars_sorted: Vec<String> = all_sidecars.into_iter().collect();
    sidecars_sorted.sort();
    let fingerprint = json!({
        "top_level": top_names,
        "slug_list": all_slugs,
        "session_sidecars": sidecars_sorted,
    });
    let manifest = json!({
        "format_version": FORMAT_VERSION,
        "exported_at": Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        "source_home": source.to_string_lossy(),
        "structure_fingerprint": fingerprint,
        "files": manifest_files,
        "sessions": session_records,
        "warnings": warnings.clone(),
    });
    let _ = zf.start_file("manifest.json".to_string(), file_options);
    let _ = zf.write_all(serde_json::to_string_pretty(&manifest).unwrap_or_default().as_bytes());
    let _ = zf.finish();

    Ok(ExportSummary {
        session_count: filtered_groups.values().map(|g| g.len()).sum(),
        file_count,
        dir_count,
        total_bytes,
        warnings,
        output: opts.output.to_path_buf(),
        source: source.to_path_buf(),
    })
}

fn session_allowed(
    groups: Option<&HashMap<String, SessionGroup>>,
    session_filters: &[String],
    since_dt: Option<SystemTime>,
) -> HashMap<String, SessionGroup> {
    groups
        .map(|g| g.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|(base, g)| {
            if !session_filters.is_empty() && !session_filters.iter().any(|f| base.starts_with(f)) {
                return false;
            }
            if let Some(since) = since_dt {
                if g.max_mtime < since {
                    return false;
                }
            }
            true
        })
        .collect()
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct VerifySummary {
    pub file_count: usize,
    pub session_count: usize,
    pub exported_at: Option<String>,
    pub source_home: Option<String>,
}

/// 校验备份 zip 完整性（对照 manifest 哈希，不解包）。
/// Ok 表示全部一致；Err 携带失败原因（格式/缺 manifest/版本不符/哈希不一致）。
pub fn verify(zip_path: &Path) -> Result<VerifySummary, String> {
    let file = fs::File::open(zip_path).map_err(|e| format!("无法打开备份 zip: {}", e))?;
    let mut zf = zip::ZipArchive::new(file).map_err(|e| format!("无法打开备份 zip: {}", e))?;
    let manifest = zf
        .by_name("manifest.json")
        .map_err(|_| "zip 内缺少 manifest.json，不是 reasonix-migrate 备份".to_string())?;
    let manifest: Value =
        serde_json::from_reader(manifest).map_err(|e| format!("manifest.json 解析失败: {}", e))?;
    let fmt = manifest.get("format_version").and_then(|v| v.as_u64());
    if fmt != Some(FORMAT_VERSION as u64) {
        return Err(format!(
            "不支持的格式版本: {:?}（工具支持 {}）",
            manifest.get("format_version"),
            FORMAT_VERSION
        ));
    }

    let mut bad = 0usize;
    let mut bad_msgs: Vec<String> = Vec::new();
    let files = manifest
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for fi in &files {
        let rel = fi.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let expected = fi.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
        let data = match zf.by_name(rel) {
            Ok(mut r) => {
                let mut buf = Vec::new();
                let _ = r.read_to_end(&mut buf);
                buf
            }
            Err(_) => {
                bad += 1;
                bad_msgs.push(format!("manifest 记录的文件不在 zip 内: {}", rel));
                continue;
            }
        };
        if sha256_bytes(&data) != expected {
            bad += 1;
            bad_msgs.push(format!("哈希不一致: {}", rel));
        }
    }

    if bad > 0 {
        return Err(format!("{}/{} 个文件校验失败：{}", bad, files.len(), bad_msgs.join("；")));
    }
    let sessions = manifest.get("sessions").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
    Ok(VerifySummary {
        file_count: files.len(),
        session_count: sessions,
        exported_at: manifest.get("exported_at").and_then(|v| v.as_str()).map(String::from),
        source_home: manifest.get("source_home").and_then(|v| v.as_str()).map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_home(home: &Path) {
        fs::create_dir_all(home.join("memory/global")).unwrap();
        fs::write(home.join("config.toml"), "theme=dark\n").unwrap();
        fs::write(home.join(".env"), "SECRET=xx\n").unwrap();
        // 项目 A：两个会话 + recovery + ckpt + subagents
        let sd = home.join("projects/c--fake-machine-proj-a/sessions");
        fs::create_dir_all(sd.join("20260809-100000.123456789-model-a.ckpt")).unwrap();
        fs::create_dir_all(sd.join("subagents/x")).unwrap();
        fs::write(sd.join("20260809-100000.123456789-model-a.jsonl"), r#"{"events":[]}"#).unwrap();
        fs::write(
            sd.join("20260809-100000.123456789-model-a.jsonl.meta"),
            r#"{"scope":"project","workspace_root":"X:\\fake\\proj-a","topic_title":"会话A"}"#,
        )
        .unwrap();
        fs::write(sd.join("20260809-100000.123456789-model-a.events.jsonl"), "event\n").unwrap();
        fs::write(sd.join("20260809-100000.123456789-model-a.ckpt/turn-0.json"), "{}").unwrap();
        fs::write(
            sd.join("20260809-100000.123456789-model-a-recovery-abc123.jsonl"),
            "recovery",
        )
        .unwrap();
        fs::write(sd.join("subagents/x/note.txt"), "sub\n").unwrap();
        fs::write(sd.join("20260809-110000.123456789-model-b.jsonl"), "b\n").unwrap();
        fs::write(
            sd.join("20260809-110000.123456789-model-b.jsonl.meta"),
            r#"{"topic_id":"topic_b","topic_title":"会话B"}"#,
        )
        .unwrap();
        // 项目内非会话内容
        fs::write(home.join("projects/c--fake-machine-proj-a/memory.md"), "m\n").unwrap();
        // .trash 应被跳过
        let tr = sd.join(".trash");
        fs::create_dir_all(&tr).unwrap();
        fs::write(tr.join("20260808-000000.111111111-old.jsonl"), "x").unwrap();
        // 遗留空项目
        fs::create_dir_all(home.join("projects/c--fake-machine-legacy")).unwrap();
        fs::write(home.join("projects/c--fake-machine-legacy/notes.txt"), "legacy\n").unwrap();
    }

    #[test]
    fn scan_sessions_two_phase() {
        let home = tempdir().unwrap();
        make_home(home.path());
        let sd = home.path().join("projects/c--fake-machine-proj-a/sessions");
        let scan = scan_sessions(&sd);
        // 两个主会话 + recovery 并入
        assert_eq!(scan.groups.len(), 2);
        let main = &scan.groups["20260809-100000.123456789-model-a"];
        assert_eq!(main.recovery.len(), 1);
        // ckpt 目录条目在 group.files 中（内部文件由 include 展开阶段 walk_skip 带入）
        assert!(main.files.iter().any(|f| f.ends_with("model-a.ckpt")));
        // subagents 整体打包
        assert!(scan.non_session.iter().any(|f| f.ends_with("note.txt")));
    }

    #[test]
    fn export_roundtrip_and_verify() {
        let home = tempdir().unwrap();
        make_home(home.path());
        let out = home.path().join("backup.zip");
        let opts = ExportOptions {
            source: home.path(),
            output: &out,
            project_filters: &[],
            session_filters: &[],
            since: None,
            include_secrets: false,
        };
        let summary = export(&opts, &mut |_, _| {}).expect("export 应成功");
        assert_eq!(summary.session_count, 2);
        assert!(summary.file_count >= 8);
        assert_eq!(summary.warnings.len(), 0);

        // verify 通过
        let vs = verify(&out).expect("verify 应通过");
        assert_eq!(vs.file_count, summary.file_count);
        assert_eq!(vs.session_count, 2);

        // 检查 manifest 会话记录的 workspace_root 非空
        let f = fs::File::open(&out).unwrap();
        let mut zf = zip::ZipArchive::new(f).unwrap();
        let manifest: Value = serde_json::from_reader(zf.by_name("manifest.json").unwrap()).unwrap();
        let sessions = manifest.get("sessions").unwrap().as_array().unwrap();
        assert!(
            sessions.iter().any(|s| s.get("workspace_root").and_then(|v| v.as_str()).is_some()),
            "workspace_root 不应全为 null: {:?}",
            sessions
        );
        drop(zf);

        // zip 内无 .env / .trash
        let f = fs::File::open(&out).unwrap();
        let mut zf = zip::ZipArchive::new(f).unwrap();
        for i in 0..zf.len() {
            let name = zf.by_index(i).unwrap().name().to_string();
            assert!(!name.contains(".env") && !name.contains(".trash"), "不应包含: {}", name);
        }
    }

    #[test]
    fn export_filters() {
        let home = tempdir().unwrap();
        make_home(home.path());
        let out = home.path().join("f.zip");
        let opts = ExportOptions {
            source: home.path(),
            output: &out,
            project_filters: &["proj-a".to_string()],
            session_filters: &["20260809-110000".to_string()],
            since: None,
            include_secrets: false,
        };
        let summary = export(&opts, &mut |_, _| {}).expect("export 应成功");
        assert_eq!(summary.session_count, 1);
        // 会话记录只有 model-b
        let f = fs::File::open(&out).unwrap();
        let mut zf = zip::ZipArchive::new(f).unwrap();
        let manifest: Value = serde_json::from_reader(zf.by_name("manifest.json").unwrap()).unwrap();
        let sessions = manifest.get("sessions").and_then(|v| v.as_array()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].get("id").unwrap().as_str().unwrap().contains("model-b"));
    }

    #[test]
    fn verify_detects_tamper() {
        let home = tempdir().unwrap();
        make_home(home.path());
        let out = home.path().join("v.zip");
        let opts = ExportOptions {
            source: home.path(),
            output: &out,
            project_filters: &[],
            session_filters: &[],
            since: None,
            include_secrets: false,
        };
        export(&opts, &mut |_, _| {}).unwrap();
        // 篡改一个文件
        let f = fs::File::open(&out).unwrap();
        let mut zf = zip::ZipArchive::new(f).unwrap();
        let mut target: Option<String> = None;
        for i in 0..zf.len() {
            let name = zf.by_index(i).unwrap().name().to_string();
            if name.ends_with(".jsonl") && !name.ends_with("manifest.json") {
                target = Some(name);
                break;
            }
        }
        drop(zf);
        // 重写 zip：把目标文件内容换掉（简单做法：重新打包一份篡改的）
        let tgt = target.expect("有 jsonl 文件");
        let data = fs::read(&out).unwrap();
        let _ = data;
        // 用 zip crate 读取-修改-重写
        let f2 = fs::File::open(&out).unwrap();
        let mut zf2 = zip::ZipArchive::new(f2).unwrap();
        let mut w = ZipWriter::new(fs::File::create(home.path().join("tampered.zip")).unwrap());
        let opts2 = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for i in 0..zf2.len() {
            let mut entry = zf2.by_index(i).unwrap();
            let name = entry.name().to_string();
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).unwrap();
            if name == tgt {
                buf = b"TAMPERED".to_vec();
            }
            if name.ends_with('/') {
                w.add_directory(name, opts2).unwrap();
            } else {
                w.start_file(name, opts2).unwrap();
                w.write_all(&buf).unwrap();
            }
        }
        w.finish().unwrap();
        drop(zf2);
        let r = verify(&home.path().join("tampered.zip"));
        assert!(r.is_err(), "篡改后应校验失败: {:?}", r);
    }
}
