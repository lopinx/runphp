//! 数据库服务端管理：检测接管、便携托管与建库建用户。
//!
//! 与 `remote.rs`（客户端连接档案）互补：本模块面向本机数据库服务——
//! - 检测：端口探测 + 常见安装目录二进制扫描 + 系统服务名匹配
//! - 接管（takeover）：登记本机已安装的系统服务，启停走 sc/systemctl
//! - 便携托管（portable）：导入本地二进制或下载便携包，首次启动自动初始化数据目录
//! - 管理：以 root 凭据连 127.0.0.1 执行建库/建用户/改密（复用 RemoteDbManager）
//!
//! SQL 一律经标识符校验 + 转义拼接，不使用字符串裸拼用户输入。

use crate::db::remote::{ConnectionProfile, DbDriver, RemoteDbManager};
use crate::services::{
    probe_port, wait_port_ready, ManagedService, ServiceKind, ServiceManager, ServiceSource,
};
use crate::{AppConfig, Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// 数据库服务管理器。
pub struct DbServiceManager {
    cfg: AppConfig,
    services: ServiceManager,
}

/// 检测到的本机数据库服务候选。
#[derive(Debug, Clone, Serialize)]
pub struct DbServiceCandidate {
    /// 服务类型。
    pub kind: ServiceKind,
    /// 显示名称。
    pub name: String,
    /// 默认端口。
    pub port: u16,
    /// 端口是否可达（服务运行中）。
    pub running: bool,
    /// 检测到的服务端二进制（可便携导入；接管场景仅作展示）。
    pub binary_path: Option<PathBuf>,
    /// 匹配到的系统服务名（Windows 服务名 / systemd 单元名）。
    pub os_service_name: Option<String>,
}

/// 注册/导入服务的请求参数（接管与便携共用，按是否提供 binary_path 区分来源）。
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceInput {
    pub kind: ServiceKind,
    pub name: String,
    pub port: u16,
    /// 便携二进制绝对路径（提供则注册为便携托管，否则为接管）。
    #[serde(default)]
    pub binary_path: Option<PathBuf>,
    /// 接管场景的系统服务名。
    #[serde(default)]
    pub os_service_name: Option<String>,
    #[serde(default)]
    pub root_username: String,
    #[serde(default)]
    pub root_password: String,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// 服务端数据库账号。
#[derive(Debug, Clone, Serialize)]
pub struct ServiceDbUser {
    pub username: String,
    /// MySQL 的 host 字段（PostgreSQL 恒为空串）。
    pub host: String,
}

/// 在线下载预设。
#[derive(Debug, Clone, Serialize)]
pub struct DownloadPreset {
    pub kind: ServiceKind,
    pub label: String,
    pub url: String,
    /// 体积提示（展示用）。
    pub size_hint: String,
}

/// 引擎检测规格。
struct EngineSpec {
    kind: ServiceKind,
    /// 服务端二进制候选名（Windows 自动补 .exe）。
    binary: &'static str,
    /// 初始化辅助二进制候选名（按顺序在主二进制同级目录查找）。
    init_helpers: &'static [&'static str],
    win_services: &'static [&'static str],
    systemd_units: &'static [&'static str],
}

const ENGINE_SPECS: &[EngineSpec] = &[
    EngineSpec {
        kind: ServiceKind::Mysql,
        binary: "mysqld",
        init_helpers: &["mysql_install_db"],
        win_services: &["MySQL84", "MySQL80", "MySQL57", "MySQL"],
        systemd_units: &["mysqld", "mysql"],
    },
    EngineSpec {
        kind: ServiceKind::Mariadb,
        binary: "mariadbd",
        init_helpers: &["mariadb-install-db", "mysql_install_db"],
        win_services: &["MariaDB"],
        systemd_units: &["mariadb"],
    },
    EngineSpec {
        kind: ServiceKind::Postgresql,
        binary: "postgres",
        init_helpers: &["initdb"],
        win_services: &["postgresql-x64-17", "postgresql-x64-16", "postgresql-x64-15", "postgresql-x64-14"],
        systemd_units: &["postgresql"],
    },
    EngineSpec {
        kind: ServiceKind::Redis,
        binary: "redis-server",
        init_helpers: &[],
        win_services: &["Redis", "Memurai"],
        systemd_units: &["redis-server", "redis"],
    },
];

impl EngineSpec {
    fn by_kind(kind: ServiceKind) -> &'static EngineSpec {
        ENGINE_SPECS
            .iter()
            .find(|s| s.kind == kind)
            .expect("ENGINE_SPECS 覆盖全部数据库引擎")
    }
}

impl DbServiceManager {
    pub fn new(cfg: AppConfig) -> Self {
        Self {
            services: ServiceManager::new(cfg.clone()),
            cfg,
        }
    }

    /// 按 id 取服务定义。
    pub fn get_service(&self, id: &str) -> Result<ManagedService> {
        self.services.get(id)
    }

    /// 读取服务日志末尾若干行。
    pub fn read_log(&self, id: &str, lines: usize) -> Result<String> {
        self.services.read_log(id, lines)
    }

    /// 把受管服务注册为连接档案（供连接页浏览/SQL/Adminer 复用）。
    pub fn register_connection(&self, id: &str) -> Result<ConnectionProfile> {
        let svc = self.services.get(id)?;
        let profile = Self::admin_profile(&svc)?;
        RemoteDbManager::new(&self.cfg.data_dir).add(profile.clone())?;
        Ok(profile)
    }

    /// 列出全部受管数据库服务。
    pub fn list(&self) -> Result<Vec<ManagedService>> {
        Ok(self
            .services
            .list()?
            .into_iter()
            .filter(|s| s.kind != ServiceKind::Ftp)
            .collect())
    }

    /// 检测本机数据库服务（端口 + 二进制 + 系统服务名）。
    pub async fn detect(&self) -> Result<Vec<DbServiceCandidate>> {
        let registered: Vec<ServiceKind> = self
            .services
            .list()?
            .into_iter()
            .map(|s| s.kind)
            .collect();
        let mut out = Vec::new();
        for spec in ENGINE_SPECS {
            let port = spec.kind.default_port();
            let running = probe_port(port).await;
            let binary_path = scan_engine_binary(spec);
            let os_service_name = detect_os_service(spec).await;
            // 已注册同类型服务或三项信号全空时不产出候选
            let interesting = running || binary_path.is_some() || os_service_name.is_some();
            if interesting && !registered.contains(&spec.kind) {
                out.push(DbServiceCandidate {
                    kind: spec.kind,
                    name: spec.kind.display_name().to_string(),
                    port,
                    running,
                    binary_path,
                    os_service_name,
                });
            }
        }
        Ok(out)
    }

    /// 注册服务：提供 binary_path 走便携托管，否则按接管登记。
    pub fn register(&self, input: ServiceInput) -> Result<ManagedService> {
        if input.name.trim().is_empty() {
            return Err(Error::Config("服务名称不能为空".into()));
        }
        let mut svc = ManagedService::new(input.kind, input.name.trim().to_string(), input.port);
        svc.autostart = input.autostart;
        svc.root_username = input.root_username;
        svc.root_password = input.root_password;
        svc.extra_args = input.extra_args;
        if let Some(bin) = input.binary_path.filter(|p| p.is_file()) {
            svc.source = ServiceSource::Portable;
            svc.binary_path = Some(bin);
            std::fs::create_dir_all(self.services.service_data_dir(&svc.id))?;
        } else {
            svc.source = ServiceSource::Takeover;
            svc.os_service_name = input.os_service_name;
        }
        self.services.upsert(svc.clone())?;
        Ok(svc)
    }

    /// 更新服务定义（端口/自启/凭据等；来源与类型不可变）。
    pub fn update(&self, service: ManagedService) -> Result<()> {
        let existing = self.services.get(&service.id)?;
        let mut svc = service;
        svc.source = existing.source;
        svc.kind = existing.kind;
        svc.created_at = existing.created_at;
        self.services.upsert(svc)
    }

    /// 删除服务注册（便携服务先尽力停止）。
    pub async fn remove(&self, id: &str) -> Result<()> {
        let _ = self.services.stop(id).await;
        self.services.remove(id)
    }

    /// 启动服务：便携服务先做数据目录初始化，再托管进程并等待端口就绪。
    pub async fn start(&self, id: &str) -> Result<()> {
        let svc = self.services.get(id)?;
        if svc.source == ServiceSource::Portable {
            self.ensure_bootstrap(&svc).await?;
        }
        self.services.start(id).await?;
        if !wait_port_ready(svc.port, 60).await {
            let log = self.services.read_log(id, 10).unwrap_or_default();
            return Err(Error::Other(format!(
                "服务 {} 启动后 60 秒内端口 {} 未就绪。日志末尾：\n{log}",
                svc.name, svc.port
            )));
        }
        Ok(())
    }

    /// 停止服务。
    pub async fn stop(&self, id: &str) -> Result<()> {
        self.services.stop(id).await
    }

    /// 查询服务状态。
    pub async fn status(&self, id: &str) -> Result<crate::services::ServiceStatus> {
        self.services.status(id).await
    }

    /// 便携服务首次启动前的数据目录初始化。
    async fn ensure_bootstrap(&self, svc: &ManagedService) -> Result<()> {
        let data = self.services.service_data_dir(&svc.id);
        match svc.kind {
            ServiceKind::Redis | ServiceKind::Ftp => Ok(()),
            ServiceKind::Mysql | ServiceKind::Mariadb => {
                if mysql_initialized(&data) {
                    return Ok(());
                }
                let spec = EngineSpec::by_kind(svc.kind);
                let helper = find_helper(svc.binary_path.as_deref(), spec.init_helpers)
                    .ok_or_else(|| {
                        Error::Other(format!(
                            "未找到 {} 初始化工具（{}），请确认导入的是完整安装目录",
                            svc.kind.display_name(),
                            spec.init_helpers.join(" / ")
                        ))
                    })?;
                // MariaDB 用 install_db 脚本初始化；MySQL 用 mysqld --initialize-insecure
                let output = if svc.kind == ServiceKind::Mysql {
                    run_step(svc.binary_path.as_deref().unwrap(), &[
                        "--initialize-insecure",
                        "--datadir",
                        &data.to_string_lossy(),
                    ])
                    .await?
                } else {
                    run_step(&helper, &["--datadir", &data.to_string_lossy()]).await?
                };
                if !mysql_initialized(&data) {
                    return Err(Error::Other(format!(
                        "{} 数据目录初始化未生效: {}",
                        svc.kind.display_name(),
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
                Ok(())
            }
            ServiceKind::Postgresql => {
                if data.join("PG_VERSION").is_file() {
                    return Ok(());
                }
                let helper = find_helper(svc.binary_path.as_deref(), &["initdb"])
                    .ok_or_else(|| Error::Other("未找到 initdb 初始化工具".into()))?;
                let output = run_step(
                    &helper,
                    &[
                        "-D",
                        &data.to_string_lossy(),
                        "-U",
                        "postgres",
                        // 本地监听场景使用 trust，改密后仍可经 ALTER ROLE 生效
                        "--auth=trust",
                        "-E",
                        "UTF8",
                    ],
                )
                .await?;
                if !data.join("PG_VERSION").is_file() {
                    return Err(Error::Other(format!(
                        "PostgreSQL 数据目录初始化未生效: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
                Ok(())
            }
        }
    }

    // ---- 建库建用户（复用远程客户端的执行通道） ----

    /// 以服务 root 凭据构造连接档案（复用 RemoteDbManager 执行 SQL）。
    pub fn admin_profile(svc: &ManagedService) -> Result<ConnectionProfile> {
        let driver = match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => DbDriver::Mysql,
            ServiceKind::Postgresql => DbDriver::Postgres,
            ServiceKind::Redis => DbDriver::Redis,
            ServiceKind::Ftp => return Err(Error::Other("FTP 服务无数据库操作".into())),
        };
        let username = if svc.root_username.is_empty() {
            match svc.kind {
                ServiceKind::Postgresql => "postgres".to_string(),
                ServiceKind::Mysql | ServiceKind::Mariadb => "root".to_string(),
                _ => String::new(),
            }
        } else {
            svc.root_username.clone()
        };
        let mut p = ConnectionProfile::new(svc.name.clone(), driver, "127.0.0.1".into(), svc.port);
        p.username = username;
        p.password = svc.root_password.clone();
        Ok(p)
    }

    /// 执行管理 SQL 并要求无结果集返回（DDL）。
    async fn admin_exec(svc: &ManagedService, sql: &str) -> Result<()> {
        let profile = Self::admin_profile(svc)?;
        RemoteDbManager::execute(&profile, sql).await.map(|_| ())
    }

    /// 列出用户数据库（过滤系统库）。
    pub async fn list_databases(&self, id: &str) -> Result<Vec<String>> {
        let svc = self.services.get(id)?;
        let profile = Self::admin_profile(&svc)?;
        let sql = match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => {
                "SELECT schema_name FROM information_schema.schemata"
            }
            ServiceKind::Postgresql => {
                "SELECT datname FROM pg_database WHERE NOT datistemplate AND datname <> 'postgres'"
            }
            _ => return Err(Error::Other("该服务类型无数据库列表概念".into())),
        };
        let result = RemoteDbManager::execute(&profile, sql).await?;
        let system: &[&str] = &["information_schema", "mysql", "performance_schema", "sys"];
        Ok(result
            .rows
            .iter()
            .filter_map(|r| r.first().and_then(|v| v.as_str()))
            .map(|s| s.to_string())
            .filter(|s| !system.contains(&s.as_str()))
            .collect())
    }

    /// 创建数据库。
    pub async fn create_database(&self, id: &str, name: &str) -> Result<()> {
        let svc = self.services.get(id)?;
        validate_ident(name)?;
        let sql = match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => format!(
                "CREATE DATABASE {} DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci",
                mysql_ident(name)
            ),
            ServiceKind::Postgresql => format!("CREATE DATABASE {}", pg_ident(name)),
            _ => return Err(Error::Other("该服务类型不支持建库".into())),
        };
        Self::admin_exec(&svc, &sql).await
    }

    /// 删除数据库。
    pub async fn drop_database(&self, id: &str, name: &str) -> Result<()> {
        let svc = self.services.get(id)?;
        validate_ident(name)?;
        let sql = match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => format!("DROP DATABASE {}", mysql_ident(name)),
            ServiceKind::Postgresql => format!("DROP DATABASE {}", pg_ident(name)),
            _ => return Err(Error::Other("该服务类型不支持删库".into())),
        };
        Self::admin_exec(&svc, &sql).await
    }

    /// 列出用户账号（过滤系统账号）。
    pub async fn list_users(&self, id: &str) -> Result<Vec<ServiceDbUser>> {
        let svc = self.services.get(id)?;
        let profile = Self::admin_profile(&svc)?;
        let result = match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => {
                RemoteDbManager::execute(
                    &profile,
                    "SELECT User, Host FROM mysql.user WHERE User NOT IN ('mysql.sys','mysql.session','mysql.infoschema','root')",
                )
                .await?
            }
            ServiceKind::Postgresql => {
                RemoteDbManager::execute(
                    &profile,
                    "SELECT rolname, '' FROM pg_roles WHERE rolname !~ '^pg_' AND rolname <> 'postgres' AND rolcanlogin",
                )
                .await?
            }
            _ => return Err(Error::Other("该服务类型无账号概念".into())),
        };
        Ok(result
            .rows
            .iter()
            .filter_map(|r| {
                let username = r.first().and_then(|v| v.as_str())?.to_string();
                let host = r.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string();
                Some(ServiceDbUser { username, host })
            })
            .collect())
    }

    /// 创建账号；指定 database 时授予该库全部权限。
    pub async fn create_user(
        &self,
        id: &str,
        username: &str,
        password: &str,
        database: Option<&str>,
    ) -> Result<()> {
        let svc = self.services.get(id)?;
        validate_ident(username)?;
        if password.is_empty() {
            return Err(Error::Config("密码不能为空".into()));
        }
        match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => {
                Self::admin_exec(
                    &svc,
                    &format!(
                        "CREATE USER {} IDENTIFIED BY {}",
                        mysql_user_host(username, "%"),
                        sql_str(password)
                    ),
                )
                .await?;
                if let Some(db) = database {
                    validate_ident(db)?;
                    Self::admin_exec(
                        &svc,
                        &format!(
                            "GRANT ALL PRIVILEGES ON {}.* TO {}",
                            mysql_ident(db),
                            mysql_user_host(username, "%")
                        ),
                    )
                    .await?;
                }
                Ok(())
            }
            ServiceKind::Postgresql => {
                Self::admin_exec(
                    &svc,
                    &format!("CREATE ROLE {} LOGIN PASSWORD {}", pg_ident(username), sql_str(password)),
                )
                .await?;
                if let Some(db) = database {
                    validate_ident(db)?;
                    Self::admin_exec(
                        &svc,
                        &format!(
                            "GRANT ALL PRIVILEGES ON DATABASE {} TO {}",
                            pg_ident(db),
                            pg_ident(username)
                        ),
                    )
                    .await?;
                }
                Ok(())
            }
            _ => Err(Error::Other("该服务类型不支持建账号".into())),
        }
    }

    /// 删除账号。
    pub async fn drop_user(&self, id: &str, username: &str, host: &str) -> Result<()> {
        let svc = self.services.get(id)?;
        validate_ident(username)?;
        let sql = match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => {
                let h = if host.is_empty() { "%" } else { host };
                format!("DROP USER {}", mysql_user_host(username, h))
            }
            ServiceKind::Postgresql => format!("DROP ROLE {}", pg_ident(username)),
            _ => return Err(Error::Other("该服务类型不支持删账号".into())),
        };
        Self::admin_exec(&svc, &sql).await
    }

    /// 修改账号密码。
    pub async fn set_user_password(
        &self,
        id: &str,
        username: &str,
        host: &str,
        password: &str,
    ) -> Result<()> {
        let svc = self.services.get(id)?;
        validate_ident(username)?;
        if password.is_empty() {
            return Err(Error::Config("密码不能为空".into()));
        }
        let sql = match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => {
                let h = if host.is_empty() { "%" } else { host };
                format!(
                    "ALTER USER {} IDENTIFIED BY {}",
                    mysql_user_host(username, h),
                    sql_str(password)
                )
            }
            ServiceKind::Postgresql => {
                format!("ALTER ROLE {} WITH PASSWORD {}", pg_ident(username), sql_str(password))
            }
            _ => return Err(Error::Other("该服务类型不支持改密".into())),
        };
        Self::admin_exec(&svc, &sql).await
    }

    /// 修改 root 密码（同步更新注册表中的凭据）。
    pub async fn set_root_password(&self, id: &str, password: &str) -> Result<()> {
        let mut svc = self.services.get(id)?;
        if password.is_empty() {
            return Err(Error::Config("密码不能为空".into()));
        }
        match svc.kind {
            ServiceKind::Mysql | ServiceKind::Mariadb => {
                let root_user = if svc.root_username.is_empty() {
                    "root"
                } else {
                    &svc.root_username
                };
                Self::admin_exec(
                    &svc,
                    &format!(
                        "ALTER USER {} IDENTIFIED BY {}",
                        mysql_user_host(root_user, "localhost"),
                        sql_str(password)
                    ),
                )
                .await?;
            }
            ServiceKind::Postgresql => {
                let root_user = if svc.root_username.is_empty() {
                    "postgres"
                } else {
                    &svc.root_username
                };
                Self::admin_exec(
                    &svc,
                    &format!(
                        "ALTER ROLE {} WITH PASSWORD {}",
                        pg_ident(root_user),
                        sql_str(password)
                    ),
                )
                .await?;
            }
            ServiceKind::Redis => return self.set_redis_password(id, password).await,
            ServiceKind::Ftp => return Err(Error::Other("FTP 服务无此操作".into())),
        }
        svc.root_password = password.to_string();
        self.services.upsert(svc)
    }

    /// 设置 Redis requirepass（在线生效 + 更新启动参数与注册表，重启不丢失）。
    pub async fn set_redis_password(&self, id: &str, password: &str) -> Result<()> {
        let mut svc = self.services.get(id)?;
        if svc.kind != ServiceKind::Redis {
            return Err(Error::Other("仅 Redis 服务支持该操作".into()));
        }
        let addr = format!("127.0.0.1:{}", svc.port);
        let resp = redis_command(&addr, &svc.root_password, &["CONFIG", "SET", "requirepass", password])
            .await?;
        if !resp.starts_with('+') {
            return Err(Error::Other(format!("Redis 设置密码失败: {resp}")));
        }
        // 同步启动参数与注册表凭据，保证重启后密码仍生效
        svc.extra_args = redis_password_args(&svc.extra_args, password);
        svc.root_password = password.to_string();
        self.services.upsert(svc)
    }
}

// ---- 在线下载 ----

/// Windows 平台便携包下载预设（体积大，均为官方/社区发布源，可被自定义 URL 覆盖）。
pub fn download_presets() -> Vec<DownloadPreset> {
    if cfg!(windows) {
        vec![
            DownloadPreset {
                kind: ServiceKind::Redis,
                label: "Redis 5.0.14（tporadowski Windows 构建）".into(),
                url: "https://github.com/tporadowski/redis/releases/download/v5.0.14.1/Redis-x64-5.0.14.1.zip".into(),
                size_hint: "约 10 MB".into(),
            },
            DownloadPreset {
                kind: ServiceKind::Mariadb,
                label: "MariaDB 11.4（官方 winx64 便携包）".into(),
                url: "https://archive.mariadb.org/mariadb-11.4.5/winx64-packages/mariadb-11.4.5-winx64.zip".into(),
                size_hint: "约 850 MB".into(),
            },
            DownloadPreset {
                kind: ServiceKind::Mysql,
                label: "MySQL 8.0（官方 winx64 便携包）".into(),
                url: "https://cdn.mysql.com/Downloads/MySQL-8.0/mysql-8.0.43-winx64.zip".into(),
                size_hint: "约 280 MB".into(),
            },
            DownloadPreset {
                kind: ServiceKind::Postgresql,
                label: "PostgreSQL 16（EDB 官方二进制包）".into(),
                url: "https://get.enterprisedb.com/postgresql/postgresql-16.9-1-windows-x64-binaries.zip".into(),
                size_hint: "约 360 MB".into(),
            },
        ]
    } else {
        // Linux 便携包碎片化严重，建议用系统包管理器安装后接管
        Vec::new()
    }
}

impl DbServiceManager {
    /// 下载便携包并注册服务：下载 zip → 解压到 services/<id>/engine/ → 定位二进制。
    pub async fn download_install(
        &self,
        kind: ServiceKind,
        name: &str,
        url: &str,
        on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<ManagedService> {
        if !kind.is_db() {
            return Err(Error::Config("仅支持数据库引擎下载".into()));
        }
        // 先生成 id 以规划目录，再走 register 落注册表
        let probe = ManagedService::new(kind, name.to_string(), 0);
        let engine_dir = self.services.service_dir(&probe.id).join("engine");
        std::fs::create_dir_all(&engine_dir)?;

        let tmp = engine_dir.join("download.zip");
        let result = download_and_extract(url, &tmp, &engine_dir, on_progress).await;
        std::fs::remove_file(&tmp).ok();
        result?;

        // 递归定位服务端二进制（zip 通常带一层版本目录）
        let spec = EngineSpec::by_kind(kind);
        let binary = find_binary_recursive(&engine_dir, spec.binary)
            .ok_or_else(|| Error::Other(format!("解压后未找到 {} 二进制", spec.binary)))?;

        self.register(ServiceInput {
            kind,
            name: name.to_string(),
            port: 0,
            binary_path: Some(binary),
            os_service_name: None,
            root_username: String::new(),
            root_password: String::new(),
            autostart: false,
            extra_args: Vec::new(),
        })
    }
}

/// 流式下载 zip 并解压（复用运行时下载实现）。
async fn download_and_extract(
    url: &str,
    tmp: &Path,
    dest: &Path,
    on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("RunPHP/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(Error::Other(format!("下载失败: HTTP {}", resp.status())));
    }
    let total = resp.content_length().unwrap_or(0);
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut file = std::fs::File::create(tmp)?;
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        std::io::Write::write_all(&mut file, &chunk)?;
        downloaded += chunk.len() as u64;
        if let Some(cb) = &on_progress {
            cb(downloaded, total);
        }
    }
    drop(file);
    crate::runtime::extract_zip(tmp, dest)
}

// ---- 检测辅助 ----

impl ServiceKind {
    /// 是否为数据库引擎（排除 FTP）。
    pub fn is_db(self) -> bool {
        !matches!(self, ServiceKind::Ftp)
    }
}

/// 在常见安装位置扫描引擎二进制。
fn scan_engine_binary(spec: &EngineSpec) -> Option<PathBuf> {
    common_engine_dirs(spec.kind)
        .into_iter()
        .map(|d| join_binary(&d, spec.binary))
        .find(|p| p.is_file())
}

/// 引擎常见安装目录（Windows 官方安装器默认位置 + 各平台包管理器位置）。
fn common_engine_dirs(kind: ServiceKind) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            let pf = PathBuf::from(pf);
            match kind {
                ServiceKind::Mysql => {
                    if let Ok(entries) = std::fs::read_dir(pf.join("MySQL")) {
                        for e in entries.flatten() {
                            dirs.push(e.path().join("bin"));
                        }
                    }
                }
                ServiceKind::Mariadb => {
                    if let Ok(entries) = std::fs::read_dir(&pf) {
                        for e in entries.flatten() {
                            let n = e.file_name().to_string_lossy().to_lowercase();
                            if n.starts_with("mariadb") {
                                dirs.push(e.path().join("bin"));
                            }
                        }
                    }
                }
                ServiceKind::Postgresql => {
                    if let Ok(entries) = std::fs::read_dir(pf.join("PostgreSQL")) {
                        for e in entries.flatten() {
                            dirs.push(e.path().join("bin"));
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(home) = std::env::var_os("USERPROFILE") {
            dirs.push(PathBuf::from(home).join("Redis"));
        }
    }
    #[cfg(not(windows))]
    {
        dirs.push(PathBuf::from("/usr/sbin"));
        dirs.push(PathBuf::from("/usr/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        if kind == ServiceKind::Postgresql {
            for ver in ["17", "16", "15", "14", "13"] {
                dirs.push(PathBuf::from(format!("/usr/lib/postgresql/{ver}/bin")));
            }
        }
    }
    dirs
}

/// 平台相关二进制名拼接（Windows 补 .exe）。
fn join_binary(dir: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        dir.join(format!("{name}.exe"))
    } else {
        dir.join(name)
    }
}

/// 在主二进制同级目录查找初始化辅助工具。
fn find_helper(binary: Option<&Path>, names: &[&str]) -> Option<PathBuf> {
    let dir = binary?.parent()?;
    names
        .iter()
        .map(|n| join_binary(dir, n))
        .find(|p| p.is_file())
}

/// 匹配系统服务名（Windows 查询 sc；Linux 查单元文件是否存在）。
async fn detect_os_service(spec: &EngineSpec) -> Option<String> {
    if cfg!(windows) {
        for name in spec.win_services {
            if let Ok(out) = Command::new("sc").arg("query").arg(name).output().await {
                if out.status.success() {
                    return Some(name.to_string());
                }
            }
        }
    } else {
        const UNIT_BASES: &[&str] = &[
            "/etc/systemd/system",
            "/lib/systemd/system",
            "/usr/lib/systemd/system",
        ];
        for name in spec.systemd_units {
            let installed = UNIT_BASES
                .iter()
                .any(|base| PathBuf::from(base).join(format!("{name}.service")).exists());
            if installed {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// 递归查找目录内的二进制（限深度 4，优先浅层命中）。
fn find_binary_recursive(root: &Path, name: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
        if depth > 4 {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                let file_name = p.file_name()?.to_string_lossy().to_lowercase();
                let want = if cfg!(windows) {
                    format!("{}.exe", name.to_lowercase())
                } else {
                    name.to_lowercase()
                };
                if file_name == want {
                    return Some(p);
                }
            } else if p.is_dir() {
                subdirs.push(p);
            }
        }
        for sub in subdirs {
            if let Some(hit) = walk(&sub, name, depth + 1) {
                return Some(hit);
            }
        }
        None
    }
    walk(root, name, 0)
}

/// MySQL/MariaDB 数据目录是否已完成初始化。
fn mysql_initialized(data: &Path) -> bool {
    data.join("mysql").is_dir()
}

/// 执行初始化命令（最长等待 5 分钟，MySQL 初始化可能较慢）。
async fn run_step(binary: &Path, args: &[&str]) -> Result<std::process::Output> {
    tokio::time::timeout(
        std::time::Duration::from_secs(300),
        Command::new(binary).args(args).output(),
    )
    .await
    .map_err(|_| Error::Other("初始化超时（5 分钟）".into()))?
    .map_err(|e| Error::Other(format!("执行初始化失败: {e}")))
}

/// 对目标 Redis 发送单条命令（可选先 AUTH）。
async fn redis_command(addr: &str, password: &str, args: &[&str]) -> Result<String> {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| Error::Other(format!("Redis 连接失败: {e}")))?;
    if !password.is_empty() {
        let auth = crate::db::remote::format_resp_command(&["AUTH", password]);
        stream.write_all(auth.as_bytes()).await?;
        let mut buf = [0u8; 128];
        let n = stream.read(&mut buf).await?;
        let resp = String::from_utf8_lossy(&buf[..n]).to_string();
        if !resp.starts_with("+") {
            return Err(Error::Other(format!("Redis 鉴权失败: {resp}")));
        }
    }
    let cmd = crate::db::remote::format_resp_command(args);
    stream.write_all(cmd.as_bytes()).await?;
    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
}

/// 更新 Redis 启动参数中的 requirepass（保留其余参数）。
fn redis_password_args(current: &[String], password: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut skip_next = false;
    for arg in current {
        if skip_next {
            skip_next = false;
            continue;
        }
        let lower = arg.to_lowercase();
        if lower == "--requirepass" {
            skip_next = true;
            continue;
        }
        if let Some(val) = lower.strip_prefix("--requirepass=") {
            let _ = val;
            continue;
        }
        out.push(arg.clone());
    }
    out.push("--requirepass".into());
    out.push(password.into());
    out
}

// ---- SQL 转义与校验 ----

/// 标识符校验：字母/数字/下划线/连字符，1-64 位。
fn validate_ident(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(Error::Config(
            "名称仅允许字母、数字、下划线、连字符，长度 1-64".into(),
        ))
    }
}

/// SQL 字符串字面量（单引号转义）。
fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// MySQL 标识符（反引号转义）。
fn mysql_ident(s: &str) -> String {
    format!("`{}`", s.replace('`', "``"))
}

/// PostgreSQL 标识符（双引号转义）。
fn pg_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// MySQL 用户@主机字面量。
fn mysql_user_host(user: &str, host: &str) -> String {
    format!("'{}'@'{}'", user.replace('\'', "''"), host.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 标识符校验() {
        assert!(validate_ident("shop_db-1").is_ok());
        assert!(validate_ident("").is_err());
        assert!(validate_ident("a".repeat(65).as_str()).is_err());
        assert!(validate_ident("db;drop").is_err());
        assert!(validate_ident("带中文").is_err());
    }

    #[test]
    fn sql_转义() {
        assert_eq!(sql_str("pa'ss"), "'pa''ss'");
        assert_eq!(mysql_ident("d`b"), "`d``b`");
        assert_eq!(pg_ident("d\"b"), "\"d\"\"b\"");
        assert_eq!(mysql_user_host("u'x", "%"), "'u''x'@'%'");
    }

    #[test]
    fn redis_密码参数更新() {
        let base = vec!["--port".to_string(), "6380".to_string(), "--requirepass".to_string(), "old".to_string()];
        let out = redis_password_args(&base, "new");
        assert_eq!(out, vec!["--port", "6380", "--requirepass", "new"]);

        let eq_style = vec!["--requirepass=old".to_string()];
        assert_eq!(redis_password_args(&eq_style, "n2"), vec!["--requirepass", "n2"]);

        let empty: Vec<String> = vec![];
        assert_eq!(redis_password_args(&empty, "p"), vec!["--requirepass", "p"]);
    }

    #[test]
    fn mysql_初始化判定() {
        let dir = std::env::temp_dir().join("runphp-dbsvc-init-test");
        std::fs::remove_dir_all(&dir).ok();
        assert!(!mysql_initialized(&dir));
        std::fs::create_dir_all(dir.join("mysql")).unwrap();
        assert!(mysql_initialized(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 递归查找二进制() {
        let dir = std::env::temp_dir().join("runphp-dbsvc-find-test");
        std::fs::remove_dir_all(&dir).ok();
        let nested = dir.join("pkg").join("bin");
        std::fs::create_dir_all(&nested).unwrap();
        #[cfg(windows)]
        let bin = nested.join("mysqld.exe");
        #[cfg(not(windows))]
        let bin = nested.join("mysqld");
        std::fs::write(&bin, b"x").unwrap();
        assert_eq!(find_binary_recursive(&dir, "mysqld"), Some(bin));
        assert_eq!(find_binary_recursive(&dir, "nope"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn 注册接管与便携服务() {
        let dir = std::env::temp_dir().join("runphp-dbsvc-reg-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = AppConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let mgr = DbServiceManager::new(cfg);

        // 接管：无 binary_path
        let takeover = mgr
            .register(ServiceInput {
                kind: ServiceKind::Mysql,
                name: "本机 MySQL".into(),
                port: 3306,
                binary_path: None,
                os_service_name: Some("MySQL80".into()),
                root_username: "root".into(),
                root_password: String::new(),
                autostart: false,
                extra_args: vec![],
            })
            .unwrap();
        assert_eq!(takeover.source, ServiceSource::Takeover);
        assert_eq!(mgr.list().unwrap().len(), 1);

        // 便携：提供 binary_path（伪造文件）
        let bin = dir.join("mariadbd.exe");
        std::fs::write(&bin, b"x").unwrap();
        let portable = mgr
            .register(ServiceInput {
                kind: ServiceKind::Mariadb,
                name: "便携 MariaDB".into(),
                port: 3307,
                binary_path: Some(bin),
                os_service_name: None,
                root_username: String::new(),
                root_password: String::new(),
                autostart: true,
                extra_args: vec![],
            })
            .unwrap();
        assert_eq!(portable.source, ServiceSource::Portable);
        assert!(mgr.services.service_data_dir(&portable.id).exists());

        // 空名称拒绝
        assert!(mgr
            .register(ServiceInput {
                kind: ServiceKind::Redis,
                name: "  ".into(),
                port: 0,
                binary_path: None,
                os_service_name: None,
                root_username: String::new(),
                root_password: String::new(),
                autostart: false,
                extra_args: vec![],
            })
            .is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 管理账号默认用户名() {
        let mut mysql = ManagedService::new(ServiceKind::Mysql, "m".into(), 3306);
        let p = DbServiceManager::admin_profile(&mysql).unwrap();
        assert_eq!(p.username, "root");
        assert_eq!(p.driver, DbDriver::Mysql);

        let pg = ManagedService::new(ServiceKind::Postgresql, "p".into(), 5432);
        assert_eq!(DbServiceManager::admin_profile(&pg).unwrap().username, "postgres");

        mysql.kind = ServiceKind::Ftp;
        assert!(DbServiceManager::admin_profile(&mysql).is_err());
    }
}
