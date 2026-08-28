// Zod schema：daemon RPC 响应的运行时契约
// 字段名与 Rust serde 序列化（snake_case）严格对应

import { z } from "zod";

// ===== 服务定义 =====

export const ArgKindSchema = z.enum(["option", "flag", "positional"]);
export type ArgKind = z.infer<typeof ArgKindSchema>;

export const ArgSchema = z.object({
  id: z.string(),
  key: z.string(),
  value: z.string().nullable(),
  enabled: z.boolean(),
  kind: ArgKindSchema,
  description: z.string().default(""),
});
export type Arg = z.infer<typeof ArgSchema>;

export const EnvVarSchema = z.object({
  name: z.string(),
  value: z.string(),
  secret: z.boolean(),
});
export type EnvVar = z.infer<typeof EnvVarSchema>;

export const ConsoleModeSchema = z.enum(["new_console_visible", "new_console_hidden", "no_console"]);
export const WindowStartupSchema = z.enum(["normal", "minimized", "hidden"]);

export const ServiceDefinitionSchema = z.object({
  id: z.string().uuid(),
  name: z.string().min(1, "名称不能为空"),
  description: z.string(),
  enabled: z.boolean(),
  autostart: z.boolean(),
  exe: z.string().min(1, "exe 路径不能为空"),
  args: z.array(ArgSchema),
  argument_delimiter: z.string(),
  working_dir: z.string().nullable(),
  env: z.array(EnvVarSchema),
  run_as: z.object({ kind: z.string() }),
  console: z.object({
    mode: ConsoleModeSchema,
    startup: WindowStartupSchema,
  }),
  stop: z.object({
    signal: z.string(),
    graceful_timeout_ms: z.number(),
    kill_timeout_ms: z.number(),
  }),
  health: z.object({
    kind: z.unknown(),
    interval_ms: z.number(),
    failure_threshold: z.number(),
    success_threshold: z.number(),
  }),
  restart: z.object({
    policy: z.enum(["always", "on_failure", "never"]),
    max_attempts_10m: z.number(),
    backoff: z.object({
      initial_ms: z.number(),
      max_ms: z.number(),
      multiplier: z.number(),
    }),
  }),
  labels: z.array(z.string()),
  created_at: z.string(),
  updated_at: z.string(),
});
export type ServiceDefinition = z.infer<typeof ServiceDefinitionSchema>;

// ===== 运行时状态 =====

export const ServiceStatusSchema = z.enum([
  "stopped",
  "starting",
  "running",
  "stopping",
  "restarting",
  "failed",
]);
export type ServiceStatus = z.infer<typeof ServiceStatusSchema>;

export const RuntimeStateSchema = z.object({
  status: ServiceStatusSchema,
  pid: z.number().optional().nullable(),
  started_at: z.string().optional().nullable(),
  restart_count: z.number(),
  restarts_recent_10m: z.number(),
  last_exit_code: z.number().optional().nullable(),
  last_health: z.string().optional().nullable(),
});
export type RuntimeState = z.infer<typeof RuntimeStateSchema>;

/** service.list 的行 */
export const ServiceRowSchema = z.object({
  service: ServiceDefinitionSchema,
  runtime: RuntimeStateSchema,
});
export type ServiceRow = z.infer<typeof ServiceRowSchema>;

// ===== 应用配置 =====

export const AppConfigSchema = z.object({
  version: z.number(),
  general: z.object({
    language: z.string(),
    theme: z.string(),
    close_to_tray: z.boolean(),
  }),
  webdav: z.object({
    enabled: z.boolean(),
    url: z.string(),
    username: z.string(),
    remote_dir: z.string(),
    sync_interval_minutes: z.number(),
    verify_tls: z.boolean(),
    sync_config: z.boolean(),
    sync_cli_apps: z.boolean(),
  }),
});
export type AppConfig = z.infer<typeof AppConfigSchema>;

/** config.get 响应 */
export const ConfigGetSchema = z.object({
  services: z.object({ version: z.number(), services: z.array(ServiceDefinitionSchema) }),
  app: AppConfigSchema,
});

// ===== 同步 =====

export const SyncStatusSchema = z.object({
  enabled: z.boolean(),
  url: z.string(),
  username: z.string(),
  remote_dir: z.string(),
  sync_interval_minutes: z.number(),
  password_set: z.boolean(),
  state: z.object({
    last_run: z.string().nullable().optional(),
    last_direction: z.string().nullable().optional(),
    last_action: z.string().nullable().optional(),
    last_error: z.string().nullable().optional(),
  }),
});
export type SyncStatus = z.infer<typeof SyncStatusSchema>;
