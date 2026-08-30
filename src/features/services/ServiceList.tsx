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
  TerminalSquare,
  Stethoscope,
  AppWindow,
  Copy,
  X,
} from "lucide-react";
import { useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { rpc } from "../../shared/rpc/client";
import { useServices, useServiceAction, useServiceMetrics } from "../../shared/hooks/useDaemon";
import { StatusBadge } from "../../shared/components/StatusBadge";
import { EmptyState } from "../../shared/components/EmptyState";
import { ConfirmDialog } from "../../shared/components/ConfirmDialog";
import { MetricChips } from "../../shared/components/MetricChips";
import { useUiStore } from "../../stores/uiStore";
import { describeError } from "../../shared/rpc/errors";
import { ServiceForm } from "./ServiceForm";
import type { ServiceRow } from "../../shared/rpc/schema";
import type { MethodName } from "../../shared/rpc/client";

export function ServiceList() {
  const { data, isPending, isError, error, refetch } = useServices();
  const action = useServiceAction();
  const metrics = useServiceMetrics(!isError);
  const pushToast = useUiStore((s) => s.pushToast);
  const [editing, setEditing] = useState<ServiceRow | null>(null);
  const [creating, setCreating] = useState(false);
  // v2.2.0 任务7：克隆（以"新建"提交副本）
  const [cloning, setCloning] = useState<ServiceRow | null>(null);
  // 删除二次确认（需求：删除必须有明确二次确认对话框）
  const [deleteTarget, setDeleteTarget] = useState<ServiceRow | null>(null);

  const rows = useMemo(() => data ?? [], [data]);
  const metricOf = (id: string) => metrics.data?.metrics.find((m) => m.service_id === id);

  // v2.2.0 任务7：拖拽排序（拖拽中本地预览，松手经 config.update 落盘）
  const [ordered, setOrdered] = useState<ServiceRow[] | null>(null);
  const dragId = useRef<string | null>(null);
  const displayRows = ordered ?? rows;
  const handleDragOver = (e: React.DragEvent, targetId: string) => {
    e.preventDefault();
    const from = dragId.current;
    if (!from || from === targetId) return;
    const list = [...(ordered ?? rows)];
    const fi = list.findIndex((r) => r.service.id === from);
    const ti = list.findIndex((r) => r.service.id === targetId);
    if (fi < 0 || ti < 0) return;
    const [moved] = list.splice(fi, 1);
    list.splice(ti, 0, moved);
    setOrdered(list);
  };
  const commitOrder = async () => {
    const list = ordered;
    dragId.current = null;
    setOrdered(null);
    if (!list) return;
    try {
      // 复用既有 config.update 全量保存；version 与当前 schema 一致
      await rpc("config.update", {
        services: { version: 2, services: list.map((r) => r.service) },
      });
      pushToast("ok", "排序已保存");
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  const openTerminal = async (row: ServiceRow) => {
    try {
      await invoke("open_service_terminal", { serviceId: row.service.id });
      pushToast("ok", `已打开「${row.service.name}」调试终端（含其环境变量与工作目录）`);
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  // v2.2.0 任务4：崩溃诊断查看（失败态服务行）
  const [diag, setDiag] = useState<{
    service: string;
    exitCode: number;
    info: Record<string, unknown>;
    logTail: string;
  } | null>(null);
  const showDiag = async (row: ServiceRow) => {
    try {
      const { reports } = await rpc<{ reports: { name: string; service: string }[] }>(
        "crashreport.list",
      );
      const rep = reports.find((r) => r.service === row.service.name);
      if (!rep) {
        pushToast("info", "暂无该服务的崩溃诊断记录");
        return;
      }
      const detail = await rpc<{ info: Record<string, unknown>; log_tail: string }>(
        "crashreport.get",
        { name: rep.name },
      );
      const info = detail.info as { service?: string; exit_code?: number };
      setDiag({
        service: info.service ?? row.service.name,
        exitCode: info.exit_code ?? 0,
        info: detail.info,
        logTail: detail.log_tail,
      });
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  const doAction = (
    method: Extract<
      MethodName,
      "service.start" | "service.stop" | "service.restart" | "service.delete"
    >,
    row: ServiceRow,
  ) => {
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
          {displayRows.map((row) => (
            <li
              key={row.service.id}
              draggable
              onDragStart={(e) => {
                dragId.current = row.service.id;
                // WebView2/Chromium：dragstart 必须写入拖拽数据，否则拖拽不启动
                e.dataTransfer.effectAllowed = "move";
                e.dataTransfer.setData("text/plain", row.service.id);
              }}
              onDragOver={(e) => handleDragOver(e, row.service.id)}
              onDragEnter={(e) => handleDragOver(e, row.service.id)}
              onDrop={(e) => {
                e.preventDefault();
                void commitOrder();
              }}
              onDragEnd={() => {
                if (ordered) void commitOrder();
              }}
              className="cursor-grab px-4 py-3 active:cursor-grabbing"
              title="功能：按住整行拖动可调整服务顺序，松手自动保存"
            >
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
                    <p className="mt-0.5 flex items-center gap-x-2 overflow-hidden whitespace-nowrap text-xs text-muted">
                      <span className="shrink-0 font-mono text-[10px] text-err">
                        PID {row.runtime.pid}
                      </span>
                      <MetricChips metric={metricOf(row.service.id)} className="text-[8px]" />
                    </p>
                  )}
                </div>
                <StatusBadge status={row.runtime.status} />
                {row.runtime.status === "failed" && (
                  <button
                    onClick={() => void showDiag(row)}
                    className="inline-flex min-h-8 items-center gap-1 rounded-md border border-warn/40 px-2 text-xs text-warn hover:bg-warn/10"
                  >
                    <Stethoscope size={13} aria-hidden /> 诊断
                  </button>
                )}
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
                      onClick={() => {
                      if (window.confirm(`确定停止「${row.service.name}」吗？`)) {
                        doAction("service.stop", row);
                      }
                    }}
                      disabled={action.isPending}
                    >
                      <Square size={15} aria-hidden />
                    </IconBtn>
                  )}
                  <IconBtn
                    label={`重启 ${row.service.name}`}
                    onClick={() => {
                      if (window.confirm(`确定重启「${row.service.name}」吗？`)) {
                        doAction("service.restart", row);
                      }
                    }}
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
                    label={`打开 ${row.service.name} 调试终端（含其环境变量与工作目录）`}
                    onClick={() => void openTerminal(row)}
                  >
                    <TerminalSquare size={15} aria-hidden />
                  </IconBtn>
                  <Link
                    to={`/terminal/${row.service.id}`}
                    aria-label={`打开 ${row.service.name} 内嵌终端`}
                    className="inline-flex size-9 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content"
                  >
                    <AppWindow size={15} aria-hidden />
                  </Link>
                  <IconBtn
                    label={`克隆 ${row.service.name}`}
                    onClick={() => setCloning(row)}
                  >
                    <Copy size={15} aria-hidden />
                  </IconBtn>
                  <IconBtn
                    label={`编辑 ${row.service.name}`}
                    onClick={() => setEditing(row)}
                  >
                    <Pencil size={15} aria-hidden />
                  </IconBtn>
                  <IconBtn
                    label={`删除 ${row.service.name}`}
                    onClick={() => setDeleteTarget(row)}
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

      {(creating || editing || cloning) && (
        <ServiceForm
          initial={editing?.service ?? null}
          cloneOf={cloning?.service ?? null}
          onClose={() => {
            setCreating(false);
            setEditing(null);
            setCloning(null);
          }}
        />
      )}

      {/* v2.2.0 任务4：崩溃诊断弹窗 */}
      {diag && (
        <div
          role="dialog"
          aria-modal="true"
          aria-label={`崩溃诊断 ${diag.service}`}
          className="fixed inset-0 z-40 flex items-center justify-center bg-black/50 p-6"
          onClick={() => setDiag(null)}
        >
          <div
            className="flex max-h-[80vh] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-surface-3 bg-surface-2 shadow-2xl"
            onClick={(e) => e.stopPropagation()}
          >
            <header className="flex items-center justify-between border-b border-surface-3 px-5 py-3">
              <h2 className="text-base font-semibold">
                崩溃诊断 · {diag.service}（退出码 {diag.exitCode}）
              </h2>
              <button
                aria-label="关闭诊断"
                onClick={() => setDiag(null)}
                className="inline-flex size-8 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content"
              >
                <X size={16} aria-hidden />
              </button>
            </header>
            <pre className="flex-1 overflow-auto whitespace-pre-wrap p-4 font-mono text-xs leading-5">
              {JSON.stringify(diag.info, null, 2)}
              {diag.logTail ? `\n\n===== 日志末尾 =====\n${diag.logTail}` : ""}
            </pre>
          </div>
        </div>
      )}

      {/* 删除二次确认 */}
      <ConfirmDialog
        open={deleteTarget !== null}
        title="确认删除服务"
        message={
          deleteTarget
            ? `确定删除「${deleteTarget.service.name}」吗？\n运行中的服务会先被停止，删除后不可恢复。`
            : ""
        }
        actions={[
          { key: "cancel", label: "取消" },
          { key: "confirm", label: "确认删除", danger: true },
        ]}
        onAction={(key) => {
          if (key === "confirm" && deleteTarget) {
            doAction("service.delete", deleteTarget);
          }
          setDeleteTarget(null);
        }}
      />
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
