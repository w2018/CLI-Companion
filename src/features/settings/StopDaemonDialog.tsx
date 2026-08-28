// 停止守护进程进度弹窗：逐条显示每个服务 + 守护进程的关闭状态
// 性能：服务并行停止（总时长 = 最慢者），但每条完成时独立更新 UI，保留逐条视觉效果
import { useEffect, useRef, useState } from "react";
import { CheckCircle2, XCircle, Loader2, Hourglass, Server } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { rpc } from "../../shared/rpc/client";
import { describeError } from "../../shared/rpc/errors";
import type { ServiceDefinition } from "../../shared/rpc/schema";

type StepStatus = "pending" | "stopping" | "stopped" | "failed";

interface Step {
  id: string;
  name: string;
  status: StepStatus;
}

interface Props {
  open: boolean;
  /** stop = 设置页停止 daemon；quit = 完全退出（完成后销毁窗口） */
  mode?: "stop" | "quit";
  onClose: () => void;
  onFinished: () => void; // 全部完成后回调（刷新查询）
}

const DAEMON_STEP_ID = "__daemon__";

export function StopDaemonDialog({ open, mode = "stop", onClose, onFinished }: Props) {
  const [steps, setSteps] = useState<Step[]>([]);
  const [phase, setPhase] = useState<"running" | "done" | "error">("running");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const startedRef = useRef(false);

  useEffect(() => {
    // 关闭时重置执行标记，保证下次打开能重新执行
    if (!open) {
      startedRef.current = false;
      return;
    }
    if (startedRef.current) return;
    startedRef.current = true;
    void runStopSequence();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const setStep = (id: string, status: StepStatus) =>
    setSteps((prev) => prev.map((st) => (st.id === id ? { ...st, status } : st)));

  /** 并行停止全部服务 → 关闭守护进程 */
  const runStopSequence = async () => {
    try {
      // 1. 获取当前服务列表（含守护进程占位行，需求4）
      const list = await rpc<{ services: { service: ServiceDefinition }[] }>("service.list");
      const items = list.services;
      setSteps([
        ...items.map((s) => ({ id: s.service.id, name: s.service.name, status: "pending" as StepStatus })),
        { id: DAEMON_STEP_ID, name: "守护进程 (daemon)", status: "pending" as StepStatus },
      ]);

      // 2. 并行发出全部停止命令（快），每条完成独立刷新状态（逐条视觉效果）
      await Promise.all(
        items.map((item) => {
          setStep(item.service.id, "stopping");
          return rpc("service.stop", { service_id: item.service.id })
            .then(() => setStep(item.service.id, "stopped"))
            .catch((e) => {
              setStep(item.service.id, "failed");
              console.warn(`停止服务 ${item.service.name} 失败:`, describeError(e as never));
            });
        }),
      );

      // 3. 全部服务处理完毕 → 停止守护进程本体（也在列表中显示状态）
      setStep(DAEMON_STEP_ID, "stopping");
      // 先关闭 GUI 侧的"不可达即自动拉起"，否则 8s 轮询会立刻把 daemon 复活
      await invoke("set_daemon_autostart", { allowed: false }).catch(() => {});
      try {
        await rpc("daemon.shutdown", { stop_services: false });
      } catch {
        // daemon 已自行退出（不可达）：目标已达成，不算失败
      }
      // 4. 确认 daemon 真正退出（管道不可达即成功；兜底最多等 4 秒）
      const deadline = Date.now() + 4000;
      while (Date.now() < deadline) {
        const alive = await invoke<boolean>("daemon_status").catch(() => false);
        if (!alive) break;
        await new Promise((r) => setTimeout(r, 300));
      }
      setStep(DAEMON_STEP_ID, "stopped");
      setPhase("done");
      onFinished();
    } catch (e) {
      // 完全退出模式：daemon 不可达 = 没有服务需要停止，直接退出 GUI，
      // 不卡在错误提示上（此前会停在这里等兜底定时器，体验很差）
      if (mode === "quit") {
        onClose();
        return;
      }
      setErrorMsg(describeError(e as never));
      setPhase("error");
    }
  };

  if (!open) return null;

  const STATUS_META: Record<StepStatus, { icon: React.ReactNode; label: string; cls: string }> = {
    pending: { icon: <Hourglass size={14} className="text-muted" aria-hidden />, label: "等待中", cls: "text-muted" },
    stopping: { icon: <Loader2 size={14} className="animate-spin text-warn" aria-hidden />, label: "正在停止…", cls: "text-warn" },
    stopped: { icon: <CheckCircle2 size={14} className="text-ok" aria-hidden />, label: "已停止", cls: "text-ok" },
    failed: { icon: <XCircle size={14} className="text-err" aria-hidden />, label: "停止失败", cls: "text-err" },
  };

  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-label="正在停止守护进程"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6"
    >
      <div className="w-full max-w-md rounded-2xl border border-surface-3 bg-surface-2 p-5 shadow-2xl">
        <h2 className="flex items-center gap-2 text-base font-semibold">
          <Server size={17} className="text-accent" aria-hidden />
          正在停止守护进程
        </h2>
        <p className="mt-1 text-xs text-muted">停止全部受管服务后，关闭守护进程</p>

        {/* 服务 + 守护进程关闭进度列表 */}
        {steps.length > 0 ? (
          <ul className="mt-4 max-h-64 space-y-2 overflow-y-auto">
            {steps.map((st) => {
              const meta = STATUS_META[st.status];
              const isDaemon = st.id === DAEMON_STEP_ID;
              return (
                <li
                  key={st.id}
                  className={`flex items-center gap-2.5 rounded-lg border px-3 py-2 text-sm ${
                    isDaemon ? "border-accent/30 bg-accent/5 font-medium" : "border-surface-3 bg-surface"
                  }`}
                >
                  {meta.icon}
                  <span className="flex-1 truncate">{st.name}</span>
                  <span className={`text-xs ${meta.cls}`}>{meta.label}</span>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="mt-4 flex items-center gap-2 text-sm text-muted">
            <Loader2 size={14} className="animate-spin" aria-hidden /> 正在获取服务列表…
          </p>
        )}

        {phase === "done" && (
          <p className="mt-4 flex items-center gap-2 rounded-lg bg-ok/10 px-3 py-2 text-sm text-ok">
            <CheckCircle2 size={15} aria-hidden />
            {mode === "quit"
              ? "全部服务与守护进程已停止，正在退出…"
              : "守护进程已停止，可稍后在下方重新启动"}
          </p>
        )}
        {phase === "error" && errorMsg && (
          <p role="alert" className="mt-4 rounded-lg bg-err/10 px-3 py-2 text-sm text-err">
            {errorMsg}
          </p>
        )}

        <div className="mt-5 flex justify-end">
          <button
            onClick={onClose}
            disabled={phase === "running"}
            className="min-h-9 rounded-lg border border-surface-3 px-4 text-sm hover:bg-surface-3 disabled:opacity-40"
          >
            {phase === "running" ? "请等待…" : mode === "quit" ? "退出" : "关闭"}
          </button>
        </div>
      </div>
    </div>
  );
}
