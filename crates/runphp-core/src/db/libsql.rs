//! libSQL 数据库管理（本地文件 / 远程连接 / 嵌入式副本）。
//!
//! libSQL 是 SQLite 的开源分支（Turso 维护），100% 兼容 SQLite API，
//! 额外支持远程连接和嵌入式副本。本模块通过 `libsql` crate 管理三种连接模式。
//!
//! 复用 `sqlite::TableInfo` / `sqlite::QueryResult`（字段结构通用）。

use crate::db::sqlite::{QueryResult, TableInfo};
use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// libSQL 连接模式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LibsqlMode {
    /// 本地文件。
    Local,
    /// 远程 URL（Turso 或自建 libsql 服务器）。
    Remote,
    /// 嵌入式副本（本地文件 + 远程同步）。
    Replica,
}

/// libSQL 连接档案。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibsqlProfile {
    /// 唯一标识。
    pub id: String,
    /// 显示名称。
    pub name: String,
    /// 连接模式。
    pub mode: LibsqlMode,
    /// 本地文件路径（Local 和 Replica 模式使用）。
    #[serde(default)]
    pub path: Option<String>,
    /// 远程 URL（Remote 和 Replica 模式使用）。
    #[serde(default)]
    pub url: Option<String>,
    /// 认证 Token（Remote 和 Replica 模式使用）。
    #[serde(default)]
    pub auth_token: Option<String>,
    /// 创建时间。
    pub created_at: String,
}

impl LibsqlProfile {
    /// 创建空白档案。
    pub fn new(name: String, mode: LibsqlMode) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            mode,
            path: None,
            url: None,
            auth_token: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// 档案注册表（持久化为 `libsql_profiles.json`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibsqlProfileRegistry {
    pub profiles: Vec<LibsqlProfile>,
}

/// libSQL 数据库管理器。
pub struct LibsqlManager {
    /// 档案文件路径（`libsql_profiles.json`）。
    path: PathBuf,
    /// 本地文件存放目录。
    data_dir: PathBuf,
}

impl LibsqlManager {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            path: data_dir.join("libsql_profiles.json"),
            data_dir: data_dir.join("libsql"),
        }
    }

    pub fn load(&self) -> Result<LibsqlProfileRegistry, Error> {
        if !self.path.exists() {
            return Ok(LibsqlProfileRegistry::default());
        }
        let raw = std::fs::read_to_string(&self.path).map_err(Error::Io)?;
        serde_json::from_str(&raw).map_err(Error::Json)
    }

    pub fn save(&self, reg: &LibsqlProfileRegistry) -> Result<(), Error> {
        let raw = serde_json::to_string_pretty(reg).map_err(Error::Json)?;
        std::fs::write(&self.path, raw).map_err(Error::Io)?;
        Ok(())
    }

    pub fn list_profiles(&self) -> Result<Vec<LibsqlProfile>, Error> {
        Ok(self.load()?.profiles)
    }

    pub fn add_profile(&self, profile: LibsqlProfile) -> Result<(), Error> {
        let mut reg = self.load()?;
        reg.profiles.push(profile);
        self.save(&reg)
    }

    pub fn remove_profile(&self, id: &str) -> Result<(), Error> {
        let mut reg = self.load()?;
        reg.profiles.retain(|p| p.id != id);
        self.save(&reg)
    }

    /// 本地文件默认存放目录。
    pub fn local_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// 根据 profile 建立 libsql 连接。
    async fn connect(profile: &LibsqlProfile) -> Result<libsql::Connection, Error> {
        let db = match profile.mode {
            LibsqlMode::Local => {
                let path = profile
                    .path
                    .as_ref()
                    .ok_or_else(|| Error::Other("libSQL 本地模式需要 path".into()))?;
                libsql::Builder::new_local(path)
                    .build()
                    .await
                    .map_err(|e| Error::Other(format!("libSQL 本地连接失败: {e}")))?
            }
            LibsqlMode::Remote => {
                let url = profile
                    .url
                    .as_ref()
                    .ok_or_else(|| Error::Other("libSQL 远程模式需要 url".into()))?;
                let token = profile.auth_token.clone().unwrap_or_default();
                libsql::Builder::new_remote(url.clone(), token)
                    .build()
                    .await
                    .map_err(|e| Error::Other(format!("libSQL 远程连接失败: {e}")))?
            }
            LibsqlMode::Replica => {
                let path = profile
                    .path
                    .as_ref()
                    .ok_or_else(|| Error::Other("libSQL 副本模式需要 path".into()))?;
                let url = profile
                    .url
                    .as_ref()
                    .ok_or_else(|| Error::Other("libSQL 副本模式需要 url".into()))?;
                let token = profile.auth_token.clone().unwrap_or_default();
                libsql::Builder::new_remote_replica(path, url.clone(), token)
                    .build()
                    .await
                    .map_err(|e| Error::Other(format!("libSQL 副本连接失败: {e}")))?
            }
        };
        db.connect()
            .map_err(|e| Error::Other(format!("libSQL 获取连接失败: {e}")))
    }

    /// 测试连接。
    pub async fn test_connection(profile: &LibsqlProfile) -> Result<String, Error> {
        let conn = Self::connect(profile).await?;
        let mut rows = conn
            .query("SELECT 1", ())
            .await
            .map_err(|e| Error::Other(format!("libSQL 测试查询失败: {e}")))?;
        let row = rows
            .next()
            .await
            .map_err(|e| Error::Other(format!("libSQL 读取结果失败: {e}")))?;
        if row.is_some() {
            Ok("libSQL 连接成功".into())
        } else {
            Err(Error::Other("libSQL 测试查询无结果".into()))
        }
    }

    /// 列出表（查询 sqlite_master）。
    pub async fn list_tables(profile: &LibsqlProfile) -> Result<Vec<TableInfo>, Error> {
        let conn = Self::connect(profile).await?;
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                (),
            )
            .await
            .map_err(|e| Error::Other(format!("libSQL 查询表失败: {e}")))?;

        let mut tables = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| Error::Other(format!("libSQL 读取表名失败: {e}")))?
        {
            let name: String = row
                .get(0)
                .map_err(|e| Error::Other(format!("libSQL 解析表名失败: {e}")))?;

            // 查询行数
            let escaped = name.replace('\'', "''");
            let count_sql = format!("SELECT count(*) FROM \"{escaped}\"");
            let row_count: i64 = match conn.query(&count_sql, ()).await {
                Ok(mut r) => r
                    .next()
                    .await
                    .ok()
                    .flatten()
                    .and_then(|row| row.get::<i64>(0).ok())
                    .unwrap_or(0),
                Err(_) => 0,
            };

            // 查询列数
            let col_sql = format!("PRAGMA table_info(\"{escaped}\")");
            let col_count: i64 = match conn.query(&col_sql, ()).await {
                Ok(mut r) => {
                    let mut count = 0i64;
                    while r.next().await.ok().flatten().is_some() {
                        count += 1;
                    }
                    count
                }
                Err(_) => 0,
            };

            tables.push(TableInfo {
                name,
                column_count: col_count,
                row_count,
            });
        }
        Ok(tables)
    }

    /// 查询表数据。
    pub async fn query_table(
        profile: &LibsqlProfile,
        table: &str,
        limit: i64,
        offset: i64,
    ) -> Result<QueryResult, Error> {
        let safe_limit = if limit < 1 { 100 } else { limit };
        let safe_offset = if offset < 0 { 0 } else { offset };
        let escaped = table.replace('"', "\"\"");
        let sql = format!("SELECT * FROM \"{escaped}\" LIMIT {safe_limit} OFFSET {safe_offset}");
        Self::execute(profile, &sql).await
    }

    /// 执行任意 SQL。
    pub async fn execute(profile: &LibsqlProfile, sql: &str) -> Result<QueryResult, Error> {
        let conn = Self::connect(profile).await?;
        let trimmed = sql.trim();
        let upper = trimmed.to_uppercase();
        let is_query = upper.starts_with("SELECT")
            || upper.starts_with("WITH")
            || upper.starts_with("PRAGMA")
            || upper.starts_with("EXPLAIN");

        if is_query {
            let mut rows = conn
                .query(trimmed, ())
                .await
                .map_err(|e| Error::Other(format!("libSQL 查询失败: {e}")))?;

            let col_count = rows.column_count();
            let mut columns = Vec::with_capacity(col_count as usize);
            for i in 0..col_count {
                let name = rows
                    .column_name(i)
                    .unwrap_or_default()
                    .to_string();
                columns.push(name);
            }

            let mut rows_out = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| Error::Other(format!("libSQL 读取行失败: {e}")))?
            {
                let values: Vec<serde_json::Value> = (0..col_count)
                    .map(|i| {
                        row.get_value(i)
                            .map(libsql_value_to_json)
                            .unwrap_or(serde_json::Value::Null)
                    })
                    .collect();
                rows_out.push(values);
            }
            let affected = rows_out.len();
            Ok(QueryResult {
                columns,
                rows: rows_out,
                affected,
            })
        } else {
            let affected = conn
                .execute(trimmed, ())
                .await
                .map_err(|e| Error::Other(format!("libSQL 执行失败: {e}")))?;
            Ok(QueryResult {
                columns: vec![],
                rows: vec![],
                affected: affected as usize,
            })
        }
    }
}

/// 将 libsql::Value 转为 serde_json::Value。
fn libsql_value_to_json(v: libsql::Value) -> serde_json::Value {
    match v {
        libsql::Value::Null => serde_json::Value::Null,
        libsql::Value::Integer(n) => serde_json::Value::from(n),
        libsql::Value::Real(f) => serde_json::Value::from(f),
        libsql::Value::Text(s) => serde_json::Value::from(s),
        libsql::Value::Blob(b) => {
            // 尝试 UTF-8 文本，否则显示占位
            String::from_utf8(b.clone())
                .map(serde_json::Value::from)
                .unwrap_or_else(|_| serde_json::Value::from(format!("[BLOB {} 字节]", b.len())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_serialization() {
        let p = LibsqlProfile::new("测试".into(), LibsqlMode::Local);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"mode\":\"local\""));
        let back: LibsqlProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mode, LibsqlMode::Local);
        assert_eq!(back.name, "测试");
    }

    #[test]
    fn mode_lowercase() {
        let p = LibsqlProfile::new("r".into(), LibsqlMode::Remote);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"mode\":\"remote\""));

        let p2 = LibsqlProfile::new("rep".into(), LibsqlMode::Replica);
        let json2 = serde_json::to_string(&p2).unwrap();
        assert!(json2.contains("\"mode\":\"replica\""));
    }

    #[test]
    fn registry_roundtrip() {
        let reg = LibsqlProfileRegistry {
            profiles: vec![
                LibsqlProfile::new("a".into(), LibsqlMode::Local),
                LibsqlProfile::new("b".into(), LibsqlMode::Remote),
            ],
        };
        let json = serde_json::to_string(&reg).unwrap();
        let back: LibsqlProfileRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.profiles.len(), 2);
    }
}
