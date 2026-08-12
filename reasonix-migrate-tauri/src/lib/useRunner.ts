import { useCallback, useRef, useState } from "react";
import { LogLevel, LogLine } from "@/components/LogPanel";

/** 执行器：管理 busy 状态 + 彩色日志行，串行执行异步操作。 */
export function useRunner() {
  const [busy, setBusy] = useState(false);
  const [lines, setLines] = useState<LogLine[]>([]);
  const seq = useRef(0);

  /** 追加一行日志；level 决定颜色与图标，opts.showTs 才显示时间戳。 */
  const push = useCallback(
    (text: string, level: LogLevel = "info", opts: { showTs?: boolean; noIcon?: boolean; indent?: number } = {}) => {
      const d = new Date();
      const ts = [d.getHours(), d.getMinutes(), d.getSeconds()]
        .map((n) => String(n).padStart(2, "0"))
        .join(":");
      setLines((prev) => [...prev, { ts, level, text, ...opts }]);
    },
    []
  );

  const clear = useCallback(() => setLines([]), []);

  /** 执行 fn；fn 返回 {text, level}[] 作为输出行；抛错转为 err 行。 */
  const run = useCallback(
    async (fn: () => Promise<{ text: string; level?: LogLevel }[]>) => {
      if (busy) return;
      seq.current += 1;
      const id = seq.current;
      setBusy(true);
      try {
        const out = await fn();
        if (id === seq.current) {
          for (const l of out) push(l.text, l.level ?? "info");
        }
      } catch (e) {
        if (id === seq.current) {
          push(String(e), "err");
        }
      } finally {
        if (id === seq.current) setBusy(false);
      }
    },
    [busy, push]
  );

  return { busy, lines, push, clear, run };
}

/** 把后端返回的 summary 转成日志行。 */
export function summaryLines(title: string, extra: Record<string, unknown>[]): { text: string; level?: LogLevel }[] {
  const rows: { text: string; level?: LogLevel }[] = [{ text: title, level: "ok" }];
  for (const e of extra) {
    const text = String(e.text ?? "");
    const level = (e.level as LogLevel) ?? "info";
    rows.push({ text, level });
  }
  return rows;
}
