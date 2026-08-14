//! Tauri 2 桌面壳：薄封装 runphp-core，不含业务逻辑。

use runphp_core::{
    caddy, hosts::{entries_from_sites, HostEntry, HostsManager},
    AppConfig, RuntimeManager, Site,
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

/// 列出全部站点。
#[tauri::command]
fn site_list() -> Vec<Site> {
    let cfg = cfg();
    cfg.load_sites().map(|r| r.sites).unwrap_or_default()
}

/// 新增站点并写 Caddyfile。
#[tauri::command]
fn site_add(mut site: Site) -> Result<(), String> {
    let cfg = cfg();
    let mut reg = cfg.load_sites().map_err(|e| e.to_string())?;
    reg.validate(&site, None).map_err(|e| e.to_string())?;
    site.touch();
    reg.add(site);
    cfg.save_sites(&reg).map_err(|e| e.to_string())?;
    caddy::write_caddyfile(&cfg, &reg.sites).map_err(|e| e.to_string())?;
    Ok(())
}

/// 更新站点。
#[tauri::command]
fn site_update(mut site: Site) -> Result<(), String> {
    let cfg = cfg();
    let mut reg = cfg.load_sites().map_err(|e| e.to_string())?;
    // 先取出原站点信息（避免借用冲突）
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
    caddy::write_caddyfile(&cfg, &reg.sites).map_err(|e| e.to_string())?;
    Ok(())
}

/// 删除站点。
#[tauri::command]
fn site_remove(id: String) -> Result<(), String> {
    let cfg = cfg();
    let mut reg = cfg.load_sites().map_err(|e| e.to_string())?;
    reg.remove(&id).ok_or("站点不存在".to_string())?;
    cfg.save_sites(&reg).map_err(|e| e.to_string())?;
    caddy::write_caddyfile(&cfg, &reg.sites).map_err(|e| e.to_string())?;
    Ok(())
}

/// 启动 FrankenPHP。
#[tauri::command]
async fn runtime_start() -> Result<u32, String> {
    let cfg = cfg();
    let mgr = RuntimeManager::new(cfg.clone());
    let rt = mgr.resolve(None).map_err(|e| e.to_string())?;
    let (info, child) = caddy::start(&cfg, &rt.path).await.map_err(|e| e.to_string())?;
    // 桌面端：后台等待子进程，不阻塞命令
    tokio::spawn(async move {
        let mut child = child;
        let _ = child.wait().await;
    });
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
            runtime_start,
            runtime_stop,
            runtime_reload,
            runtime_status,
            site_list,
            site_add,
            site_update,
            site_remove,
            hosts_list,
            hosts_writable,
            hosts_sync,
            hosts_content,
            hosts_elevation,
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用时出错");
}
