//! RunPHP 核心库：全部业务逻辑，无 UI 依赖。
//!
//! 桌面端（Tauri）、命令行（runphp-cli）、Web 面板三端共用本库。

pub mod caddy;
pub mod adminer;
pub mod config;
pub mod db;
pub mod detect;
pub mod error;
pub mod fs;
pub mod ftp;
pub mod ftpd;
pub mod hosts;
pub mod runtime;
pub mod services;
pub mod site;
pub mod system;

pub use config::AppConfig;
pub use error::Error;
pub use hosts::{HostEntry, HostsManager};
pub use runtime::RuntimeManager;
pub use site::{Site, SiteRegistry};

/// 核心库统一结果类型。
pub type Result<T> = std::result::Result<T, Error>;

/// 默认数据目录：Windows 为 `%APPDATA%\RunPHP`，Linux 为 `~/.local/share/runphp`。
pub fn default_data_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        dirs::config_dir()
            .map(|p| p.join("RunPHP"))
            .unwrap_or_else(|| std::path::PathBuf::from(".data"))
    } else {
        dirs::data_dir()
            .map(|p| p.join("runphp"))
            .unwrap_or_else(|| std::path::PathBuf::from(".data"))
    }
}

/// 启动全部标记为自启的服务（数据库服务 + FTP 服务端）。
///
/// 供桌面端与面板端启动时调用；单项失败仅记录日志不中断其余服务。
pub async fn autostart_services(cfg: &AppConfig) {
    let db = crate::db::service::DbServiceManager::new(cfg.clone());
    if let Ok(list) = db.list() {
        for svc in list.into_iter().filter(|s| s.autostart) {
            if let Err(e) = db.start(&svc.id).await {
                tracing::warn!("自启动服务 {} 失败: {e}", svc.name);
            }
        }
    }
    let ftpd = crate::ftpd::FtpdManager::new(cfg.clone());
    if ftpd.config().autostart {
        if let Err(e) = ftpd.start().await {
            tracing::warn!("自启动 FTP 服务失败: {e}");
        }
    }
}
