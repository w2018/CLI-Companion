// 确认对话框（受控组件）
import { AlertTriangle } from "lucide-react";

export interface ConfirmAction {
  key: string;
  label: string;
  /** danger 样式按钮 */
  danger?: boolean;
}

export function ConfirmDialog({
  open,
  title,
  message,
  actions,
  onAction,
}: {
  open: boolean;
  title: string;
  message: string;
  actions: ConfirmAction[];
  onAction: (key: string) => void;
}) {
  if (!open) return null;
  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-label={title}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6"
    >
      <div className="w-full max-w-sm rounded-2xl border border-surface-3 bg-surface-2 p-5 shadow-2xl">
        <div className="flex items-start gap-3">
          <div className="mt-0.5 rounded-full bg-warn/15 p-2">
            <AlertTriangle size={18} className="text-warn" aria-hidden />
          </div>
          <div className="flex-1">
            <h2 className="text-base font-semibold">{title}</h2>
            <p className="mt-1.5 text-sm text-muted">{message}</p>
          </div>
        </div>
        <div className="mt-5 flex justify-end gap-2">
          {actions.map((a) => (
            <button
              key={a.key}
              autoFocus={a.key === actions[0].key}
              onClick={() => onAction(a.key)}
              className={
                a.danger
                  ? "min-h-9 rounded-lg bg-err px-4 text-sm font-medium text-white hover:opacity-90"
                  : "min-h-9 rounded-lg border border-surface-3 px-4 text-sm hover:bg-surface-3"
              }
            >
              {a.label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
