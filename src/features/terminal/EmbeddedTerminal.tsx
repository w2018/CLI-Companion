// v2.2.0 任务9 / v2.3.0 增强：内嵌终端（xterm.js + daemon ConPTY）
// 会话链路：xterm onData → pty_write → PTY；PTY 输出 → pty-output:<id> → xterm
// - 会话保持：返回页面不终止，重进自动 attach 并回放缓冲；仅「关闭会话」终止
// - 复制粘贴：左键选中即复制、右键粘贴；Ctrl+Shift+C/V 同样可用
// - 自动换行：DECAWM 模式开关（ESC[?7h / ESC[?7l）
// - 内置主题：6 套可切换（含明显的选区配色）
import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams, Link } from "react-router-dom";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowLeft, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useServices } from "../../shared/hooks/useDaemon";

/** 内置主题：选区配色 = 前景/背景互换（相反色），保证选区文字任何情况下可见 */
const THEMES = {
  dark: {
    label: "深色（默认）",
    background: "#101418",
    foreground: "#d6dae0",
    cursor: "#5fb3ff",
    cursorAccent: "#101418",
    selectionBackground: "#d6dae0",
    selectionForeground: "#101418",
    selectionInactiveBackground: "#4a545e",
  },
  solarizedDark: {
    label: "Solarized Dark（默认）",
    background: "#002b36",
    foreground: "#93a1a1",
    cursor: "#93a1a1",
    cursorAccent: "#002b36",
    selectionBackground: "#93a1a1",
    selectionForeground: "#002b36",
    selectionInactiveBackground: "#0e4449",
  },
  solarizedLight: {
    label: "Solarized Light",
    background: "#fdf6e3",
    foreground: "#586e75",
    cursor: "#657b83",
    cursorAccent: "#fdf6e3",
    selectionBackground: "#586e75",
    selectionForeground: "#fdf6e3",
    selectionInactiveBackground: "#eee8d5",
  },
  light: {
    label: "浅色",
    background: "#ffffff",
    foreground: "#24292f",
    cursor: "#1f6feb",
    cursorAccent: "#ffffff",
    selectionBackground: "#24292f",
    selectionForeground: "#ffffff",
    selectionInactiveBackground: "#d0d7de",
  },
  hicon: {
    label: "高对比黑",
    background: "#000000",
    foreground: "#eaeaea",
    cursor: "#ffffff",
    cursorAccent: "#000000",
    selectionBackground: "#eaeaea",
    selectionForeground: "#000000",
    selectionInactiveBackground: "#555555",
  },
  green: {
    label: "经典绿屏",
    background: "#0b1000",
    foreground: "#33ff66",
    cursor: "#33ff66",
    cursorAccent: "#0b1000",
    selectionBackground: "#33ff66",
    selectionForeground: "#0b1000",
    selectionInactiveBackground: "#1e5c31",
  },
} as const;

type ThemeKey = keyof typeof THEMES;

const THEME_STORE_KEY = "cc-term-theme";
const WRAP_STORE_KEY = "cc-term-wrap";

function loadStored<T extends string>(key: string, fallback: T, valid: readonly T[]): T {
  try {
    const v = localStorage.getItem(key) as T | null;
    return v && valid.includes(v) ? v : fallback;
  } catch {
    return fallback;
  }
}

export function EmbeddedTerminal() {
  const { serviceId = "" } = useParams();
  const navigate = useNavigate();
  const services = useServices();
  const serviceName = services.data?.find((r) => r.service.id === serviceId)?.service.name ?? "";
  const boxRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const ptyIdRef = useRef<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [themeKey, setThemeKey] = useState<ThemeKey>(() =>
    loadStored<ThemeKey>(THEME_STORE_KEY, "solarizedDark", Object.keys(THEMES) as ThemeKey[]),
  );
  const [wrap, setWrap] = useState<boolean>(() => {
    try {
      return localStorage.getItem(WRAP_STORE_KEY) !== "0";
    } catch {
      return true;
    }
  });

  // ===== 会话建立（attach 优先）+ 事件接线 =====
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    let disposed = false;
    const unsubs: (() => void)[] = [];
    const theme = THEMES[themeKey];

    const term = new Terminal({
      fontSize: 12,
      cursorBlink: true,
      fontFamily: "Consolas, 'Courier New', monospace",
      scrollback: 5000,
      theme: { ...theme },
    });
    termRef.current = term;
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(box);

    // 尺寸同步：布局与字体就绪后各校准一次，之后由 ResizeObserver / onResize 维持；
    // PTY 与 xterm 行列数一致是"输入回显位置正确"的前提
    const doFit = () => {
      try {
        fit.fit();
        if (ptyIdRef.current != null) {
          void invoke("pty_resize_cmd", {
            id: ptyIdRef.current,
            rows: term.rows,
            cols: term.cols,
          }).catch(() => {});
        }
      } catch {
        // 容器尺寸非法（0）时忽略，下一轮再校
      }
    };
    requestAnimationFrame(doFit);
    setTimeout(doFit, 200);

    (async () => {
      // 会话保持：先找该服务既有的 PTY 会话（返回即回放缓冲恢复屏幕）；没有才新开。
      // 行列数用 xterm 首次 fit 后的实际值——PTY 与 UI 几何一致是回显正确的前提
      const existing = await invoke<{ id: number; backlog: string } | null>("pty_attach", {
        serviceId,
      });
      let id: number;
      if (existing && typeof existing.id === "number") {
        id = existing.id;
        term.write(existing.backlog ?? "");
      } else {
        id = await invoke<number>("pty_open", {
          serviceId,
          rows: term.rows,
          cols: term.cols,
        });
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

      doFit();
      term.scrollToBottom();
      const ro = new ResizeObserver(() => doFit());
      ro.observe(box);
      unsubs.push(() => ro.disconnect());
    })().catch((e) => {
      setError(String(e));
      term.writeln(`\x1b[31m终端启动失败: ${String(e)}\x1b[0m`);
    });

    // ===== 复制粘贴：左键选中即复制（Rust 侧 arboard，绕开 WebView2 剪贴板限制）；
    // 右键粘贴；Ctrl+Shift+C/V =====
    const copySel = () => {
      const sel = term.getSelection();
      if (sel) {
        void invoke("copy_to_clipboard", { text: sel }).catch(() => {});
      }
    };
    const selSub = term.onSelectionChange(copySel);
    const pasteClipboard = () => {
      navigator.clipboard
        .readText()
        .then((t) => {
          if (t) void invoke("pty_write_cmd", { id: ptyIdRef.current, data: t }).catch(() => {});
        })
        .catch(() => {});
    };
    const onCtx = (e: MouseEvent) => {
      e.preventDefault();
      pasteClipboard();
    };
    box.addEventListener("contextmenu", onCtx);
    term.attachCustomKeyEventHandler(({ key, ctrlKey, shiftKey, type }) => {      if (type !== "keydown" || !ctrlKey || !shiftKey) return true;
      const k = key.toLowerCase();
      if (k === "c") {
        copySel();
        return false;
      }
      if (k === "v") {
        pasteClipboard();
        return false;
      }
      return true;
    });
    // attachCustomKeyEventHandler 随终端销毁自动清理，无需手动 dispose
    unsubs.push(
      () => selSub.dispose(),
      () => box.removeEventListener("contextmenu", onCtx),
    );

    return () => {
      disposed = true;
      unsubs.forEach((f) => f());
      // 会话保持：卸载只断开 UI，不终止 PTY；仅「关闭会话」按钮会真正关闭
      term.dispose();
      termRef.current = null;
    };
    // themeKey 不参与此依赖：主题切换走独立 effect，避免重建会话
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serviceId]);

  // ===== 主题切换（不重建会话，热应用）=====
  useEffect(() => {
    const theme = THEMES[themeKey];
    if (termRef.current) {
      termRef.current.options.theme = { ...theme };
    }
    if (boxRef.current) {
      boxRef.current.style.background = theme.background;
    }
    try {
      localStorage.setItem(THEME_STORE_KEY, themeKey);
    } catch {
      // 存储不可用时仅本次会话内生效
    }
  }, [themeKey]);

  // ===== 自动换行（DECAWM）：开启 \x1b[?7h，关闭 \x1b[?7l =====
  useEffect(() => {
    termRef.current?.write(wrap ? "\x1b[?7h" : "\x1b[?7l");
    try {
      localStorage.setItem(WRAP_STORE_KEY, wrap ? "1" : "0");
    } catch {
      // 存储不可用时仅本次会话内生效
    }
  }, [wrap]);

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
      <header className="flex flex-wrap items-center gap-3">
        <Link
          to="/services"
          aria-label="返回服务列表（会话保持）"
          className="inline-flex size-9 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content"
        >
          <ArrowLeft size={16} aria-hidden />
        </Link>
        <h1 className="text-lg font-semibold">
          内嵌终端{serviceName ? ` · ${serviceName}` : ""}
        </h1>
        <span className="text-xs text-muted">
          {ready ? "已连接 · 返回页面会话保持，重新进入继续" : "连接中…"}
        </span>

        {/* 主题 / 自动换行 / 关闭会话 */}
        <div className="ml-auto flex flex-wrap items-center gap-3 text-xs text-muted">
          <label className="flex items-center gap-1.5">
            主题
            <select
              value={themeKey}
              onChange={(e) => setThemeKey(e.target.value as ThemeKey)}
              className="h-8 rounded-lg border border-surface-3 bg-surface px-2 text-xs focus:border-accent focus:outline-none"
            >
              {(Object.keys(THEMES) as ThemeKey[]).map((k) => (
                <option key={k} value={k}>
                  {THEMES[k].label}
                </option>
              ))}
            </select>
          </label>
          <label className="flex cursor-pointer items-center gap-1.5" title="关闭后长行不折行（DEC 自动换行模式切换）">
            <input
              type="checkbox"
              className="size-3.5 accent-[rgb(var(--accent))]"
              checked={wrap}
              onChange={(e) => setWrap(e.target.checked)}
            />
            自动换行
          </label>
          {ready && (
            <button
              onClick={() => void closeSession()}
              className="inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-err/40 px-3 text-err hover:bg-err/10"
            >
              <X size={13} aria-hidden /> 关闭会话
            </button>
          )}
        </div>
      </header>

      {error && (
        <p role="alert" className="rounded-lg bg-err/10 px-3 py-2 text-sm text-err">
          {error}
        </p>
      )}

      <div
        ref={boxRef}
        aria-label="内嵌终端内容"
        // 注意：不加内边距——FitAddon 按容器尺寸计算行列，padding 会导致
        // PTY 几何大于可视区，输入行跑到屏幕外
        className="min-h-0 flex-1 overflow-hidden rounded-xl border border-surface-3"
        style={{ background: THEMES[themeKey].background }}
      />
      <p className="text-xs text-muted">
        左键选中即复制，右键粘贴（Ctrl+Shift+C/V 同样可用）· 会话以服务配置的环境变量与工作目录运行 ·
        返回页面会话保持，仅「关闭会话」终止并重置。
      </p>
    </div>
  );
}
