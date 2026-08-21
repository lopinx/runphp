//! 数据库管理模块。
//!
//! - SQLite（内置引擎）：库管理对象的浏览与查询
//! - MySQL/MariaDB、PostgreSQL：连接管理已有实例 + 表浏览与 SQL 执行
//! - SSH 隧道：通过 SSH 端口转发安全连接远程数据库

pub mod libsql;
pub mod remote;
pub mod sqlite;
pub mod tunnel;

pub use sqlite::{SqliteManager, TableInfo, QueryResult, DatabaseFile};
pub use remote::{
    ConnectionProfile, DbDriver, ProfileRegistry, RemoteDbManager, RemoteQueryResult,
    RemoteTableInfo, SslMode,
};
pub use libsql::{LibsqlManager, LibsqlMode, LibsqlProfile, LibsqlProfileRegistry};
