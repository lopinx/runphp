/**
 * API 适配层：桌面端走 Tauri invoke，Web 面板走 fetch。
 * 运行模式由构建时注入：VITE_RUNPHP_MODE = "desktop" | "panel"
 */

const mode = import.meta.env.VITE_RUNPHP_MODE ?? "desktop";

/** 判断当前是否运行在 Tauri 桌面壳中。 */
export const isDesktop = mode === "desktop";

/** 调用后端命令。桌面端走 invoke，面板端走 REST。 */
export async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isDesktop) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  const res = await fetch(`/api/${cmd}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args),
  });
  if (!res.ok) {
    throw new Error(await res.text());
  }
  return res.json();
}

// ---- 类型定义（与 Rust 端 serde 序列化对齐） ----

export interface WorkerConfig {
  script: string;
  num: number;
}

export interface Site {
  id: string;
  name: string;
  domains: string[];
  port: number;
  root: string;
  https: boolean;
  worker: WorkerConfig | null;
  runtime_version: string;
  php_ini: string[];
  created_at: string;
  updated_at: string;
}

export interface RuntimeInfo {
  version: string;
  path: string;
  is_default: boolean;
}

// ---- 具体接口 ----

/** 获取数据目录。 */
export const getDataDir = () => call<string>("data_dir");

// 运行时
export const runtimeList = () => call<RuntimeInfo[]>("runtime_list");
export const runtimeInstall = (version: string) =>
  call<string>("runtime_install", { version });
export const runtimeStart = () => call<number>("runtime_start");
export const runtimeStop = () => call<void>("runtime_stop");
export const runtimeReload = () => call<void>("runtime_reload");
export const runtimeStatus = () => call<boolean>("runtime_status");

// 站点
export const siteList = () => call<Site[]>("site_list");
export const siteAdd = (site: Site) => call<void>("site_add", { site });
export const siteUpdate = (site: Site) => call<void>("site_update", { site });
export const siteRemove = (id: string) => call<void>("site_remove", { id });

// 示例（M1 骨架验证用）
export const greet = (name: string) => call<string>("greet", { name });
