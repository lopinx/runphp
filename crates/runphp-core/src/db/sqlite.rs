//! SQLite 管理：列出文件、浏览表结构、查询数据。
//!
//! 使用 rusqlite bundled 引擎，零外部依赖。
//! 数据库文件存放于数据目录下的 `databases/` 子目录。

use crate::Error;
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// 单个数据库文件信息。
#[derive(Debug, Clone, Serialize)]
pub struct DatabaseFile {
    /// 文件名。
    pub name: String,
    /// 完整路径。
    pub path: String,
    /// 文件大小（字节）。
    pub size: u64,
}

/// 单个表的结构信息。
#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub name: String,
    pub column_count: i64,
    pub row_count: i64,
}

/// 查询结果。
#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub affected: usize,
}

/// SQLite 管理器。
pub struct SqliteManager {
    /// 数据库存放目录。
    dir: PathBuf,
}

impl SqliteManager {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// 确保数据目录存在。
    pub fn ensure_dir(&self) -> Result<(), Error> {
        if !self.dir.exists() {
            std::fs::create_dir_all(&self.dir).map_err(Error::Io)?;
        }
        Ok(())
    }

    /// 列出所有 .db/.sqlite 文件。
    pub fn list_databases(&self) -> Result<Vec<DatabaseFile>, Error> {
        self.ensure_dir()?;
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".db") || name.ends_with(".sqlite") {
                    let meta = entry.metadata().map_err(Error::Io)?;
                    files.push(DatabaseFile {
                        name,
                        path: entry.path().to_string_lossy().to_string(),
                        size: meta.len(),
                    });
                }
            }
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    /// 创建新数据库（空文件）。
    pub fn create_database(&self, name: &str) -> Result<PathBuf, Error> {
        self.ensure_dir()?;
        let safe_name = sanitize_name(name);
        let path = self.dir.join(format!("{safe_name}.db"));
        // 创建空数据库：打开并关闭即创建文件
        Connection::open(&path).map_err(rusqlite_err)?;
        Ok(path)
    }

    /// 删除数据库文件。
    pub fn delete_database(&self, name: &str) -> Result<(), Error> {
        let safe_name = sanitize_name(name);
        // 尝试带 .db 和不带后缀两种形式
        let path_with_ext = self.dir.join(format!("{safe_name}.db"));
        let path_plain = self.dir.join(&safe_name);
        let path = if path_with_ext.exists() {
            path_with_ext
        } else if path_plain.exists() {
            path_plain
        } else {
            // 直接用原始名称尝试（兼容旧逻辑）
            self.dir.join(name)
        };
        if path.exists() {
            std::fs::remove_file(&path).map_err(Error::Io)?;
        }
        Ok(())
    }

    /// 打开指定数据库连接。
    ///
    /// 仅允许访问管理器目录内的数据库文件，防止路径穿越。
    fn connect(&self, name: &str) -> Result<Connection, Error> {
        // 拒绝绝对路径和路径穿越
        if Path::new(name).is_absolute() {
            return Err(Error::Other("不允许使用绝对路径访问数据库".into()));
        }
        if name.contains("..") {
            return Err(Error::Other("数据库名称不允许包含 '..'".into()));
        }
        let path = self.dir.join(name);
        // 确保最终路径仍在管理目录下
        if !path.starts_with(&self.dir) {
            return Err(Error::Other("数据库路径越权".into()));
        }
        Connection::open(&path).map_err(rusqlite_err)
    }

    /// 列出数据库中的所有表。
    pub fn list_tables(&self, db: &str) -> Result<Vec<TableInfo>, Error> {
        let conn = self.connect(db)?;
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(rusqlite_err)?;
        let names: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .map_err(rusqlite_err)?
            .filter_map(|r| r.ok())
            .collect();

        let mut tables = Vec::new();
        for name in names {
            let escaped = escape_ident(&name);
            let col_count: i64 = conn
                .query_row(
                    &format!("SELECT count(*) FROM pragma_table_info('{escaped}')"),
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let row_count: i64 = if name == "sqlite_sequence" {
                0
            } else {
                conn.query_row(&format!("SELECT count(*) FROM '{escaped}'"), [], |r| {
                    r.get(0)
                })
                .unwrap_or(0)
            };
            tables.push(TableInfo {
                name,
                column_count: col_count,
                row_count,
            });
        }
        Ok(tables)
    }

    /// 查询表中的前 N 行数据。
    pub fn query_table(&self, db: &str, table: &str, mut limit: i64, mut offset: i64) -> Result<QueryResult, Error> {
        // 防止负数参数生成无效 SQL
        if limit < 1 { limit = 100; }
        if offset < 0 { offset = 0; }
        let escaped_table = escape_ident(table);
        let sql = format!("SELECT * FROM '{escaped_table}' LIMIT {limit} OFFSET {offset}");
        self.execute(&db, &sql)
    }

    /// 执行任意 SQL（SELECT 返回结果集，其他返回 affected=0）。
    pub fn execute(&self, db: &str, sql: &str) -> Result<QueryResult, Error> {
        let conn = self.connect(db)?;

        let trimmed = sql.trim();
        if trimmed
            .to_uppercase()
            .starts_with("SELECT")
            || trimmed.to_uppercase().starts_with("PRAGMA")
            || trimmed.to_uppercase().starts_with("WITH")
        {
            // 查询语句
            let mut stmt = conn.prepare(trimmed).map_err(rusqlite_err)?;
            let col_count = stmt.column_count();
            let columns: Vec<String> = (0..col_count)
                .map(|i| stmt.column_name(i).unwrap_or_default().to_string())
                .collect();

            let rows_iter = stmt.query_map([], |row| {
                let mut values = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    let val = match row.get_ref(i) {
                        Ok(rusqlite::types::ValueRef::Null) => serde_json::Value::Null,
                        Ok(rusqlite::types::ValueRef::Integer(n)) => serde_json::Value::from(n),
                        Ok(rusqlite::types::ValueRef::Real(f)) => serde_json::Value::from(f),
                        Ok(rusqlite::types::ValueRef::Text(s)) => {
                            serde_json::Value::from(String::from_utf8_lossy(s).to_string())
                        }
                        Ok(rusqlite::types::ValueRef::Blob(b)) => {
                            serde_json::Value::from(format!("[BLOB {} 字节]", b.len()))
                        }
                        Err(_) => serde_json::Value::Null,
                    };
                    values.push(val);
                }
                Ok(values)
            }).map_err(rusqlite_err)?;

            let mut rows = Vec::new();
            for r in rows_iter {
                rows.push(r.map_err(rusqlite_err)?);
            }
            let affected = rows.len();
            Ok(QueryResult { columns, rows, affected })
        } else {
            // 非查询语句
            let affected = conn.execute(trimmed, []).map_err(rusqlite_err)?;
            Ok(QueryResult {
                columns: vec![],
                rows: vec![],
                affected: affected as usize,
            })
        }
    }
}

/// 将 rusqlite::Error 转为 crate Error。
fn rusqlite_err(e: rusqlite::Error) -> Error {
    Error::Other(format!("SQLite 错误: {e}"))
}

/// 清理数据库名称，防止路径穿越。
fn sanitize_name(name: &str) -> String {
    name.replace(['/', '\\', '.', ' ', ':'], "_")
}

/// 转义 SQL 标识符（表名/列名），防止标识符注入。
/// 将内部的单引号翻倍（SQLite 标准转义）。
fn escape_ident(name: &str) -> String {
    name.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn 创建与查询数据库() {
        let dir = std::env::temp_dir().join("runphp-sqlite-test");
        // 清理
        let _ = std::fs::remove_dir_all(&dir);
        let mgr = SqliteManager::new(dir.clone());

        let path = mgr.create_database("测试库").unwrap();
        assert!(path.exists());

        let dbs = mgr.list_databases().unwrap();
        assert_eq!(dbs.len(), 1);

        // 建表
        let result = mgr
            .execute(
                "测试库.db",
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
            )
            .unwrap();
        assert_eq!(result.affected, 0);

        // 插入
        mgr.execute("测试库.db", "INSERT INTO users (name) VALUES ('张三')")
            .unwrap();
        mgr.execute("测试库.db", "INSERT INTO users (name) VALUES ('李四')")
            .unwrap();

        // 列出表
        let tables = mgr.list_tables("测试库.db").unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "users");
        assert_eq!(tables[0].row_count, 2);

        // 查询
        let result = mgr.query_table("测试库.db", "users", 10, 0).unwrap();
        assert_eq!(result.columns, vec!["id", "name"]);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][1], serde_json::Value::from("张三"));

        // 清理
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 名称清理防路径穿越() {
        assert_eq!(sanitize_name("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_name("my.db"), "my_db");
    }
}
