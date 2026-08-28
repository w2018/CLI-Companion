// RPC 错误结构（与 protocol crate 的 RpcError 对应）
import { z } from "zod";

export const RpcErrorSchema = z.object({
  code: z.string(),
  message: z.string(),
  data: z.unknown().optional(),
});

export type RpcError = z.infer<typeof RpcErrorSchema>;

/** 错误码 → 用户可读的中文描述 */
export function describeError(err: RpcError): string {
  const map: Record<string, string> = {
    VALIDATION: "参数校验失败",
    NOT_FOUND: "资源不存在",
    LOCKED: "服务或配置正被修改，请稍后重试",
    TIMEOUT: "操作超时",
    PERMISSION_DENIED: "权限不足",
    PATH_DENIED: "路径不被允许",
    PROCESS_SPAWN_FAILED: "进程启动失败",
    CONFLICT: "配置冲突",
    SYNC_BUSY: "已有同步任务在运行",
    DAEMON_BUSY: "守护进程忙碌",
    DAEMON_UNAVAILABLE: "守护进程不可达",
    METHOD_NOT_FOUND: "未知方法",
    WEBDAV_PROTOCOL: "WebDAV 协议错误",
    WEBDAV_AUTH: "WebDAV 认证失败",
    WEBDAV_SERVER: "WebDAV 服务器错误",
    INTERNAL: "内部错误",
  };
  return `${map[err.code] ?? "未知错误"}：${err.message}`;
}
