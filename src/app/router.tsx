// 路由定义（开发文档 §7.1）
import { createBrowserRouter } from "react-router-dom";
import { App } from "../App";
import { Dashboard } from "../features/dashboard/Dashboard";
import { ServiceList } from "../features/services/ServiceList";
import { LogViewer } from "../features/logs/LogViewer";
import { DaemonLogPage } from "../features/logs/DaemonLogPage";
import { SettingsPage } from "../features/settings/SettingsPage";
import { AboutPage } from "../features/about/AboutPage";
import { AppsPage } from "../features/apps/AppsPage";
import { EmbeddedTerminal } from "../features/terminal/EmbeddedTerminal";

export const router = createBrowserRouter([
  {
    path: "/",
    element: <App />,
    children: [
      { index: true, element: <Dashboard /> },
      { path: "services", element: <ServiceList /> },
      { path: "apps", element: <AppsPage /> },
      { path: "logs/:serviceId", element: <LogViewer /> },
      { path: "daemon-log", element: <DaemonLogPage /> },
      { path: "settings", element: <SettingsPage /> },
      { path: "about", element: <AboutPage /> },
      { path: "terminal/:serviceId", element: <EmbeddedTerminal /> },
    ],
  },
]);
