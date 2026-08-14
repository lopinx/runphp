//! 远程数据库连接档案管理（MySQL/MariaDB、PostgreSQL）。
//!
//! 仅连接管理已有实例，不捆绑服务器。
//! 连接档案（含密码）以 JSON 存储于数据目录。

use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// MySQL 异步 trait 需要引入 prelude。
use mysql_async::prelude::*;

/// 数据库类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DbDriver {
    Mysql,
    Postgres,
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

/// 简单的 URL 转义。
fn url_escape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
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
                drop(conn);
                pool.disconnect().await.ok();
                Ok("MySQL 连接成功".into())
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
        }
    }
}
