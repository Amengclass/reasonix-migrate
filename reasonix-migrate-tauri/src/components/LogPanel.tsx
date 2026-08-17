import { useCallback, useEffect, useRef, useState } from "react";
import { Copy, Download, Eraser } from "lucide-react";
import { save } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { saveLogFile } from "@/lib/api";
import { cn } from "@/lib/utils";

export type LogLevel = "task" | "cmd" | "info" | "ok" | "warn" | "err";

export interface LogLine {
  ts: string;
  level: LogLevel;
  text: string;
  /** 仅命令/关键节点行显示时间戳 */
  showTs?: boolean;
  /** 文件列表等缩进行不显示级别图标 */
  noIcon?: boolean;
  /** 缩进级数（每级 12px） */
  indent?: number;
}

// 深色背景（slate-900）上的高对比色板
const LEVEL_COLOR: Record<LogLevel, string> = {
  task: "text-cyan-300",
  cmd: "text-slate-300",
  info: "text-slate-100",
  ok: "text-emerald-300",
  warn: "text-amber-300",
  err: "text-red-300",
};

const LEVEL_ICON: Record<LogLevel, string> = {
  task: "┌",
  cmd: "$",
  info: "ℹ",
  ok: "✔",
  warn: "⚠",
  err: "✖",
};

function now() {
  const d = new Date();
  return [d.getHours(), d.getMinutes(), d.getSeconds()]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
}

/** 彩色分级日志面板：任务分组 + 语义前缀 + 时间戳仅在关键行 + 自动滚动 + 复制/导出/清空。 */
export function LogPanel({ lines, onClear }: { lines: LogLine[]; onClear: () => void }) {
  const boxRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);

  useEffect(() => {
    if (autoScroll && boxRef.current) {
      boxRef.current.scrollTop = boxRef.current.scrollHeight;
    }
  }, [lines, autoScroll]);

  const copyAll = useCallback(async () => {
    const text = lines.map((l) => `[${l.ts}] ${l.text}`).join("\n");
    if (text) await navigator.clipboard.writeText(text);
  }, [lines]);

  const exportLog = useCallback(async () => {
    const text = lines.map((l) => `[${l.ts}] ${l.text}`).join("\n");
    if (!text) return;
    const path = await save({
      title: "导出日志",
      defaultPath: `reasonix-migrate-log-${now().replace(/:/g, "")}.txt`,
      filters: [{ name: "文本文件", extensions: ["txt"] }],
    });
    if (path) {
      await saveLogFile(path, text);
    }
  }, [lines]);

  return (
    <div className="flex min-h-[200px] flex-1 flex-col overflow-hidden rounded-md border">
      <div className="flex items-center justify-between border-b bg-muted/40 px-2 py-1">
        <span className="text-xs font-medium text-muted-foreground">日志</span>
        <div className="flex items-center gap-0.5">
          <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" onClick={copyAll}>
            <Copy className="mr-1 h-3 w-3" />
            复制
          </Button>
          <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" onClick={exportLog}>
            <Download className="mr-1 h-3 w-3" />
            导出
          </Button>
          <Button variant="ghost" size="sm" className="h-6 px-2 text-xs" onClick={onClear}>
            <Eraser className="mr-1 h-3 w-3" />
            清空
          </Button>
          <label className="ml-1 flex cursor-pointer items-center gap-1 text-xs text-muted-foreground">
            <input
              type="checkbox"
              checked={autoScroll}
              onChange={(e) => setAutoScroll(e.target.checked)}
              className="h-3 w-3"
            />
            自动滚动
          </label>
        </div>
      </div>
      <div
        ref={boxRef}
        className="flex-1 overflow-auto bg-slate-900 p-2 font-mono text-xs leading-relaxed"
      >
        {lines.length === 0 && (
          <div className="text-slate-400">（还没有输出，点「开始执行」查看结果）</div>
        )}
        {lines.map((l, i) => {
          const isTaskStart = l.level === "task" && i > 0;
          return (
            <div key={i}>
              {isTaskStart && <div className="my-1 border-t border-slate-700/60" />}
              <div
                className={cn(
                  "whitespace-pre-wrap break-all",
                  LEVEL_COLOR[l.level],
                  l.level === "task" && "font-semibold"
                )}
                style={{ paddingLeft: (l.indent ?? 0) * 12 }}
              >
                {l.showTs ? <span className="text-slate-400">[{l.ts}]</span> : null}
                {!l.noIcon ? <span className="mr-1.5">{LEVEL_ICON[l.level]}</span> : null}
                {l.text}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
