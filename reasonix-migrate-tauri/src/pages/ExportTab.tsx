import { useEffect, useState } from "react";
import { Archive, FileArchive, Folder, HardDrive, Play, RefreshCw, X } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { ScrollArea } from "@/components/ui/scroll-area";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from "@/components/ui/alert-dialog";
import { PathField } from "@/components/PathField";
import { LogPanel } from "@/components/LogPanel";
import { useRunner } from "@/lib/useRunner";
import { detectHome, doExport, listSessions, pickDirectory, pickSavePath, ListEntry } from "@/lib/api";

function readableDate(sid: string): string {
  const m = sid.match(/^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})/);
  return m ? `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}` : "";
}

function projectName(slug: string | null): string {
  if (!slug) return "";
  if (slug.includes("global")) return "Global";
  return slug.split("-").pop() ?? slug;
}

function modelOf(sid: string): string {
  const m = sid.match(/^\d{8}-\d{6}\.\d{6,}-(.+)$/);
  return m ? m[1] : "";
}

/** 导出页：把 REASONIX_HOME 打包成 zip（可只勾选部分项目/会话）。 */

// 模块级缓存：源 home → { 会话列表, 缓存时间 }；点击选项目/选会话秒开旧数据，后台刷新缓存
const exportScanCache = new Map<string, { v: ListEntry[]; t: number }>();
// 后台刷新缓存防重入
let bgRefreshing = false;

export function ExportTab() {
  const { busy, lines, push, clear, run } = useRunner();
  const [src, setSrc] = useState("");
  const [out, setOut] = useState("");
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [project, setProject] = useState("");
  const [session, setSession] = useState("");
  const [since, setSince] = useState("");

  // 「选项目…」/「选会话…」弹窗
  const [projPicker, setProjPicker] = useState<Picker | null>(null);
  const [sessPicker, setSessPicker] = useState<Picker | null>(null);
  // 「刷新列表」进行中
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    detectHome().then((h) => h && !src && setSrc(h.home)).catch(() => undefined);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // 带缓存读取源 home 会话列表：有缓存即秒开（不阻塞弹窗），缓存超 3s 后台刷新；force 强制等待
  const loadSessionsCached = async (force = false) => {
    const key = src.trim();
    const cached = exportScanCache.get(key);
    if (!force && cached) {
      // 秒开旧数据；缓存超 3s → 后台刷新缓存（下次打开即最新，本次不等待）
      if (Date.now() - cached.t > 3000 && !bgRefreshing) {
        bgRefreshing = true;
        listSessions("home", key, false) // 非 force → 走后端 30s 缓存保护，低频真扫
          .then((fresh) => {
            exportScanCache.set(key, { v: fresh, t: Date.now() });
          })
          .catch(() => undefined)
          .finally(() => {
            bgRefreshing = false;
          });
      }
      return cached.v;
    }
    const all = await listSessions("home", key, force);
    exportScanCache.set(key, { v: all, t: Date.now() });
    return all;
  };

  const openProjectPicker = async () => {
    if (!src.trim()) throw new Error("请先填写源 home");
    const all = await loadSessionsCached();
    const slugs = Array.from(new Set(all.map((s) => s.slug).filter((x): x is string => !!x)));
    setProjPicker({
      rows: slugs.map((sl) => ({
        value: sl,
        label: projectName(sl),
        sub: `${all.filter((s) => s.slug === sl).length} 个会话`,
      })),
      sel: new Set(project.trim() ? project.trim().split(/[,\s]+/).filter(Boolean) : []),
    });
  };

  const openSessionPicker = async () => {
    if (!src.trim()) throw new Error("请先填写源 home");
    const all = await loadSessionsCached();
    // 按已选项目过滤：选了项目只显示该项目的会话，没选则显示全部
    const selectedProjects = project.trim() ? project.trim().split(/[,\s]+/).filter(Boolean) : [];
    const filtered = selectedProjects.length > 0
      ? all.filter((s) => s.slug && selectedProjects.includes(s.slug))
      : all;
    setSessPicker({
      rows: filtered.map((s) => ({
        value: s.id,
        label: s.title ? `「${s.title.slice(0, 36)}」` : s.id,
        sub: `${readableDate(s.id)} · ${projectName(s.slug)}`,
        session: s,
      })),
      sel: new Set(session.trim() ? session.trim().split(/[,\s]+/).filter(Boolean) : []),
    });
  };

  const doRun = async () => {
    if (!src.trim()) throw new Error("请填写源 home");
    if (!out.trim()) throw new Error("请填写输出 zip 路径");
    push("导出备份", "task");
    push(`从${src.trim()}导出到 ${out.trim()}`, "cmd", { showTs: true });
    const s = await doExport(
      {
        source: src.trim(),
        output: out.trim(),
        projectFilters: project.trim() ? project.trim().split(/[,\s]+/).filter(Boolean) : [],
        sessionFilters: session.trim() ? session.trim().split(/[,\s]+/).filter(Boolean) : [],
        since: since.trim() || null,
        includeSecrets,
      },
      (done, total) => {
        // 进度节流：每 100 个文件或最后一批
        if (done % 100 === 0 || done === total) {
          push(`正在打包 ${done}/${total} 个文件（${Math.round((done / Math.max(1, total)) * 100)}%）`, "info", {
            noIcon: true,
            indent: 1,
          });
        }
      }
    );
    push(`导出 ${s.session_count} 个会话 / ${s.file_count} 文件 / ${s.dir_count} 目录 / ${s.total_bytes} 字节`, "ok", { showTs: true });
    push(`输出：${s.output}`, "info", { noIcon: true, indent: 1 });
    if (s.warnings.length > 0) {
      push(`${s.warnings.length} 个文件被并发改动而跳过（建议退出桌面端后重导）`, "warn");
      for (const w of s.warnings.slice(0, 10)) push(w, "warn", { noIcon: true, indent: 1 });
    }
    if (includeSecrets) push("已包含 .env 凭据，请妥善保管 zip", "warn");
    return [];
  };

  return (
    <div className="flex h-full flex-col gap-3 overflow-y-auto px-3">
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="flex items-center gap-1.5 text-sm">
            <Archive className="h-4 w-4 text-blue-500" />
            把 Reasonix 数据打包成 zip（换电脑 / 定期备份）
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-2">
          <PathField
            label="源 home："
            value={src}
            onChange={setSrc}
            onBrowse={() => pickDirectory("选择源 REASONIX_HOME")}
            hint="已自动识别，一般不用改"
          />
          <PathField
            label="输出 zip："
            kind="file"
            value={out}
            onChange={setOut}
            onBrowse={() => pickSavePath("保存备份 zip 到", "reasonix-backup.zip")}
            hint="备份保存位置，例如 D:\backup\reasonix.zip"
          />
          {/* 过滤区：bordered 分组，标题右侧放「刷新列表」 */}
          <div className="space-y-2 rounded-md border bg-muted/30 p-2.5">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-medium text-muted-foreground">导出范围（可选，默认全部）</span>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-6 gap-1 px-2 text-xs text-muted-foreground hover:text-foreground"
                onClick={async () => {
                  setRefreshing(true);
                  try {
                    await loadSessionsCached(true);
                    push(`[列表] 已强制刷新（${exportScanCache.get(src.trim())?.v.length ?? 0} 个会话）`, "info");
                  } catch (e) {
                    push(`刷新失败：${e}`, "err");
                  } finally {
                    setRefreshing(false);
                  }
                }}
                disabled={busy || !src.trim() || refreshing}
              >
                <RefreshCw className={`h-3 w-3 ${refreshing ? "animate-spin" : ""}`} />
                {refreshing ? "刷新中…" : "刷新列表"}
              </Button>
            </div>
            <div className="space-y-1.5">
              <div className="flex items-center gap-2">
                <Label className="w-20 shrink-0 text-xs text-muted-foreground">项目</Label>
                {project.trim() ? (
                  <>
                    <div className="h-7 flex-1 overflow-hidden text-ellipsis whitespace-nowrap rounded-md border bg-muted/30 px-2 py-1 text-xs">
                      {project.trim().split(/[,\s]+/).filter(Boolean).map(projectName).join("、")}
                    </div>
                    <Button type="button" variant="ghost" size="sm" className="h-7 w-7 shrink-0 px-0" onClick={() => { setProject(""); setSession(""); }}>
                      <X className="h-3 w-3" />
                    </Button>
                  </>
                ) : (
                  <Input
                    value={project}
                    onChange={(e) => setProject(e.target.value)}
                    placeholder="全部（可多选）"
                    className="h-7 flex-1 text-xs"
                  />
                )}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="shrink-0"
                  onClick={() => openProjectPicker().catch((e) => push(String(e), "err"))}
                  disabled={busy || !src.trim()}
                >
                  选项目…
                </Button>
              </div>
              <div className="flex items-center gap-2">
                <Label className="w-20 shrink-0 text-xs text-muted-foreground">会话</Label>
                {session.trim() ? (
                  <>
                    <div className="h-7 flex-1 overflow-hidden text-ellipsis whitespace-nowrap rounded-md border bg-muted/30 px-2 py-1 text-xs">
                      {session.trim().split(/[,\s]+/).filter(Boolean).length} 个会话已选
                    </div>
                    <Button type="button" variant="ghost" size="sm" className="h-7 w-7 shrink-0 px-0" onClick={() => setSession("")}>
                      <X className="h-3 w-3" />
                    </Button>
                  </>
                ) : (
                  <Input
                    value={session}
                    onChange={(e) => setSession(e.target.value)}
                    placeholder="全部（可多选）"
                    className="h-7 flex-1 text-xs"
                  />
                )}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="shrink-0"
                  onClick={() => openSessionPicker().catch((e) => push(String(e), "err"))}
                  disabled={busy || !src.trim()}
                >
                  选会话…
                </Button>
              </div>
              <div className="flex items-center gap-2">
                <Label className="w-20 shrink-0 text-xs text-muted-foreground">起始日期</Label>
                <Input type="date" value={since} onChange={(e) => setSince(e.target.value)} className="h-7 flex-1 text-xs" />
                <span className="w-14 shrink-0 text-right text-xs text-muted-foreground">之后</span>
              </div>
              <Label className="flex cursor-pointer items-center gap-2 pt-0.5 text-xs text-muted-foreground">
                <Checkbox checked={includeSecrets} onCheckedChange={(v) => setIncludeSecrets(v === true)} />
                包含 .env 凭据（默认排除，勾选请妥善保管 zip）
              </Label>
            </div>
          </div>
          <p className="text-xs text-muted-foreground">过滤全部留空 = 导出全部。导出前建议先退出 Reasonix 桌面端。</p>
        </CardContent>
      </Card>

      <PickerDialog
        title="选择要导出的项目（可多选）"
        picker={projPicker}
        setPicker={setProjPicker}
        onConfirm={() => {
          if (projPicker) setProject(Array.from(projPicker.sel).join(", "));
          setProjPicker(null);
        }}
      />
      <PickerDialog
        title="选择要导出的会话（可多选）"
        picker={sessPicker}
        setPicker={setSessPicker}
        onConfirm={() => {
          if (sessPicker) setSession(Array.from(sessPicker.sel).join(", "));
          setSessPicker(null);
        }}
      />

      <div className="flex items-center justify-end">
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button className="gap-1.5" disabled={busy}>
              <Play className="h-4 w-4" />
              {busy ? "执行中…" : "开始执行"}
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent className="max-w-md">
            <AlertDialogHeader>
              <AlertDialogTitle>确认导出？</AlertDialogTitle>
              <AlertDialogDescription asChild>
                <div className="space-y-3">
                  <div className="space-y-1.5 rounded-md border bg-muted/40 px-3 py-2.5 text-xs">
                    <div className="flex items-center gap-2">
                      <HardDrive className="h-3.5 w-3.5 shrink-0 text-blue-500" />
                      <span className="w-20 shrink-0 text-muted-foreground">源 home</span>
                      <span className="min-w-0 break-all font-mono text-foreground">{src || "(未填)"}</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <FileArchive className="h-3.5 w-3.5 shrink-0 text-emerald-500" />
                      <span className="w-20 shrink-0 text-muted-foreground">输出 zip</span>
                      <span className="min-w-0 break-all font-mono text-foreground">{out || "(未填)"}</span>
                    </div>
                  </div>
                  {project || session || includeSecrets ? (
                    <div className="flex flex-wrap gap-1.5">
                      {project && (
                        <span className="rounded-full border border-blue-400/40 bg-blue-500/10 px-2 py-0.5 text-[11px] font-medium text-blue-600">项目：{project}</span>
                      )}
                      {session && (
                        <span className="rounded-full border border-blue-400/40 bg-blue-500/10 px-2 py-0.5 text-[11px] font-medium text-blue-600">会话：{session}</span>
                      )}
                      {includeSecrets && (
                        <span className="rounded-full border border-amber-400/40 bg-amber-500/10 px-2 py-0.5 text-[11px] font-medium text-amber-600">包含 .env 凭据</span>
                      )}
                    </div>
                  ) : (
                    <p className="text-xs text-muted-foreground">无过滤，导出全部会话。</p>
                  )}
                </div>
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>取消</AlertDialogCancel>
              <AlertDialogAction onClick={() => run(doRun)}>执行</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>

      <LogPanel lines={lines} onClear={clear} />
    </div>
  );
}

interface PickerRow {
  value: string;
  label: string;
  sub?: string;
  session?: ListEntry;
}

interface Picker {
  rows: PickerRow[];
  sel: Set<string>;
}

/** 「选项目…」/「选会话…」多选弹窗。 */
function PickerDialog({
  title,
  picker,
  setPicker,
  onConfirm,
}: {
  title: string;
  picker: Picker | null;
  setPicker: React.Dispatch<React.SetStateAction<Picker | null>>;
  onConfirm: () => void;
}) {
  const toggle = (v: string, on: boolean) =>
    setPicker((p) => (p ? { ...p, sel: new Set(on ? [...p.sel, v] : [...p.sel].filter((x) => x !== v)) } : p));
  return (
    <AlertDialog open={!!picker} onOpenChange={(open) => !open && setPicker(null)}>
      <AlertDialogContent className="max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>勾选后点确定，多个以逗号分隔填入</AlertDialogDescription>
        </AlertDialogHeader>
        <ScrollArea className="max-h-72">
          <div className="space-y-0.5 pr-3">
            {picker?.rows.map((r) => (
              <Label
                key={r.value}
                className="flex cursor-pointer items-center gap-2 rounded px-2 py-1 text-xs font-normal hover:bg-muted/50"
              >
                <Checkbox
                  className="shrink-0"
                  checked={picker.sel.has(r.value)}
                  onCheckedChange={(v) => toggle(r.value, v === true)}
                />
                {r.session ? (
                  // 会话：与迁移页一致（标题在上，📁项目·日期·条数在下，不显示 id）
                  <span className="flex min-w-0 flex-col gap-0.5">
                    <span className="truncate">
                      {r.session.title ? `「${r.session.title.slice(0, 36)}」` : `（无标题）${modelOf(r.session.id) || r.session.id.slice(0, 24)}`}
                    </span>
                    <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                      {r.session.slug && (
                        <span className="inline-flex shrink-0 items-center gap-0.5 rounded bg-blue-500/10 px-1 py-px font-medium text-blue-600">
                          <Folder className="h-3 w-3" />
                          {projectName(r.session.slug)}
                        </span>
                      )}
                      <span>{[readableDate(r.session.id), r.session.authored_turns ?? r.session.turns ? `${r.session.authored_turns ?? r.session.turns} 轮` : ""].filter(Boolean).join(" · ")}</span>
                    </span>
                  </span>
                ) : (
                  // 项目：与迁移页一致（项目短名 + 会话数，不显示完整 slug）
                  <span className="flex min-w-0 items-center gap-1.5 truncate">
                    <Folder className="h-3 w-3 shrink-0 text-blue-600" />
                    <span className="truncate">{projectName(r.label)}</span>
                    {r.sub ? <span className="shrink-0 text-muted-foreground">（{r.sub}）</span> : null}
                  </span>
                )}
              </Label>
            ))}
          </div>
        </ScrollArea>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={() => setPicker(null)}>取消</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>确定</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
