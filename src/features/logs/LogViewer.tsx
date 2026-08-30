// 日志查看器：tail 轮询 + 自动滚动（role="log"）+ 清理日志文件
import { useEffect, useRef, useState } from "react";
import { useParams, Link } from "react-router-dom";
import { ArrowLeft, Trash2 } from "lucide-react";
import { rpc } from "../../shared/rpc/client";
import { describeError } from "../../shared/rpc/errors";
import { useUiStore } from "../../stores/uiStore";

interface LogsResult {
  lines: string[];
  total: number;
}

export function LogViewer() {
  const { serviceId = "" } = useParams();
  const [logs, setLogs] = useState<LogsResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [autoScroll, setAutoScroll] = useState(true);
  // v2.3.0：自动换行开关（勾选=长行折行显示；不勾=单行横向滚动）
  const [wrap, setWrap] = useState(true);
  const boxRef = useRef<HTMLPreElement>(null);
  const pushToast = useUiStore((s) => s.pushToast);

  /** 清理对应服务的本地日志文件（真实删除 log 内容） */
  const clearLogs = async () => {
    if (!window.confirm("确定清理该服务的本地日志内容吗？此操作会删除日志文件中的全部记录。")) {
      return;
    }
    try {
      await rpc("service.logs.clear", { service_id: serviceId });
      setLogs({ lines: [], total: 0 });
      pushToast("ok", "日志已清理");
    } catch (e) {
      pushToast("err", describeError(e as never));
    }
  };

  // 轮询日志（2s）
  useEffect(() => {
    let alive = true;
    const fetchLogs = async () => {
      try {
        const r = await rpc<LogsResult>("service.logs", {
          service_id: serviceId,
          tail: 500,
        });
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
  }, [serviceId]);

  // 自动滚动到底部
  useEffect(() => {
    if (autoScroll && boxRef.current) {
      boxRef.current.scrollTop = boxRef.current.scrollHeight;
    }
  }, [logs, autoScroll]);

  return (
    <div className="mx-auto flex h-full max-w-5xl flex-col gap-3">
      <header className="flex items-center gap-3">
        <Link
          to="/services"
          aria-label="返回服务列表"
          className="inline-flex size-9 items-center justify-center rounded-lg text-muted hover:bg-surface-3 hover:text-content"
        >
          <ArrowLeft size={16} aria-hidden />
        </Link>
        <h1 className="text-lg font-semibold">服务日志</h1>
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
          <label className="flex items-center gap-2 text-sm text-muted">
            <input
              type="checkbox"
              checked={wrap}
              onChange={(e) => setWrap(e.target.checked)}
              className="size-4 accent-[rgb(var(--accent))]"
            />
            自动换行
          </label>
          {/* 日志清理：删除对应服务的本地 log 内容 */}
          <button
            onClick={() => void clearLogs()}
            className="inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-err/40 px-3 text-sm text-err hover:bg-err/10"
          >
            <Trash2 size={14} aria-hidden /> 清理日志
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
        aria-label="服务日志内容"
        onWheel={() => setAutoScroll(false)} // 用户滚动时暂停跟随
        className={`flex-1 overflow-auto rounded-xl border border-surface-3 bg-surface-2 p-4 font-mono text-xs leading-5 ${
          wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre"
        }`}
      >
        {logs && logs.lines.length > 0 ? (
          logs.lines.join("\n")
        ) : (
          <span className="text-muted">
            {logs ? "暂无日志输出" : "加载中…"}
          </span>
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
