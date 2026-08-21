//! 本地环境检测：扫描 PATH 与常见安装位置中已有的 FrankenPHP 二进制，探测常见数据库服务端口。
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
    /// PATH 与常见安装位置中检测到的 FrankenPHP 二进制。
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

/// 扫描 PATH 与常见安装位置中的 FrankenPHP 二进制。
fn scan_frankenphp() -> Vec<DetectedBinary> {
    let mut dirs: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    dirs.extend(common_install_dirs());
    // 同时扫描当前工作目录（用户经常把工具放在项目根目录）
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    // 对每个常见安装目录，再向下搜索一层子目录（便携版常放在版本子目录中）
    let mut sub_dirs = Vec::new();
    for d in &dirs {
        if let Ok(entries) = std::fs::read_dir(d) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    sub_dirs.push(p);
                }
            }
        }
    }
    dirs.extend(sub_dirs);
    scan_dirs(&dirs)
}

/// PATH 之外的常见安装位置。
///
/// Windows 用户常将官方 zip 解压到盘符根目录（如 `D:\FrankenPHP`）或
/// `Tools` 目录，这些位置通常不在 PATH 中。
fn common_install_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        if let Some(home) = std::env::var_os("USERPROFILE") {
            dirs.push(PathBuf::from(home).join("FrankenPHP"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            dirs.push(PathBuf::from(pf).join("FrankenPHP"));
        }
        for drive in 'C'..='Z' {
            dirs.push(PathBuf::from(format!("{drive}:\\FrankenPHP")));
            dirs.push(PathBuf::from(format!("{drive}:\\Tools\\FrankenPHP")));
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            dirs.push(home.join(".local").join("bin"));
            dirs.push(home.join("frankenphp"));
        }
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/opt/frankenphp"));
    }
    dirs
}

/// 在给定目录列表中查找 FrankenPHP 二进制（结果按路径去重）。
fn scan_dirs(dirs: &[PathBuf]) -> Vec<DetectedBinary> {
    // Windows 下同时尝试有无 .exe 后缀（部分便携包会去掉后缀）
    #[cfg(windows)]
    let names = ["frankenphp.exe", "frankenphp"];
    #[cfg(not(windows))]
    let names = ["frankenphp"];

    let mut found = Vec::new();
    for dir in dirs {
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
    // 按路径去重（PATH 与常见位置可能存在重复目录）
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 扫描目录能发现二进制并去重() {
        let tmp = std::env::temp_dir().join(format!("runphp-detect-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        #[cfg(windows)]
        let bin_name = "frankenphp.exe";
        #[cfg(not(windows))]
        let bin_name = "frankenphp";
        std::fs::write(tmp.join(bin_name), b"fake").unwrap();

        let found = scan_dirs(&[tmp.clone(), tmp.clone()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, tmp.join(bin_name));

        std::fs::remove_dir_all(&tmp).ok();
    }
}
