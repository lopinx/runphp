//! 本地环境检测：扫描 PATH 中已有的 FrankenPHP 二进制，探测常见数据库服务端口。
//!
//! 检测结果用于「运行时管理」页面：本地已有 FrankenPHP 时可一键导入，
//! 无需重新下载；检测到数据库服务时以链接形式引导用户添加连接。

use crate::db::remote::DbDriver;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

/// 系统中检测到的 FrankenPHP 二进制。
#[derive(Debug, Clone, Serialize)]
pub struct DetectedBinary {
    /// 文件名（如 `frankenphp.exe`）。
    pub name: String,
    /// 绝对路径。
    pub path: PathBuf,
}

/// 系统中检测到的数据库服务。
#[derive(Debug, Clone, Serialize)]
pub struct DetectedService {
    /// 数据库类型。
    pub driver: DbDriver,
    /// 显示名称（如 `MySQL`）。
    pub name: String,
    /// 主机。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 端口是否可达（服务是否运行中）。
    pub running: bool,
}

/// 本地环境检测结果。
#[derive(Debug, Clone, Serialize)]
pub struct LocalDetection {
    /// PATH 中检测到的 FrankenPHP 二进制。
    pub frankenphp: Vec<DetectedBinary>,
    /// 数据库服务探测结果。
    pub services: Vec<DetectedService>,
}

/// 内置探测的数据库服务清单：(显示名称, 默认端口, 类型)。
const SERVICE_PROBES: &[(&str, u16, DbDriver)] = &[
    ("MySQL", 3306, DbDriver::Mysql),
    ("PostgreSQL", 5432, DbDriver::Postgres),
    ("MongoDB", 27017, DbDriver::Mongodb),
    ("Redis", 6379, DbDriver::Redis),
    ("Qdrant", 6333, DbDriver::Qdrant),
];

/// 执行本地环境检测（PATH 扫描 + 数据库端口探测）。
pub async fn detect() -> LocalDetection {
    LocalDetection {
        frankenphp: scan_frankenphp(),
        services: probe_services().await,
    }
}

/// 扫描 PATH 环境变量中的 FrankenPHP 二进制。
fn scan_frankenphp() -> Vec<DetectedBinary> {
    #[cfg(windows)]
    let names = ["frankenphp.exe"];
    #[cfg(not(windows))]
    let names = ["frankenphp"];

    let mut found = Vec::new();
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    found.push(DetectedBinary {
                        name: name.to_string(),
                        path: candidate,
                    });
                }
            }
        }
    }
    // 按路径去重（PATH 中可能存在重复目录）
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

/// 探测本机默认端口上的数据库服务（各 500ms 超时，并行执行）。
async fn probe_services() -> Vec<DetectedService> {
    let probes: Vec<_> = SERVICE_PROBES
        .iter()
        .map(|(name, port, driver)| async move {
            let addr = format!("127.0.0.1:{port}");
            let running = tokio::time::timeout(
                Duration::from_millis(500),
                tokio::net::TcpStream::connect(&addr),
            )
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false);
            DetectedService {
                driver: *driver,
                name: (*name).to_string(),
                host: "127.0.0.1".to_string(),
                port: *port,
                running,
            }
        })
        .collect();
    futures_util::future::join_all(probes).await
}

</file_content>