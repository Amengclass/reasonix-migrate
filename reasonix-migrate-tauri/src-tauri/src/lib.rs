// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

pub mod core;

use core::common::{list_sessions_groups, list_sessions_home, SessionGroupView};
use core::export::{export, verify, ExportOptions, ExportSummary, VerifySummary};
use core::import::{import, list_zip_workspaces, ImportOptions, ImportSummary};
use core::one::{migrate_one, OneOptions, OneSummary};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct ListEntry {
    pub id: String,
    pub slug: Option<String>,
    pub title: String,
    pub topic_id: Option<String>,
    pub workspace_root: Option<String>,
    pub turns: Option<i64>,
    /// reasonix 实时轮数（display-index.json 的 authored_turns）
    pub authored_turns: Option<i64>,
}

/// home 探测结果：路径 + 来源说明（告诉用户工具从哪拿到的）
#[derive(serde::Serialize)]
pub struct HomeDetect {
    pub home: String,
    pub via: String,
}

/// 探测 Reasonix home：优先 REASONIX_HOME 环境变量，其次常见位置。
#[tauri::command]
fn detect_home() -> Option<HomeDetect> {
    let env = std::env::var("REASONIX_HOME").ok().map(|s| s.trim().to_string());
    if let Some(e) = &env {
        if PathBuf::from(e).join("projects").is_dir() {
            return Some(HomeDetect {
                home: e.clone(),
                via: "环境变量 REASONIX_HOME".to_string(),
            });
        }
    }
    let mut cands: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            cands.push(PathBuf::from(appdata).join("reasonix"));
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = dirs::home_dir() {
            cands.push(home.join("Library/Application Support/reasonix"));
            cands.push(home.join(".config/reasonix"));
        }
    }
    for c in cands {
        if c.is_dir() && c.join("projects").is_dir() {
            return Some(HomeDetect {
                home: c.to_string_lossy().to_string(),
                via: "默认位置（应用数据目录）".to_string(),
            });
        }
    }
    env.map(|e| HomeDetect {
        home: e,
        via: "环境变量 REASONIX_HOME（未校验目录）".to_string(),
    })
}

/// 列会话：home / sessions 目录 / zip 三种源。`force` 为 true 时忽略缓存强制重扫。
#[tauri::command]
async fn list_sessions(
    kind: String,
    source: String,
    force: Option<bool>,
    project: Option<String>,
) -> Result<Vec<ListEntry>, String> {
    let v: Vec<core::common::Session> = match kind.as_str() {
        "home" => core::common::list_sessions_home(
            std::path::Path::new(&source),
            force.unwrap_or(false),
            project.as_deref(),
        ),
        "sessions" => core::common::list_sessions_dir(std::path::Path::new(&source), None),
        "zip" => core::common::list_sessions_zip(std::path::Path::new(&source)),
        _ => return Err(format!("未知源类型: {}", kind)),
    };
    Ok(v.into_iter()
        .map(|s| ListEntry {
            id: s.id,
            slug: s.slug,
            title: s.title,
            topic_id: s.topic_id,
            workspace_root: s.workspace_root,
            turns: s.turns,
            authored_turns: s.authored_turns,
        })
        .collect())
}

/// 列出备份 zip 里出现过的项目路径（导入页「从备份读取项目」）。
#[tauri::command]
fn list_zip_workspaces_cmd(zip_path: String) -> Result<Vec<String>, String> {
    list_zip_workspaces(Path::new(&zip_path))
}

/// 只读项目列表（desktop-projects.json，不扫目录）。点开项目下拉时自动刷新用。
#[tauri::command]
fn list_projects_cmd(home_path: String) -> Vec<core::common::ProjectInfo> {
    core::common::list_projects(std::path::Path::new(&home_path))
}

/// 单会话迁移。
#[tauri::command]
async fn migrate(opt: OneOptions) -> Result<OneSummary, String> {
    migrate_one(&opt)
}

/// 会话分组视图（所见即所得：一组 = Reasonix 左侧一个会话）。
#[tauri::command]
async fn list_sessions_groups_cmd(home_path: String) -> Result<Vec<SessionGroupView>, String> {
    Ok(list_sessions_groups(std::path::Path::new(&home_path)))
}

/// 导出请求（owned，供 Tauri IPC 反序列化；ExportOptions 内部用引用）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub source: String,
    pub output: String,
    pub project_filters: Vec<String>,
    pub session_filters: Vec<String>,
    pub since: Option<String>,
    pub include_secrets: bool,
}

/// 导入请求。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub backup: String,
    pub target: String,
    pub maps: Vec<String>,
    pub overwrite: bool,
    pub verify: bool,
    pub skip_hash_check: bool,
}

/// 导出 zip（带进度事件 export-progress，payload { done, total }）。
#[tauri::command]
async fn do_export(app: tauri::AppHandle, r: ExportRequest) -> Result<ExportSummary, String> {
    let opts = ExportOptions {
        source: Path::new(&r.source),
        output: Path::new(&r.output),
        project_filters: &r.project_filters,
        session_filters: &r.session_filters,
        since: r.since.as_deref(),
        include_secrets: r.include_secrets,
    };
    let mut cb = |done: usize, total: usize| {
        use tauri::Emitter;
        let _ = app.emit(
            "export-progress",
            serde_json::json!({ "done": done, "total": total }),
        );
    };
    export(&opts, &mut cb)
}

/// 校验备份。
#[tauri::command]
async fn do_verify(zip_path: String) -> Result<VerifySummary, String> {
    verify(Path::new(&zip_path))
}

/// 导入。
#[tauri::command]
async fn do_import(r: ImportRequest) -> Result<ImportSummary, String> {
    let opts = ImportOptions {
        backup: Path::new(&r.backup),
        target: Path::new(&r.target),
        maps: &r.maps,
        overwrite: r.overwrite,
        verify: r.verify,
        skip_hash_check: r.skip_hash_check,
    };
    import(&opts)
}

/// 迁移成功后重启桌面端（返回提示文本）。
#[tauri::command]
fn restart_reasonix() -> String {
    core::one::restart_reasonix_app()
}

/// 保存日志文本到文件。
#[tauri::command]
fn save_log_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| format!("写入失败: {e}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            detect_home,
            list_sessions,
            list_projects_cmd,
        list_sessions_groups_cmd,
            migrate,
            do_export,
            do_verify,
            do_import,
            list_zip_workspaces_cmd,
            restart_reasonix,
            save_log_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
