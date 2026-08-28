// daemon 连接与服务列表的 query hooks
import { useQuery, useQueryClient, useMutation } from "@tanstack/react-query";
import { z } from "zod";
import { rpc, rpcSchema } from "../rpc/client";
import { ServiceRowSchema, type ServiceRow } from "../rpc/schema";
import type { MethodName } from "../rpc/client";

export type ConnState = "connecting" | "connected" | "unavailable";

/** service.list 响应结构 */
const ServiceRowsSchema = z.object({ services: z.array(ServiceRowSchema) });

/** daemon 连接状态：轮询 system.ping（3s） */
export function useDaemonConnection() {
  const q = useQuery({
    queryKey: ["ping"],
    queryFn: () => rpc<{ ok: boolean; daemon_version: string }>("system.ping"),
    refetchInterval: 3000,
    retry: false,
  });
  const state: ConnState = q.isPending
    ? "connecting"
    : q.isError
      ? "unavailable"
      : "connected";
  return { state, version: q.data?.daemon_version };
}

/** 服务列表（含运行时状态）：轮询 2s */
export function useServices() {
  return useQuery({
    queryKey: ["services"],
    queryFn: () => rpcSchema(ServiceRowsSchema, "service.list"),
    select: (data) => data.services,
    refetchInterval: 2000,
    retry: 1,
  });
}

/** 类型守卫：服务行 */
export function asRows(data: unknown): ServiceRow[] {
  const parsed = ServiceRowsSchema.safeParse({ services: data });
  return parsed.success ? parsed.data.services : [];
}

/** 服务操作 mutation：操作后使列表失效 */
export function useServiceAction() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      method,
      service_id,
    }: {
      method: Extract<
        MethodName,
        "service.start" | "service.stop" | "service.restart" | "service.delete"
      >;
      service_id: string;
    }) => rpc(method, { service_id }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["services"] });
    },
  });
}
