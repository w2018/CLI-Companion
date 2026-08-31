// schema 契约测试：确保 Zod 定义与 daemon 实际响应格式一致
import { describe, expect, it } from "vitest";
import {
  ServiceRowSchema,
  SyncStatusSchema,
  AppConfigSchema,
  FtpSettingsSchema,
  FtpStatusSchema,
} from "./schema";
import { RpcErrorSchema, describeError } from "./errors";

describe("ServiceRowSchema", () => {
  it("解析 daemon service.list 的行结构", () => {
    const row = {
      service: {
        id: "a2b9c0d1-0000-4000-8000-000000000001",
        name: "示例代理",
        description: "演示",
        enabled: true,
        autostart: false,
        exe: "C:/Tools/agent.exe",
        args: [
          { id: "a1", key: "--port", value: "8080", enabled: true, kind: "option", description: "" },
          { id: "a2", key: "-v", value: null, enabled: false, kind: "flag", description: "" },
        ],
        argument_delimiter: " ",
        working_dir: null,
        env: [],
        run_as: { kind: "current_user" },
        console: { mode: "no_console", startup: "normal" },
        stop: { signal: "ctrl_c", graceful_timeout_ms: 15000, kill_timeout_ms: 10000 },
        health: { kind: "process", interval_ms: 5000, failure_threshold: 3, success_threshold: 1 },
        restart: {
          policy: "on_failure",
          max_attempts_10m: 10,
          backoff: { initial_ms: 2000, max_ms: 300000, multiplier: 2 },
        },
        labels: [],
        created_at: "2026-08-28T08:00:00+08:00",
        updated_at: "2026-08-28T08:00:00+08:00",
      },
      runtime: {
        status: "running",
        pid: 1234,
        started_at: "2026-08-28T09:00:00+08:00",
        restart_count: 0,
        restarts_recent_10m: 0,
        last_exit_code: null,
      },
    };
    const parsed = ServiceRowSchema.safeParse(row);
    expect(parsed.success).toBe(true);
  });

  it("拒绝非法状态值", () => {
    const bad = ServiceRowSchema.safeParse({
      service: { exe: "" },
      runtime: { status: "weird" },
    });
    expect(bad.success).toBe(false);
  });
});

describe("RpcErrorSchema", () => {
  it("解析 Rust 侧 RpcError JSON 字符串", () => {
    const raw = JSON.stringify({ code: "PATH_DENIED", message: "越界路径" });
    const parsed = RpcErrorSchema.safeParse(JSON.parse(raw));
    expect(parsed.success).toBe(true);
    if (parsed.success) {
      expect(describeError(parsed.data)).toContain("路径不被允许");
    }
  });
});

describe("AppConfigSchema", () => {
  it("解析 app 配置", () => {
    const app = {
      version: 1,
      general: { language: "zh-CN", theme: "system", close_to_tray: true },
      webdav: {
        enabled: false,
        url: "",
        username: "",
        remote_dir: "cli-companion",
        sync_interval_minutes: 15,
        verify_tls: true,
        sync_config: true,
        sync_cli_apps: false,
      },
    };
    expect(AppConfigSchema.safeParse(app).success).toBe(true);
  });

  it("notify_on_failure 缺省与有值均可解析（向后兼容）", () => {
    const general = { language: "zh-CN", theme: "system", close_to_tray: true };
    expect(AppConfigSchema.shape.general.safeParse(general).success).toBe(true);
    expect(
      AppConfigSchema.shape.general.safeParse({ ...general, notify_on_failure: false }).success,
    ).toBe(true);
  });
});

describe("MetricsSchema", () => {
  it("解析 service.metrics 响应（可选字段缺省兼容）", async () => {
    const { MetricsSchema } = await import("./schema");
    const r = MetricsSchema.safeParse({
      metrics: [
        { service_id: "a", cpu_percent: 12.5, mem_bytes: 268435456 },
        { service_id: "b" },
      ],
    });
    expect(r.success).toBe(true);
    if (r.success) {
      expect(r.data.metrics[0].cpu_percent).toBe(12.5);
      expect(r.data.metrics[1].mem_bytes).toBeUndefined();
    }
  });

  it("runtime 新增 cpu/mem 采样字段可解析", async () => {
    const { RuntimeStateSchema } = await import("./schema");
    const r = RuntimeStateSchema.safeParse({
      status: "running",
      pid: 100,
      restart_count: 0,
      restarts_recent_10m: 0,
      cpu_percent: 3.5,
      mem_bytes: 1048576,
    });
    expect(r.success).toBe(true);
  });

  it("v2.4.0 扩展指标字段可解析（旧载荷缺省兼容）", async () => {
    const { MetricsSchema } = await import("./schema");
    // 旧 daemon 载荷（无任何新字段）照常解析
    expect(MetricsSchema.safeParse({ metrics: [{ service_id: "a" }] }).success).toBe(true);
    // 新 daemon 载荷全字段解析
    const r = MetricsSchema.safeParse({
      metrics: [
        {
          service_id: "a",
          cpu_percent: 3.1,
          mem_bytes: 1048576,
          mem_percent: 0.05,
          gpu_percent: 12,
          gpu_mem_bytes: 52428800,
          disk_read_bytes_per_sec: 1024,
          disk_write_bytes_per_sec: 2048,
          net_rx_bytes_per_sec: 4096,
          net_tx_bytes_per_sec: 8192,
        },
      ],
    });
    expect(r.success).toBe(true);
    if (r.success) {
      expect(r.data.metrics[0].gpu_percent).toBe(12);
      expect(r.data.metrics[0].mem_percent).toBe(0.05);
      expect(r.data.metrics[0].net_tx_bytes_per_sec).toBe(8192);
    }
  });

  it("runtime v2.4.0 扩展字段可解析（旧载荷缺省兼容）", async () => {
    const { RuntimeStateSchema } = await import("./schema");
    expect(
      RuntimeStateSchema.safeParse({
        status: "running",
        restart_count: 0,
        restarts_recent_10m: 0,
      }).success,
    ).toBe(true);
    const r = RuntimeStateSchema.safeParse({
      status: "running",
      restart_count: 0,
      restarts_recent_10m: 0,
      gpu_percent: 7.5,
      disk_write_bytes_per_sec: 4096,
    });
    expect(r.success).toBe(true);
  });
});

describe("SyncStatusSchema", () => {
  it("解析同步状态", () => {
    const st = {
      enabled: true,
      url: "https://dav.example.com/dav/",
      username: "u",
      remote_dir: "cli-companion",
      sync_interval_minutes: 15,
      password_set: true,
      state: { last_run: null, last_error: null },
    };
    expect(SyncStatusSchema.safeParse(st).success).toBe(true);
  });
});

describe("FtpSettingsSchema（v2.6.0 应用功能·FTP）", () => {
  it("解析完整 FTP 配置（多监听器 + 多目录授权 + 细粒度权限）", () => {
    const ftp = {
      enabled: true,
      passive_port_start: 50000,
      passive_port_end: 50100,
      listeners: [
        { name: "文件分发", port: 21, root: "D:\\ftp", enabled: true },
        { name: "媒体库", port: 2121, root: "D:\\media", enabled: false },
      ],
      users: [
        {
          username: "alice",
          allowed_roots: ["D:\\ftp", "D:\\media"],
          permissions: { list: true, download: true, upload: true, delete: false, rename: false, mkdir: true },
          enabled: true,
        },
        {
          username: "guest",
          allowed_roots: [],
          permissions: { list: true, download: true, upload: false, delete: false, rename: false, mkdir: false },
          enabled: false,
        },
      ],
    };
    const parsed = FtpSettingsSchema.parse(ftp);
    expect(parsed.listeners).toHaveLength(2);
    expect(parsed.users[0].allowed_roots).toHaveLength(2);
    expect(parsed.users[0].permissions.upload).toBe(true);
    expect(parsed.users[1].permissions.upload).toBe(false);
  });

  it("旧版本 app.json 无 ftp 段时 AppConfig 兼容（optional）", () => {
    const oldApp = {
      version: 1,
      general: { language: "zh-CN", theme: "system", close_to_tray: true },
      webdav: {
        enabled: false,
        url: "",
        username: "",
        remote_dir: "cli-companion",
        sync_interval_minutes: 15,
        verify_tls: true,
        sync_config: true,
        sync_cli_apps: false,
      },
    };
    const parsed = AppConfigSchema.parse(oldApp);
    expect(parsed.ftp).toBeUndefined();
    // 新版本带 ftp 段也正常
    const newApp = {
      ...oldApp,
      ftp: {
        enabled: false,
        passive_port_start: 50000,
        passive_port_end: 50100,
        listeners: [],
        users: [],
      },
    };
    expect(AppConfigSchema.parse(newApp).ftp?.enabled).toBe(false);
  });

  it("解析 ftp.status 运行时状态", () => {
    const running = FtpStatusSchema.parse({
      enabled: true,
      running: true,
      ports: [21, 2121],
      passive_port_start: 50000,
      passive_port_end: 50100,
      listeners: 2,
      users: 1,
      sessions: 3,
      local_ip: "192.168.1.10",
      last_error: null,
    });
    expect(running.ports).toEqual([21, 2121]);
    expect(running.sessions).toBe(3);
    // 旧 daemon 无 ports/local_ip 字段也兼容
    const legacy = FtpStatusSchema.parse({
      enabled: false,
      running: false,
      passive_port_start: 0,
      passive_port_end: 0,
      listeners: 0,
      users: 0,
      sessions: 0,
    });
    expect(legacy.ports).toBeUndefined();
  });
});
