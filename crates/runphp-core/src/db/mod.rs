//! 数据库管理模块。
//!
//! - SQLite（内置引擎）：库管理对象的浏览与查询
//! - MySQL/MariaDB、PostgreSQL：连接管理已有实例 + 表浏览与 SQL 执行

pub mod remote;
pub mod sqlite;

pub use sqlite::{SqliteManager, TableInfo, QueryResult, DatabaseFile};
pub use remote::{RemoteTableInfo, RemoteQueryResult};
