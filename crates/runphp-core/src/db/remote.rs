//! 远程数据库连接档案管理（MySQL/MariaDB、PostgreSQL）。
//!
//! 仅连接管理已有实例，不捆绑服务器。
//! 连接档案（含密码）以 JSON 存储于数据目录。
//! MySQL 与 PostgreSQL 支持表浏览与 SQL 执行；其余类型仅做连接测试。

use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// MySQL 异步 trait 需要引入 prelude。
use mysql_async::prelude::*;
use mysql_async::{SslOpts, OptsBuilder};

/// 构建 rustls 客户端配置。
///
/// - 内置 webpki-roots 根证书（常见 CA）
/// - 可选加载用户自定义 CA 证书（PEM 文件路径，用于自签证书）
fn rustls_client_config(ca_pem_path: Option<&str>) -> Result<rustls::ClientConfig, Error> {
    let mut roots = rustls::RootCertStore::empty();
    // 内置 CA 证书
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // 用户自定义 CA 证书（PEM 文件）
    if let Some(path) = ca_pem_path {
        if !path.is_empty() {
            let pem = std::fs::read(path)
                .map_err(|e| Error::Other(format!("读取 CA 证书失败: {e}")))?;
            let mut reader = std::io::BufReader::new(&pem[..]);
            for cert in rustls_pemfile::certs(&mut reader) {
                let cert = cert
                    .map_err(|e| Error::Other(format!("解析 CA 证书失败: {e}")))?;
                roots.add(cert).ok();
            }
        }
    }
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

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

/// SSL 加密模式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SslMode {
    /// 不使用 SSL。
    Disabled,
    /// 优先使用 SSL，失败则回退明文。
    Preferred,
    /// 强制使用 SSL，证书校验可选。
    Required,
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
    /// SSL 模式（None 等同 Disabled，向后兼容）。
    #[serde(default)]
    pub ssl_mode: Option<SslMode>,
    /// CA 证书路径或 PEM 内容（可选，用于自签证书）。
    #[serde(default)]
    pub ssl_ca: Option<String>,
    /// SSH 隧道主机（None 表示不使用 SSH 隧道）。
    #[serde(default)]
    pub ssh_host: Option<String>,
    /// SSH 端口（默认 22）。
    #[serde(default)]
    pub ssh_port: Option<u16>,
    /// SSH 用户名。
    #[serde(default)]
    pub ssh_user: Option<String>,
    /// SSH 私钥路径（与密码二选一）。
    #[serde(default)]
    pub ssh_key: Option<String>,
    /// SSH 密码认证。
    #[serde(default)]
    pub ssh_password: Option<String>,
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
            ssl_mode: None,
            ssl_ca: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_key: None,
            ssh_password: None,
        }
    }

    /// 生成 MySQL DSN（mysql_async 格式）。
    ///
    /// `host` / `port` 可被覆盖（用于 SSH 隧道场景下替换为本地隧道地址）。
    fn mysql_dsn_with(&self, host: &str, port: u16) -> String {
        let db = self.database.as_deref().unwrap_or("");
        format!(
            "mysql://{}:{}@{}:{}/{}",
            url_escape(&self.username),
            url_escape(&self.password),
            host,
            port,
            db
        )
    }

    /// 生成 PostgreSQL 连接参数。
    ///
    /// `host` / `port` 可被覆盖（用于 SSH 隧道场景下替换为本地隧道地址）。
    fn pg_config_with(&self, host: &str, port: u16) -> tokio_postgres::Config {
        let mut config = tokio_postgres::Config::new();
        config
            .user(&self.username)
            .password(&self.password)
            .host(host)
            .port(port);
        if let Some(db) = &self.database {
            config.dbname(db);
        }
        config
    }

    /// 构建 MySQL 连接选项（含 SSL 与隧道地址覆盖）。
    fn mysql_opts(&self, host: &str, port: u16) -> Result<mysql_async::Opts, Error> {
        let dsn = self.mysql_dsn_with(host, port);
        let mut builder = OptsBuilder::from_opts(
            mysql_async::Opts::from_url(&dsn)
                .map_err(|e| Error::Other(format!("MySQL 连接参数错误: {e}")))?,
        );
        if self.ssl_enabled() {
            let mut ssl = SslOpts::default();
            // Required 模式下若用户未提供 CA，则跳过域名校验（自签场景友好）
            if matches!(self.ssl_mode, Some(SslMode::Required)) && self.ssl_ca.is_none() {
                ssl = ssl.with_danger_skip_domain_validation(true);
                ssl = ssl.with_danger_accept_invalid_certs(true);
            }
            if let Some(ca_path) = &self.ssl_ca {
                if !ca_path.is_empty() {
                    ssl = ssl.with_root_certs(vec![std::path::PathBuf::from(ca_path).into()]);
                }
            }
            builder = builder.ssl_opts(Some(ssl));
        }
        Ok(builder.into())
    }

    /// 是否启用了 SSL（Required 或 Preferred）。
    fn ssl_enabled(&self) -> bool {
        matches!(
            self.ssl_mode,
            Some(SslMode::Required) | Some(SslMode::Preferred)
        )
    }

    /// 是否使用 SSH 隧道。
    pub fn ssh_enabled(&self) -> bool {
        self.ssh_host.as_ref().map(|h| !h.is_empty()).unwrap_or(false)
    }
}

/// 远程表结构信息（与 SQLite 的 TableInfo 字段对齐，便于前端复用）。
#[derive(Debug, Clone, Serialize)]
pub struct RemoteTableInfo {
    pub name: String,
    pub column_count: i64,
    pub row_count: i64,
}

/// 远程查询结果（与 SQLite 的 QueryResult 字段对齐，便于前端复用）。
#[derive(Debug, Clone, Serialize)]
pub struct RemoteQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub affected: usize,
}

/// 将 mysql_async::Value 转为 serde_json::Value。
fn mysql_value_to_json(v: mysql_async::Value) -> serde_json::Value {
    use mysql_async::Value;
    match v {
        Value::NULL => serde_json::Value::Null,
        Value::Int(n) => serde_json::Value::from(n),
        Value::UInt(n) => serde_json::Value::from(n),
        Value::Float(f) => {
            serde_json::Value::from(f as f64)
        }
        Value::Double(f) => serde_json::Value::from(f),
        Value::Bytes(b) => {
            // 尝试 UTF-8 文本，否则显示占位
            String::from_utf8(b)
                .map(serde_json::Value::from)
                .unwrap_or_else(|e| serde_json::Value::from(format!("[BLOB {} 字节]", e.into_bytes().len())))
        }
        Value::Date(..) | Value::Time(..) => {
            serde_json::Value::from(v.as_sql(false))
        }
    }
}

/// 将 tokio-postgres 行值转为 serde_json::Value。
///
/// 使用 serde_json::Value 的 FromSql 实现（依赖 with-serde_json-1 feature），
/// 失败时回退为 Null，避免单列类型不匹配导致整行查询失败。
fn pg_value_to_json(row: &tokio_postgres::Row, idx: usize) -> serde_json::Value {
    row.try_get::<usize, serde_json::Value>(idx).unwrap_or(serde_json::Value::Null)
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

/// Redis 连接流（TCP 或 TLS），统一读写接口。
enum RedisStream {
    Tcp(tokio::net::TcpStream),
    Tls(tokio_rustls::client::TlsStream<tokio::net::TcpStream>),
}

impl tokio::io::AsyncRead for RedisStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            RedisStream::Tcp(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            RedisStream::Tls(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for RedisStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            RedisStream::Tcp(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            RedisStream::Tls(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            RedisStream::Tcp(s) => std::pin::Pin::new(s).poll_flush(cx),
            RedisStream::Tls(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            RedisStream::Tcp(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            RedisStream::Tls(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
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

    /// 解析实际连接地址（host, port）。
    ///
    /// 若启用了 SSH 隧道，先建立隧道，返回本地隧道地址；
    /// 否则直接返回 profile 中的原始地址。
    async fn resolve_endpoint(
        profile: &ConnectionProfile,
    ) -> Result<(String, u16, Option<crate::db::tunnel::SshTunnel>), Error> {
        if let Some(tunnel) = crate::db::tunnel::SshTunnel::open(profile).await? {
            Ok(("127.0.0.1".to_string(), tunnel.local_port(), Some(tunnel)))
        } else {
            Ok((profile.host.clone(), profile.port, None))
        }
    }

    /// 建立 PostgreSQL 连接（根据 profile 选择 SSL 或明文）。
    ///
    /// 返回的 Client 已可用于查询，连接后台任务自动驱动。
    async fn pg_connect(
        profile: &ConnectionProfile,
        host: &str,
        port: u16,
    ) -> Result<tokio_postgres::Client, Error> {
        if profile.ssl_enabled() {
            let config = rustls_client_config(profile.ssl_ca.as_deref())?;
            let connector = tokio_postgres_rustls::MakeRustlsConnect::new(config);
            let (client, connection) = profile
                .pg_config_with(host, port)
                .connect(connector)
                .await
                .map_err(|e| Error::Other(format!("PostgreSQL 连接失败: {e}")))?;
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    tracing::error!("PostgreSQL 连接驱动异常: {e}");
                }
            });
            Ok(client)
        } else {
            let (client, connection) = profile
                .pg_config_with(host, port)
                .connect(tokio_postgres::NoTls)
                .await
                .map_err(|e| Error::Other(format!("PostgreSQL 连接失败: {e}")))?;
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    tracing::error!("PostgreSQL 连接驱动异常: {e}");
                }
            });
            Ok(client)
        }
    }

    /// 测试连接。
    pub async fn test_connection(profile: &ConnectionProfile) -> Result<String, Error> {
        match profile.driver {
            DbDriver::Mysql => {
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let opts = profile.mysql_opts(&host, port)?;
                let pool = mysql_async::Pool::new(opts);
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
                pool.disconnect().await.ok();
                match result {
                    Ok(()) => Ok("MySQL 连接成功".into()),
                    Err(e) => Err(e),
                }
            }
            DbDriver::Postgres => {
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let client = Self::pg_connect(profile, &host, port).await?;
                client
                    .simple_query("SELECT 1")
                    .await
                    .map_err(|e| Error::Other(format!("PostgreSQL 查询失败: {e}")))?;
                Ok("PostgreSQL 连接成功".into())
            }
            DbDriver::Mongodb => {
                // MongoDB：通过 TCP（或 TLS）探测端口可达性
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let addr = format!("{host}:{port}");
                let timeout = std::time::Duration::from_secs(5);
                tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr))
                    .await
                    .map_err(|_| Error::Other(format!("MongoDB 连接超时: {addr}")))?
                    .map_err(|e| Error::Other(format!("MongoDB 连接失败: {e}")))?;
                Ok(format!("MongoDB 端口可达: {addr}"))
            }
            DbDriver::Redis => {
                // Redis：通过原始 RESP 协议发送 PING，可选 TLS 包装
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let addr = format!("{host}:{port}");
                let timeout = std::time::Duration::from_secs(5);
                let tcp = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr))
                    .await
                    .map_err(|_| Error::Other(format!("Redis 连接超时: {addr}")))?
                    .map_err(|e| Error::Other(format!("Redis 连接失败: {e}")))?;
                let mut stream = if profile.ssl_enabled() {
                    let config = rustls_client_config(profile.ssl_ca.as_deref())?;
                    let connector = tokio_rustls::TlsConnector::from(
                        std::sync::Arc::new(config),
                    );
                    let server_name = rustls::pki_types::ServerName::try_from(host.clone())
                        .map_err(|e| Error::Other(format!("TLS 域名解析失败: {e}")))?;
                    let tls = connector.connect(server_name, tcp).await
                        .map_err(|e| Error::Other(format!("Redis TLS 握手失败: {e}")))?;
                    RedisStream::Tls(tls)
                } else {
                    RedisStream::Tcp(tcp)
                };

                // 若配置了密码，先发送 AUTH 命令
                if !profile.password.is_empty() {
                    let auth_cmd = if profile.username.is_empty() {
                        format_resp_command(&["AUTH", &profile.password])
                    } else {
                        format_resp_command(&["AUTH", &profile.username, &profile.password])
                    };
                    stream.write_all(auth_cmd.as_bytes()).await
                        .map_err(|e| Error::Other(format!("Redis 发送失败: {e}")))?;
                    let mut buf = [0u8; 64];
                    let n = stream.read(&mut buf).await
                        .map_err(|e| Error::Other(format!("Redis 读取失败: {e}")))?;
                    let resp = String::from_utf8_lossy(&buf[..n]);
                    if !resp.starts_with("+OK") {
                        return Err(Error::Other(format!("Redis 鉴权失败: {resp}")));
                    }
                }

                // 发送 PING
                let ping = format_resp_command(&["PING"]);
                stream.write_all(ping.as_bytes()).await
                    .map_err(|e| Error::Other(format!("Redis 发送失败: {e}")))?;
                let mut buf = [0u8; 64];
                let n = stream.read(&mut buf).await
                    .map_err(|e| Error::Other(format!("Redis 读取失败: {e}")))?;
                let resp = String::from_utf8_lossy(&buf[..n]);
                if resp.starts_with("+PONG") {
                    Ok("Redis 连接成功".into())
                } else {
                    Err(Error::Other(format!("Redis 响应异常: {resp}")))
                }
            }
            DbDriver::Qdrant => {
                // Qdrant：通过 HTTP REST API 探测（https 用于 SSL）
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let scheme = if profile.ssl_enabled() { "https" } else { "http" };
                let url = format!("{scheme}://{host}:{port}/readyz");
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

    /// 列出远程数据库中的所有表（仅支持 MySQL / PostgreSQL）。
    pub async fn list_tables(profile: &ConnectionProfile) -> Result<Vec<RemoteTableInfo>, Error> {
        match profile.driver {
            DbDriver::Mysql => {
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let opts = profile.mysql_opts(&host, port)?;
                let pool = mysql_async::Pool::new(opts);
                let result = async {
                    let mut conn = pool
                        .get_conn()
                        .await
                        .map_err(|e| Error::Other(format!("MySQL 连接失败: {e}")))?;
                    let db = profile.database.as_deref().unwrap_or("");
                    if !db.is_empty() {
                        conn.query::<mysql_async::Row, _>(
                            format!("USE `{}`", db.replace('`', "``"))
                        ).await.map_err(|e| Error::Other(format!("MySQL 切库失败: {e}")))?;
                    }
                    // 单次查询获取表名、行数、列数（消除 N+1 查询）
                    let rows: Vec<mysql_async::Row> = conn
                        .query("SELECT t.table_name, COALESCE(t.table_rows, 0), COUNT(c.column_name) FROM information_schema.tables t LEFT JOIN information_schema.columns c ON c.table_schema = t.table_schema AND c.table_name = t.table_name WHERE t.table_schema = DATABASE() GROUP BY t.table_name, t.table_rows ORDER BY t.table_name")
                        .await
                        .map_err(|e| Error::Other(format!("MySQL 查询表失败: {e}")))?;
                    let mut tables = Vec::new();
                    for row in rows {
                        let name: String = row.get(0).unwrap_or_default();
                        let row_count: i64 = row.get(1).unwrap_or(0);
                        let col_count: i64 = row.get(2).unwrap_or(0);
                        tables.push(RemoteTableInfo { name, column_count: col_count, row_count });
                    }
                    Ok::<Vec<RemoteTableInfo>, Error>(tables)
                }
                .await;
                pool.disconnect().await.ok();
                result
            }
            DbDriver::Postgres => {
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let client = Self::pg_connect(profile, &host, port).await?;
                let rows = client
                    .query(
                        "SELECT c.relname, COALESCE(s.n_live_tup, 0), count(a.attname)
                         FROM pg_class c
                         JOIN pg_namespace n ON n.oid = c.relnamespace
                         LEFT JOIN pg_stat_user_tables s ON s.relid = c.oid
                         LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
                         WHERE c.relkind = 'r' AND n.nspname NOT IN ('pg_catalog', 'information_schema')
                         GROUP BY c.relname, s.n_live_tup
                         ORDER BY c.relname",
                        &[],
                    )
                    .await
                    .map_err(|e| Error::Other(format!("PostgreSQL 查询表失败: {e}")))?;
                let mut tables = Vec::new();
                for row in &rows {
                    let name: String = row.get(0);
                    let row_count: i64 = row.get(1);
                    let column_count: i64 = row.get(2);
                    tables.push(RemoteTableInfo { name, column_count, row_count });
                }
                Ok(tables)
            }
            _ => Err(Error::Other("该数据库类型不支持表浏览".into())),
        }
    }

    /// 查询远程表数据（仅支持 MySQL / PostgreSQL）。
    pub async fn query_table(
        profile: &ConnectionProfile,
        table: &str,
        limit: i64,
        offset: i64,
    ) -> Result<RemoteQueryResult, Error> {
        let safe_limit = if limit < 1 { 100 } else { limit };
        let safe_offset = if offset < 0 { 0 } else { offset };
        match profile.driver {
            DbDriver::Mysql => {
                let safe_table = table.replace('`', "``");
                let sql = format!(
                    "SELECT * FROM `{safe_table}` LIMIT {safe_limit} OFFSET {safe_offset}"
                );
                Self::execute(profile, &sql).await
            }
            DbDriver::Postgres => {
                let safe_table = table.replace('"', "\"\"");
                let sql = format!(
                    "SELECT * FROM \"{safe_table}\" LIMIT {safe_limit} OFFSET {safe_offset}"
                );
                Self::execute(profile, &sql).await
            }
            _ => Err(Error::Other("该数据库类型不支持表浏览".into())),
        }
    }

    /// 执行任意 SQL（仅支持 MySQL / PostgreSQL）。
    pub async fn execute(
        profile: &ConnectionProfile,
        sql: &str,
    ) -> Result<RemoteQueryResult, Error> {
        let trimmed = sql.trim();
        let upper = trimmed.to_uppercase();
        let is_query = upper.starts_with("SELECT")
            || upper.starts_with("WITH")
            || upper.starts_with("SHOW")
            || upper.starts_with("DESC")
            || upper.starts_with("EXPLAIN")
            || upper.starts_with("PRAGMA");

        match profile.driver {
            DbDriver::Mysql => {
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let opts = profile.mysql_opts(&host, port)?;
                let pool = mysql_async::Pool::new(opts);
                let result = async {
                    let mut conn = pool
                        .get_conn()
                        .await
                        .map_err(|e| Error::Other(format!("MySQL 连接失败: {e}")))?;
                    let db = profile.database.as_deref().unwrap_or("");
                    if !db.is_empty() {
                        conn.query::<mysql_async::Row, _>(
                            format!("USE `{}`", db.replace('`', "``"))
                        ).await.map_err(|e| Error::Other(format!("MySQL 切库失败: {e}")))?;
                    }
                    if is_query {
                        let mut result_set = conn
                            .query_iter(trimmed)
                            .await
                            .map_err(|e| Error::Other(format!("MySQL 查询失败: {e}")))?;
                        let cols: Vec<String> = result_set
                            .columns_ref()
                            .iter()
                            .map(|c| c.name_str().to_string())
                            .collect();
                        let rows: Vec<mysql_async::Row> = result_set
                            .collect()
                            .await
                            .map_err(|e| Error::Other(format!("MySQL 行读取失败: {e}")))?;
                        let mut rows_out = Vec::new();
                        for row in &rows {
                            let values: Vec<serde_json::Value> = (0..cols.len())
                                .map(|i| mysql_value_to_json(row.get(i).unwrap_or(mysql_async::Value::NULL)))
                                .collect();
                            rows_out.push(values);
                        }
                        let affected = rows_out.len();
                        Ok::<RemoteQueryResult, Error>(RemoteQueryResult {
                            columns: cols,
                            rows: rows_out,
                            affected,
                        })
                    } else {
                        let mut result_set = conn
                            .query_iter(trimmed)
                            .await
                            .map_err(|e| Error::Other(format!("MySQL 执行失败: {e}")))?;
                        // 消费剩余行以避免连接状态错误
                        let _: Vec<mysql_async::Row> = result_set.collect().await.map_err(|e| Error::Other(format!("MySQL 结果清理失败: {e}")))?;
                        Ok(RemoteQueryResult {
                            columns: vec![],
                            rows: vec![],
                            affected: result_set.affected_rows() as usize,
                        })
                    }
                }
                .await;
                pool.disconnect().await.ok();
                result
            }
            DbDriver::Postgres => {
                let (host, port, _tunnel) = Self::resolve_endpoint(profile).await?;
                let client = Self::pg_connect(profile, &host, port).await?;
                if is_query {
                    let stmt = client
                        .prepare(trimmed)
                        .await
                        .map_err(|e| Error::Other(format!("PostgreSQL 预编译失败: {e}")))?;
                    let cols: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
                    let rows = client
                        .query(&stmt, &[])
                        .await
                        .map_err(|e| Error::Other(format!("PostgreSQL 查询失败: {e}")))?;
                    let mut rows_out = Vec::new();
                    for row in &rows {
                        let values: Vec<serde_json::Value> = (0..cols.len())
                            .map(|i| pg_value_to_json(row, i))
                            .collect();
                        rows_out.push(values);
                    }
                    let affected = rows_out.len();
                    Ok(RemoteQueryResult {
                        columns: cols,
                        rows: rows_out,
                        affected,
                    })
                } else {
                    let affected = client
                        .execute(trimmed, &[])
                        .await
                        .map_err(|e| Error::Other(format!("PostgreSQL 执行失败: {e}")))?;
                    Ok(RemoteQueryResult {
                        columns: vec![],
                        rows: vec![],
                        affected: affected as usize,
                    })
                }
            }
            _ => Err(Error::Other("该数据库类型不支持 SQL 执行".into())),
        }
    }
}
