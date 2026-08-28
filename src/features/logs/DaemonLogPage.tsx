// 守护进程日志页：tail 轮询 + 自动滚动（role="log"）+ 清理 daemon.log
// 数据源：daemon.logs RPC（读取 <数据目录>/logs/daemon.log，即 daemon 自身运行日志）
import { useEffect, useRef, useState } from "react";
import { Trash2 } from "lucide-react";
import { rpc } from "../../shared/rpc/client";
import { describeError } from "../../shared/rpc/errors";
import { useUiStore } from "../../stores/uiStore";

interface LogsResult {
  lines: string[];
  total: number;
}

export function DaemonLogPage() {
  const [logs, setLogs] = useState<LogsResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [autoScroll, setAutoScroll] = useState(true);
  const boxRef = useRef<HTMLPreElement>(null);
  const pushToast = useUiStore((s) => s.pushToast);

  const clearLogs = async () => {
    if (!window.confirm("确定清空守护进程日志吗？此操作会删除 daemon.log 中的全部记录。")) {
      return;
    }
    try {
      await rpc("daemon.logs.clear");
      setLogs({ lines: [], total: 0 });
      pushToast("ok", "守护进程日志已清空");
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  // 轮询日志（2s）
  useEffect(() => {
    let alive = true;
    const fetchLogs = async () => {
      try {
        const r = await rpc<LogsResult>("daemon.logs", { tail: 500 });
        if (alive) {
          setLogs(r);
          setError(null);
        }
      } catch (e) {
        if (alive) setError(describeError(e as never));
      }
    };
    void fetchLogs();
    const timer = setInterval(fetchLogs, 2000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

  // 自动滚动到底部
  useEffect(() => {
    if (autoScroll && boxRef.current) {
      boxRef.current.scrollTop = boxRef.current.scrollHeight;
    }
  }, [logs, autoScroll]);

  return (
    <div className="mx-auto flex h-full max-w-5xl flex-col gap-3">
      <header className="flex items-center gap-3">
        <h1 className="text-lg font-semibold">守护进程日志</h1>
        <p className="text-xs text-muted">daemon 自身的运行日志（连接、服务编排、同步调度）</p>
        <div className="ml-auto flex items-center gap-3">
          <label className="flex items-center gap-2 text-sm text-muted">
            <input
              type="checkbox"
              checked={autoScroll}
              onChange={(e) => setAutoScroll(e.target.checked)}
              className="size-4 accent-[rgb(var(--accent))]"
            />
            自动滚动
          </label>
          <button
            onClick={() => void clearLogs()}
            className="inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-err/40 px-3 text-sm text-err hover:bg-err/10"
          >
            <Trash2 size={14} aria-hidden /> 清空日志
          </button>
        </div>
      </header>

      {error && (
        <p role="alert" className="rounded-lg bg-err/10 px-3 py-2 text-sm text-err">
          {error}
        </p>
      )}

      <pre
        ref={boxRef}
        role="log"
        aria-label="守护进程日志内容"
        onWheel={() => setAutoScroll(false)} // 用户滚动时暂停跟随
        className="flex-1 overflow-auto rounded-xl border border-surface-3 bg-surface-2 p-4 font-mono text-xs leading-5"
      >
        {logs && logs.lines.length > 0 ? (
          logs.lines.join("\n")
        ) : (
          <span className="text-muted">{logs ? "暂无日志输出" : "加载中…"}</span>
        )}
      </pre>

      {logs && (
        <p className="text-xs text-muted">
          共 {logs.total} 行，显示最近 {logs.lines.length} 行
          {" · "}
          <button
            className="text-accent hover:underline"
            onClick={() => {
              setLogs({ lines: [], total: 0 });
              pushToast("info", "已清空显示（不影响日志文件）");
            }}
          >
            清空显示
          </button>
        </p>
      )}
    </div>
  );
}
