// 仪表盘：daemon 信息 + 服务状态总览（大图标卡片 + 服务详细行）
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  Activity,
  CircleStop,
  Play,
  Server,
  Clock,
  CalendarDays,
  FileText,
} from "lucide-react";
import { rpc } from "../../shared/rpc/client";
import { useDaemonConnection, useServices } from "../../shared/hooks/useDaemon";
import { StatusBadge } from "../../shared/components/StatusBadge";
import { EmptyState } from "../../shared/components/EmptyState";
import { formatDateTime, formatDuration } from "../../shared/utils/format";
import type { ServiceRow } from "../../shared/rpc/schema";

interface InfoResult {
  daemon_version: string;
  schema_version: number;
  data_dir: string;
  running_as_service: boolean;
}

export function Dashboard() {
  const conn = useDaemonConnection();
  const services = useServices();
  const info = useQuery({
    queryKey: ["info"],
    queryFn: () => rpc<InfoResult>("system.info"),
    retry: false,
  });

  const rows = services.data ?? [];
  const running = rows.filter((r) => r.runtime.status === "running").length;
  const failed = rows.filter((r) => r.runtime.status === "failed").length;
  const stopped = rows.length - running - failed;

  return (
    <div className="mx-auto max-w-4xl space-y-6">
      {/* ===== 欢迎横幅：大图标 + 状态 ===== */}
      <header className="flex items-center gap-5 rounded-2xl border border-surface-3 bg-gradient-to-br from-accent/10 via-surface-2 to-surface-2 p-6">
        <img
          src="/app-icon.png"
          alt="应用图标"
          className="size-16 rounded-2xl shadow-md"
        />
        <div className="flex-1">
          <h1 className="text-xl font-bold">仪表盘</h1>
          <p className="mt-1 text-sm text-muted">
            集中管理本机 CLI 服务：启动、停止、监控与配置同步
          </p>
        </div>
      </header>

      {/* ===== 状态统计：中号图标卡片 ===== */}
      <div className="grid grid-cols-3 gap-4">
        <StatCard
          icon={<Play size={26} aria-hidden />}
          label="运行中"
          value={running}
          tone="ok"
        />
        <StatCard
          icon={<CircleStop size={26} aria-hidden />}
          label="已停止"
          value={stopped}
          tone="muted"
        />
        <StatCard
          icon={<Activity size={26} aria-hidden />}
          label="异常"
          value={failed}
          tone="err"
        />
      </div>

      {/* ===== daemon 信息 ===== */}
      <section className="rounded-2xl border border-surface-3 bg-surface-2 p-5">
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <span className="rounded-lg bg-accent/12 p-1.5">
            <Server size={15} className="text-accent" aria-hidden />
          </span>
          守护进程
        </h2>
        {info.data ? (
          <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
            <Dt>版本</Dt>
            <Dd>
              {info.data.daemon_version}（协议 v{info.data.schema_version}）
            </Dd>
            <Dt>运行方式</Dt>
            <Dd>
              {info.data.running_as_service ? "Windows 服务" : "后台进程（GUI 托管）"}
            </Dd>
            <Dt>数据目录</Dt>
            <Dd className="break-all font-mono text-xs">{info.data.data_dir}</Dd>
          </dl>
        ) : (
          <p className="text-sm text-muted">
            {conn.state === "connected" ? "读取中…" : "守护进程不可达：请先启动 daemon"}
          </p>
        )}
      </section>

      {/* ===== 服务总览：详细行 ===== */}
      <section>
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold">服务总览</h2>
          <Link to="/services" className="text-sm text-accent hover:underline">
            管理全部 →
          </Link>
        </div>
        {/* 需求4：daemon 不可达时明确提示数据为最后已知状态 */}
        {services.isError && (
          <p
            role="alert"
            className="mb-3 flex items-center gap-2 rounded-lg bg-warn/10 px-3 py-2 text-xs text-warn"
          >
            <Activity size={13} aria-hidden />
            守护进程不可达，以下为最后已知状态（恢复连接后自动刷新）
          </p>
        )}
        {rows.length === 0 ? (
          <EmptyState
            title="还没有配置服务"
            hint="添加第一个 CLI 服务，让它开机常驻"
          />
        ) : (
          <ul className="space-y-3">
            {rows.slice(0, 8).map((r) => (
              <ServiceOverviewRow key={r.service.id} row={r} />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

/** 服务总览行：名称 + 说明 + 创建时间 + 启动时间 + 运行时长 + 状态 */
function ServiceOverviewRow({ row }: { row: ServiceRow }) {
  const isRunning = row.runtime.status === "running";
  return (
    <li className="rounded-xl border border-surface-3 bg-surface-2 p-4 transition-colors hover:border-accent/40">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="text-sm font-semibold">{row.service.name}</span>
        <StatusBadge status={row.runtime.status} />
        {isRunning && (
          <span className="ml-auto inline-flex items-center gap-1 text-xs text-ok">
            <Clock size={12} aria-hidden />
            已运行 {formatDuration(row.runtime.started_at)}
          </span>
        )}
      </div>
      {/* 服务说明 */}
      <p className="mt-1.5 flex items-center gap-1.5 text-xs text-muted">
        <FileText size={11} shrink-0 aria-hidden />
        <span className="truncate">{row.service.description || "（无说明）"}</span>
      </p>
      {/* 时间信息 */}
      <div className="mt-2 flex flex-wrap gap-x-5 gap-y-1 text-xs text-muted">
        <span className="inline-flex items-center gap-1">
          <CalendarDays size={11} aria-hidden />
          创建：{formatDateTime(row.service.created_at)}
        </span>
        <span className="inline-flex items-center gap-1">
          <Clock size={11} aria-hidden />
          启动：{formatDateTime(row.runtime.started_at)}
        </span>
        {row.runtime.pid != null && <span>PID {row.runtime.pid}</span>}
      </div>
    </li>
  );
}

function StatCard({
  icon,
  label,
  value,
  tone,
}: {
  icon: React.ReactNode;
  label: string;
  value: number;
  tone: "ok" | "muted" | "err";
}) {
  const toneCls = {
    ok: "bg-ok/12 text-ok",
    muted: "bg-surface-3 text-muted",
    err: "bg-err/12 text-err",
  }[tone];
  const numCls = { ok: "text-ok", muted: "text-content", err: "text-err" }[tone];
  return (
    <div className="flex items-center gap-4 rounded-2xl border border-surface-3 bg-surface-2 p-5 transition-shadow hover:shadow-md">
      <span className={`rounded-xl p-3 ${toneCls}`}>{icon}</span>
      <div>
        <p className="text-sm text-muted">{label}</p>
        <p className={`mt-0.5 text-3xl font-bold ${numCls}`}>{value}</p>
      </div>
    </div>
  );
}

function Dt({ children }: { children: React.ReactNode }) {
  return <dt className="shrink-0 text-muted">{children}</dt>;
}

function Dd({ children, className }: { children: React.ReactNode; className?: string }) {
  return <dd className={className}>{children}</dd>;
}
