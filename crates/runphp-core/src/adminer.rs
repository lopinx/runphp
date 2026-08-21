//! Adminer 集成：下载单文件 PHP 管理工具，构造带预填参数的管理 URL。
//!
//! Adminer 原生支持 MySQL / PostgreSQL / SQLite，通过 GET 参数预填连接信息。
//! MongoDB / Redis / Qdrant 不支持，仅返回 Adminer 首页 URL。
//! 出于安全考虑，密码不写入 URL（避免浏览器历史/日志泄露），仅预填用户名和主机。

use crate::{config::AppConfig, Error, Result};
use serde::{Deserialize, Serialize};

/// Adminer 版本与下载地址。
const ADMINER_VERSION: &str = "4.8.1";
const ADMINER_URL: &str = "https://github.com/vrana/adminer/releases/download/v4.8.1/adminer-4.8.1.php";

/// Adminer 内置站点端口（Caddyfile 中自动生成的 `:8999` 块）。
pub const ADMINER_PORT: u16 = 8999;

/// Adminer 管理请求参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminerParams {
    /// 数据库类型：`sqlite` / `mysql` / `postgres` / `mongodb` / `redis` / `qdrant`。
    pub db_type: String,
    /// SQLite 文件绝对路径（仅 SQLite 使用）。
    pub path: Option<String>,
    /// 远程主机（远程数据库使用）。
    pub host: Option<String>,
    /// 远程端口。
    pub port: Option<u16>,
    /// 数据库用户名。
    pub username: Option<String>,
    /// 数据库密码（不再写入 URL，仅保留字段以兼容旧调用方）。
    pub password: Option<String>,
    /// 默认数据库名。
    pub database: Option<String>,
}

/// 确认 Adminer PHP 文件已下载到数据目录；不存在则从 GitHub 下载。
pub async fn ensure_downloaded(cfg: &AppConfig) -> Result<()> {
    let adminer_dir = cfg.data_dir.join("adminer");
    let adminer_file = adminer_dir.join("adminer.php");
    if adminer_file.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&adminer_dir)?;
    download(&adminer_file).await
}

/// 下载 Adminer PHP 文件。
async fn download(dest: &std::path::Path) -> Result<()> {
    tracing::info!("开始下载 Adminer {ADMINER_VERSION}");
    let client = reqwest::Client::builder()
        .user_agent(concat!("RunPHP/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let resp = client.get(ADMINER_URL).send().await?;
    if !resp.status().is_success() {
        return Err(Error::Runtime(format!(
            "下载 Adminer 失败: HTTP {}",
            resp.status()
        )));
    }
    let bytes = resp.bytes().await?;
    std::fs::write(dest, &bytes)?;
    tracing::info!("Adminer 已下载: {}", dest.display());
    Ok(())
}

/// 根据数据库参数构造 Adminer 管理页面 URL（带预填连接信息）。
///
/// URL 基址为 `http://localhost:{ADMINER_PORT}/adminer.php`。
/// - SQLite: `auth[driver]=sqlite&auth[db]=<path>`
/// - MySQL: `auth[driver]=server&auth[server]=<host>:<port>&username=<user>`
/// - PostgreSQL: 同 MySQL，driver 改为 `pgsql`
/// - 其他类型: 返回不带参数的 Adminer 首页 URL
///
/// 密码不写入 URL（安全考虑），用户需在 Adminer 表单中手动输入。
pub fn build_url(params: &AdminerParams) -> String {
    let base = format!("http://localhost:{ADMINER_PORT}/adminer.php");

    match params.db_type.as_str() {
        "sqlite" => {
            let path = params.path.as_deref().unwrap_or("");
            format!("{base}?auth[driver]=sqlite&auth[db]={}", url_encode(path))
        }
        "mysql" | "postgres" => {
            let driver = if params.db_type == "mysql" {
                "server"
            } else {
                "pgsql"
            };
            let mut query = format!("auth[driver]={driver}");
            if let Some(host) = &params.host {
                let port = params.port.unwrap_or(0);
                if port > 0 {
                    query.push_str(&format!("&auth[server]={host}:{port}"));
                } else {
                    query.push_str(&format!("&auth[server]={host}"));
                }
            }
            if let Some(user) = &params.username {
                if !user.is_empty() {
                    query.push_str(&format!("&username={}", url_encode(user)));
                }
            }
            if let Some(db) = &params.database {
                if !db.is_empty() {
                    query.push_str(&format!("&auth[db]={}", url_encode(db)));
                }
            }
            format!("{base}?{query}")
        }
        _ => base,
    }
}

/// URL 编码（按 UTF-8 字节处理，正确支持多字节字符）。
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_url() {
        let params = AdminerParams {
            db_type: "sqlite".into(),
            path: Some("/tmp/test.db".into()),
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
        };
        let url = build_url(&params);
        assert!(url.contains("auth[driver]=sqlite"));
        assert!(url.contains("auth[db]="));
        assert!(url.contains("tmp"));
        assert!(url.contains("test.db"));
    }

    #[test]
    fn mysql_url() {
        let params = AdminerParams {
            db_type: "mysql".into(),
            path: None,
            host: Some("127.0.0.1".into()),
            port: Some(3306),
            username: Some("root".into()),
            password: Some("secret".into()),
            database: Some("mydb".into()),
        };
        let url = build_url(&params);
        assert!(url.contains("auth[driver]=server"));
        assert!(url.contains("auth[server]=127.0.0.1:3306"));
        assert!(url.contains("username=root"));
        // 密码不再写入 URL
        assert!(!url.contains("password="));
    }

    #[test]
    fn url_encode_multibyte() {
        let encoded = url_encode("用户");
        // 中文"用户"的 UTF-8 字节应被逐字节编码
        assert!(encoded.starts_with("%E7%94%A8%E6%88%B7"));
    }
}
