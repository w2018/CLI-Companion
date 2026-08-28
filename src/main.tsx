// 应用入口：先确保 daemon 在运行（未运行则自动拉起），再渲染界面
import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "react-router-dom";
import { router } from "./app/router";
import "./styles/tokens.css";

// TanStack Query：RPC 快照缓存（轮询 + 失效驱动刷新）
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 1000,
      // 修复①：Tauri 窗口失焦时轮询默认会停止，导致仪表盘数据不刷新；
      // 后台继续轮询 + 窗口聚焦时立即刷新
      refetchIntervalInBackground: true,
      refetchOnWindowFocus: true,
    },
  },
});

/** 启动引导：拉起 daemon（失败不阻塞渲染，界面会显示不可达状态） */
async function bootstrap() {
  try {
    const ready = await invoke<boolean>("ensure_daemon");
    if (!ready) {
      console.warn("daemon 未能就绪（exe 缺失或启动失败）");
    }
  } catch (e) {
    console.warn("ensure_daemon 调用失败:", e);
  }
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </React.StrictMode>,
  );
}

void bootstrap();
