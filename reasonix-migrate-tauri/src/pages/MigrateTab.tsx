import { useEffect, useRef, useState } from "react";
import { Archive, Calendar, ChevronRight, Database, Folder, FolderPlus, FolderSearch, HardDrive, Info, Monitor, Play, RefreshCw, RotateCw, Target } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from "@/components/ui/alert-dialog";
import { PathField } from "@/components/PathField";
import { LogPanel } from "@/components/LogPanel";
import { useRunner } from "@/lib/useRunner";
import {
  detectHome,
  listSessions,
  migrate,
  pickDirectory,
  pickZipFile,
  restartReasonix,
  listProjects,
  HomeDetect,
  ListEntry,
} from "@/lib/api";
import { cn } from "@/lib/utils";

// 模块级缓存：源（kind|path）→ { 会话列表, 时间 }，避免切 tab 重复扫描
const sessionListCache = new Map<string, { v: ListEntry[]; t: number }>();
// 后台静默重扫防重入 + 3s 节流（展开总是刷新，但避免频繁展开反复扫）
let silentScanning = false;
let lastSilentScan = 0;
// refreshSessions 的 projOverride 哨兵：强制全量（忽略当前项目选择，按钮用）
const ALL_PROJECTS = "__all__";

/** 让下拉弹层最大高度 = 触发器底部到窗口底：不超出窗口、不翻转、尽量多显示（超高内部滚动） */
function useDynamicMaxH<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [maxH, setMaxH] = useState(256);
  const measure = () => {
    const el = ref.current;
    if (el) {
      const h = window.innerHeight - el.getBoundingClientRect().bottom - 8;
      setMaxH(Math.max(120, h));
    }
  };
  return { ref, maxH, measure };
}

type SourceKind = "home" | "zip";

function readableDate(sid: string): string {
  const m = sid.match(/^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})/);
  return m ? `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}` : "";
}

function modelOf(sid: string): string {
  const m = sid.match(/^\d{8}-\d{6}\.\d{6,}-(.+)$/);
  return m ? m[1] : "";
}

function projectName(slug: string | null): string {
  if (!slug) return "";
  if (slug.includes("global")) return "Global";
  return slug.split("-").pop() ?? slug;
}

/** 项目显示名：Global 目录特殊处理，其余优先用 workspace_root 最后一段 */
function projectNameOf(entry: { slug: string | null; workspace_root: string | null } | null | undefined): string {
  if (!entry) return "";
  const slug = entry.slug ?? "";
  if (slug.includes("global")) return "Global";
  const ws = entry.workspace_root;
  if (ws) {
    const base = ws.replace(/[\\/]+$/, "").split(/[\\/]/).pop();
    if (base) return base;
  }
  return projectName(entry.slug);
}

export function MigrateTab() {
  const { busy, lines, push, clear, run } = useRunner();
  const [kind, setKind] = useState<SourceKind>("home");
  const [homePath, setHomePath] = useState("");
  const [zipPath, setZipPath] = useState("");
  const [toWorkspace, setToWorkspace] = useState("");
  const [toHome, setToHome] = useState("");
  const [cands, setCands] = useState<ListEntry[]>([]);
  const [projSel, setProjSel] = useState<string>("all");
  const [selIdx, setSelIdx] = useState<string>("");
  const { ref: projTrigRef, maxH: projMaxH, measure: projMeasure } = useDynamicMaxH<HTMLButtonElement>();
  const { ref: sessTrigRef, maxH: sessMaxH, measure: sessMeasure } = useDynamicMaxH<HTMLButtonElement>();
  const [overwrite, setOverwrite] = useState(false);
  const [noVerify, setNoVerify] = useState(false);
  const [dryRun, setDryRun] = useState(false);
  const [restartApp, setRestartApp] = useState(true);
  const [deleteSource, setDeleteSource] = useState(false);
  // 工具探测到的 Reasonix home（含来源说明）
  const [detectedHome, setDetectedHome] = useState<HomeDetect | null>(null);
  // 正在扫描会话列表（按钮 loading）
  const [listing, setListing] = useState(false);

  // 扫描/执行期间：全局鼠标光标变转圈（body 加类 + CSS !important 强制）
  useEffect(() => {
    if (listing || busy) {
      document.body.classList.add("cursor-wait");
    } else {
      document.body.classList.remove("cursor-wait");
    }
  }, [listing, busy]);

  // 自动填充默认路径（工具探测当前生效的 Reasonix home，来源显示在输入框提示里）
  useEffect(() => {
    detectHome().then((h) => {
      if (h) {
        setDetectedHome(h);
        if (!homePath) setHomePath(h.home);
        if (!toHome) setToHome(h.home);
      }
    }).catch(() => undefined);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const currentSource = (): { kind: SourceKind; path: string } => ({
    kind,
    path: kind === "home" ? homePath : zipPath,
  });

  const refreshSessions = async (silent = false, force = false, projOverride?: string) => {
    const { kind: k, path } = currentSource();
    if (!path) {
      if (!silent) push("请先填写源路径", "warn");
      return;
    }
    const cacheKey = `${k}|${path}`;
    // 缓存命中：切 tab 重挂载时秒显示，不重新扫描
    if (!force) {
      const cached = sessionListCache.get(cacheKey);
      if (cached) {
        setCands(cached.v);
        // 缓存命中同样要恢复项目列表（组件重挂载后 projList 为空）
        setProjList(Array.from(new Set(cached.v.map((s) => s.slug).filter((x): x is string => !!x))));
        setSelIdx(cached.v.length > 0 ? "0" : "");
        setProjSel((p) => (p === "all" || cached.v.some((s) => s.slug === p) ? p : "all"));
        // 展开时总是后台静默重扫（3s 节流防频繁展开反复扫），完成后无缝替换
        if (!silentScanning && Date.now() - lastSilentScan > 3000) {
          silentScanning = true;
          lastSilentScan = Date.now();
          refreshSessions(true, true)
            .catch(() => undefined)
            .finally(() => {
              silentScanning = false;
            });
        }
        return;
      }
    }
    try {
      setListing(true);
      // 选具体项目（或指定 projOverride）→ 只扫该项目；「全部项目」→ 全量；ALL_PROJECTS 强制全量
      const proj =
        projOverride === ALL_PROJECTS ? undefined : projOverride ?? (projSel === "all" ? undefined : projSel);
      const list = await listSessions(k, path, force, proj);
      if (proj === undefined) {
        // 全量：替换列表 + 更新缓存 + 更新项目列表
        sessionListCache.set(cacheKey, { v: list, t: Date.now() });
        setCands(list);
        setProjList(Array.from(new Set(list.map((s) => s.slug).filter((x): x is string => !!x))));
        // 用户主动扫描（非静默）才重置选择；后台静默刷新保留当前选中
        if (!silent) setSelIdx(list.length > 0 ? "0" : "");
      } else {
        // 单项目：合并进现有列表（其他项目保留上次数据，不消失）
        setCands((prev) => [...list, ...prev.filter((s) => s.slug !== proj)]);
        if (!silent) setSelIdx(list.length > 0 ? "0" : "");
      }
      // 项目下拉：projOverride（程序触发）不改变当前选择；用户操作时才回落
      if (!projOverride) {
        setProjSel((p) => (p === "all" || list.some((s) => s.slug === p) ? p : "all"));
      }
      if (!silent) {
        const scope =
          proj === undefined
            ? `全部项目（${projList.length || "?"} 个项目）`
            : `项目「${projectName(proj)}」`;
        push(`[列表] 扫描${scope}，找到 ${list.length} 个已注册会话（新的在前）`, "cmd");
      }
    } catch (e) {
      if (!silent) push(`读取失败：${e}`, "err");
    } finally {
      setListing(false);
    }
  };

  // 项目下拉展开时自动刷新项目列表 + 会话数（读 desktop-projects.json，毫秒级，不扫会话）。
  // 检测到某项目会话数变化（新增/删除）→ 只对该项目后台重扫，会话下拉一打开即可见新会话。
  const refreshProjects = async () => {
    if (!homePath.trim()) return;
    try {
      const list = await listProjects(homePath.trim());
      if (list.length > 0) {
        const counts = Object.fromEntries(list.map((p) => [p.slug, p.count]));
        const prev = lastCountsRef.current;
        // 变化判定：首次用当前会话列表 cands 对比；之后只与上次读取的 count 对比——
        // 避免 turns=0 空会话被扫描过滤后 count 永远对不上，导致每次点开下拉都重扫该项目
        const changed =
          prev === null
            ? list.filter((p) => cands.filter((s) => s.slug === p.slug).length !== p.count).map((p) => p.slug)
            : list.filter((p) => prev[p.slug] !== p.count).map((p) => p.slug);
        lastCountsRef.current = counts;
        if (changed.length > 0 && !silentScanning && Date.now() - lastSilentScan > 3000) {
          silentScanning = true;
          lastSilentScan = Date.now();
          push(`[列表] 检测到会话数变化：${changed.map(projectName).join("、")}，后台刷新`, "info");
          try {
            // 逐个单项目后台重扫（串行，避免 listing 状态互相干扰），完成后合并进 cands
            for (const slug of changed) {
              await refreshSessions(true, true, slug).catch(() => undefined);
            }
          } finally {
            silentScanning = false;
          }
        }
        setProjList(list.map((p) => p.slug));
        setProjCounts(counts);
      }
    } catch {
      /* 静默 */
    }
  };

  // 源类型/路径变化时自动刷新（去抖）
  useEffect(() => {
    const t = setTimeout(() => {
      const { path } = currentSource();
      if (path) refreshSessions(true);
    }, 150);
    return () => clearTimeout(t);
  }, [kind, homePath, zipPath]); // eslint-disable-line react-hooks/exhaustive-deps

  // 源变化后上次的 count 基准作废（不同 home 的项目不同），首次重新用 cands 对比
  useEffect(() => {
    lastCountsRef.current = null;
  }, [kind, homePath]);

  // 项目列表（slug，顺序 = 注册表顺序）：全量扫描时更新，单项目扫描保留
  const [projList, setProjList] = useState<string[]>([]);
  // 项目下拉里每个 slug 的会话数（来自注册表 topics，点开即最新；未读到时兜底 cands）
  const [projCounts, setProjCounts] = useState<Record<string, number>>({});
  // 上次读取的项目 count 基准：null = 尚未读过（首次用 cands 对比；之后只与上次对比）
  const lastCountsRef = useRef<Record<string, number> | null>(null);
  const slugs = projList;
  const filtered = projSel === "all" ? cands : cands.filter((s) => s.slug === projSel);
  const selected = filtered[Number(selIdx)];

  const doRun = async () => {
    const { path: src } = currentSource();
    if (!src) throw new Error("请填写源路径");
    if (!selected) throw new Error("请先在会话下拉框里选择一个会话");
    if (!toWorkspace.trim()) throw new Error("请填写目标工作区目录");
    if (!toHome.trim()) throw new Error("请填写目标 REASONIX_HOME");

    push(`迁移会话：${selected.title || "（无标题）"}`, "task");
    const targetName = toWorkspace.trim().replace(/[\\/]+$/, "").split(/[\\/]/).pop() || toWorkspace.trim();
    push(`从「${projectNameOf(selected)}」迁移 1 个会话 → 目标「${targetName}」`, "cmd", { showTs: true });
    const opt = {
      fromHome: kind === "home" ? src : undefined,
      fromZip: kind === "zip" ? src : undefined,
      session: selected.id,
      sessionSlug: selected.slug ?? undefined,
      toWorkspace: toWorkspace.trim(),
      toHome: toHome.trim(),
      overwrite,
      newTopic: true, // 迁移总是生成新主题（默认行为），避免与原工作区会话联动删除
      noVerify,
      dryRun,
      restartApp,
      deleteSource,
    };
    const sum = await migrate(opt);
    if (dryRun) {
      // 预览：完整预演信息
      if (sum.copied.length > 0) push(sum.copied[0], "info", { noIcon: true, indent: 1 });
      push(`目标：${sum.targetSessions}`, "info", { noIcon: true, indent: 1 });
      if (sum.metaChanges.length > 0) {
        for (const c of sum.metaChanges) push(`将修正 ${c}`, "info", { noIcon: true, indent: 1 });
      }
      push(
        sum.conflict ? "目标已存在同名会话（勾选「覆盖同名」可覆盖）" : "无冲突，可直接迁移",
        sum.conflict ? "warn" : "ok",
        { indent: 1 }
      );
      push("未写入任何文件（仅预览）", "ok", { showTs: true });
      return [];
    }
    // 正式迁移
    if (sum.copied.length > 0) {
      push(`复制 ${sum.copied.length} 个文件/目录`, "info", { noIcon: true, indent: 1 });
      const shown = sum.copied.slice(0, 8);
      shown.forEach((c, i) => {
        const last = i === shown.length - 1 && sum.copied.length <= 8;
        push(`${last ? "└" : "├"} ${c}`, "info", { noIcon: true, indent: 2 });
      });
      if (sum.copied.length > 8) push(`└ +${sum.copied.length - 8} 更多`, "info", { noIcon: true, indent: 2 });
    }
    if (sum.metaChanges.length > 0) {
      sum.metaChanges.forEach((c, i) => {
        const last = i === sum.metaChanges.length - 1;
        push(`${last ? "└" : "├"} 修正 ${c}`, "info", { noIcon: true, indent: 1 });
      });
    }
    if (sum.skipped.length > 0) push(`${sum.skipped.length} 个源文件被并发清理`, "warn", { indent: 1 });
    sum.warnings.forEach((w) => push(`⚠ ${w}`, "warn", { showTs: true, indent: 1 }));
    push(`迁移成功 → ${sum.targetSessions}`, "ok", { showTs: true });
    if (sum.deletedSource.length > 0) {
      push(`已删除源会话 ${sum.deletedSource.length} 个文件（原工作区不再保留）`, "warn", { indent: 1 });
    }
    if (restartApp) {
      const msg = await restartReasonix();
      push(msg, msg.includes("失败") ? "warn" : "ok", { showTs: true });
    } else {
      push("启动/刷新 Reasonix 桌面端后可见（未勾选自动重启）", "info");
    }
    // 迁移成功后刷新列表：目标与源在同一 home → 扫描目标项目（快）并合并进列表
    const targetSlug = sum.targetSessions.split(/[\\/]/).filter(Boolean).slice(-2, -1)[0];
    if (toHome === homePath && targetSlug) {
      await refreshSessions(true, true, targetSlug);
      push(`[列表] 已刷新项目「${projectName(targetSlug)}」`, "info");
    }
    return [];
  };

  // 迁移确认弹窗的选项徽章（只显示勾选的，用自然语言说明后果）
  const optionBadges: { on: boolean; label: string; cls: string }[] = [
    { on: dryRun, label: "仅预览 · 不写入任何文件", cls: "border-blue-400/40 bg-blue-500/10 text-blue-600" },
    { on: overwrite, label: "同名会话将覆盖", cls: "border-amber-400/40 bg-amber-500/10 text-amber-600" },
    { on: noVerify, label: "跳过完整性校验", cls: "border-amber-400/40 bg-amber-500/10 text-amber-600" },
    { on: restartApp, label: "完成后自动重启 Reasonix", cls: "border-blue-400/40 bg-blue-500/10 text-blue-600" },
    { on: deleteSource, label: "迁移后删除源会话（⚠ 不可恢复）", cls: "border-red-400/40 bg-red-500/10 text-red-600" },
  ];
  const activeBadges = optionBadges.filter((o) => o.on);
  const srcPath = currentSource().path;

  return (
    <div className="flex h-full flex-col gap-3 overflow-y-auto px-3">
      {/* 顶部引导：本页适用场景 */}
      <div className="flex items-start gap-2 rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
        <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-blue-500" />
        <span>
          本页 = <b>本机内迁移</b>（同一台电脑上，从一个 Reasonix 数据目录搬到另一个，例如旧数据目录 → 当前 REASONIX_HOME）。
          <b>跨电脑</b>请用「导出」→ 拷 zip →「导入」。
        </span>
      </div>
      {/* 源 */}
      <Card className={listing ? "pointer-events-none cursor-progress" : ""}>
        <CardHeader className="pb-2">
          <CardTitle className="flex items-center gap-1.5 text-sm">
            <FolderSearch className="h-4 w-4 text-blue-500" />
            源（会话现在在哪）
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-2">
            <div className="grid grid-cols-2 gap-2">
              {(
                [
                  ["home", "本机 home", "当前 REASONIX_HOME 数据目录", Monitor],
                  ["zip", "备份 zip", "之前导出的备份文件", Archive],
                ] as [SourceKind, string, string, typeof Monitor][]
              ).map(([k, label, desc, Icon]) => (
                <label
                  key={k}
                  className={cn(
                    "flex cursor-pointer items-start gap-2 rounded-md border px-3 py-2 transition-colors",
                    kind === k ? "border-blue-400/60 bg-blue-500/5" : "border-border hover:bg-muted/40"
                  )}
                >
                  <input
                    type="radio"
                    name="src-kind"
                    checked={kind === k}
                    onChange={() => {
                      setKind(k);
                      // 切换源类型时清空旧列表，避免残留上一源的扫描结果
                      setCands([]);
                      setSelIdx("");
                      setProjSel("all");
                    }}
                    className="mt-0.5 h-3.5 w-3.5"
                  />
                  <div className="min-w-0">
                    <div className="flex items-center gap-1.5 text-sm font-medium">
                      <Icon className="h-3.5 w-3.5 text-muted-foreground" />
                      {label}
                    </div>
                    <div className="truncate text-xs text-muted-foreground">{desc}</div>
                  </div>
                </label>
              ))}
            </div>
            {kind === "home" && (
              <PathField
                label="源数据目录："
                value={homePath}
                onChange={setHomePath}
                onBrowse={() => pickDirectory("选择会话数据目录")}
                hint={`会话数据就存在这个目录里（已自动识别：${detectedHome ? detectedHome.home : "未识别，请手动选择"}）`}
              />
            )}
            {kind === "zip" && (
              <PathField
                label="源 zip："
                kind="file"
                value={zipPath}
                onChange={setZipPath}
                onBrowse={() => pickZipFile("选择备份 zip")}
              />
            )}
            {/* 会话浏览区：bordered 分组，标题右侧放「刷新列表」（同导出页） */}
            <div className="space-y-2 rounded-md border bg-muted/30 p-2.5">
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs font-medium text-muted-foreground">选择会话</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-6 gap-1 px-2 text-xs text-muted-foreground hover:text-foreground"
                  disabled={busy || listing}
                  onClick={() => refreshSessions(false, true, ALL_PROJECTS)}
                >
                  <RefreshCw className={cn("h-3 w-3", (busy || listing) && "animate-spin")} />
                  {listing ? "刷新中…" : "刷新列表"}
                </Button>
              </div>
              <div className="space-y-1.5">
              {kind === "home" && slugs.length > 0 && (
                <div className="flex items-center gap-2">
                  <Label className="w-20 shrink-0 text-xs text-muted-foreground">项目</Label>
                <Select
                value={projSel}
                onValueChange={(v) => {
                  setProjSel(v);
                  // 选中具体项目 → 立即单项目静默刷新，会话下拉即刻有该项目最新会话
                  if (v !== "all" && !silentScanning && Date.now() - lastSilentScan > 3000) {
                    silentScanning = true;
                    lastSilentScan = Date.now();
                    refreshSessions(true, true, v)
                      .catch(() => undefined)
                      .finally(() => {
                        silentScanning = false;
                      });
                  }
                }}
                onOpenChange={(o) => {
                  if (o) {
                    projMeasure();
                    refreshProjects();
                  }
                }}
              >
                  <SelectTrigger ref={projTrigRef} className="h-8 flex-1 text-xs">
                    <SelectValue placeholder="全部项目" />
                  </SelectTrigger>
                  <SelectContent
                    side="bottom"
                    avoidCollisions={false}
                    style={{ maxHeight: projMaxH }}
                    className="overflow-y-auto"
                  >
                    <SelectItem value="all" className="text-xs">
                      全部项目（{Object.values(projCounts).reduce((a, b) => a + b, 0) || cands.length}）
                    </SelectItem>
                    {slugs.map((sl) => (
                      <SelectItem key={sl} value={sl} className="text-xs">
                        <span className="flex items-center gap-1.5">
                          <Folder className="h-3 w-3 shrink-0 text-blue-600" />
                          <span className="truncate">
                            {projectName(sl)}（{projCounts[sl] ?? cands.filter((s) => s.slug === sl).length}）
                          </span>
                        </span>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            )}
              <div className="flex items-center gap-2">
                <Label className="w-20 shrink-0 text-xs text-muted-foreground">会话</Label>
                <Select
                value={selIdx}
                onValueChange={setSelIdx}
                disabled={filtered.length === 0}
                onOpenChange={(o) => o && sessMeasure()}
              >
                <SelectTrigger ref={sessTrigRef} className="h-8 flex-1 text-xs">
                  {selected ? (
                    <span className="flex min-w-0 items-center gap-1.5">
                      <span className="truncate">
                        {selected.title
                          ? `「${selected.title.slice(0, 30)}」`
                          : `（无标题）${modelOf(selected.id) || selected.id.slice(0, 20)}`}
                      </span>
                      {selected.slug && (
                        <span className="inline-flex shrink-0 items-center gap-0.5 rounded bg-blue-500/10 px-1 py-px text-[10px] font-medium text-blue-600">
                          <Folder className="h-2.5 w-2.5" />
                          {projectNameOf(selected)}
                        </span>
                      )}
                    </span>
                  ) : (
                    <SelectValue placeholder="选择会话（先填源路径）" />
                  )}
                </SelectTrigger>
                <SelectContent
                  side="bottom"
                  avoidCollisions={false}
                  style={{ maxHeight: sessMaxH }}
                  className="overflow-y-auto"
                >
                  {filtered.map((s, i) => {
                    const date = readableDate(s.id);
                    const turns = s.authored_turns ?? s.turns ? `${s.authored_turns ?? s.turns} 轮` : "";
                    const meta = [date, turns].filter(Boolean).join(" · ");
                    return (
                      <SelectItem key={s.id} value={String(i)} className="text-xs">
                        <div className="flex flex-col gap-0.5 py-0.5">
                          <span className="truncate">
                            {s.title
                              ? `「${s.title.slice(0, 36)}」`
                              : `（无标题）${modelOf(s.id) || s.id.slice(0, 24)}`}
                          </span>
                          <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                            {s.slug && (
                              <span className="inline-flex items-center gap-0.5 rounded bg-blue-500/10 px-1.5 py-px font-medium text-blue-600">
                                <Folder className="h-3 w-3" />
                                {projectNameOf(s)}
                              </span>
                            )}
                            {meta && <span>{meta}</span>}
                          </span>
                        </div>
                      </SelectItem>
                    );
                  })}
                </SelectContent>
              </Select>
            </div>
              </div>
            </div>
          </CardContent>
        </Card>

        {/* 目标 */}
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="flex items-center gap-1.5 text-sm">
              <Target className="h-4 w-4 text-emerald-500" />
              目标（放进哪个工作目录）
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-2">
            <PathField
              label="目标数据目录："
              value={toHome}
              onChange={setToHome}
              onBrowse={() => pickDirectory("选择会话数据目录")}
              hint={`通常与源数据目录相同（本机内搬会话）；换电脑/换盘时才填不同目录。已自动识别：${detectedHome ? detectedHome.home : "未识别，请手动选择"}`}
            />
            <PathField
              label="工作区目录："
              value={toWorkspace}
              onChange={setToWorkspace}
              onBrowse={() => pickDirectory("选择目标工作区目录")}
              hint="会话要归到哪个工作区？填工作区目录路径，自动转到 projects/下对应目录"
            />
            <Collapsible>
              <CollapsibleTrigger asChild>
                <Button variant="ghost" size="sm" className="group h-7 gap-1 px-1 text-xs text-muted-foreground">
                  <ChevronRight className="h-3.5 w-3.5 transition-transform group-data-[state=open]:rotate-90" />
                  高级选项
                </Button>
              </CollapsibleTrigger>
              <CollapsibleContent className="grid grid-cols-2 gap-x-4 gap-y-1.5 pt-1">
                <OptionRow checked={overwrite} onChange={setOverwrite} label="覆盖同名" warn />
                <OptionRow checked={noVerify} onChange={setNoVerify} label="跳过校验" warn />
                <OptionRow checked={dryRun} onChange={setDryRun} label="仅预览（不写入）" />
                <OptionRow checked={restartApp} onChange={setRestartApp} label="完成后重启桌面端" />
                <OptionRow checked={deleteSource} onChange={setDeleteSource} label="迁移后删除源会话" warn />
                <p className="col-span-2 text-xs text-muted-foreground">
                  「覆盖同名」= 目标已有同名会话时用源覆盖它；「仅预览」只打印不写入；「迁移后删除源会话」= 真正搬走，源工作区不再保留（有风险，建议先确认目标正常）。
                  迁移默认在目标生成<b>新主题</b>，两个工作区互不影响（删除不联动）。
                </p>
              </CollapsibleContent>
            </Collapsible>
          </CardContent>
        </Card>

      {/* 操作 + 日志 */}
      <div className={"flex items-center justify-end gap-2" + (listing ? " pointer-events-none cursor-progress" : "")}>
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button className="gap-1.5 disabled:cursor-progress" disabled={busy}>
              <Play className="h-4 w-4" />
              {busy ? "执行中…" : "开始执行"}
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent className="max-w-lg">
            <AlertDialogHeader>
              <AlertDialogTitle>确认迁移会话</AlertDialogTitle>
              <AlertDialogDescription asChild>
                <div className="space-y-3">
                  {/* 会话摘要：项目徽章 + 标题 + 轮数/日期 */}
                  {selected && (
                    <div className="rounded-md border bg-muted/40 px-3 py-2.5">
                      <div className="flex items-center gap-1.5 text-sm font-medium text-foreground">
                        {selected.slug && (
                          <span className="inline-flex shrink-0 items-center gap-0.5 rounded bg-blue-500/10 px-1.5 py-px text-[11px] font-medium text-blue-600">
                            <Folder className="h-3 w-3" />
                            {projectNameOf(selected)}
                          </span>
                        )}
                        <span className="truncate">
                          {selected.title ? `「${selected.title}」` : `（无标题）${modelOf(selected.id) || selected.id}`}
                        </span>
                      </div>
                      <div className="mt-1 flex items-center gap-3 text-xs text-muted-foreground">
                        <span className="inline-flex items-center gap-1">
                          <Calendar className="h-3 w-3" />
                          {readableDate(selected.id)}
                        </span>
                        {selected.authored_turns ?? selected.turns ? (
                          <span className="inline-flex items-center gap-1">
                            <RotateCw className="h-3 w-3" />
                            {selected.authored_turns ?? selected.turns} 轮
                          </span>
                        ) : null}
                      </div>
                    </div>
                  )}

                  {/* 路径：源/目标分行，带图标，等宽可核对 */}
                  <div className="space-y-1.5 rounded-md border bg-muted/40 px-3 py-2.5 text-xs">
                    <div className="flex items-center gap-2">
                      <HardDrive className="h-3.5 w-3.5 shrink-0 text-blue-500" />
                      <span className="w-20 shrink-0 text-muted-foreground">源 home</span>
                      <span className="min-w-0 break-all font-mono text-foreground">{srcPath || "(未填)"}</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <Database className="h-3.5 w-3.5 shrink-0 text-emerald-500" />
                      <span className="w-20 shrink-0 text-muted-foreground">目标 home</span>
                      <span className="min-w-0 break-all font-mono text-foreground">{toHome || "(未填)"}</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <FolderPlus className="h-3.5 w-3.5 shrink-0 text-amber-500" />
                      <span className="w-20 shrink-0 text-muted-foreground">目标工作区</span>
                      <span className="min-w-0 break-all font-medium text-foreground">{toWorkspace || "(未填)"}</span>
                    </div>
                    <div className="border-t pt-1.5 leading-relaxed text-muted-foreground">
                      {deleteSource ? "迁移成功后源会话会被删除。" : "本机内复制，源会话默认保留。"}
                      将作为新会话导入，与源工作区的会话相互独立。
                    </div>
                  </div>

                  {/* 选项徽章 */}
                  {activeBadges.length > 0 && (
                    <div className="flex flex-wrap gap-1.5">
                      {activeBadges.map((b) => (
                        <span key={b.label} className={`rounded-full border px-2 py-0.5 text-[11px] font-medium ${b.cls}`}>
                          {b.label}
                        </span>
                      ))}
                    </div>
                  )}
                </div>
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>取消</AlertDialogCancel>
              <AlertDialogAction
                onClick={() => run(doRun)}
                className={deleteSource ? "bg-red-600 text-white hover:bg-red-700" : ""}
              >
                执行迁移
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>

      <LogPanel lines={lines} onClear={clear} />
    </div>
  );
}

function OptionRow({
  checked,
  onChange,
  label,
  warn,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  warn?: boolean;
}) {
  return (
    <Label className="flex cursor-pointer items-center gap-2 text-sm">
      <Checkbox checked={checked} onCheckedChange={(v) => onChange(v === true)} />
      {label}
      {warn && <span className="text-amber-600">⚠</span>}
    </Label>
  );
}
