//! Reasonix SQLite session-catalog 预置（best-effort）。
//!
//! 迁移完成后把会话直接写进 reasonix 的 catalog（cache/session-catalog/v2.sqlite），
//! 这样重启 reasonix 后左侧立即显示，无需等它后台异步扫描。
//! 失败只记录警告，不影响迁移本身（reasonix 扫描后仍会收录）。

use rusqlite::{params, Connection};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// ISO-8601（UTC，如 "2026-08-11T16:58:46.0413584Z"）→ epoch 毫秒。
fn iso_to_epoch_ms(s: &str) -> i64 {
    let b = s.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[13] != b':'
        || b[16] != b':'
    {
        return 0;
    }
    let y: i64 = s[0..4].parse().unwrap_or(0);
    let mo: i64 = s[5..7].parse().unwrap_or(0);
    let d: i64 = s[8..10].parse().unwrap_or(0);
    let h: i64 = s[11..13].parse().unwrap_or(0);
    let mi: i64 = s[14..16].parse().unwrap_or(0);
    let se: i64 = s[17..19].parse().unwrap_or(0);
    let ms: i64 = s.get(20..23).and_then(|x| x.parse().ok()).unwrap_or(0);
    // Howard Hinnant days_from_civil（UTC）：2026-08-11 → 20676
    let yy = y - if mo <= 2 { 1 } else { 0 };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days * 86400 + h * 3600 + mi * 60 + se) * 1000 + ms
}

/// reasonix fingerprint："{len}:{mtime_ns}"。
fn fingerprint(path: &Path) -> Result<String, String> {
    let md = fs::metadata(path).map_err(|e| e.to_string())?;
    let mtime_ns = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok(format!("{}:{}", md.len(), mtime_ns))
}

fn meta_str<'a>(meta: &'a Value, k: &str) -> &'a str {
    meta.get(k).and_then(|v| v.as_str()).unwrap_or("")
}

/// 把迁移好的会话预置进 reasonix catalog。
/// `jsonl_path` 为目标 home 里的会话主文件；`meta` 为修正后的 meta 内容。
pub fn ensure_catalog_session(home: &Path, jsonl_path: &Path, meta: &Value) -> Result<(), String> {
    let db_dir = home.join("cache/session-catalog");
    let db = db_dir.join("v2.sqlite");
    if !db.exists() {
        // 无 catalog 环境（reasonix 版本较旧或尚未初始化）→ 跳过，不报错
        return Ok(());
    }
    let conn = Connection::open(&db).map_err(|e| format!("打开 session-catalog 失败: {}", e))?;
    conn.busy_timeout(Duration::from_millis(3000))
        .map_err(|e| format!("设置 busy_timeout 失败: {}", e))?;

    let topic_id = meta_str(meta, "topic_id");
    if topic_id.is_empty() {
        return Ok(()); // 无 topic 的会话不进 catalog
    }
    let scope = meta_str(meta, "scope");
    let ws = meta_str(meta, "workspace_root");
    let title = {
        let t = meta_str(meta, "topic_title");
        if !t.is_empty() {
            t.to_string()
        } else {
            meta_str(meta, "title").to_string()
        }
    };
    let turns = meta.get("turns").and_then(|v| v.as_i64()).unwrap_or(0);
    let created_at = meta_str(meta, "created_at");
    let updated_at = meta_str(meta, "updated_at");
    let created_ms = if created_at.is_empty() {
        now_ms()
    } else {
        iso_to_epoch_ms(created_at)
    };
    let activity_ms = if updated_at.is_empty() {
        now_ms()
    } else {
        iso_to_epoch_ms(updated_at)
    };
    let preview = meta_str(meta, "preview").to_string();
    let jsonl_fp = fingerprint(jsonl_path)?;
    let meta_path = jsonl_path.with_extension("jsonl.meta");
    let meta_fp = fingerprint(&meta_path).unwrap_or_default();
    let directory = jsonl_path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let jsonl_str = jsonl_path.to_string_lossy().to_string();

    // 事务包裹：topics + sessions 要么都进要么都不进
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("catalog 事务失败: {}", e))?;

    tx.execute(
        "INSERT OR REPLACE INTO catalog_topics \
         (scope, workspace_root, topic_id, title, title_source, pinned, sort_order, \
          turns, turns_state, created_at, last_activity_at, recovery_state, health, metadata_present) \
         VALUES (?1, ?2, ?3, ?4, 'manual', 0, 0, ?5, 'valid', ?6, ?7, '', 'ok', 1)",
        params![scope, ws, topic_id, title, turns, created_ms, activity_ms],
    )
    .map_err(|e| format!("写入 catalog_topics 失败: {}", e))?;

    tx.execute(
        "INSERT OR REPLACE INTO catalog_sessions \
         (path, directory, scope, workspace_root, topic_id, topic_title, custom_title, \
          created_at, last_activity_at, preview, turns, turns_state, recovered, \
          recovery_reason, recovery_digest, parent_id, content_fingerprint, meta_fingerprint, \
          health, missing_since, seen_generation, recovery_copy) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', ?7, ?8, ?9, ?10, 'valid', 0, '', '', '', \
          ?11, ?12, 'ok', 0, 0, 0)",
        params![
            jsonl_str,
            directory,
            scope,
            ws,
            topic_id,
            title,
            created_ms,
            activity_ms,
            preview,
            turns,
            jsonl_fp,
            meta_fp
        ],
    )
    .map_err(|e| format!("写入 catalog_sessions 失败: {}", e))?;

    tx.commit()
        .map_err(|e| format!("catalog 提交失败: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn make_db(dir: &Path) -> Connection {
        fs::create_dir_all(dir.join("cache/session-catalog")).unwrap();
        let db = dir.join("cache/session-catalog/v2.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE catalog_topics (
                scope TEXT NOT NULL DEFAULT 'project',
                workspace_root TEXT NOT NULL,
                topic_id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                title_source TEXT NOT NULL DEFAULT 'manual',
                pinned INTEGER NOT NULL DEFAULT 0,
                sort_order INTEGER NOT NULL DEFAULT 0,
                turns INTEGER NOT NULL DEFAULT 0,
                turns_state TEXT NOT NULL DEFAULT 'valid',
                created_at INTEGER NOT NULL DEFAULT 0,
                last_activity_at INTEGER NOT NULL DEFAULT 0,
                recovery_state TEXT NOT NULL DEFAULT '',
                health TEXT NOT NULL DEFAULT 'ok',
                metadata_present INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE catalog_sessions (
                path TEXT PRIMARY KEY,
                directory TEXT NOT NULL DEFAULT '',
                scope TEXT NOT NULL DEFAULT 'project',
                workspace_root TEXT NOT NULL DEFAULT '',
                topic_id TEXT NOT NULL DEFAULT '',
                topic_title TEXT NOT NULL DEFAULT '',
                custom_title TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL DEFAULT 0,
                last_activity_at INTEGER NOT NULL DEFAULT 0,
                preview TEXT NOT NULL DEFAULT '',
                turns INTEGER NOT NULL DEFAULT 0,
                turns_state TEXT NOT NULL DEFAULT 'valid',
                recovered INTEGER NOT NULL DEFAULT 0,
                recovery_reason TEXT NOT NULL DEFAULT '',
                recovery_digest TEXT NOT NULL DEFAULT '',
                parent_id TEXT NOT NULL DEFAULT '',
                content_fingerprint TEXT NOT NULL DEFAULT '',
                meta_fingerprint TEXT NOT NULL DEFAULT '',
                health TEXT NOT NULL DEFAULT 'ok',
                missing_since INTEGER NOT NULL DEFAULT 0,
                seen_generation INTEGER NOT NULL DEFAULT 0,
                recovery_copy INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn iso_parse() {
        assert_eq!(iso_to_epoch_ms("2026-08-11T16:58:46.0413584Z"), 1786467526041);
        assert_eq!(iso_to_epoch_ms("2026-08-12T10:09:36.0000000Z"), 1786529376000);
    }

    #[test]
    fn write_topics_and_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let conn = make_db(dir.path());
        drop(conn);
        // 会话文件（jsonl + meta）
        let sd = dir.path().join("projects/c--x-test/sessions");
        fs::create_dir_all(&sd).unwrap();
        let jsonl = sd.join("20260811-165846.041358400-opencode-go-deepseek-v4-flash.jsonl");
        fs::write(&jsonl, b"{\"events\":[]}").unwrap();
        let meta_path = sd.join("20260811-165846.041358400-opencode-go-deepseek-v4-flash.jsonl.meta");
        fs::write(&meta_path, b"{}").unwrap();

        let meta = json!({
            "scope": "project",
            "workspace_root": r"C:\Users\Ameng\Desktop\claude_woker\test",
            "topic_id": "topic_20260812-100936_1234567890abcdef",
            "topic_title": "AI打工仔",
            "turns": 17,
            "created_at": "2026-08-11T16:58:46.0413584Z",
            "updated_at": "2026-08-12T01:38:36.5330731Z",
            "preview": "这是预览"
        });

        let home = dir.path();
        ensure_catalog_session(home, &jsonl, &meta).unwrap();

        let conn = Connection::open(home.join("cache/session-catalog/v2.sqlite")).unwrap();
        let t: (String, i64, i64, String) = conn
            .query_row(
                "SELECT topic_id, turns, created_at, title FROM catalog_topics",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(t.0, "topic_20260812-100936_1234567890abcdef");
        assert_eq!(t.1, 17);
        assert_eq!(t.2, 1786467526041);
        assert_eq!(t.3, "AI打工仔");

        let s: (String, String, i64) = conn
            .query_row(
                "SELECT topic_id, content_fingerprint, turns FROM catalog_sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(s.0, "topic_20260812-100936_1234567890abcdef");
        assert!(s.1.contains(':'), "fingerprint 应为 size:mtime_ns");
        assert_eq!(s.2, 17);

        // 幂等：再写一次，行数不变
        ensure_catalog_session(home, &jsonl, &meta).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM catalog_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn skip_when_no_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let meta = json!({"topic_id": "t1", "scope": "project", "workspace_root": "X:\\a"});
        // 没有 cache/session-catalog/v2.sqlite → 静默跳过
        let r = ensure_catalog_session(dir.path(), &dir.path().join("a.jsonl"), &meta);
        assert!(r.is_ok());
    }
}
