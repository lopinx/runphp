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

    /// 执行任意 SQL。
    ///
    /// 支持分号分隔的多条语句：
    /// - 如果是单条 SELECT/PRAGMA/WITH 查询，返回结果集。
    /// - 如果是多条非查询语句，逐条执行并累计 affected。
    /// - 如果最后一条是查询，返回其结果集。
    pub fn execute(&self, db: &str, sql: &str) -> Result<QueryResult, Error> {
        let conn = self.connect(db)?;

        let trimmed = sql.trim();
        // 检测是否含多条语句（分号分隔）
        let statements: Vec<&str> = trimmed
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if statements.len() > 1 {
            // 多条语句：逐条执行非查询，最后一条如果是查询则返回结果
            let mut total_affected = 0usize;
            let last_idx = statements.len() - 1;
            for (i, stmt) in statements.iter().enumerate() {
                let upper = stmt.to_uppercase();
                if i == last_idx && (upper.starts_with("SELECT")
                    || upper.starts_with("PRAGMA")
                    || upper.starts_with("WITH"))
                {
                    // 最后一条是查询，返回结果
                    return self.execute_query(&conn, stmt);
                }
                // 非查询语句
                let affected = conn.execute(stmt, []).map_err(rusqlite_err)?;
                total_affected += affected as usize;
            }
            Ok(QueryResult {
                columns: vec![],
                rows: vec![],
                affected: total_affected,
            })
        } else {
            // 单条语句
            let upper = trimmed.to_uppercase();
            if upper.starts_with("SELECT")
                || upper.starts_with("PRAGMA")
                || upper.starts_with("WITH")
            {
                self.execute_query(&conn, trimmed)
            } else {
                let affected = conn.execute(trimmed, []).map_err(rusqlite_err)?;
                Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    affected: affected as usize,
                })
            }
        }
    }

    /// 执行单条查询语句并返回结果集。
    fn execute_query(&self, conn: &Connection, sql: &str) -> Result<QueryResult, Error> {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_err)?;
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
    }
}

/// 将 rusqlite::Error 转为 crate Error。
fn rusqlite_err(e: rusqlite::Error) -> Error {
    Error::Other(format!("SQLite 错误: {e}"))
}

/// 清理数据库名称，防止路径穿越。
///
/// 先剥离常见 SQLite 扩展名（`.db`/`.sqlite`），再对剩余部分做安全替换，
/// 最后统一追加 `.db` 后缀，保证文件名始终可预测。
fn sanitize_name(name: &str) -> String {
    // 剥离已存在的 .db / .sqlite 后缀，避免后续替换产生 "my__db" 等问题
    let base = name.strip_suffix(".db").unwrap_or(name);
    let base = base.strip_suffix(".sqlite").unwrap_or(base);
    // 替换路径分隔符、点（防 .. 穿越）、空格、冒号，保留字母数字和下划线
    base.replace(['/', '\\', '.', ' ', ':'], "_") + ".db"
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
        assert_eq!(sanitize_name("../../etc/passwd"), "______etc_passwd.db");
        assert_eq!(sanitize_name("my.db"), "my.db");
        assert_eq!(sanitize_name("my.sqlite"), "my.db");
        assert_eq!(sanitize_name("my app"), "my_app.db");
    }

    #[test]
    fn 多语句执行() {
        let dir = std::env::temp_dir().join("runphp-sqlite-multi-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mgr = SqliteManager::new(dir.clone());

        mgr.create_database("多语句测试").unwrap();

        // 多条非查询语句（分号分隔）
        let result = mgr
            .execute("多语句测试.db", "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT); INSERT INTO items (name) VALUES ('A'); INSERT INTO items (name) VALUES ('B');")
            .unwrap();
        assert_eq!(result.affected, 2); // 2 条 INSERT

        // 多条语句最后一条为查询
        let result = mgr
            .execute("多语句测试.db", "INSERT INTO items (name) VALUES ('C'); SELECT * FROM items;")
            .unwrap();
        assert_eq!(result.columns, vec!["id", "name"]);
        assert_eq!(result.rows.len(), 3); // A, B, C

        let _ = std::fs::remove_dir_all(&dir);
    }
}
