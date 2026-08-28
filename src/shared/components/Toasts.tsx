// 全局 Toast 通知（aria-live polite）
import { useUiStore } from "../../stores/uiStore";
import { CheckCircle2, Info, AlertTriangle, XCircle, X } from "lucide-react";
import { clsx } from "clsx";

const ICONS = {
  ok: <CheckCircle2 size={16} className="text-ok" aria-hidden />,
  info: <Info size={16} className="text-accent" aria-hidden />,
  warn: <AlertTriangle size={16} className="text-warn" aria-hidden />,
  err: <XCircle size={16} className="text-err" aria-hidden />,
};

export function Toasts() {
  const { toasts, dismissToast } = useUiStore();
  return (
    <div
      aria-live="polite"
      className="pointer-events-none fixed right-4 top-4 z-50 flex flex-col gap-2"
    >
      {toasts.map((t) => (
        <div
          key={t.id}
          role="alert"
          className={clsx(
            "pointer-events-auto flex items-center gap-2 rounded-lg border border-surface-3 bg-surface-2 px-4 py-2.5 text-sm shadow-lg",
            "min-w-64 max-w-md",
          )}
        >
          {ICONS[t.kind]}
          <span className="flex-1">{t.message}</span>
          <button
            aria-label="关闭通知"
            onClick={() => dismissToast(t.id)}
            className="text-muted hover:text-content"
          >
            <X size={14} aria-hidden />
          </button>
        </div>
      ))}
    </div>
  );
}
