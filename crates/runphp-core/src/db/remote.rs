//! 远程数据库连接档案管理（MySQL/MariaDB、PostgreSQL）。
//!
//! 仅连接管理已有实例，不捆绑服务器。
//! 连接档案（含密码）以 JSON 存储于数据目录。

use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// MySQL 异步 trait 需要引入 prelude。
use mysql_async::prelude::*;

/// 数据库类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DbDriver {
    Mysql,
    Postgres,
    Mongodb,
    Redis,
    Qdrant,
}

/// 连接档案。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionProfile {
    /// 唯一标识。
    pub id: String,
    /// 显示名称。
    pub name: String,
    /// 数据库类型。
    pub driver: DbDriver,
    /// 主机。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 用户名。
    pub username: String,
    /// 密码（明文存储，本地工具用途）。
    pub password: String,
    /// 默认数据库（可选）。
    pub database: Option<String>,
    /// 创建时间。
    pub created_at: String,
}

impl ConnectionProfile {
    pub fn new(name: String, driver: DbDriver, host: String, port: u16) -> Self {
        let default_port = match driver {
            DbDriver::Mysql => 3306,
            DbDriver::Postgres => 5432,
            DbDriver::Mongodb => 27017,
            DbDriver::Redis => 6379,
            DbDriver::Qdrant => 6333,
        };
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            driver,
            host,
            port: if port > 0 { port } else { default_port },
            username: String::new(),
            password: String::new(),
            database: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// 生成 MySQL DSN（mysql_async 格式）。
    fn mysql_dsn(&self) -> String {
        let db = self.database.as_deref().unwrap_or("");
        format!(
            "mysql://{}:{}@{}:{}/{}",
            url_escape(&self.username),
            url_escape(&self.password),
            self.host,
            self.port,
            db
        )
    }

    /// 生成 PostgreSQL 连接参数。
    fn pg_config(&self) -> tokio_postgres::Config {
        let mut config = tokio_postgres::Config::new();
        config
            .user(&self.username)
            .password(&self.password)
            .host(&self.host)
            .port(self.port);
        if let Some(db) = &self.database {
            config.dbname(db);
        }
        config
    }
}

/// 简单的 URL 转义（正确处理 UTF-8 多字节字符）。
fn url_escape(s: &str) -> String {
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

/// 将参数列表格式化为 Redis RESP 协议命令字符串。
///
/// 例如 `["PING"]` → `*1\r\n$4\r\nPING\r\n`
fn format_resp_command(args: &[&str]) -> String {
    let mut out = format!("*{}\r\n", args.len());
    for arg in args {
        out.push_str(&format!("${}\r\n{}\r\n", arg.len(), arg));
    }
    out
}

/// 连接档案集合。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileRegistry {
    pub profiles: Vec<ConnectionProfile>,
}

/// 远程数据库档案管理器。
pub struct RemoteDbManager {
    path: PathBuf,
}

impl RemoteDbManager {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            path: data_dir.join("db_profiles.json"),
        }
    }

    pub fn load(&self) -> Result<ProfileRegistry, Error> {
        if !self.path.exists() {
            return Ok(ProfileRegistry::default());
        }
        let raw = std::fs::read_to_string(&self.path).map_err(Error::Io)?;
        serde_json::from_str(&raw).map_err(Error::Json)
    }

    pub fn save(&self, reg: &ProfileRegistry) -> Result<(), Error> {
        let raw = serde_json::to_string_pretty(reg).map_err(Error::Json)?;
        std::fs::write(&self.path, raw).map_err(Error::Io)?;
        Ok(())
    }

    pub fn add(&self, profile: ConnectionProfile) -> Result<(), Error> {
        let mut reg = self.load()?;
        reg.profiles.push(profile);
        self.save(&reg)
    }

    pub fn remove(&self, id: &str) -> Result<(), Error> {
        let mut reg = self.load()?;
        reg.profiles.retain(|p| p.id != id);
        self.save(&reg)
    }

    /// 测试连接。
    pub async fn test_connection(profile: &ConnectionProfile) -> Result<String, Error> {
        match profile.driver {
            DbDriver::Mysql => {
                let opts = mysql_async::Opts::from_url(&profile.mysql_dsn())
                    .map_err(|e| Error::Other(format!("MySQL 连接参数错误: {e}")))?;
                let pool = mysql_async::Pool::new(opts);
                // 使用 defer 模式确保 pool 在任何路径下都被 disconnect
                let result = async {
                    let mut conn = pool
                        .get_conn()
                        .await
                        .map_err(|e| Error::Other(format!("MySQL 连接失败: {e}")))?;
                    let rows: Vec<(i32,)> = conn
                        .query("SELECT 1")
                        .await
                        .map_err(|e| Error::Other(format!("MySQL 查询失败: {e}")))?;
                    if rows.is_empty() {
                        return Err(Error::Other("MySQL 查询无结果".into()));
                    }
                    Ok::<(), Error>(())
                }
                .await;

                // 无论成功或失败都清理连接池
                pool.disconnect().await.ok();

                match result {
                    Ok(()) => Ok("MySQL 连接成功".into()),
                    Err(e) => Err(e),
                }
            }
            DbDriver::Postgres => {
                let (client, connection) = profile
                    .pg_config()
                    .connect(tokio_postgres::NoTls)
                    .await
                    .map_err(|e| Error::Other(format!("PostgreSQL 连接失败: {e}")))?;
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                client
                    .simple_query("SELECT 1")
                    .await
                    .map_err(|e| Error::Other(format!("PostgreSQL 查询失败: {e}")))?;
                Ok("PostgreSQL 连接成功".into())
            }
            DbDriver::Mongodb => {
                // MongoDB：通过 TCP 探测 + 可选鉴权握手（RESP-like 检测）
                // 不引入 mongodb 官方 crate（体积过大），仅检测端口可达性
                let addr = format!("{}:{}", profile.host, profile.port);
                let timeout = std::time::Duration::from_secs(5);
                tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr))
                    .await
                    .map_err(|_| Error::Other(format!("MongoDB 连接超时: {addr}")))?
                    .map_err(|e| Error::Other(format!("MongoDB 连接失败: {e}")))?;
                Ok(format!("MongoDB 端口可达: {addr}"))
            }
            DbDriver::Redis => {
                // Redis：通过原始 RESP 协议发送 PING 命令，零依赖
                let addr = format!("{}:{}", profile.host, profile.port);
                let timeout = std::time::Duration::from_secs(5);
                let mut stream = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr))
                    .await
                    .map_err(|_| Error::Other(format!("Redis 连接超时: {addr}")))?
                    .map_err(|e| Error::Other(format!("Redis 连接失败: {e}")))?;

                // 若配置了密码，先发送 AUTH 命令
                if !profile.password.is_empty() {
                    let auth_cmd = if profile.username.is_empty() {
                        // Redis 6 之前：只有密码
                        format_resp_command(&["AUTH", &profile.password])
                    } else {
                        // Redis 6 ACL：用户名 + 密码
                        format_resp_command(&["AUTH", &profile.username, &profile.password])
                    };
                    stream.write_all(auth_cmd.as_bytes()).await.map_err(|e| Error::Other(format!("Redis 发送失败: {e}")))?;
                    let mut buf = [0u8; 64];
                    let n = stream.read(&mut buf).await.map_err(|e| Error::Other(format!("Redis 读取失败: {e}")))?;
                    let resp = String::from_utf8_lossy(&buf[..n]);
                    if !resp.starts_with("+OK") {
                        return Err(Error::Other(format!("Redis 鉴权失败: {resp}")));
                    }
                }

                // 发送 PING
                let ping = format_resp_command(&["PING"]);
                stream.write_all(ping.as_bytes()).await.map_err(|e| Error::Other(format!("Redis 发送失败: {e}")))?;
                let mut buf = [0u8; 64];
                let n = stream.read(&mut buf).await.map_err(|e| Error::Other(format!("Redis 读取失败: {e}")))?;
                let resp = String::from_utf8_lossy(&buf[..n]);
                if resp.starts_with("+PONG") {
                    Ok("Redis 连接成功".into())
                } else {
                    Err(Error::Other(format!("Redis 响应异常: {resp}")))
                }
            }
            DbDriver::Qdrant => {
                // Qdrant：通过 HTTP REST API 探测（reqwest 已有依赖）
                let url = format!("http://{}:{}/readyz", profile.host, profile.port);
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .map_err(|e| Error::Other(format!("HTTP 客户端错误: {e}")))?;
                let resp = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| Error::Other(format!("Qdrant 连接失败: {e}")))?;
                if resp.status().is_success() {
                    Ok("Qdrant 连接成功".into())
                } else {
                    Err(Error::Other(format!(
                        "Qdrant 响应异常: HTTP {}",
                        resp.status()
                    )))
                }
            }
        }
    }
}
