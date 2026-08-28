// 主布局：侧边栏 + 内容区 + Toast + 窗口关闭行为控制 + daemon 事件驱动刷新
import { useEffect, useState } from "react";
import { Outlet, NavLink } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import {
  LayoutDashboard,
  ListChecks,
  Settings,
  Info,
  TerminalSquare,
  ScrollText,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { clsx } from "clsx";
import { useDaemonConnection } from "./shared/hooks/useDaemon";
import { ConnBadge } from "./shared/components/StatusBadge";
import { Toasts } from "./shared/components/Toasts";
import { ConfirmDialog } from "./shared/components/ConfirmDialog";
import { StopDaemonDialog } from "./features/settings/StopDaemonDialog";
import { rpc } from "./shared/rpc/client";
import { useUiStore } from "./stores/uiStore";

const NAV = [
  { to: "/", label: "仪表盘", icon: <LayoutDashboard size={18} aria-hidden /> },
  { to: "/services", label: "服务管理", icon: <ListChecks size={18} aria-hidden /> },
  { to: "/daemon-log", label: "守护进程日志", icon: <ScrollText size={18} aria-hidden /> },
  { to: "/settings", label: "设置", icon: <Settings size={18} aria-hidden /> },
  { to: "/about", label: "关于", icon: <Info size={18} aria-hidden /> },
];

export function App() {
  const { state, version } = useDaemonConnection();
  // 退出确认弹窗：null=隐藏；"close"=关闭窗口确认
  const [exitConfirm, setExitConfirm] = useState(false);
  // 完全退出：先显示逐条停止进度弹窗，完成后再销毁窗口（用户建议采纳）
  const [quitDialogOpen, setQuitDialogOpen] = useState(false);

  // ===== 窗口关闭行为（需求②）=====
  // close_to_tray=true → 隐藏到托盘；false → 弹窗："仅关闭 GUI" / "完全退出"
  useEffect(() => {
    const win = getCurrentWindow();
    const unlisten = win.onCloseRequested(async (event) => {
      let closeToTray = true;
      try {
        const cfg = await rpc<{ app: { general: { close_to_tray: boolean } } }>("config.get");
        closeToTray = cfg.app.general.close_to_tray;
      } catch {
        // daemon 不可达时保持默认：隐藏到托盘
      }
      if (closeToTray) {
        event.preventDefault();
        await win.hide();
      } else {
        // 弹确认框（阻止默认关闭，等用户选择）
        event.preventDefault();
        setExitConfirm(true);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // ===== 托盘"完全退出"：由 Rust 侧发事件 → 前端显示停止进度后退出 =====
  useEffect(() => {
    const unlisten = listen("quit-all-requested", () => {
      setQuitDialogOpen(true);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // ===== daemon 事件流：替代高频轮询，事件到达时精准刷新对应数据 =====
  // 链路：daemon 事件总线 → gui-core 订阅转发（Rust）→ "daemon-event" → 这里
  const queryClient = useQueryClient();
  useEffect(() => {
    const unlisten = listen<{
      topic: string;
      service_id?: string;
      payload?: { name?: string; exit_code?: number; auto?: boolean };
    }>("daemon-event", (e) => {
      const { topic, payload } = e.payload;
      switch (topic) {
        case "service.started":
        case "service.stopped":
        case "service.health":
        case "service.restart_attempt":
          void queryClient.invalidateQueries({ queryKey: ["services"] });
          break;
        case "config.changed":
          void queryClient.invalidateQueries({ queryKey: ["services"] });
          void queryClient.invalidateQueries({ queryKey: ["config"] });
          break;
        case "sync.progress":
        case "sync.conflict":
          void queryClient.invalidateQueries({ queryKey: ["sync"] });
          break;
        default:
          break;
      }
      // 用户可感知的异常主动提示（正常运行事件不打扰）
      if (topic === "service.health") {
        useUiStore.getState().pushToast(
          "warn",
          `服务「${payload?.name ?? "未知"}」意外退出（退出码 ${payload?.exit_code ?? "?"}），正在按策略处理`,
        );
      } else if (topic === "service.restart_attempt") {
        useUiStore
          .getState()
          .pushToast("err", `服务「${payload?.name ?? "未知"}」自动重启失败或已触发熔断`);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [queryClient]);

  /** 仅关闭 GUI：daemon 保持运行（exit_app = Rust 侧 app.exit(0)，最可靠） */
  const exitGui = async () => {
    await invoke("exit_app");
  };

  return (
    <div className="flex h-screen bg-surface text-content">
      {/* 侧边栏 */}
      <nav
        aria-label="主导航"
        className="flex w-52 shrink-0 flex-col gap-1 border-r border-surface-3 bg-surface-2 p-3"
      >
        <div className="mb-4 flex items-center gap-2 px-2 py-1">
          <TerminalSquare size={22} className="text-accent" aria-hidden />
          <span className="text-base font-semibold">CLI Companion</span>
        </div>
        {NAV.map((n) => (
          <NavLink
            key={n.to}
            to={n.to}
            end={n.to === "/"}
            className={({ isActive }) =>
              clsx(
                "flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm transition-colors",
                "min-h-10",
                isActive
                  ? "bg-accent/15 font-medium text-accent"
                  : "text-muted hover:bg-surface-3 hover:text-content",
              )
            }
          >
            {n.icon}
            {n.label}
          </NavLink>
        ))}
        <div className="mt-auto px-2 pb-1">
          <ConnBadge state={state} version={version} />
        </div>
      </nav>

      {/* 内容区（key 切换页面时重挂载，触发重新请求） */}
      <main className="flex-1 overflow-y-auto p-6">
        <Outlet />
      </main>

      <Toasts />

      {/* 关闭窗口确认（close_to_tray=false 时触发） */}
      <ConfirmDialog
        open={exitConfirm}
        title="确认退出"
        message="选择退出方式：仅关闭 GUI 会保留后台服务运行；完全退出会同时停止全部受管服务。"
        actions={[
          { key: "cancel", label: "取消" },
          { key: "gui", label: "仅关闭 GUI" },
          { key: "all", label: "完全退出", danger: true },
        ]}
        onAction={(key) => {
          setExitConfirm(false);
          if (key === "gui") void exitGui();
          if (key === "all") setQuitDialogOpen(true); // 完全退出：先显示逐条停止进度
        }}
      />

      {/* 完全退出：逐条停止服务进度（完成或点"退出"后退出应用） */}
      <StopDaemonDialog
        open={quitDialogOpen}
        mode="quit"
        onClose={() => void exitGui()}
        onFinished={() => {
          // daemon 已收到关闭指令，延迟少许等待退出收尾后退出 GUI
          setTimeout(() => {
            void exitGui();
          }, 600);
        }}
      />
    </div>
  );
}
