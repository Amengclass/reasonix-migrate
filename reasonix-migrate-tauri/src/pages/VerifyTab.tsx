import { useState } from "react";
import { FileArchive, Play, ShieldCheck } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from "@/components/ui/alert-dialog";
import { PathField } from "@/components/PathField";
import { LogPanel } from "@/components/LogPanel";
import { useRunner } from "@/lib/useRunner";
import { doVerify, pickZipFile } from "@/lib/api";

/** 校验页：检查备份 zip 是否完整。 */
export function VerifyTab() {
  const { busy, lines, push, clear, run } = useRunner();
  const [backup, setBackup] = useState("");

  const doRun = async () => {
    if (!backup.trim()) throw new Error("请填写备份 zip 路径");
    push("校验备份", "task");
    push(`校验 ${backup.trim()}`, "cmd", { showTs: true });
    const v = await doVerify(backup.trim());
    push(`校验 ${v.file_count} 个文件哈希全部一致`, "ok", { showTs: true });
    push(
      `${v.session_count} 个会话 / 导出时间 ${v.exported_at ?? "-"} / 源 home ${v.source_home ?? "-"}`,
      "info",
      { noIcon: true, indent: 1 }
    );
    return [];
  };

  return (
    <div className="flex h-full flex-col gap-3 overflow-y-auto px-3">
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="flex items-center gap-1.5 text-sm">
            <ShieldCheck className="h-4 w-4 text-emerald-500" />
            检查备份 zip 完整性（文件数 + SHA-256 逐一核对）
          </CardTitle>
        </CardHeader>
        <CardContent>
          <PathField
            label="备份 zip："
            kind="file"
            value={backup}
            onChange={setBackup}
            onBrowse={() => pickZipFile("选择要检查的 zip")}
            hint="导出后 / 导入前自查用"
          />
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
              <AlertDialogTitle>确认校验？</AlertDialogTitle>
              <AlertDialogDescription asChild>
                <div className="space-y-1.5 rounded-md border bg-muted/40 px-3 py-2.5 text-xs">
                  <div className="flex items-center gap-2">
                    <FileArchive className="h-3.5 w-3.5 shrink-0 text-blue-500" />
                    <span className="w-20 shrink-0 text-muted-foreground">备份 zip</span>
                    <span className="min-w-0 break-all font-mono text-foreground">{backup || "(未填)"}</span>
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
