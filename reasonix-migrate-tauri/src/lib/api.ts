import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";

// ---- 类型（与 Rust 侧 Serialize 结构对应）----

export interface ListEntry {
  id: string;
  slug: string | null;
  title: string;
  topic_id: string | null;
  workspace_root: string | null;
  turns: number | null;
  /** reasonix 实时轮数（display-index.json 的 authored_turns），比 turns 可靠 */
  authored_turns: number | null;
}

/** 会话分组视图（一组 = Reasonix 左侧一个会话：活跃分支 + 历史副本）。 */
export interface SessionGroupView {
  topicId: string;
  title: string;
  active: ListEntry;
  branches: ListEntry[];
  branchCount: number;
  totalBytes: number;
}

/** 按项目分组列出会话（含全部 recovery 分支），组内 active 为活跃分支。 */
export async function listSessionsGroups(homePath: string): Promise<SessionGroupView[]> {
  return invoke("list_sessions_groups_cmd", { homePath });
}

export interface OneOptions {
  fromHome?: string;
  fromSessions?: string;
  fromZip?: string;
  session?: string;
  sessionSlug?: string;
  toWorkspace?: string;
  toHome?: string;
  list?: boolean;
  newTopic?: boolean;
  overwrite?: boolean;
  noVerify?: boolean;
  dryRun?: boolean;
  restartApp?: boolean;
}

export interface OneSummary {
  copied: string[];
  skipped: string[];
  metaChanges: string[];
  conflict: boolean;
  sessionId: string;
  targetSessions: string;
  deletedSource: string[];
  warnings: string[];
}

export interface ExportSummary {
  session_count: number;
  file_count: number;
  dir_count: number;
  total_bytes: number;
  warnings: string[];
  output: string;
  source: string;
}

export interface VerifySummary {
  file_count: number;
  session_count: number;
  exported_at: string | null;
  source_home: string | null;
}

export interface ImportSummary {
  imported_sessions: number;
  ok_files: number;
  skipped_ids: string[];
  skipped_files: number;
  conflict_files: number;
  unmatched: [string, string, string][];
  errors: string[];
  target: string;
}

// ---- 命令封装 ----

/** home 探测结果：路径 + 来源说明（告诉用户工具从哪拿到的） */
export interface HomeDetect {
  home: string;
  via: string;
}

export async function detectHome(): Promise<HomeDetect | null> {
  return invoke<HomeDetect | null>("detect_home");
}

/** 项目列表条目：slug + 该项目的已注册会话数（来自 desktop-projects.json topics，毫秒级）。 */
export interface ProjectInfo {
  slug: string;
  count: number;
}

/** 只读项目 slug + 会话数（desktop-projects.json，不扫目录）。点开项目下拉时自动刷新用。 */
export async function listProjects(home: string): Promise<ProjectInfo[]> {
  return invoke<ProjectInfo[]>("list_projects_cmd", { homePath: home });
}

export async function listSessions(
  kind: "home" | "sessions" | "zip",
  source: string,
  force = false,
  project?: string
): Promise<ListEntry[]> {
  return invoke<ListEntry[]>("list_sessions", { kind, source, force, project });
}

export async function migrate(opt: OneOptions): Promise<OneSummary> {
  return invoke<OneSummary>("migrate", { opt });
}

export async function doExport(
  req: {
    source: string;
    output: string;
    projectFilters: string[];
    sessionFilters: string[];
    since: string | null;
    includeSecrets: boolean;
  },
  onProgress?: (done: number, total: number) => void
): Promise<ExportSummary> {
  const un = onProgress
    ? await listen<{ done: number; total: number }>("export-progress", (e) =>
        onProgress(e.payload.done, e.payload.total)
      )
    : null;
  try {
    return await invoke<ExportSummary>("do_export", { r: req });
  } finally {
    un?.();
  }
}

export async function doVerify(zipPath: string): Promise<VerifySummary> {
  return invoke<VerifySummary>("do_verify", { zipPath });
}

export async function doImport(req: {
  backup: string;
  target: string;
  maps: string[];
  overwrite: boolean;
  verify: boolean;
  skipHashCheck: boolean;
}): Promise<ImportSummary> {
  return invoke<ImportSummary>("do_import", { r: req });
}

/** 列出备份 zip 里出现过的项目路径（导入页「从备份读取项目」）。 */
export async function listZipWorkspaces(zipPath: string): Promise<string[]> {
  return invoke<string[]>("list_zip_workspaces_cmd", { zipPath });
}

export async function restartReasonix(): Promise<string> {
  return invoke<string>("restart_reasonix");
}

// ---- 路径选择 ----

export async function pickDirectory(title: string): Promise<string | null> {
  return open({ directory: true, multiple: false, title });
}

export async function pickZipFile(title: string): Promise<string | null> {
  return open({
    multiple: false,
    title,
    filters: [{ name: "zip 备份", extensions: ["zip"] }],
  });
}

export async function pickSavePath(
  title: string,
  defaultName: string
): Promise<string | null> {
  return save({
    title,
    defaultPath: defaultName,
    filters: [{ name: "zip 备份", extensions: ["zip"] }],
  });
}
