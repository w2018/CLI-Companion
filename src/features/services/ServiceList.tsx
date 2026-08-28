// 服务管理：列表 + 启停操作 + 新建/编辑表单
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import {
  Play,
  Square,
  RefreshCw,
  ScrollText,
  Pencil,
  Trash2,
  Plus,
  FileTerminal,
} from "lucide-react";
import { useServices, useServiceAction } from "../../shared/hooks/useDaemon";
import { StatusBadge } from "../../shared/components/StatusBadge";
import { EmptyState } from "../../shared/components/EmptyState";
import { useUiStore } from "../../stores/uiStore";
import { describeError } from "../../shared/rpc/errors";
import { ServiceForm } from "./ServiceForm";
import type { ServiceRow } from "../../shared/rpc/schema";
import type { MethodName } from "../../shared/rpc/client";

export function ServiceList() {
  const { data, isPending, isError, error, refetch } = useServices();
  const action = useServiceAction();
  const pushToast = useUiStore((s) => s.pushToast);
  const [editing, setEditing] = useState<ServiceRow | null>(null);
  const [creating, setCreating] = useState(false);

  const rows = useMemo(() => data ?? [], [data]);

  const doAction = (
    method: Extract<
      MethodName,
      "service.start" | "service.stop" | "service.restart" | "service.delete"
    >,
    row: ServiceRow,
    confirmMsg?: string,
  ) => {
    if (confirmMsg && !window.confirm(confirmMsg)) return;
    action.mutate(
      { method, service_id: row.service.id },
      {
        onSuccess: () => {
          const labels: Record<string, string> = {
            "service.start": "已发送启动命令",
            "service.stop": "已发送停止命令",
            "service.restart": "已发送重启命令",
            "service.delete": "服务已删除",
          };
          pushToast("ok", `${row.service.name}：${labels[method]}`);
        },
        onError: (e) => pushToast("err", describeError(e as never)),
      },
    );
  };

  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold">服务管理</h1>
          <p className="mt-1 text-sm text-muted">
            共 {rows.length} 个服务
          </p>
        </div>
        <button
          onClick={() => setCreating(true)}
          className="inline-flex min-h-10 items-center gap-2 rounded-lg bg-accent px-4 text-sm font-medium text-white hover:opacity-90"
        >
          <Plus size={16} aria-hidden /> 新建服务
        </button>
      </header>

      {isError ? (
        // 错误态：明确提示 + 重试，绝不静默
        <div className="rounded-xl border border-err/40 bg-err/5 p-5 text-center">
          <p className="text-sm font-medium text-err">无法获取服务列表</p>
          <p className="mt-1 text-xs text-muted">
            {error instanceof Error ? error.message : "daemon 连接失败，请确认守护进程已运行"}
          </p>
          <button
            onClick={() => void refetch()}
            className="mt-3 min-h-9 rounded-lg border border-surface-3 px-4 text-sm hover:bg-surface-3"
          >
            重试
          </button>
        </div>
      ) : isPending ? (
        <p className="py-10 text-center text-sm text-muted">加载中…</p>
      ) : rows.length === 0 ? (
        <EmptyState
          icon={<FileTerminal size={36} aria-hidden />}
          title="还没有配置服务"
          hint="点击右上角「新建服务」添加 java/node/python 等 CLI 程序"
        />
      ) : (
        <ul className="divide-y divide-surface-3 rounded-xl border border-surface-3 bg-surface-2">
          {rows.map((row) => (
            <li key={row.service.id} className="px-4 py-3">
              <div className="flex flex-wrap items-center gap-3">
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium">
                    {row.service.name}
                    {row.service.autostart && (
                      <span className="ml-2 rounded bg-surface-3 px-1.5 py-0.5 text-[10px] text-muted">
                        开机自启
                      </span>
                    )}
                  </p>
                  <p className="mt-0.5 truncate font-mono text-xs text-muted">
                    {row.service.exe}
                    {row.service.args.length > 0 && " …"}
                  </p>
                  {row.runtime.status === "running" && row.runtime.pid != null && (
                    <p className="mt-0.5 text-xs text-muted">PID {row.runtime.pid}</p>
                  )}
                </div>
                <StatusBadge status={row.runtime.status} />
                <div className="flex items-center gap-1">
                  {row.runtime.status !== "running" ? (
                    <IconBtn
                      label={`启动 ${row.service.name}`}
                      onClick={() => doAction("service.start", row)}
                      disabled={action.isPending}
                    >
                      <Play size={15} aria-hidden />
                    </IconBtn>
                  ) : (
                    <IconBtn
                      label={`停止 ${row.service.name}`}
                      onClick={() =>
                        doAction("service.stop", row, `确定停止「${row.service.name}」吗？`)
                      }
                      disabled={action.isPending}
                    >
                      <Square size={15} aria-hidden />
                    </IconBtn>
                  )}
                  <IconBtn
                    label={`重启 ${row.service.name}`}
                    onClick={() =>
                      doAction("service.restart", row, `确定重启「${row.service.name}」吗？`)
                    }
                    disabled={action.isPending}
                  >
                    <RefreshCw size={15} aria-hidden />
                  </IconBtn>
                  <Link
                    to={`/logs/${row.service.id}`}
                    aria-label={`查看 ${row.service.name} 日志`}
                    className="inline-flex size-9 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content"
                  >
                    <ScrollText size={15} aria-hidden />
                  </Link>
                  <IconBtn
                    label={`编辑 ${row.service.name}`}
                    onClick={() => setEditing(row)}
                  >
                    <Pencil size={15} aria-hidden />
                  </IconBtn>
                  <IconBtn
                    label={`删除 ${row.service.name}`}
                    onClick={() =>
                      doAction(
                        "service.delete",
                        row,
                        `确定删除「${row.service.name}」吗？运行中的服务会先被停止。`,
                      )
                    }
                    disabled={action.isPending}
                  >
                    <Trash2 size={15} aria-hidden />
                  </IconBtn>
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}

      {(creating || editing) && (
        <ServiceForm
          initial={editing?.service ?? null}
          onClose={() => {
            setCreating(false);
            setEditing(null);
          }}
        />
      )}
    </div>
  );
}

function IconBtn({
  label,
  onClick,
  disabled,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      title={label}
      onClick={onClick}
      disabled={disabled}
      className="inline-flex size-9 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content disabled:opacity-40"
    >
      {children}
    </button>
  );
}
