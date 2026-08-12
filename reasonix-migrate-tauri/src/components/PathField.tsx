import { useEffect, useState } from "react";
import { FolderOpen, FolderSearch } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

interface PathFieldProps {
  label: string;
  value: string;
  onChange: (v: string) => void;
  onBrowse: () => Promise<string | null> | string | null;
  kind?: "dir" | "file";
  hint?: string;
  placeholder?: string;
}

/** 路径输入行：Entry + 浏览 + （目录型）打开 + 合法性状态提示。 */
export function PathField({
  label,
  value,
  onChange,
  onBrowse,
  kind = "dir",
  hint,
  placeholder,
}: PathFieldProps) {
  const [status, setStatus] = useState<"ok" | "empty" | "missing" | "unknown">("unknown");

  useEffect(() => {
    const v = value.trim();
    if (!v) setStatus("empty");
    else setStatus("ok");
  }, [value]);

  const openInExplorer = () => {
    const p = value.trim();
    if (!p) return;
    import("@tauri-apps/plugin-opener")
      .then((m) => m.revealItemInDir(p))
      .catch(() => undefined);
  };

  return (
    <div className="space-y-0.5">
      <div className="flex items-center gap-1.5">
        <span className="w-32 shrink-0 text-sm font-medium text-muted-foreground">{label}</span>
        <Input
          value={value}
          placeholder={placeholder}
          onChange={(e) => onChange(e.target.value)}
          className="h-8 font-mono text-xs"
        />
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-8 shrink-0 gap-1 px-2 text-xs"
          onClick={async () => {
            const r = await onBrowse();
            if (r) onChange(r);
          }}
        >
          <FolderSearch className="h-3.5 w-3.5" />
          浏览…
        </Button>
        {kind === "dir" && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-8 shrink-0 gap-1 px-2 text-xs"
            onClick={openInExplorer}
          >
            <FolderOpen className="h-3.5 w-3.5" />
            打开
          </Button>
        )}
      </div>
      <div className="flex items-center gap-2 pl-[8.5rem]">
        {status === "empty" && (
          <span className="text-xs text-amber-600">⚠️ 未填写</span>
        )}
        {status === "ok" && <span className="text-xs text-emerald-600">✅ 已填写</span>}
        {hint && <span className="truncate text-xs text-muted-foreground">{hint}</span>}
      </div>
    </div>
  );
}
