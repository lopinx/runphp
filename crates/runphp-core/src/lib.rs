//! RunPHP 核心库：全部业务逻辑，无 UI 依赖。
//!
//! 桌面端（Tauri）、命令行（runphp-cli）、Web 面板三端共用本库。

pub mod config;
pub mod error;

pub use config::AppConfig;
pub use error::Error;

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
