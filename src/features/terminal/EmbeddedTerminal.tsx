// v2.2.0 任务9：内嵌终端（xterm.js + daemon ConPTY）
// 会话链路：xterm onData → pty_write → PTY；PTY 输出 → pty-output:<id> → xterm
import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams, Link } from "react-router-dom";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowLeft, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useServices } from "../../shared/hooks/useDaemon";

export function EmbeddedTerminal() {
  const { serviceId = "" } = useParams();
  const navigate = useNavigate();
  const services = useServices();
  const serviceName = services.data?.find((r) => r.service.id === serviceId)?.service.name ?? "";
  const boxRef = useRef<HTMLDivElement>(null);
  const ptyIdRef = useRef<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    let disposed = false;
    const unsubs: (() => void)[] = [];

    const term = new Terminal({
      fontSize: 12,
      cursorBlink: true,
      fontFamily: "Consolas, 'Courier New', monospace",
      theme: { background: "#101418", foreground: "#d6dae0" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(box);
    try {
      fit.fit();
    } catch {
      // 容器尚未布局时忽略，ResizeObserver 会再次触发
    }

    (async () => {
      // v2.2.0 会话保持：先找该服务既有的 PTY 会话（返回即回放缓冲恢复屏幕）；
      // 没有才新开。离开页面只会"断开"不会关闭会话。
      const existing = await invoke<{ id: number; backlog: string } | null>("pty_attach", {
        serviceId,
      });
      let id: number;
      if (existing) {
        id = existing.id;
        term.write(existing.backlog);
      } else {
        id = await invoke<number>("pty_open", { serviceId });
      }
      if (disposed) {
        // 会话保留（不关闭），下次进入可继续
        return;
      }
      ptyIdRef.current = id;
      setReady(true);

      const offOut = await listen<string>(`pty-output:${id}`, (e) => {
        term.write(e.payload);
      });
      const offExit = await listen(`pty-exit:${id}`, () => {
        term.writeln("\r\n\x1b[90m[会话已结束]\x1b[0m");
      });
      const dataSub = term.onData((d) => {
        void invoke("pty_write_cmd", { id, data: d }).catch(() => {});
      });
      const resizeSub = term.onResize(({ rows, cols }) => {
        void invoke("pty_resize_cmd", { id, rows, cols }).catch(() => {});
      });

      unsubs.push(
        () => offOut(),
        () => offExit(),
        () => dataSub.dispose(),
        () => resizeSub.dispose(),
      );

      // 初始尺寸同步 + 跟随窗口自适应
      void invoke("pty_resize_cmd", { id, rows: term.rows, cols: term.cols }).catch(() => {});
      const ro = new ResizeObserver(() => {
        try {
          fit.fit();
        } catch {
          // 尺寸非法时忽略
        }
      });
      ro.observe(box);
      unsubs.push(() => ro.disconnect());
    })().catch((e) => {
      setError(String(e));
      term.writeln(`\x1b[31m终端启动失败: ${String(e)}\x1b[0m`);
    });

    return () => {
      disposed = true;
      unsubs.forEach((f) => f());
      // 会话保持：卸载只断开 UI，不终止 PTY；仅「关闭会话」按钮会真正关闭
      term.dispose();
    };
  }, [serviceId]);

  /** 主动关闭会话（终止 PTY 并清空），返回服务列表 */
  const closeSession = async () => {
    if (ptyIdRef.current != null) {
      try {
        await invoke("pty_close_cmd", { id: ptyIdRef.current });
      } catch {
        // 会话可能已随子进程退出被清理
      }
      ptyIdRef.current = null;
    }
    navigate("/services");
  };

  return (
    <div className="mx-auto flex h-[calc(100vh-96px)] max-w-6xl flex-col gap-3">
      <header className="flex items-center gap-3">
        <Link
          to="/services"
          aria-label="返回服务列表"
          className="inline-flex size-9 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content"
        >
          <ArrowLeft size={16} aria-hidden />
        </Link>
        <h1 className="text-lg font-semibold">
          内嵌终端{serviceName ? ` · ${serviceName}` : ""}
        </h1>
        <span className="text-xs text-muted">
          {ready
            ? "已连接 · 返回页面会话保持，重新进入继续"
            : "连接中…"}
        </span>
        {ready && (
          <button
            onClick={() => void closeSession()}
            className="ml-auto inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-err/40 px-3 text-xs text-err hover:bg-err/10"
          >
            <X size={13} aria-hidden /> 关闭会话
          </button>
        )}
      </header>

      {error && (
        <p role="alert" className="rounded-lg bg-err/10 px-3 py-2 text-sm text-err">
          {error}
        </p>
      )}

      <div
        ref={boxRef}
        aria-label="内嵌终端内容"
        className="min-h-0 flex-1 overflow-hidden rounded-xl border border-surface-3 bg-[#101418] p-2"
      />
      <p className="text-xs text-muted">
        会话在本机以服务配置的环境变量与工作目录运行；返回或切换页面时终端会话保持（重新进入继续），
        仅点击「关闭会话」才会终止并重置。
      </p>
    </div>
  );
}
