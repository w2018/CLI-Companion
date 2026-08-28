// 服务状态徽章：图标 + 文字 + 边框（状态不单独依赖颜色）
import { clsx } from "clsx";
import {
  CircleDot,
  Play,
  Loader2,
  RefreshCw,
  AlertTriangle,
} from "lucide-react";
import type { ServiceStatus } from "../rpc/schema";

const META: Record<
  ServiceStatus,
  { label: string; icon: React.ReactNode; cls: string }
> = {
  stopped: {
    label: "已停止",
    icon: <CircleDot size={13} aria-hidden />,
    cls: "border-muted/50 text-muted",
  },
  starting: {
    label: "启动中",
    icon: <Loader2 size={13} className="animate-spin" aria-hidden />,
    cls: "border-warn/60 text-warn",
  },
  running: {
    label: "运行中",
    icon: <Play size={13} aria-hidden />,
    cls: "border-ok/60 text-ok",
  },
  stopping: {
    label: "停止中",
    icon: <Loader2 size={13} className="animate-spin" aria-hidden />,
    cls: "border-warn/60 text-warn",
  },
  restarting: {
    label: "重启中",
    icon: <RefreshCw size={13} className="animate-spin" aria-hidden />,
    cls: "border-warn/60 text-warn",
  },
  failed: {
    label: "已失败",
    icon: <AlertTriangle size={13} aria-hidden />,
    cls: "border-err/60 text-err",
  },
};

export function StatusBadge({ status }: { status: ServiceStatus }) {
  const meta = META[status];
  return (
    <span
      role="status"
      aria-label={`服务状态：${meta.label}`}
      className={clsx(
        "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium",
        meta.cls,
      )}
    >
      {meta.icon}
      {meta.label}
    </span>
  );
}

/** daemon 连接状态条 */
export function ConnBadge({ state, version }: { state: string; version?: string }) {
  const map: Record<string, { label: string; cls: string; dot: string }> = {
    connected: { label: `已连接守护进程${version ? ` v${version}` : ""}`, cls: "text-ok border-ok/50", dot: "bg-ok" },
    connecting: { label: "正在连接守护进程…", cls: "text-warn border-warn/50", dot: "bg-warn" },
    unavailable: { label: "守护进程不可达", cls: "text-err border-err/50", dot: "bg-err" },
  };
  const m = map[state] ?? map.connecting;
  return (
    <span
      aria-live="polite"
      className={clsx("inline-flex items-center gap-2 rounded-full border px-3 py-1 text-xs", m.cls)}
    >
      <span className={clsx("size-2 rounded-full", m.dot)} aria-hidden />
      {m.label}
    </span>
  );
}
