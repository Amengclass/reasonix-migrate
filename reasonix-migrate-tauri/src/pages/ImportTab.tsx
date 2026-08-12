import { useEffect, useState } from "react";
import { Download, FileArchive, HardDrive, Play } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Checkbox } from "@/components/ui/checkbox";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from "@/components/ui/alert-dialog";
import { PathField } from "@/components/PathField";
import { LogPanel } from "@/components/LogPanel";
import { useRunner } from "@/lib/useRunner";
import { detectHome, doImport, listZipWorkspaces, pickDirectory, pickZipFile } from "@/lib/api";

/** 导入页：把备份 zip 恢复到目标 REASONIX_HOME。 */
export function ImportTab() {
  const { busy, lines, push, clear, run } = useRunner();
  const [backup, setBackup] = useState("");
  const [target, setTarget] = useState("");
  const [overwrite, setOverwrite] = useState(false);
  const [verify, setVerify] = useState(true);
  const [mapText, setMapText] = useState("");

  useEffect(() => {
    detectHome().then((h) => h && !target && setTarget(h.home)).catch(() => undefined);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  /** 从备份 zip 读出所有项目路径，生成「原路径=新路径」映射行（只改等号右边即可）。 */
  const loadMappings = async () => {
    if (!backup.trim()) throw new Error("请先填写备份 zip 路径");
    const ws = await listZipWorkspaces(backup.trim());
    if (ws.length === 0) {
      push("备份里没读到项目路径（manifest 缺失或空备份）", "warn");
      return [];
    }
    const generated = ws.map((w) => `${w}=${w}`).join("\n");
    setMapText(generated);
    push(`已从备份读出 ${ws.length} 个项目。需要换目录时只改等号右边的新路径；不改就按原路径恢复`, "info");
    return [];
  };

  const doRun = async () => {
    if (!backup.trim()) throw new Error("请填写备份 zip 路径");
    if (!target.trim()) throw new Error("请填写目标 REASONIX_HOME");
    push("从备份恢复", "task");
    push(`导入 ${backup.trim()} → ${target.trim()}`, "cmd", { showTs: true });
    const maps = mapText
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    const s = await doImport({
      backup: backup.trim(),
      target: target.trim(),
      maps,
      overwrite,
      verify,
      skipHashCheck: false,
    });
    push(`导入 ${s.imported_sessions} 个会话 / ${s.ok_files} 文件 → ${s.target}`, "ok", { showTs: true });
    if (s.skipped_ids.length > 0) {
      const shown = s.skipped_ids.slice(0, 10).join(", ");
      push(`冲突会话 ${s.skipped_ids.length} 个已跳过（勾选「覆盖冲突」可覆盖）：${shown}`, "warn");
    }
    if (s.conflict_files > 0) push(`冲突文件 ${s.conflict_files} 个已跳过（勾选「覆盖冲突」可覆盖）`, "warn");
    if (s.unmatched.length > 0) {
      push(`${s.unmatched.length} 个会话的项目路径未匹配，保留原路径恢复`, "warn");
      for (const [ws, , mapped] of s.unmatched.slice(0, 10)) {
        push(`${ws} → ${mapped}`, "warn", { noIcon: true, indent: 1 });
      }
    }
    if (s.errors.length > 0) {
      push(`${s.errors.length} 个文件写入失败：`, "err", { showTs: true });
      for (const e of s.errors.slice(0, 10)) push(e, "err", { noIcon: true, indent: 1 });
    }
    if (verify) push("复查导入文件哈希一致", "ok", { indent: 1 });
    push("启动一次 Reasonix 桌面端，应用会自动重建项目注册表与显示缓存", "info");
    return [];
  };

  return (
    <div className="flex h-full flex-col gap-3 overflow-y-auto px-3">
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="flex items-center gap-1.5 text-sm">
            <Download className="h-4 w-4 text-blue-500" />
            把备份 zip 恢复到目标电脑的 Reasonix 数据目录
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-2">
          <PathField
            label="备份 zip："
            kind="file"
            value={backup}
            onChange={setBackup}
            onBrowse={() => pickZipFile("选择备份 zip")}
            hint="「导出」页生成的 .zip"
          />
          <PathField
            label="目标 home："
            value={target}
            onChange={setTarget}
            onBrowse={() => pickDirectory("选择目标 REASONIX_HOME")}
            hint="目录不存在会自动创建；已自动识别当前 home"
          />
          <div className="flex items-center gap-6 pt-1">
            <Label className="flex cursor-pointer items-center gap-2 text-sm">
              <Checkbox checked={overwrite} onCheckedChange={(v) => setOverwrite(v === true)} />
              覆盖冲突（默认跳过）
            </Label>
            <Label className="flex cursor-pointer items-center gap-2 text-sm">
              <Checkbox checked={verify} onCheckedChange={(v) => setVerify(v === true)} />
              导入后复查哈希
            </Label>
          </div>
          <div className="space-y-1">
            <Label className="text-xs text-muted-foreground">
              项目目录映射（可选）换电脑后项目目录路径变了才需要填；每行一条：备份里的原路径=新电脑上的新路径
            </Label>
            <textarea
              value={mapText}
              onChange={(e) => setMapText(e.target.value)}
              rows={4}
              className="w-full rounded-md border bg-background px-2 py-1 font-mono text-xs outline-none focus-visible:ring-1"
              placeholder={"C:\\Users\\Ameng\\Desktop\\claude_woker\\vllm_windows=D:\\projects\\vllm_windows"}
            />
            <div className="flex items-center gap-2">
              <Button type="button" variant="outline" size="sm" onClick={() => run(loadMappings)} disabled={busy || !backup.trim()}>
                从备份读取项目
              </Button>
              <span className="text-xs text-muted-foreground">自动列出备份里的项目，只改等号右边；不改则按原路径恢复</span>
            </div>
          </div>
        </CardContent>
      </Card>

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
              <AlertDialogTitle>确认导入？</AlertDialogTitle>
              <AlertDialogDescription asChild>
                <div className="space-y-3">
                  <div className="space-y-1.5 rounded-md border bg-muted/40 px-3 py-2.5 text-xs">
                    <div className="flex items-center gap-2">
                      <FileArchive className="h-3.5 w-3.5 shrink-0 text-blue-500" />
                      <span className="w-20 shrink-0 text-muted-foreground">备份 zip</span>
                      <span className="min-w-0 break-all font-mono text-foreground">{backup || "(未填)"}</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <HardDrive className="h-3.5 w-3.5 shrink-0 text-emerald-500" />
                      <span className="w-20 shrink-0 text-muted-foreground">目标 home</span>
                      <span className="min-w-0 break-all font-mono text-foreground">{target || "(未填)"}</span>
                    </div>
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {overwrite && (
                      <span className="rounded-full border border-amber-400/40 bg-amber-500/10 px-2 py-0.5 text-[11px] font-medium text-amber-600">覆盖冲突</span>
                    )}
                    {verify && (
                      <span className="rounded-full border border-blue-400/40 bg-blue-500/10 px-2 py-0.5 text-[11px] font-medium text-blue-600">导入后复查哈希</span>
                    )}
                    {!overwrite && (
                      <span className="rounded-full border border-border bg-muted/40 px-2 py-0.5 text-[11px] font-medium text-muted-foreground">冲突默认跳过</span>
                    )}
                  </div>
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
