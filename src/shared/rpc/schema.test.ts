// schema 契约测试：确保 Zod 定义与 daemon 实际响应格式一致
import { describe, expect, it } from "vitest";
import { ServiceRowSchema, SyncStatusSchema, AppConfigSchema } from "./schema";
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
      },
    };
    expect(AppConfigSchema.safeParse(app).success).toBe(true);
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
