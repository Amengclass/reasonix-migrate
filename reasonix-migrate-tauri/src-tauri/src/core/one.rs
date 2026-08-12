//! 单会话迁移：把某一个会话搬到指定工作目录（对齐 Python `reasonix-migrate-one.py`）。
//!
//! 源三选一（home / sessions 目录 / 备份 zip）+ 目标工作区 → 复制会话文件族、
//! 修正 meta 归属、SHA-256 复查、自动注册项目、可选重启桌面端。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use super::catalog::ensure_catalog_session;
use super::common::{
    dedupe_main_sessions, ensure_project_registered, list_sessions_dir, list_sessions_home,
    list_sessions_zip, load_desktop_projects, name_skipped, read_meta, sha256_bytes, sha256_file,
    slug_of, Session,
};

const RESTART_PROC_NAME: &str = "reasonix-desktop";

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OneOptions {
    pub from_home: Option<String>,
    pub from_sessions: Option<String>,
    pub from_zip: Option<String>,
    pub session: Option<String>,
    /// 源会话所属项目 slug（用于精确定位源目录，避免命中其他项目的同名副本）
    pub session_slug: Option<String>,
    pub to_workspace: Option<String>,
    pub to_home: Option<String>,
    pub list: bool,
    /// 迁移总是生成新主题（默认 true，避免与原工作区会话联动删除）
    #[serde(default = "default_true")]
    pub new_topic: bool,
    pub overwrite: bool,
    pub no_verify: bool,
    pub dry_run: bool,
    pub restart_app: bool,
    /// 迁移成功后删除源会话文件（真·搬走；仅目录源生效，zip 源不删）
    pub delete_source: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OneSummary {
    pub copied: Vec<String>,
    pub skipped: Vec<String>,
    pub meta_changes: Vec<String>,
    pub conflict: bool,
    pub session_id: String,
    pub target_sessions: PathBuf,
    /// 迁移后删除的源文件（delete_source 时）
    pub deleted_source: Vec<String>,
    /// 校验/过程中的警告（如源会话仍被 Reasonix 写入导致校验不一致）
    pub warnings: Vec<String>,
}

fn fname(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 按 id 前缀选会话；匹配到同一会话族（主会话 + recovery/fork 后代）时取主会话。
pub fn pick_session<'a>(candidates: &'a [Session], prefix: &str) -> Result<&'a Session, String> {
    if prefix.is_empty() {
        return Err("需要 --session <会话id或前缀>（可先 --list 查看候选）".to_string());
    }
    let matched: Vec<&Session> = candidates
        .iter()
        .filter(|s| s.id == prefix || s.id.starts_with(prefix))
        .collect();
    if matched.is_empty() {
        return Err(format!("没找到匹配 '{}' 的会话（可先 --list 查看）", prefix));
    }
    if matched.len() == 1 {
        return Ok(matched[0]);
    }
    // 共享同一最短前缀 id 时，取最短 id 为主会话
    let main = matched.iter().min_by_key(|s| s.id.len()).unwrap();
    if matched
        .iter()
        .all(|s| s.id == main.id || s.id.starts_with(&format!("{}-", main.id)))
    {
        return Ok(main);
    }
    let shown: Vec<&str> = matched.iter().take(10).map(|s| s.id.as_str()).collect();
    Err(format!(
        "'{}' 匹配到多个不相关的会话，请给更精确的前缀：\n{}",
        prefix,
        shown.join("\n  ")
    ))
}

/// 目录源：收集会话 id 及其 recovery 分支的文件/目录（跳过锁/缓存）。
fn source_files_dir(sessions_dir: &Path, sid: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(sessions_dir) else {
        return out;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for entry in entries {
        let name = fname(&entry);
        if name == sid || name.starts_with(&format!("{}.", sid)) || name.starts_with(&format!("{}-", sid)) {
            if !name_skipped(&name) {
                out.push(entry);
            }
        }
    }
    out
}

/// zip 源：按「sessions/ 之后的首段」匹配会话条目（保留子目录结构）。
fn source_files_zip(zip_path: &Path, sid: &str) -> Result<Vec<String>, String> {
    let f = fs::File::open(zip_path).map_err(|e| format!("无法打开 zip: {}", e))?;
    let mut zf = zip::ZipArchive::new(f).map_err(|e| format!("无法打开 zip: {}", e))?;
    let mut out = Vec::new();
    for i in 0..zf.len() {
        let name = zf.by_index(i).map_err(|e| e.to_string())?.name().to_string();
        let parts: Vec<&str> = name.split('/').collect();
        let Some(pos) = parts.iter().position(|p| *p == "sessions") else {
            continue;
        };
        let rel = parts[pos + 1..].join("/");
        if rel.is_empty() {
            continue;
        }
        let first = rel.split('/').next().unwrap_or("");
        if first == sid || first.starts_with(&format!("{}.", sid)) || first.starts_with(&format!("{}-", sid)) {
            if !name_skipped(first) {
                out.push(rel);
            }
        }
    }
    Ok(out)
}

/// 递归复制目录；源文件被并发清理而消失时跳过（返回跳过的相对路径）。
fn copy_tree_tolerant(src: &Path, dst: &Path, skipped: &mut Vec<String>) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    let Ok(rd) = fs::read_dir(src) else {
        return Ok(());
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for entry in entries {
        let name = fname(&entry);
        if name_skipped(&name) {
            continue;
        }
        let d = dst.join(&name);
        if entry.is_dir() {
            copy_tree_tolerant(&entry, &d, skipped)?;
        } else if entry.is_file() {
            match copy_file_preserve_mtime(&entry, &d) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    skipped.push(entry.to_string_lossy().to_string());
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

fn copy_file_preserve_mtime(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = fs::metadata(src)?;
    fs::copy(src, dst)?;
    if let Ok(mt) = meta.modified() {
        let f = fs::OpenOptions::new().write(true).open(dst)?;
        let _ = f.set_modified(mt);
    }
    Ok(())
}

/// 修正目标 meta：scope=project、workspace_root=目标工作区；返回变更说明。
fn fix_meta(meta_path: &Path, workspace_root: &str, new_topic: bool) -> Vec<String> {
    let Some(mut meta) = read_meta(meta_path) else {
        return Vec::new();
    };
    let mut changed = Vec::new();
    if meta.get("scope").and_then(|v| v.as_str()) != Some("project") {
        meta["scope"] = json!("project");
        changed.push("scope->project".to_string());
    }
    let wr = abs_workspace(workspace_root);
    if meta.get("workspace_root").and_then(|v| v.as_str()) != Some(wr.as_str()) {
        meta["workspace_root"] = json!(wr);
        changed.push(format!("workspace_root->{}", wr));
    }
    if new_topic {
        let tid = gen_topic_id();
        meta["topic_id"] = json!(tid);
        changed.push(format!("topic_id->{}", tid));
    }
    if !changed.is_empty() {
        let _ = fs::write(meta_path, serde_json::to_string_pretty(&meta).unwrap_or_default());
    }
    changed
}

fn abs_workspace(workspace_root: &str) -> String {
    let p = Path::new(workspace_root);
    if p.is_absolute() {
        p.to_string_lossy().to_string()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(p).to_string_lossy().to_string()
    } else {
        workspace_root.to_string()
    }
}

static TOPIC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成 topic_id：`topic_YYYYMMDD-HHMMSS_<16 hex>`（时间戳 + 计数器派生，无需外部 crate）。
fn gen_topic_id() -> String {
    let now = chrono::Local::now();
    let n = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ TOPIC_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("topic_{}_{:016x}", now.format("%Y%m%d-%H%M%S"), n)
}

/// 迁移单会话。返回 Summary；Err 为致命错误。
pub fn migrate_one(opts: &OneOptions) -> Result<OneSummary, String> {
    // 源会话候选
    let candidates: Vec<Session> = if let Some(h) = &opts.from_home {
        list_sessions_home(Path::new(h), false, None)
    } else if let Some(d) = &opts.from_sessions {
        list_sessions_dir(Path::new(d), None)
    } else if let Some(z) = &opts.from_zip {
        list_sessions_zip(Path::new(z))
    } else {
        return Err("需要 --from-home / --from-sessions / --from-zip 之一".to_string());
    };

    // --list
    if opts.list {
        if let Some(h) = &opts.from_home {
            let dp = load_desktop_projects(Path::new(h));
            let reg_slugs: HashSet<String> = dp
                .projects
                .iter()
                .filter(|p| !p.root.is_empty())
                .map(|p| slug_of(&p.root))
                .collect();
            let mains = dedupe_main_sessions(candidates, Some(&reg_slugs));
            let filtered: Vec<Session> = mains
                .into_iter()
                .filter(|s| s.topic_id.as_deref().map(|t| dp.visible_topics.contains(t)).unwrap_or(false))
                .collect();
            print_list(&filtered);
        } else {
            let mains = dedupe_main_sessions(candidates, None);
            print_list(&mains);
        }
        return Ok(OneSummary {
            copied: Vec::new(),
            skipped: Vec::new(),
            meta_changes: Vec::new(),
            conflict: false,
            session_id: String::new(),
            target_sessions: PathBuf::new(),
            deleted_source: Vec::new(),
            warnings: Vec::new(),
        });
    }

    if candidates.is_empty() {
        return Err("源里没有可迁移的会话".to_string());
    }
    let prefix = opts.session.as_deref().unwrap_or("");
    let session = pick_session(&candidates, prefix)?.clone();
    let sid = session.id.clone();

    // 目标位置
    let Some(ws) = opts.to_workspace.as_deref() else {
        return Err("需要 --to-workspace <工作区路径>".to_string());
    };
    let Some(home) = opts.to_home.as_deref() else {
        return Err("需要 --to-home（或设置环境变量 REASONIX_HOME）".to_string());
    };
    let target_home = PathBuf::from(home);
    if !opts.dry_run {
        fs::create_dir_all(&target_home).map_err(|e| e.to_string())?;
    }
    let slug = slug_of(ws);
    let target_sessions = target_home.join("projects").join(&slug).join("sessions");
    if !opts.dry_run {
        fs::create_dir_all(&target_sessions).map_err(|e| e.to_string())?;
    }
    let mut warnings: Vec<String> = Vec::new();

    // 定位源文件
    let mut src_entries: Vec<SourceEntry> = Vec::new();
    let mut src_file_count = 0usize;
    if opts.from_home.is_some() || opts.from_sessions.is_some() {
        let dir = if opts.from_home.is_some() {
            // home 源：优先按源会话所属项目 slug 定位，找不到再遍历
            let projects = PathBuf::from(opts.from_home.as_ref().unwrap()).join("projects");
            let mut found: Option<PathBuf> = None;
            if let Ok(rd) = fs::read_dir(&projects) {
                // 1) 精确匹配 session_slug（用户所选会话的项目）
                if let Some(slug) = &opts.session_slug {
                    let cand = projects.join(slug).join("sessions");
                    if cand.is_dir() && cand.join(format!("{}.jsonl", sid)).is_file() {
                        found = Some(cand);
                    }
                }
                // 2) 兜底遍历（兼容未传 slug 的场景）
                if found.is_none() {
                    for slug_dir in rd.flatten() {
                        let cand = slug_dir.path().join("sessions");
                        if cand.is_dir() && cand.join(format!("{}.jsonl", sid)).is_file() {
                            found = Some(cand);
                            break;
                        }
                    }
                }
            }
            found.ok_or_else(|| format!("在 home 中找不到会话 {} 的文件", sid))?
        } else {
            PathBuf::from(opts.from_sessions.as_ref().unwrap())
        };
        let entries = source_files_dir(&dir, &sid);
        if entries.is_empty() {
            return Err(format!("源会话 {} 的文件为空（可能目录不完整），请检查源数据", sid));
        }
        src_file_count = entries
            .iter()
            .map(|e| {
                if e.is_dir() {
                    count_files(e)
                } else {
                    1
                }
            })
            .sum();
        src_entries = entries.into_iter().map(SourceEntry::Fs).collect();
    } else {
        let zip = PathBuf::from(opts.from_zip.as_ref().unwrap());
        let rels = source_files_zip(&zip, &sid)?;
        src_file_count = rels.iter().filter(|r| !r.ends_with('/')).count();
        src_entries = rels.into_iter().map(SourceEntry::ZipRel).collect();
    }

    // 冲突检测
    let conflict = target_sessions.join(format!("{}.jsonl", sid)).exists();

    // dry-run：预演完整流程（源文件数/目标/冲突/meta 将变更），不写入
    if opts.dry_run {
        let mut src_file_count = 0usize;
        for e in &src_entries {
            match e {
                SourceEntry::Fs(p) => {
                    src_file_count += if p.is_dir() { count_files(p) } else { 1 };
                }
                SourceEntry::ZipRel(rel) => {
                    if !rel.ends_with('/') {
                        src_file_count += 1;
                    }
                }
            }
        }
        let mut meta_changes = vec![
            "scope -> project".to_string(),
            format!("workspace_root -> {}", ws),
        ];
        if opts.new_topic {
            meta_changes.push(format!("topic_id -> {}（新主题）", gen_topic_id()));
        }
        return Ok(OneSummary {
            copied: vec![format!("源文件 {} 个", src_file_count)],
            skipped: Vec::new(),
            meta_changes,
            conflict,
            session_id: sid,
            target_sessions,
            deleted_source: Vec::new(),
            warnings: Vec::new(),
        });
    }

    // 冲突处理
    if conflict && !opts.overwrite {
        return Err(format!("目标已存在同名会话，勾选「覆盖同名」后重试（会话 {}）", sid));
    }
    if conflict && opts.overwrite {
        if let Ok(rd) = fs::read_dir(&target_sessions) {
            for old in rd.flatten() {
                let name = old.file_name().to_string_lossy().to_string();
                if name == sid || name.starts_with(&format!("{}.", sid)) || name.starts_with(&format!("{}-", sid)) {
                    let p = old.path();
                    if p.is_dir() {
                        let _ = fs::remove_dir_all(&p);
                    } else {
                        let _ = fs::remove_file(&p);
                    }
                }
            }
        }
    }

    // 复制
    let mut copied: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for entry in &src_entries {
        match entry {
            SourceEntry::Fs(p) => {
                let dst = target_sessions.join(p.file_name().unwrap_or_default());
                if p.is_dir() {
                    if dst.exists() {
                        let _ = fs::remove_dir_all(&dst);
                    }
                    let mut sk = Vec::new();
                    let _ = copy_tree_tolerant(p, &dst, &mut sk);
                    skipped.extend(sk);
                    if let Ok(rd) = fs::read_dir(&dst) {
                        let n = rd.count();
                        copied.push(format!("{}/ ({} 项)", p.file_name().unwrap_or_default().to_string_lossy(), n));
                    }
                } else if p.is_file() {
                    match copy_file_preserve_mtime(p, &dst) {
                        Ok(()) => copied.push(p.file_name().unwrap_or_default().to_string_lossy().to_string()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            skipped.push(p.to_string_lossy().to_string());
                        }
                        Err(e) => return Err(format!("复制失败 {}: {}", p.display(), e)),
                    }
                }
            }
            SourceEntry::ZipRel(rel) => {
                let dst = target_sessions.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                if rel.ends_with('/') {
                    let _ = fs::create_dir_all(&dst);
                    continue;
                }
                if let Some(parent) = dst.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let zip_path = PathBuf::from(opts.from_zip.as_ref().unwrap());
                let f = fs::File::open(&zip_path).map_err(|e| e.to_string())?;
                let mut zf = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
                let mut reader = zf.by_name(&format!("projects/{}/sessions/{}", session.slug.as_deref().unwrap_or(""), rel)).map_err(|e| e.to_string())?;
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                fs::write(&dst, &buf).map_err(|e| format!("{}: {}", rel, e))?;
                copied.push(rel.clone());
            }
        }
    }

    // meta 修正
    let meta_dst = target_sessions.join(format!("{}.jsonl.meta", sid));
    let meta_changes = if meta_dst.exists() {
        fix_meta(&meta_dst, ws, opts.new_topic)
    } else {
        Vec::new()
    };

    // 复查
    if !opts.no_verify {
        let mut bad_names: Vec<String> = Vec::new();
        for entry in &src_entries {
            match entry {
                SourceEntry::Fs(p) => {
                    let dst = target_sessions.join(p.file_name().unwrap_or_default());
                    if p.is_file() {
                        if fname(p) == format!("{}.jsonl.meta", sid) {
                            continue; // meta 已被修正
                        }
                        if fname(p).ends_with(".lock") {
                            continue; // 瞬态锁文件（如 inbox/transaction.lock）不校验
                        }
                        match (sha256_file(p), sha256_file(&dst)) {
                            (Ok(a), Ok(b)) if a == b => {}
                            _ => bad_names.push(fname(p)),
                        }
                    } else if p.is_dir() {
                        for sf in walk_files(p) {
                            if sf.file_name().map(|n| n.to_string_lossy().ends_with(".lock")).unwrap_or(false) {
                                continue; // 目录内瞬态锁文件不校验
                            }
                            let df = dst.join(sf.strip_prefix(p).unwrap_or(&sf));
                            match (sha256_file(&sf), sha256_file(&df)) {
                                (Ok(a), Ok(b)) if a == b => {}
                                _ => bad_names.push(sf.to_string_lossy().to_string()),
                            }
                        }
                    }
                }
                SourceEntry::ZipRel(rel) => {
                    if rel.ends_with('/') {
                        continue; // 目录条目（zip 导出时写成 `xxx/`）不参与哈希校验
                    }
                    let name = rel.rsplit('/').next().unwrap_or("");
                    if name == format!("{}.jsonl.meta", sid) {
                        continue;
                    }
                    let dst = target_sessions.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                    let zip_path = PathBuf::from(opts.from_zip.as_ref().unwrap());
                    let f = fs::File::open(&zip_path).map_err(|e| e.to_string())?;
                    let mut zf = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
                    let src_hash = zf
                        .by_name(&format!("projects/{}/sessions/{}", session.slug.as_deref().unwrap_or(""), rel))
                        .ok()
                        .map(|mut r| {
                            let mut buf = Vec::new();
                            let _ = r.read_to_end(&mut buf);
                            sha256_bytes(&buf)
                        });
                    match (src_hash, sha256_file(&dst)) {
                        (Some(a), Ok(b)) if a == b => {}
                        _ => bad_names.push(name.to_string()),
                    }
                }
            }
        }
        if !bad_names.is_empty() {
            let detail = format!(
                "{} 个文件校验不一致：{}",
                bad_names.len(),
                bad_names.join(", ")
            );
            // 源会话仍被 Reasonix 写入（存在 .jsonl.lock）时，复制后源文件可能已更新，
            // 属并发写导致的正常差异——降级为警告，不阻断迁移
            let session_dir = match src_entries.iter().find_map(|e| match e {
                SourceEntry::Fs(p) => p.parent().map(|d| d.to_path_buf()),
                SourceEntry::ZipRel(_) => None,
            }) {
                Some(d) => d,
                None => std::path::PathBuf::new(),
            };
            let active = fs::read_dir(&session_dir)
                .map(|rd| {
                    rd.flatten().any(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        (n == format!("{}.jsonl.lock", sid)
                            || n.starts_with(&format!("{}-", sid)))
                            && n.ends_with(".jsonl.lock")
                    })
                })
                .unwrap_or(false);
            if active {
                warnings.push(format!(
                    "{}（源会话仍在被 Reasonix 写入，目标可能有细微差异，建议稍后重跑一次）",
                    detail
                ));
            } else {
                return Err(detail);
            }
        }
    }

    // 项目注册
    if opts.from_home.is_some() || opts.from_sessions.is_some() {
        ensure_project_registered(&target_home, ws);
    }

    // 预置 reasonix SQLite catalog（best-effort）：重启 reasonix 后立即显示，无需等后台扫描
    let jsonl_path = target_sessions.join(format!("{}.jsonl", sid));
    if let Some(meta) = read_meta(&jsonl_path.with_extension("jsonl.meta")) {
        if let Err(e) = ensure_catalog_session(&target_home, &jsonl_path, &meta) {
            warnings.push(format!(
                "catalog 预置失败（不影响迁移，reasonix 扫描后仍会显示）: {}",
                e
            ));
        }
    }

    // 迁移后删除源会话（真·搬走）：仅目录源（home/sessions）生效，zip 源是备份不删
    let mut deleted_source: Vec<String> = Vec::new();
    if opts.delete_source && !src_entries.is_empty() && matches!(src_entries[0], SourceEntry::Fs(_)) {
        for entry in &src_entries {
            if let SourceEntry::Fs(p) = entry {
                let name = fname(p);
                let ok = if p.is_dir() {
                    fs::remove_dir_all(p).is_ok()
                } else {
                    fs::remove_file(p).is_ok()
                };
                if ok {
                    deleted_source.push(name);
                }
            }
        }
    }

    Ok(OneSummary {
        copied,
        skipped,
        meta_changes,
        conflict,
        session_id: sid,
        target_sessions,
        deleted_source,
        warnings,
    })
}

fn count_files(dir: &Path) -> usize {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| if e.path().is_dir() { count_files(&e.path()) } else { 1 })
        .sum()
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_files(&p));
        } else {
            out.push(p);
        }
    }
    out
}

fn print_list(candidates: &[Session]) {
    println!("[会话] 源里共 {} 个可迁移会话（新的在前）：", candidates.len());
    for s in candidates {
        let slug = s.slug.as_deref().unwrap_or("?");
        let title = s.title.replace('\n', " ");
        let title = if title.chars().count() > 40 { title.chars().take(40).collect() } else { title };
        println!("  {}  [{}]  {}", s.id, slug, title);
    }
}

enum SourceEntry {
    Fs(PathBuf),
    ZipRel(String),
}

/// 迁移成功后重启 Reasonix 桌面端（对齐 Python `restart_reasonix_app`）。
/// 环境变量 REASONIX_MIGRATE_NO_RESTART=1 时跳过实际重启（测试用）。
pub fn restart_reasonix_app() -> String {
    if std::env::var("REASONIX_MIGRATE_NO_RESTART").is_ok() {
        return "REASONIX_MIGRATE_NO_RESTART=1：跳过实际重启桌面端".to_string();
    }
    let Some(exe) = probe_app_exe() else {
        return "未发现运行中的 Reasonix 桌面端，跳过自动重启（下次启动时即可看到新会话）".to_string();
    };
    // 结束进程
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/IM", &format!("{}.exe", RESTART_PROC_NAME)])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("pkill").args(["-f", RESTART_PROC_NAME]).output();
    }
    std::thread::sleep(Duration::from_millis(600));
    // 重新启动
    #[cfg(windows)]
    {
        use std::process::Command;
        match Command::new(&exe).creation_flags(CREATE_NO_WINDOW).spawn() {
            Ok(_) => format!("已重启 Reasonix 桌面端（{}）", exe),
            Err(e) => format!("重新启动桌面端失败：{}（可手动启动 {}）", e, exe),
        }
    }
    #[cfg(not(windows))]
    {
        use std::process::Command;
        match Command::new(&exe).spawn() {
            Ok(_) => format!("已重启 Reasonix 桌面端（{}）", exe),
            Err(e) => format!("重新启动桌面端失败：{}（可手动启动 {}）", e, exe),
        }
    }
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

fn probe_app_exe() -> Option<String> {
    #[cfg(windows)]
    {
        // 优先从运行中的进程取实际路径（PowerShell 无窗口查询）
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-Process -Name reasonix-desktop -ErrorAction SilentlyContinue | Select-Object -First 1).Path",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() && Path::new(&s).is_file() {
            return Some(s);
        }
    }
    // 兜底常见安装位置
    for cand in [
        std::env::var("LOCALAPPDATA").map(|d| format!("{}\\reasonix-desktop.exe", d)).unwrap_or_default(),
        std::env::var("APPDATA").map(|d| format!("{}\\reasonix-desktop.exe", d)).unwrap_or_default(),
        "/Applications/Reasonix.app/Contents/MacOS/reasonix-desktop".to_string(),
        "/usr/local/bin/reasonix-desktop".to_string(),
    ] {
        if !cand.is_empty() && Path::new(&cand).is_file() {
            return Some(cand);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_sessions_dir(sdir: &Path) {
        fs::create_dir_all(sdir.join("20260809-100000.123456789-model-a.ckpt")).unwrap();
        fs::write(sdir.join("20260809-100000.123456789-model-a.jsonl"), r#"{"events":[]}"#).unwrap();
        fs::write(
            sdir.join("20260809-100000.123456789-model-a.jsonl.meta"),
            r#"{"scope":"global","topic_title":"会话A","topic_id":"topic_old"}"#,
        )
        .unwrap();
        fs::write(sdir.join("20260809-100000.123456789-model-a.events.jsonl"), "event\n").unwrap();
        fs::write(sdir.join("20260809-100000.123456789-model-a.ckpt/turn-0.json"), "{}").unwrap();
    }

    #[test]
    fn migrate_from_sessions_dir() {
        let src = tempdir().unwrap();
        make_sessions_dir(&src.path().join("sessions"));
        let tgt_home = tempdir().unwrap();
        let ws = src.path().join("proj-ws");

        let opts = OneOptions {
            from_sessions: Some(src.path().join("sessions").to_string_lossy().to_string()),
            session: Some("20260809-100000".to_string()),
            to_workspace: Some(ws.to_string_lossy().to_string()),
            to_home: Some(tgt_home.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        let sum = migrate_one(&opts).expect("migrate ok");
        assert!(!sum.conflict);
        let slug = slug_of(&ws.to_string_lossy());
        let dst = tgt_home.path().join("projects").join(&slug).join("sessions");
        assert!(dst.join("20260809-100000.123456789-model-a.jsonl").is_file());
        assert!(dst.join("20260809-100000.123456789-model-a.ckpt/turn-0.json").is_file());
        // meta 修正：scope->project + workspace_root
        let meta = read_meta(&dst.join("20260809-100000.123456789-model-a.jsonl.meta")).unwrap();
        assert_eq!(meta.get("scope").and_then(|v| v.as_str()), Some("project"));
        assert!(meta.get("workspace_root").and_then(|v| v.as_str()).is_some());
        assert!(sum.meta_changes.iter().any(|c| c.starts_with("scope->project")));
        // 全新 target home 不创建 desktop-projects.json（设计行为：首次启动应用自建）
        assert!(!tgt_home.path().join("desktop-projects.json").exists());
    }

    #[test]
    fn migrate_dry_run_and_conflict() {
        let src = tempdir().unwrap();
        make_sessions_dir(&src.path().join("sessions"));
        let tgt_home = tempdir().unwrap();
        let ws = src.path().join("proj-ws");

        // dry-run 不写文件
        let opts = OneOptions {
            from_sessions: Some(src.path().join("sessions").to_string_lossy().to_string()),
            session: Some("20260809-100000".to_string()),
            to_workspace: Some(ws.to_string_lossy().to_string()),
            to_home: Some(tgt_home.path().to_string_lossy().to_string()),
            dry_run: true,
            ..Default::default()
        };
        let sum = migrate_one(&opts).expect("dry-run ok");
        assert!(!tgt_home.path().join("projects").exists());

        // 正式迁移
        let opts2 = OneOptions {
            from_sessions: Some(src.path().join("sessions").to_string_lossy().to_string()),
            session: Some("20260809-100000".to_string()),
            to_workspace: Some(ws.to_string_lossy().to_string()),
            to_home: Some(tgt_home.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        migrate_one(&opts2).expect("migrate ok");

        // 冲突：默认拒绝
        let r = migrate_one(&opts2);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("已存在同名会话"));
        // --overwrite 覆盖
        let opts3 = OneOptions {
            overwrite: true,
            ..opts2
        };
        migrate_one(&opts3).expect("overwrite ok");
    }

    #[test]
    fn migrate_zip_source() {
        // 先导出 zip，再从 zip 迁移
        let src = tempdir().unwrap();
        make_sessions_dir(&src.path().join("projects/c--fake-machine-proj-a/sessions"));
        let zip = src.path().join("b.zip");
        let eopts = crate::core::export::ExportOptions {
            source: src.path(),
            output: &zip,
            project_filters: &[],
            session_filters: &[],
            since: None,
            include_secrets: false,
        };
        crate::core::export::export(&eopts, &mut |_, _| {}).expect("export ok");

        let tgt_home = tempdir().unwrap();
        let ws = src.path().join("proj-ws");
        let opts = OneOptions {
            from_zip: Some(zip.to_string_lossy().to_string()),
            session: Some("20260809-100000".to_string()),
            to_workspace: Some(ws.to_string_lossy().to_string()),
            to_home: Some(tgt_home.path().to_string_lossy().to_string()),
            ..Default::default()
        };
        let sum = migrate_one(&opts).expect("zip migrate ok");
        let slug = slug_of(&ws.to_string_lossy());
        let dst = tgt_home.path().join("projects").join(&slug).join("sessions");
        assert!(dst.join("20260809-100000.123456789-model-a.jsonl").is_file());
        assert!(dst.join("20260809-100000.123456789-model-a.ckpt/turn-0.json").is_file());
        assert!(sum.copied.len() >= 4);
    }

    #[test]
    fn restart_test_mode() {
        std::env::set_var("REASONIX_MIGRATE_NO_RESTART", "1");
        let msg = restart_reasonix_app();
        assert!(msg.contains("跳过实际重启"));
    }

    #[test]
    fn pick_session_prefix() {
        let cands = vec![
            Session { id: "20260809-100000.123456789-model-a".into(), slug: None, title: String::new(), topic_id: None, workspace_root: None, meta_path: None, turns: None, authored_turns: None, registered: false },
            Session { id: "20260809-100000.123456789-model-a-recovery-abc".into(), slug: None, title: String::new(), topic_id: None, workspace_root: None, meta_path: None, turns: None, authored_turns: None, registered: false },
        ];
        let p = pick_session(&cands, "20260809-100000").unwrap();
        assert_eq!(p.id, "20260809-100000.123456789-model-a"); // 取最短主会话
    }
}
