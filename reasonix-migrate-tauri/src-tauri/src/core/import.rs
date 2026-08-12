//! 导入：解包 zip → 目标 REASONIX_HOME（slug 三层映射 + 冲突处理 + 穿越防护 + 校验）。
//!
//! 逻辑与 Python 版 `reasonix-migrate-import.py` 对齐。

use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::common::{
    base_session_id, name_skipped, session_id_of, sha256_bytes, slug_of, FORMAT_VERSION,
};

fn normp(p: &str) -> String {
    let norm = Path::new(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("\\");
    norm.to_lowercase()
}

/// 读取备份 zip 的 manifest，列出其中出现过的所有项目（workspace）路径，去重排序。
/// 用于导入页「从备份读取项目」——用户看到的是真实路径而不是内部 slug。
pub fn list_zip_workspaces(zip_path: &Path) -> Result<Vec<String>, String> {
    let file = fs::File::open(zip_path).map_err(|e| format!("无法打开备份 zip: {}", e))?;
    let mut zf = zip::ZipArchive::new(file).map_err(|e| format!("无法打开备份 zip: {}", e))?;
    let manifest: Value = zf
        .by_name("manifest.json")
        .map_err(|_| "zip 内缺少 manifest.json，不是 reasonix-migrate 备份".to_string())
        .and_then(|mut r| {
            let mut s = String::new();
            r.read_to_string(&mut s).map_err(|e| e.to_string())?;
            serde_json::from_str(&s).map_err(|e| e.to_string())
        })?;
    let mut workspaces: Vec<String> = Vec::new();
    if let Some(sessions) = manifest.get("sessions").and_then(|v| v.as_object()) {
        for list in sessions.values() {
            if let Some(arr) = list.as_array() {
                for s in arr {
                    if let Some(w) = s.get("workspace_root").and_then(|v| v.as_str()) {
                        if !w.is_empty() && !workspaces.iter().any(|x| x == w) {
                            workspaces.push(w.to_string());
                        }
                    }
                }
            }
        }
    }
    workspaces.sort();
    Ok(workspaces)
}

/// 三层映射：--map 显式 > 原路径存在(同机器) > 目录名匹配 > 兜底保留原 slug。
fn map_slug(
    workspace_root: Option<&str>,
    orig_slug: &str,
    existing_slugs: &[String],
    manual_norm: &HashMap<String, String>,
) -> (String, bool) {
    if let Some(ws) = workspace_root {
        let key = normp(ws);
        if let Some(v) = manual_norm.get(&key) {
            return (v.clone(), true);
        }
        if Path::new(ws).is_dir() {
            return (orig_slug.to_string(), true); // 同机器：原路径仍存在
        }
        let base = ws.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
        for slug in existing_slugs {
            if slug.to_lowercase() == base || slug.to_lowercase().ends_with(&format!("-{}", base)) {
                return (slug.clone(), true);
            }
        }
    }
    (orig_slug.to_string(), false) // 兜底：保留原 slug（未匹配，进报告）
}

fn scan_existing_slugs(target: &Path) -> Vec<String> {
    let projects = target.join("projects");
    let mut v: Vec<String> = fs::read_dir(&projects)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .map(|p| p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// 目标 home 已有会话的主 id 集合（冲突检测用）。
fn scan_existing_session_ids(target: &Path) -> HashSet<String> {
    let mut ids = HashSet::new();
    let projects = target.join("projects");
    let Ok(rd) = fs::read_dir(&projects) else {
        return ids;
    };
    for slug_dir in rd.flatten() {
        let sdir = slug_dir.path().join("sessions");
        let Ok(srd) = fs::read_dir(&sdir) else {
            continue;
        };
        for c in srd.flatten() {
            let name = c.file_name().to_string_lossy().to_string();
            if name_skipped(&name) {
                continue;
            }
            if let Some(sid) = session_id_of(&name) {
                ids.insert(base_session_id(sid).to_string());
            }
        }
    }
    ids
}

/// 分类 zip 内相对路径：
/// ("session", 原slug, 主会话id) / ("non_session", 原slug, None) / ("other", None, None)。
pub fn classify_rel(rel: &str, session_ids: &HashSet<String>) -> (&'static str, Option<String>, Option<String>) {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() >= 3 && parts[0] == "projects" {
        if parts[2] == "sessions" && parts.len() >= 4 {
            let name = parts[3];
            if let Some(sid) = session_id_of(name) {
                return ("session", Some(parts[1].to_string()), Some(base_session_id(sid).to_string()));
            }
            for known in session_ids {
                if name.starts_with(&format!("{}.", known)) {
                    return ("session", Some(parts[1].to_string()), Some(known.clone()));
                }
            }
        }
        return ("non_session", Some(parts[1].to_string()), None);
    }
    ("other", None, None)
}

/// 把 zip 内相对路径解析到 target 内；词法级防穿越（`..` 越界返回 None）。
pub fn safe_dst(target: &Path, new_rel: &str) -> Option<PathBuf> {
    let mut parts: Vec<&str> = Vec::new();
    for seg in new_rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return None; // 越出根 → 逃逸
                }
            }
            s => parts.push(s),
        }
    }
    Some(parts.iter().fold(target.to_path_buf(), |acc, s| acc.join(s)))
}

pub struct ImportOptions<'a> {
    pub backup: &'a Path,
    pub target: &'a Path,
    pub maps: &'a [String],
    pub overwrite: bool,
    pub verify: bool,
    pub skip_hash_check: bool,
}

#[derive(Serialize)]
pub struct ImportSummary {
    pub imported_sessions: usize,
    pub ok_files: usize,
    pub skipped_ids: Vec<String>,
    pub skipped_files: usize,
    pub conflict_files: usize,
    pub unmatched: Vec<(String, String, String)>,
    pub errors: Vec<String>,
    pub target: PathBuf,
}

/// 导入：解包前校验 → slug 映射 → 冲突检测 → 解包 → 可选复查 → 报告。
pub fn import(opts: &ImportOptions) -> Result<ImportSummary, String> {
    if !opts.backup.is_file() {
        return Err(format!("备份文件不存在: {}", opts.backup.display()));
    }
    let target = opts.target.to_path_buf();
    fs::create_dir_all(&target).map_err(|e| format!("无法创建目标目录: {}", e))?;

    // --map 解析校验：每行 原项目路径=新项目路径（或新 slug），右侧填路径会自动转成 slug
    let mut manual_norm: HashMap<String, String> = HashMap::new();
    for m in opts.maps {
        let Some((k, v)) = m.split_once('=') else {
            return Err(format!("--map 格式应为 原项目路径=新项目路径: {}", m));
        };
        if v.is_empty() {
            return Err(format!("--map 的等号右边不能为空: {}", m));
        }
        // 右侧填「路径」或「slug」都支持：看起来像路径就自动转成 slug
        let dst = if v.contains(":\\") || v.contains("\\\\") || v.starts_with('/') {
            slug_of(v)
        } else {
            v.to_string()
        };
        if dst.is_empty() || dst.split('/').any(|s| s == "..") {
            return Err(format!("--map 的目标路径/slug 非法: {}", v));
        }
        manual_norm.insert(normp(k), dst);
    }

    // 打开 zip + manifest
    let file = fs::File::open(opts.backup).map_err(|e| format!("无法打开备份 zip: {}", e))?;
    let mut zf = zip::ZipArchive::new(file).map_err(|e| format!("无法打开备份 zip: {}", e))?;
    let manifest: Value = zf
        .by_name("manifest.json")
        .map_err(|_| "zip 内缺少合法 manifest.json，不是 reasonix-migrate 备份".to_string())
        .and_then(|mut r| {
            let mut s = String::new();
            r.read_to_string(&mut s).map_err(|e| e.to_string())?;
            serde_json::from_str(&s).map_err(|e| format!("manifest.json 解析失败: {}", e))
        })?;
    let fmt = manifest.get("format_version").and_then(|v| v.as_u64());
    if fmt != Some(FORMAT_VERSION as u64) {
        return Err(format!(
            "不支持的格式版本: {:?}（工具支持 {}）",
            manifest.get("format_version"),
            FORMAT_VERSION
        ));
    }

    let files = manifest
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let sessions = manifest
        .get("sessions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 解包前全量哈希校验
    if !opts.skip_hash_check {
        let mut bad = 0usize;
        let mut bad_msgs: Vec<String> = Vec::new();
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
                    return Err(format!("manifest 记录的文件不在 zip 内: {}", rel));
                }
            };
            if sha256_bytes(&data) != expected {
                bad += 1;
                bad_msgs.push(rel.to_string());
            }
        }
        if bad > 0 {
            return Err(format!("{} 个文件校验失败（损坏或篡改），中止导入：{}", bad, bad_msgs.join("；")));
        }
    }

    // slug 映射
    let session_ids: HashSet<String> = sessions
        .iter()
        .filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect();
    let existing_slugs = scan_existing_slugs(&target);
    let mut slug_map: HashMap<String, String> = HashMap::new();
    let mut unmatched: Vec<(String, String, String)> = Vec::new();
    for s in &sessions {
        let orig = s.get("slug").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if slug_map.contains_key(&orig) {
            continue;
        }
        let ws = s.get("workspace_root").and_then(|v| v.as_str());
        let (mapped, ok) = map_slug(ws, &orig, &existing_slugs, &manual_norm);
        slug_map.insert(orig.clone(), mapped.clone());
        if !ok {
            unmatched.push((ws.unwrap_or("?").to_string(), orig, mapped));
        }
    }
    // 覆盖 manifest 中出现但 sessions 未列出的 slug（无会话的遗留项目目录）
    for fi in &files {
        let rel = fi.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let (_, orig_slug, _) = classify_rel(rel, &session_ids);
        if let Some(os) = orig_slug {
            slug_map.entry(os.clone()).or_insert(os);
        }
    }

    // 冲突检测
    let existing_ids = scan_existing_session_ids(&target);
    let mut conflict_ids: Vec<String> = sessions
        .iter()
        .filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(String::from))
        .filter(|id| existing_ids.contains(id))
        .collect();
    conflict_ids.sort();
    let mut conflict_files: HashMap<String, PathBuf> = HashMap::new();
    for fi in &files {
        let rel = fi.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let (kind, _, _) = classify_rel(rel, &session_ids);
        if kind != "session" {
            if let Some(dst) = safe_dst(&target, rel) {
                if dst.exists() {
                    conflict_files.insert(rel.to_string(), dst);
                }
            }
        }
    }

    // 解包
    let mut ok_files = 0usize;
    let skipped_ids: HashSet<String> = if opts.overwrite {
        HashSet::new()
    } else {
        conflict_ids.iter().cloned().collect()
    };
    let mut skipped_files = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for fi in &files {
        let rel = fi.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let (kind, orig_slug, sid) = classify_rel(rel, &session_ids);
        if kind == "session" {
            if let Some(s) = &sid {
                if skipped_ids.contains(s) {
                    skipped_files += 1;
                    continue;
                }
            }
        }
        if kind != "session" && conflict_files.contains_key(rel) && !opts.overwrite {
            skipped_files += 1;
            continue;
        }
        let mut new_rel = rel.to_string();
        if kind != "other" {
            if let Some(os) = &orig_slug {
                let mapped = slug_map.get(os).cloned().unwrap_or_else(|| os.clone());
                new_rel = rel.replacen(&format!("projects/{}/", os), &format!("projects/{}/", mapped), 1);
            }
        }
        let Some(dst) = safe_dst(&target, &new_rel) else {
            errors.push(format!("{} -> {}（路径逃逸目标目录，拒绝写入）", rel, new_rel));
            continue;
        };
        if let Some(parent) = dst.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let data = match zf.by_name(rel) {
            Ok(mut r) => {
                let mut buf = Vec::new();
                let _ = r.read_to_end(&mut buf);
                buf
            }
            Err(e) => {
                errors.push(format!("{}: {}", rel, e));
                continue;
            }
        };
        match fs::write(&dst, &data) {
            Ok(()) => ok_files += 1,
            Err(e) => errors.push(format!("{}: {}", rel, e)),
        }
    }

    // 导入后复查
    let mut verify_failed = 0usize;
    if opts.verify {
        for fi in &files {
            let rel = fi.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let (kind, orig_slug, sid) = classify_rel(rel, &session_ids);
            if kind == "session" {
                if let Some(s) = &sid {
                    if skipped_ids.contains(s) {
                        continue;
                    }
                }
            }
            if kind != "session" && conflict_files.contains_key(rel) && !opts.overwrite {
                continue;
            }
            let mut new_rel = rel.to_string();
            if kind != "other" {
                if let Some(os) = &orig_slug {
                    let mapped = slug_map.get(os).cloned().unwrap_or_else(|| os.clone());
                    new_rel = rel.replacen(&format!("projects/{}/", os), &format!("projects/{}/", mapped), 1);
                }
            }
            let Some(dst) = safe_dst(&target, &new_rel) else {
                verify_failed += 1;
                continue;
            };
            if !dst.is_file() {
                verify_failed += 1;
                continue;
            }
            let expected = fi.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
            let data = fs::read(&dst).unwrap_or_default();
            if sha256_bytes(&data) != expected {
                verify_failed += 1;
            }
        }
        if verify_failed > 0 {
            return Err(format!("--verify 发现 {} 个问题", verify_failed));
        }
    }

    let imported_sessions = sessions
        .iter()
        .filter(|s| {
            s.get("id")
                .and_then(|v| v.as_str())
                .map(|id| !skipped_ids.contains(id))
                .unwrap_or(false)
        })
        .count();

    // 实际跳过的 id（--overwrite 时为空，与 Python 一致）
    let reported_skipped: Vec<String> = if opts.overwrite { Vec::new() } else { conflict_ids };

    Ok(ImportSummary {
        imported_sessions,
        ok_files,
        skipped_ids: reported_skipped,
        skipped_files,
        conflict_files: conflict_files.len(),
        unmatched,
        errors,
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::export::{export, ExportOptions, ExportSummary};
    use std::io::Write;
    use tempfile::tempdir;

    fn make_src_home(home: &Path) {
        let sd = home.join("projects/c--fake-machine-proj-a/sessions");
        fs::create_dir_all(sd.join("20260809-100000.123456789-model-a.ckpt")).unwrap();
        fs::write(sd.join("20260809-100000.123456789-model-a.jsonl"), r#"{"events":[]}"#).unwrap();
        fs::write(
            sd.join("20260809-100000.123456789-model-a.jsonl.meta"),
            r#"{"scope":"project","workspace_root":"X:\\fake\\proj-a","topic_title":"会话A"}"#,
        )
        .unwrap();
        fs::write(sd.join("20260809-100000.123456789-model-a.events.jsonl"), "event\n").unwrap();
        fs::write(sd.join("20260809-100000.123456789-model-a.ckpt/turn-0.json"), "{}").unwrap();
        fs::write(sd.join("20260809-110000.123456789-model-b.jsonl"), "b\n").unwrap();
        fs::write(
            sd.join("20260809-110000.123456789-model-b.jsonl.meta"),
            r#"{"topic_title":"会话B"}"#,
        )
        .unwrap();
    }

    fn do_export(home: &Path, out: &Path) -> ExportSummary {
        let opts = ExportOptions {
            source: home,
            output: out,
            project_filters: &[],
            session_filters: &[],
            since: None,
            include_secrets: false,
        };
        export(&opts, &mut |_, _| {}).expect("export ok")
    }

    #[test]
    fn import_cross_machine_and_map() {
        let src = tempdir().unwrap();
        make_src_home(src.path());
        let zip = src.path().join("b.zip");
        do_export(src.path(), &zip);

        // 跨机器导入：X:\fake 不存在 → 保留原 slug
        let tgt = tempdir().unwrap();
        let opts = ImportOptions {
            backup: &zip,
            target: tgt.path(),
            maps: &[],
            overwrite: false,
            verify: true,
            skip_hash_check: false,
        };
        let sum = import(&opts).expect("import ok");
        assert_eq!(sum.imported_sessions, 2);
        assert_eq!(sum.errors.len(), 0);
        assert!(sum.unmatched.iter().any(|u| u.1 == "c--fake-machine-proj-a"));
        assert!(
            tgt.path()
                .join("projects/c--fake-machine-proj-a/sessions/20260809-100000.123456789-model-a.jsonl")
                .is_file()
        );

        // --map 显式映射到自定义 slug
        let tgt2 = tempdir().unwrap();
        let opts2 = ImportOptions {
            backup: &zip,
            target: tgt2.path(),
            maps: &[r"X:\fake\proj-a=slug-mapped-xyz".to_string()],
            overwrite: false,
            verify: true,
            skip_hash_check: false,
        };
        let sum2 = import(&opts2).expect("import ok");
        eprintln!("DEBUG unmatched={:?}", sum2.unmatched);
        assert!(sum2.unmatched.is_empty());
        assert!(
            tgt2.path()
                .join("projects/slug-mapped-xyz/sessions/20260809-100000.123456789-model-a.jsonl")
                .is_file()
        );
    }

    #[test]
    fn import_conflict_skip_and_overwrite() {
        let src = tempdir().unwrap();
        make_src_home(src.path());
        let zip = src.path().join("b.zip");
        do_export(src.path(), &zip);

        let tgt = tempdir().unwrap();
        // 目标已有同 id 会话
        let td = tgt.path().join("projects/c--fake-machine-proj-a/sessions");
        fs::create_dir_all(&td).unwrap();
        fs::write(td.join("20260809-100000.123456789-model-a.jsonl"), "EXISTING").unwrap();
        fs::write(
            td.join("20260809-100000.123456789-model-a.jsonl.meta"),
            r#"{"topic_title":"已有"}"#,
        )
        .unwrap();

        // 默认跳过冲突
        let opts = ImportOptions {
            backup: &zip,
            target: tgt.path(),
            maps: &[],
            overwrite: false,
            verify: true,
            skip_hash_check: false,
        };
        let sum = import(&opts).expect("import ok");
        assert_eq!(sum.imported_sessions, 1); // model-b 导入，model-a 跳过
        assert_eq!(sum.skipped_ids, vec!["20260809-100000.123456789-model-a".to_string()]);
        // 已有文件未被覆盖
        let existing = fs::read_to_string(
            td.join("20260809-100000.123456789-model-a.jsonl"),
        )
        .unwrap();
        assert_eq!(existing, "EXISTING");

        // --overwrite 覆盖
        let opts2 = ImportOptions {
            backup: &zip,
            target: tgt.path(),
            maps: &[],
            overwrite: true,
            verify: true,
            skip_hash_check: false,
        };
        let sum2 = import(&opts2).expect("import ok");
        assert_eq!(sum2.imported_sessions, 2);
        assert!(sum2.skipped_ids.is_empty());
    }

    #[test]
    fn import_rejects_traversal() {
        // 构造恶意 zip：路径含 ../ 且 manifest 记录之
        let evil_dir = tempdir().unwrap();
        let zip_path = evil_dir.path().join("evil.zip");
        let f = fs::File::create(&zip_path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let payload = b"evil".to_vec();
        let manifest = serde_json::json!({
            "format_version": FORMAT_VERSION,
            "files": [{"path": "../evil.txt", "size": payload.len(), "sha256": sha256_bytes(&payload)}],
            "sessions": [],
            "warnings": []
        });
        w.start_file("manifest.json".to_string(), opts).unwrap();
        w.write_all(serde_json::to_string(&manifest).unwrap().as_bytes()).unwrap();
        w.start_file("../evil.txt".to_string(), opts).unwrap();
        w.write_all(&payload).unwrap();
        w.finish().unwrap();

        let tgt = tempdir().unwrap();
        let opts = ImportOptions {
            backup: &zip_path,
            target: tgt.path(),
            maps: &[],
            overwrite: false,
            verify: false,
            skip_hash_check: false,
        };
        // 穿越路径被 safe_dst 拒绝 → 写入失败进 errors（不产生逃逸文件）
        let sum = import(&opts).expect("import 不应因穿越整体失败");
        assert!(!sum.errors.is_empty());
        assert!(!evil_dir.path().parent().unwrap().join("evil.txt").exists());
        assert!(!tgt.path().join("../evil.txt").exists());

        // --map 注入被拒（整体 Err）
        let opts2 = ImportOptions {
            backup: &zip_path,
            target: tgt.path(),
            maps: &[r"X:\fake\proj-a=../escape".to_string()],
            overwrite: false,
            verify: false,
            skip_hash_check: false,
        };
        assert!(import(&opts2).is_err());
    }
}
