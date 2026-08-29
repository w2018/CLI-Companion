// v2.2.0 任务9：内嵌终端（xterm.js + daemon ConPTY）
// 会话链路：xterm onData → pty_write → PTY；PTY 输出 → pty-output:<id> → xterm
import { useEffect, useRef, useState } from "react";
import { useParams, Link } from "react-router-dom";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowLeft, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useServices } from "../../shared/hooks/useDaemon";

export function EmbeddedTerminal() {
  const { serviceId = "" } = useParams();
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
      const id = await invoke<number>("pty_open", { serviceId });
      if (disposed) {
        void invoke("pty_close", { id });
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
      if (ptyIdRef.current != null) {
        void invoke("pty_close_cmd", { id: ptyIdRef.current });
        ptyIdRef.current = null;
      }
      term.dispose();
    };
  }, [serviceId]);

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
          {ready ? "已连接（环境变量与工作目录同服务配置）" : "连接中…"}
        </span>
        {ready && (
          <button
            onClick={() => window.history.back()}
            className="ml-auto inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-surface-3 px-3 text-xs hover:bg-surface-3"
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
        会话在本机以服务配置的环境变量与工作目录运行；关闭页面即结束会话，不影响服务运行状态。
      </p>
    </div>
  );
}
