// RPC 客户端：前端唯一的 daemon 通信入口
// 链路：React → Tauri invoke(daemon_rpc) → 命名管道 → daemon

import { invoke } from "@tauri-apps/api/core";
import type { z } from "zod";
import { RpcErrorSchema, type RpcError } from "./errors";

/** JSON-RPC 方法名（与 protocol crate 的 Method 枚举一一对应） */
export type MethodName =
  | "system.ping"
  | "system.info"
  | "config.get"
  | "config.update"
  | "config.import"
  | "config.export"
  | "service.list"
  | "service.create"
  | "service.update"
  | "service.delete"
  | "service.start"
  | "service.stop"
  | "service.restart"
  | "service.logs"
  | "service.logs.clear"
  | "daemon.shutdown"
  | "daemon.logs"
  | "daemon.logs.clear"
  | "sync.status"
  | "sync.run_now"
  | "sync.test"
  | "event.subscribe";

/** 解析 Rust 侧返回的错误字符串（RpcError 的 JSON 序列化） */
export function parseRpcError(raw: unknown): RpcError {
  if (typeof raw === "string") {
    const parsed = RpcErrorSchema.safeParse(JSON.parse(raw));
    if (parsed.success) return parsed.data;
    return { code: "INTERNAL", message: raw };
  }
  return { code: "INTERNAL", message: String(raw) };
}

/** 发起一次 RPC 调用；失败抛出结构化 RpcError */
export async function rpc<T>(method: MethodName, params?: unknown): Promise<T> {
  try {
    return await invoke<serde_value<T>>("daemon_rpc", {
      method,
      params: params ?? null,
    }) as T;
  } catch (e) {
    throw parseRpcError(e);
  }
}

/** 发起 RPC 并用 Zod 校验响应（运行时契约检查） */
export async function rpcSchema<S extends z.ZodTypeAny>(
  schema: S,
  method: MethodName,
  params?: unknown,
): Promise<z.infer<S>> {
  const raw = await rpc<unknown>(method, params);
  const parsed = schema.safeParse(raw);
  if (!parsed.success) {
    throw { code: "INTERNAL", message: `响应校验失败(${method}): ${parsed.error.message}` } as RpcError;
  }
  return parsed.data;
}

type serde_value<T> = T;
