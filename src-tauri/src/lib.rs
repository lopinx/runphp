//! Tauri 2 桌面壳：薄封装 runphp-core，不含业务逻辑。

use runphp_core::{
    adminer,
    caddy,
    db::libsql::{LibsqlManager, LibsqlProfile},
    db::remote::{ConnectionProfile, RemoteDbManager, RemoteQueryResult, RemoteTableInfo},
    db::sqlite::{DatabaseFile, QueryResult, SqliteManager, TableInfo},
    detect::{self, LocalDetection},
    fs,
    hosts::{entries_from_sites, HostEntry, HostsManager},
    runtime::{self, ImportResult}, system, AppConfig, RuntimeManager, Site,
};
use tauri::Emitter;

/// 取应用配置。
fn cfg() -> AppConfig {
    let data_dir = runphp_core::default_data_dir();
    AppConfig::load(&data_dir).unwrap_or_default()
}

/// 返回数据目录路径。
#[tauri::command]
fn data_dir() -> String {
    runphp_core::default_data_dir().to_string_lossy().to_string()
}

/// 列出已安装运行时。
#[tauri::command]
fn runtime_list() -> Vec<RuntimeInfo> {
    let mgr = RuntimeManager::new(cfg());
    mgr.list_installed()
        .into_iter()
        .map(|r| RuntimeInfo {
            version: r.version,
            path: r.path.to_string_lossy().to_string(),
            is_default: r.is_default,
        })
        .collect()
}

/// 异步安装运行时（返回是否成功，进度通过事件推送）。
#[tauri::command]
async fn runtime_install(version: String, app: tauri::AppHandle) -> Result<String, String> {
    let mgr = RuntimeManager::new(cfg());
    let progress_app = std::sync::Arc::new(app);
    let path = mgr
        .install(&version, Some(Box::new(move |d, t| {
            // 通过事件推送进度
            let _ = progress_app.emit("runtime-download-progress", (d, t));
        })))
        .await
        .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// 检测本地 FrankenPHP 二进制与数据库服务。
#[tauri::command]
async fn runtime_detect_local() -> Result<LocalDetection, String> {
    Ok(detect::detect().await)
}

/// 导入本地 FrankenPHP 二进制到托管目录（返回导入结果）。
#[tauri::command]
async fn runtime_import_local(path: String) -> Result<ImportResult, String> {
    let cfg = cfg();
    let mgr = RuntimeManager::new(cfg.clone());
    let result = mgr
        .import(std::path::Path::new(&path))
        .await
        .map_err(|e| e.to_string())?;
    // 首次导入自动设为默认
    if cfg.default_runtime_version.is_empty() {
        let mut new_cfg = cfg;
        new_cfg.default_runtime_version = result.version.clone();
        new_cfg.save().map_err(|e| e.to_string())?;
    }
    Ok(result)
}

/// 拉取 GitHub Releases 发布的可安装版本列表。
#[tauri::command]
async fn runtime_versions() -> Result<Vec<String>, String> {
    runtime::available_versions().await.map_err(|e| e.to_string())
}

/// 浏览本地目录（为 UI 目录选择器提供数据）。
#[tauri::command]
fn fs_browse(path: Option<String>) -> Result<fs::DirListing, String> {
    fs::browse(path.as_deref()).map_err(|e| e.to_string())
}

/// 获取主机系统信息（仪表盘底部状态栏）。
#[tauri::command]
fn system_info() -> system::SystemInfo {
    system::collect()
}

/// 确认 Adminer 已下载并构造带预填参数的管理 URL。
#[tauri::command]
async fn adminer_manage(params: adminer::AdminerParams) -> Result<String, String> {
    let c = cfg();
    adminer::ensure_downloaded(&c).await.map_err(|e| e.to_string())?;
    Ok(adminer::build_url(&params))
}

/// 列出全部站点。
#[tauri::command]
fn site_list() -> Vec<Site> {
    let cfg = cfg();
    cfg.load_sites().map(|r| r.sites).unwrap_or_default()
}

/// 新增站点并写 Caddyfile（运行中时自动热重载）。
#[tauri::command]
async fn site_add(site: Site) -> Result<(), String> {
    let cfg = cfg();
    let mut reg = cfg.load_sites().map_err(|e| e.to_string())?;
    reg.validate(&site, None).map_err(|e| e.to_string())?;
    reg.add(site);
    cfg.save_sites(&reg).map_err(|e| e.to_string())?;
    let binary = RuntimeManager::new(cfg.clone()).resolve(None).map_err(|e| e.to_string())?;
    caddy::write_and_reload(&cfg, &reg.sites, &binary.path).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 更新站点。
#[tauri::command]
async fn site_update(mut site: Site) -> Result<(), String> {
    let cfg = cfg();
    let mut reg = cfg.load_sites().map_err(|e| e.to_string())?;
    let created_at = reg
        .get(&site.id)
        .map(|s| s.created_at.clone())
        .ok_or("站点不存在".to_string())?;
    reg.validate(&site, Some(&site.id)).map_err(|e| e.to_string())?;
    site.created_at = created_at;
    site.touch();
    if let Some(existing) = reg.get_mut(&site.id) {
        *existing = site;
    }
    cfg.save_sites(&reg).map_err(|e| e.to_string())?;
    let binary = RuntimeManager::new(cfg.clone()).resolve(None).map_err(|e| e.to_string())?;
    caddy::write_and_reload(&cfg, &reg.sites, &binary.path).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 删除站点。
#[tauri::command]
async fn site_remove(id: String) -> Result<(), String> {
    let cfg = cfg();
    let mut reg = cfg.load_sites().map_err(|e| e.to_string())?;
    reg.remove(&id).ok_or("站点不存在".to_string())?;
    cfg.save_sites(&reg).map_err(|e| e.to_string())?;
    let binary = RuntimeManager::new(cfg.clone()).resolve(None).map_err(|e| e.to_string())?;
    caddy::write_and_reload(&cfg, &reg.sites, &binary.path).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 启动 FrankenPHP（带崩溃自动重启监控）。
#[tauri::command]
async fn runtime_start() -> Result<u32, String> {
    let cfg = cfg();
    let mgr = RuntimeManager::new(cfg.clone());
    let rt = mgr.resolve(None).map_err(|e| e.to_string())?;
    let info = caddy::start(&cfg, &rt.path).await.map_err(|e| e.to_string())?;
    Ok(info.pid)
}

/// 停止运行时。
#[tauri::command]
async fn runtime_stop() -> Result<(), String> {
    let cfg = cfg();
    caddy::stop(&cfg).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 热重载配置。
#[tauri::command]
async fn runtime_reload() -> Result<(), String> {
    let cfg = cfg();
    let mgr = RuntimeManager::new(cfg.clone());
    let rt = mgr.resolve(None).map_err(|e| e.to_string())?;
    caddy::reload(&cfg, &rt.path).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 运行状态。
#[tauri::command]
async fn runtime_status() -> bool {
    caddy::status().await
}

/// 设置默认运行时版本。
#[tauri::command]
fn runtime_set_default(version: String) -> Result<(), String> {
    let mut cfg = cfg();
    let mgr = RuntimeManager::new(cfg.clone());
    if !mgr.list_installed().iter().any(|r| r.version == version) {
        return Err(format!("运行时 {version} 未安装"));
    }
    cfg.default_runtime_version = version;
    cfg.save().map_err(|e| e.to_string())
}

/// 读取运行时日志末尾若干行。
#[tauri::command]
fn logs_read(lines: Option<usize>) -> Result<String, String> {
    let cfg = cfg();
    caddy::read_log(&cfg, lines.unwrap_or(200)).map_err(|e| e.to_string())
}

// ---- Hosts 管理 ----

/// 列出受管区块内的 hosts 条目。
#[tauri::command]
fn hosts_list() -> Result<Vec<HostEntry>, String> {
    let hm = HostsManager::system();
    hm.list_managed().map_err(|e| e.to_string())
}

/// 检测 hosts 是否可直接写入。
#[tauri::command]
fn hosts_writable() -> bool {
    HostsManager::system().check_writable()
}

/// 同步全部站点域名到 hosts。
#[tauri::command]
fn hosts_sync() -> Result<usize, String> {
    let cfg = cfg();
    let reg = cfg.load_sites().map_err(|e| e.to_string())?;
    let entries = entries_from_sites(&reg.sites);
    let count = entries.len();
    let hm = HostsManager::system();
    hm.sync(&entries).map_err(|e| e.to_string())?;
    Ok(count)
}

/// 显示 hosts 全文（只读查看）。
#[tauri::command]
fn hosts_content() -> Result<String, String> {
    let hm = HostsManager::system();
    hm.read().map_err(|e| e.to_string())
}

/// 获取提权命令。
#[tauri::command]
fn hosts_elevation() -> String {
    HostsManager::system().elevation_command("sync")
}

// ---- 数据库管理 ----

/// SQLite 管理器实例。
fn sqlite_mgr() -> SqliteManager {
    SqliteManager::new(cfg().data_dir.join("databases"))
}

/// 列出 SQLite 数据库文件。
#[tauri::command]
fn db_sqlite_list() -> Result<Vec<DatabaseFile>, String> {
    sqlite_mgr().list_databases().map_err(|e| e.to_string())
}

/// 创建 SQLite 数据库。
#[tauri::command]
fn db_sqlite_create(name: String) -> Result<String, String> {
    let path = sqlite_mgr().create_database(&name).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// 删除 SQLite 数据库。
#[tauri::command]
fn db_sqlite_delete(name: String) -> Result<(), String> {
    sqlite_mgr().delete_database(&name).map_err(|e| e.to_string())
}

/// 列出表。
#[tauri::command]
fn db_sqlite_tables(name: String) -> Result<Vec<TableInfo>, String> {
    sqlite_mgr().list_tables(&name).map_err(|e| e.to_string())
}

/// 查询表数据。
#[tauri::command]
fn db_sqlite_query_table(name: String, table: String, limit: Option<i64>, offset: Option<i64>) -> Result<QueryResult, String> {
    let lim = limit.unwrap_or(100);
    let off = offset.unwrap_or(0);
    sqlite_mgr()
        .query_table(&name, &table, lim, off)
        .map_err(|e| e.to_string())
}

/// 执行 SQL。
#[tauri::command]
fn db_sqlite_execute(name: String, sql: String) -> Result<QueryResult, String> {
    sqlite_mgr().execute(&name, &sql).map_err(|e| e.to_string())
}

/// 列出远程数据库连接档案。
#[tauri::command]
fn db_remote_list() -> Result<Vec<ConnectionProfile>, String> {
    let mgr = RemoteDbManager::new(&cfg().data_dir);
    mgr.load().map(|r| r.profiles).map_err(|e| e.to_string())
}

/// 添加远程连接档案。
#[tauri::command]
fn db_remote_add(profile: ConnectionProfile) -> Result<(), String> {
    let mgr = RemoteDbManager::new(&cfg().data_dir);
    mgr.add(profile).map_err(|e| e.to_string())
}

/// 删除远程连接档案。
#[tauri::command]
fn db_remote_remove(id: String) -> Result<(), String> {
    let mgr = RemoteDbManager::new(&cfg().data_dir);
    mgr.remove(&id).map_err(|e| e.to_string())
}

/// 测试远程连接。
#[tauri::command]
async fn db_remote_test(profile: ConnectionProfile) -> Result<String, String> {
    RemoteDbManager::test_connection(&profile)
        .await
        .map_err(|e| e.to_string())
}

/// 列出远程数据库的表。
#[tauri::command]
async fn db_remote_tables(profile: ConnectionProfile) -> Result<Vec<RemoteTableInfo>, String> {
    RemoteDbManager::list_tables(&profile)
        .await
        .map_err(|e| e.to_string())
}

/// 查询远程表数据。
#[tauri::command]
async fn db_remote_query_table(
    profile: ConnectionProfile,
    table: String,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<RemoteQueryResult, String> {
    let lim = limit.unwrap_or(100);
    let off = offset.unwrap_or(0);
    RemoteDbManager::query_table(&profile, &table, lim, off)
        .await
        .map_err(|e| e.to_string())
}

/// 执行远程 SQL。
#[tauri::command]
async fn db_remote_execute(profile: ConnectionProfile, sql: String) -> Result<RemoteQueryResult, String> {
    RemoteDbManager::execute(&profile, &sql)
        .await
        .map_err(|e| e.to_string())
}

/// 列出 libSQL 连接档案。
#[tauri::command]
fn db_libsql_list() -> Result<Vec<LibsqlProfile>, String> {
    LibsqlManager::new(&cfg().data_dir)
        .list_profiles()
        .map_err(|e| e.to_string())
}

/// 添加 libSQL 连接档案。
#[tauri::command]
fn db_libsql_add(profile: LibsqlProfile) -> Result<(), String> {
    LibsqlManager::new(&cfg().data_dir)
        .add_profile(profile)
        .map_err(|e| e.to_string())
}

/// 删除 libSQL 连接档案。
#[tauri::command]
fn db_libsql_remove(id: String) -> Result<(), String> {
    LibsqlManager::new(&cfg().data_dir)
        .remove_profile(&id)
        .map_err(|e| e.to_string())
}

/// 测试 libSQL 连接。
#[tauri::command]
async fn db_libsql_test(profile: LibsqlProfile) -> Result<String, String> {
    LibsqlManager::test_connection(&profile)
        .await
        .map_err(|e| e.to_string())
}

/// 列出 libSQL 表。
#[tauri::command]
async fn db_libsql_tables(profile: LibsqlProfile) -> Result<Vec<TableInfo>, String> {
    LibsqlManager::list_tables(&profile)
        .await
        .map_err(|e| e.to_string())
}

/// 查询 libSQL 表数据。
#[tauri::command]
async fn db_libsql_query_table(
    profile: LibsqlProfile,
    table: String,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<QueryResult, String> {
    let lim = limit.unwrap_or(100);
    let off = offset.unwrap_or(0);
    LibsqlManager::query_table(&profile, &table, lim, off)
        .await
        .map_err(|e| e.to_string())
}

/// 执行 libSQL SQL。
#[tauri::command]
async fn db_libsql_execute(profile: LibsqlProfile, sql: String) -> Result<QueryResult, String> {
    LibsqlManager::execute(&profile, &sql)
        .await
        .map_err(|e| e.to_string())
}

// 给前端的简化结构（路径转字符串）。
#[derive(serde::Serialize)]
struct RuntimeInfo {
    version: String,
    path: String,
    is_default: bool,
}

/// 库入口：由桌面二进制与移动端共用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            data_dir,
            runtime_list,
            runtime_install,
            runtime_detect_local,
            runtime_import_local,
            runtime_versions,
            fs_browse,
            system_info,
            adminer_manage,
            runtime_start,
            runtime_stop,
            runtime_reload,
            runtime_status,
            runtime_set_default,
            logs_read,
            site_list,
            site_add,
            site_update,
            site_remove,
            hosts_list,
            hosts_writable,
            hosts_sync,
            hosts_content,
            hosts_elevation,
            db_sqlite_list,
            db_sqlite_create,
            db_sqlite_delete,
            db_sqlite_tables,
            db_sqlite_query_table,
            db_sqlite_execute,
            db_remote_list,
            db_remote_add,
            db_remote_remove,
            db_remote_test,
            db_remote_tables,
            db_remote_query_table,
            db_remote_execute,
            db_libsql_list,
            db_libsql_add,
            db_libsql_remove,
            db_libsql_test,
            db_libsql_tables,
            db_libsql_query_table,
            db_libsql_execute,
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用时出错");
}
