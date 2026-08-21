/**
 * API 适配层：桌面端走 Tauri invoke，Web 面板走 fetch。
 * 运行模式通过运行时检测自动判断：Tauri 2 会在 WebView 中注入
 * window.__TAURI_INTERNALS__，浏览器面板中没有该对象。
 * 这样同一份前端构建产物可在桌面壳与 Web 面板两端运行。
 */

/** 判断当前是否运行在 Tauri 桌面壳中。 */
export const isDesktop =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** 调用后端命令。桌面端走 invoke，面板端走 REST。 */
export async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isDesktop) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  const token = panelToken();
  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }
  const res = await fetch(`/api/${cmd}`, {
    method: "POST",
    headers,
    body: JSON.stringify(args),
  });
  if (res.status === 401) {
    throw new Error("未授权：请在 URL 中携带 ?token=<面板令牌>");
  }
  if (!res.ok) {
    throw new Error(await res.text());
  }
  return res.json();
}

/**
 * 面板鉴权 token：优先取 URL 参数 ?token=xxx（自动存入 localStorage），
 * 之后同域访问直接读 localStorage。
 */
export function panelToken(): string | null {
  if (typeof window === "undefined") return null;
  try {
    const fromUrl = new URLSearchParams(window.location.search).get("token");
    if (fromUrl) {
      window.localStorage.setItem("runphp_token", fromUrl);
      return fromUrl;
    }
    return window.localStorage.getItem("runphp_token");
  } catch {
    return null;
  }
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
  php_ini: string[];
  created_at: string;
  updated_at: string;
}

export interface RuntimeInfo {
  version: string;
  path: string;
  is_default: boolean;
}

// 本地环境检测结果
export interface DetectedBinary {
  name: string;
  path: string;
}
export interface DetectedService {
  driver: DbDriver;
  name: string;
  host: string;
  port: number;
  running: boolean;
}
export interface LocalDetection {
  frankenphp: DetectedBinary[];
  services: DetectedService[];
}
export interface ImportResult {
  path: string;
  version: string;
}

// ---- 具体接口 ----

/** 获取数据目录。 */
export const getDataDir = () => call<string>("data_dir");

// 运行时
export const runtimeList = () => call<RuntimeInfo[]>("runtime_list");
export const runtimeInstall = (version: string) =>
  call<string>("runtime_install", { version });
export const runtimeVersions = () => call<string[]>("runtime_versions");
export const runtimeStart = () => call<number>("runtime_start");
export const runtimeStop = () => call<void>("runtime_stop");
export const runtimeReload = () => call<void>("runtime_reload");
export const runtimeStatus = () => call<boolean>("runtime_status");
export const runtimeSetDefault = (version: string) =>
  call<void>("runtime_set_default", { version });
export const logsRead = (lines?: number) =>
  call<string>("logs_read", { lines: lines ?? 200 });

// 站点
export const siteList = () => call<Site[]>("site_list");
export const siteAdd = (site: Site) => call<void>("site_add", { site });
export const siteUpdate = (site: Site) => call<void>("site_update", { site });
export const siteRemove = (id: string) => call<void>("site_remove", { id });

// Hosts
export interface HostEntry {
  ip: string;
  host: string;
  comment: string | null;
}
export const hostsList = () => call<HostEntry[]>("hosts_list");
export const hostsWritable = () => call<boolean>("hosts_writable");
export const hostsSync = () => call<number>("hosts_sync");
export const hostsContent = () => call<string>("hosts_content");
export const hostsElevation = () => call<string>("hosts_elevation");

// 数据库
export interface DatabaseFile {
  name: string;
  size: number;
}
export interface TableInfo {
  name: string;
  column_count: number;
  row_count: number;
}
export interface QueryResult {
  columns: string[];
  rows: any[][];
  affected: number;
}
export type DbDriver = "mysql" | "postgres" | "mongodb" | "redis" | "qdrant";
export type SslMode = "disabled" | "preferred" | "required";
export interface ConnectionProfile {
  id: string;
  name: string;
  driver: DbDriver;
  host: string;
  port: number;
  username: string;
  password: string;
  database: string | null;
  created_at: string;
  ssl_mode?: SslMode | null;
  ssl_ca?: string | null;
  ssh_host?: string | null;
  ssh_port?: number | null;
  ssh_user?: string | null;
  ssh_key?: string | null;
  ssh_password?: string | null;
}

export const dbSqliteList = () => call<DatabaseFile[]>("db_sqlite_list");
export const dbSqliteCreate = (name: string) =>
  call<string>("db_sqlite_create", { name });
export const dbSqliteDelete = (name: string) =>
  call<void>("db_sqlite_delete", { name });
export const dbSqliteTables = (name: string) =>
  call<TableInfo[]>("db_sqlite_tables", { name });
export const dbSqliteQueryTable = (
  name: string,
  table: string,
  limit?: number,
  offset?: number,
) =>
  call<QueryResult>("db_sqlite_query_table", { name, table, limit, offset });
export const dbSqliteExecute = (name: string, sql: string) =>
  call<QueryResult>("db_sqlite_execute", { name, sql });

export const dbRemoteList = () => call<ConnectionProfile[]>("db_remote_list");
export const dbRemoteAdd = (profile: ConnectionProfile) =>
  call<void>("db_remote_add", { profile });
export const dbRemoteRemove = (id: string) =>
  call<void>("db_remote_remove", { id });
export const dbRemoteTest = (profile: ConnectionProfile) =>
  call<string>("db_remote_test", { profile });
export const dbRemoteTables = (profile: ConnectionProfile) =>
  call<TableInfo[]>("db_remote_tables", { profile });
export const dbRemoteQueryTable = (
  profile: ConnectionProfile,
  table: string,
  limit?: number,
  offset?: number,
) =>
  call<QueryResult>("db_remote_query_table", { profile, table, limit, offset });
export const dbRemoteExecute = (profile: ConnectionProfile, sql: string) =>
  call<QueryResult>("db_remote_execute", { profile, sql });

// libSQL 数据库（本地文件 / 远程连接 / 嵌入式副本）
export type LibsqlMode = "local" | "remote" | "replica";
export interface LibsqlProfile {
  id: string;
  name: string;
  mode: LibsqlMode;
  path: string | null;
  url: string | null;
  auth_token: string | null;
  created_at: string;
}
export const dbLibsqlList = () => call<LibsqlProfile[]>("db_libsql_list");
export const dbLibsqlAdd = (profile: LibsqlProfile) =>
  call<void>("db_libsql_add", { profile });
export const dbLibsqlRemove = (id: string) =>
  call<void>("db_libsql_remove", { id });
export const dbLibsqlTest = (profile: LibsqlProfile) =>
  call<string>("db_libsql_test", { profile });
export const dbLibsqlTables = (profile: LibsqlProfile) =>
  call<TableInfo[]>("db_libsql_tables", { profile });
export const dbLibsqlQueryTable = (
  profile: LibsqlProfile,
  table: string,
  limit?: number,
  offset?: number,
) =>
  call<QueryResult>("db_libsql_query_table", { profile, table, limit, offset });
export const dbLibsqlExecute = (profile: LibsqlProfile, sql: string) =>
  call<QueryResult>("db_libsql_execute", { profile, sql });

// 本地环境检测（类型定义见上方 DetectedBinary / DetectedService / LocalDetection / ImportResult）
export const runtimeDetectLocal = () =>
  call<LocalDetection>("runtime_detect_local");
export const runtimeImportLocal = (path: string) =>
  call<ImportResult>("runtime_import_local", { path });

// 文件系统浏览（目录选择器）
export interface DirEntry {
  name: string;
  path: string;
}
export interface DirListing {
  current: string;
  parent: string | null;
  dirs: DirEntry[];
}
export const fsBrowse = (path?: string) =>
  call<DirListing>("fs_browse", { path: path ?? null });

// 主机系统信息（仪表盘底部状态栏）
export interface SystemInfo {
  cpu_arch: string;
  memory_total: number;
  disk_total: number;
  disk_free: number;
  os: string;
}
export const systemInfo = () => call<SystemInfo>("system_info");

// Adminer 数据库管理
export interface AdminerParams {
  db_type: string;
  path?: string | null;
  host?: string | null;
  port?: number | null;
  username?: string | null;
  password?: string | null;
  database?: string | null;
}
export const adminerManage = (params: AdminerParams) =>
  call<string>("adminer_manage", { ...params });
