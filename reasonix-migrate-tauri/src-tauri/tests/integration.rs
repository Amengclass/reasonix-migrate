//! 集成测试：把 Python `test_boundary.py` 的 29 项断言翻译到 Rust（补单元测试未覆盖的场景）。
//!
//! 覆盖：空 home 导出导入 / --include-secrets / 目录名 slug 匹配 / --since 过滤。
//! 其余场景（.trash/.env 排除、recovery 归并、同 id 冲突、--map、穿越防护、--restart-app）
//! 已在 core::export / core::import / core::one 的单元测试中覆盖。

use reasonix_migrate_tauri_lib::core::export::{export, verify, ExportOptions};
use reasonix_migrate_tauri_lib::core::import::{import, ImportOptions};
use reasonix_migrate_tauri_lib::core::one::migrate_one;
use reasonix_migrate_tauri_lib::core::one::OneOptions;
use serde_json::Value;
use std::fs;
use std::io::Read;
use tempfile::tempdir;

fn write_meta(sdir: &std::path::Path, sid: &str, ws: &str, title: &str) {
    fs::write(
        sdir.join(format!("{}.jsonl", sid)),
        format!(r#"{{"events":["{}"]}}"#, sid),
    )
    .unwrap();
    fs::write(
        sdir.join(format!("{}.jsonl.meta", sid)),
        format!(
            r#"{{"scope":"project","workspace_root":"{}","topic_title":"{}","topic_id":"topic_{}"}}"#,
            ws.replace('\\', "\\\\"),
            title,
            sid
        ),
    )
    .unwrap();
    fs::write(sdir.join(format!("{}.events.jsonl", sid)), "event\n").unwrap();
    let ck = sdir.join(format!("{}.ckpt", sid));
    fs::create_dir_all(&ck).unwrap();
    fs::write(ck.join("turn-0.json"), "{}").unwrap();
}

#[test]
fn empty_home_export_import() {
    let empty = tempdir().unwrap();
    fs::write(empty.path().join("config.toml"), "x=1\n").unwrap();
    let zip = empty.path().join("empty.zip");
    let opts = ExportOptions {
        source: empty.path(),
        output: &zip,
        project_filters: &[],
        session_filters: &[],
        since: None,
        include_secrets: false,
    };
    export(&opts, &mut |_, _| {}).expect("空 home 导出成功");

    let tgt = tempdir().unwrap();
    let iopts = ImportOptions {
        backup: &zip,
        target: tgt.path(),
        maps: &[],
        overwrite: false,
        verify: true,
        skip_hash_check: false,
    };
    let sum = import(&iopts).expect("空 home 导入成功");
    assert_eq!(sum.imported_sessions, 0);
    assert!(tgt.path().join("config.toml").is_file());
}

#[test]
fn include_secrets_flag() {
    let src = tempdir().unwrap();
    fs::write(src.path().join(".env"), "SECRET=xx\n").unwrap();
    let sdir = src.path().join("projects/c--fake-machine-proj-a/sessions");
    fs::create_dir_all(&sdir).unwrap();
    write_meta(&sdir, "20260809-100000.123456789-model-a", r"X:\fake\proj-a", "会话A");

    // 默认排除 .env
    let zip1 = src.path().join("no-secret.zip");
    let opts1 = ExportOptions {
        source: src.path(),
        output: &zip1,
        project_filters: &[],
        session_filters: &[],
        since: None,
        include_secrets: false,
    };
    export(&opts1, &mut |_, _| {}).unwrap();
    let f = fs::File::open(&zip1).unwrap();
    let mut zf = zip::ZipArchive::new(f).unwrap();
    for i in 0..zf.len() {
        let name = zf.by_index(i).unwrap().name().to_string();
        assert!(!name.ends_with(".env"), "不应包含 .env: {}", name);
    }
    drop(zf);

    // --include-secrets 包含 .env
    let zip2 = src.path().join("with-secret.zip");
    let opts2 = ExportOptions {
        source: src.path(),
        output: &zip2,
        project_filters: &[],
        session_filters: &[],
        since: None,
        include_secrets: true,
    };
    export(&opts2, &mut |_, _| {}).unwrap();
    let f = fs::File::open(&zip2).unwrap();
    let mut zf = zip::ZipArchive::new(f).unwrap();
    let has_env = (0..zf.len()).any(|i| zf.by_index(i).unwrap().name().ends_with(".env"));
    assert!(has_env, "include_secrets 应包含 .env");
}

#[test]
fn dir_name_slug_match() {
    // 源导出（workspace_root=X:\fake\proj-a 不存在）
    let src = tempdir().unwrap();
    let sdir = src.path().join("projects/c--fake-machine-proj-a/sessions");
    fs::create_dir_all(&sdir).unwrap();
    write_meta(&sdir, "20260809-100000.123456789-model-a", r"X:\fake\proj-a", "会话A");
    let zip = src.path().join("b.zip");
    let eopts = ExportOptions {
        source: src.path(),
        output: &zip,
        project_filters: &[],
        session_filters: &[],
        since: None,
        include_secrets: false,
    };
    export(&eopts, &mut |_, _| {}).unwrap();

    // 目标已有以 "proj-a" 结尾的 slug → 目录名匹配映射过去
    let tgt = tempdir().unwrap();
    fs::create_dir_all(tgt.path().join("projects/d--other-machine-proj-a")).unwrap();
    let iopts = ImportOptions {
        backup: &zip,
        target: tgt.path(),
        maps: &[],
        overwrite: false,
        verify: true,
        skip_hash_check: false,
    };
    let sum = import(&iopts).expect("目录名匹配导入成功");
    assert!(sum.unmatched.is_empty(), "目录名匹配应成功，unmatched={:?}", sum.unmatched);
    assert!(
        tgt.path()
            .join("projects/d--other-machine-proj-a/sessions/20260809-100000.123456789-model-a.jsonl")
            .is_file()
    );
}

#[test]
fn since_filter_export() {
    let src = tempdir().unwrap();
    let sdir = src.path().join("projects/c--fake-machine-proj-a/sessions");
    fs::create_dir_all(&sdir).unwrap();
    write_meta(&sdir, "20260809-100000.123456789-model-a", r"X:\fake\proj-a", "会话A");
    // --since 设为未来时间 → 无会话导出（文件仍打包，但 sessions 记录为空）
    let zip = src.path().join("s.zip");
    let opts = ExportOptions {
        source: src.path(),
        output: &zip,
        project_filters: &[],
        session_filters: &[],
        since: Some("2099-01-01"),
        include_secrets: false,
    };
    let summary = export(&opts, &mut |_, _| {}).expect("since 过滤导出成功");
    assert_eq!(summary.session_count, 0, "since=未来应过滤掉所有会话");
}

#[test]
fn one_migrate_registers_existing_project() {
    // 目标 home 已有 desktop-projects.json 且 workspace 未注册 → 迁移后自动注册
    let src = tempdir().unwrap();
    let sdir = src.path().join("sessions");
    fs::create_dir_all(&sdir).unwrap();
    write_meta(&sdir, "20260809-100000.123456789-model-a", r"X:\fake\proj-a", "会话A");
    let tgt_home = tempdir().unwrap();
    fs::write(
        tgt_home.path().join("desktop-projects.json"),
        r#"{"projects":[{"root":"C:\\existing","topics":["t1"]}],"deletedTopics":[]}"#,
    )
    .unwrap();
    let ws = src.path().join("proj-ws");
    let opts = OneOptions {
        from_sessions: Some(sdir.to_string_lossy().to_string()),
        session: Some("20260809-100000".to_string()),
        to_workspace: Some(ws.to_string_lossy().to_string()),
        to_home: Some(tgt_home.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    migrate_one(&opts).expect("迁移成功");
    // 项目已注册（root 为规范化后的 ws 路径）
    let raw = fs::read_to_string(tgt_home.path().join("desktop-projects.json")).unwrap();
    let data: Value = serde_json::from_str(&raw).unwrap();
    let roots: Vec<&str> = data["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["root"].as_str().unwrap_or(""))
        .collect();
    assert!(
        roots.iter().any(|r| r.to_lowercase().contains("proj-ws")),
        "应注册新项目: {:?}",
        roots
    );
}

#[test]
fn zip_manifest_roundtrip_structure() {
    // 导出 → 读 manifest → 关键字段结构对齐 Python 版
    let src = tempdir().unwrap();
    let sdir = src.path().join("projects/c--fake-machine-proj-a/sessions");
    fs::create_dir_all(&sdir).unwrap();
    write_meta(&sdir, "20260809-100000.123456789-model-a", r"X:\fake\proj-a", "会话A");
    let zip = src.path().join("m.zip");
    let opts = ExportOptions {
        source: src.path(),
        output: &zip,
        project_filters: &[],
        session_filters: &[],
        since: None,
        include_secrets: false,
    };
    export(&opts, &mut |_, _| {}).unwrap();
    let f = fs::File::open(&zip).unwrap();
    let mut zf = zip::ZipArchive::new(f).unwrap();
    let mut raw = String::new();
    zf.by_name("manifest.json").unwrap().read_to_string(&mut raw).unwrap();
    let m: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(m["format_version"], 1);
    assert!(m["exported_at"].is_string());
    assert!(m["source_home"].is_string());
    let fp = &m["structure_fingerprint"];
    assert!(fp["top_level"].is_array());
    assert!(fp["slug_list"].is_array());
    assert!(fp["session_sidecars"].is_array());
    let s0 = &m["sessions"][0];
    assert!(s0["id"].is_string());
    assert!(s0["slug"].is_string());
    assert!(s0["workspace_root"].is_string());
    assert!(s0["last_updated"].is_string());
    assert_eq!(s0["recovery_branch_count"], 0);
    // 所有 files 记录的路径都在 zip 内且哈希一致
    for fi in m["files"].as_array().unwrap() {
        let rel = fi["path"].as_str().unwrap();
        let mut buf = Vec::new();
        zf.by_name(rel).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(fi["sha256"], sha256(&buf), "哈希不一致: {}", rel);
    }
}

fn sha256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}
