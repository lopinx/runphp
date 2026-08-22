//! FTP 服务端管理：虚拟用户 + 双后端。
//!
//! 后端选择：Linux 检测到 pure-ftpd/pure-pw 时优先对接 Pure-FTPd
//! （子进程托管，虚拟用户经 pure-pw 落 puredb）；其余环境（含 Windows）
//! 内嵌 libunftp FTP 服务端，认证与用户目录直接读 JSON 存储，即时生效。
//!
//! 虚拟用户存储为 `ftp_users.json`（JSON 为准源，Pure-FTPd 后端同步重建 puredb）；
//! 服务配置存储为 `ftpd.json`；用户默认根目录 `<数据目录>/ftp/<用户名>`，
//! 所有用户强制锁定（chroot）在各自根目录内。
//!
//! 密码沿用连接档案先例明文存储（本地工具用途），文档中有标注。

use crate::services::{probe_port, wait_port_ready, ManagedService, ServiceKind, ServiceManager, ServiceSource};
use crate::{AppConfig, Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

/// FTP 服务在服务注册表中的固定 id（Pure-FTPd 后端复用 ServiceManager 托管）。
const FTP_SERVICE_ID: &str = "ftp";

/// FTP 虚拟用户。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpUser {
    /// 唯一标识。
    pub id: String,
    /// 登录用户名（字母/数字/下划线/连字符）。
    pub username: String,
    /// 密码（明文存储，本地工具用途）。
    pub password: String,
    /// 自定义根目录（None 表示默认 `<数据目录>/ftp/<用户名>`）。
    #[serde(default)]
    pub home_dir: Option<String>,
    /// 关联站点 id（可选）。
    #[serde(default)]
    pub linked_site: Option<String>,
    /// 是否启用（禁用后拒绝登录）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 创建时间。
    pub created_at: String,
}

fn default_true() -> bool {
    true
}

impl FtpUser {
    /// 创建新用户（生成 id 与时间戳）。
    pub fn new(username: String, password: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            username,
            password,
            home_dir: None,
            linked_site: None,
            enabled: true,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// 用户注册表（持久化为 `ftp_users.json`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FtpUserRegistry {
    pub users: Vec<FtpUser>,
}

/// FTP 服务端配置（持久化为 `ftpd.json`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FtpdConfig {
    /// 控制端口。
    pub port: u16,
    /// 被动模式端口区间起止。
    pub passive_from: u16,
    pub passive_to: u16,
    /// 是否随应用自动启动。
    pub autostart: bool,
}

impl Default for FtpdConfig {
    fn default() -> Self {
        Self {
            port: 21,
            passive_from: 50000,
            passive_to: 50010,
            autostart: false,
        }
    }
}

/// 可用后端。
#[derive(Debug, Clone, Serialize)]
pub enum FtpdBackend {
    /// 内嵌 libunftp。
    Embedded,
    /// Pure-FTPd（携带二进制路径）。
    PureFtpd { binary: PathBuf },
}

/// 内嵌服务端实例句柄（进程内运行，停止即 abort）。
static EMBEDDED: Mutex<Option<tokio::task::JoinHandle<std::result::Result<(), libunftp::ServerError>>>> =
    Mutex::const_new(None);

/// FTP 服务端管理器。
pub struct FtpdManager {
    cfg: AppConfig,
    services: ServiceManager,
}

impl FtpdManager {
    pub fn new(cfg: AppConfig) -> Self {
        Self {
            services: ServiceManager::new(cfg.clone()),
            cfg,
        }
    }

    fn users_path(&self) -> PathBuf {
        self.cfg.data_dir.join("ftp_users.json")
    }

    fn config_path(&self) -> PathBuf {
        self.cfg.data_dir.join("ftpd.json")
    }

    /// Pure-FTPd 后端的 passwd / puredb 文件路径。
    fn pure_passwd(&self) -> PathBuf {
        self.cfg.data_dir.join("ftp").join("pureftpd.passwd")
    }

    fn pure_puredb(&self) -> PathBuf {
        self.cfg.data_dir.join("ftp").join("pureftpd.pdb")
    }

    /// 读取用户注册表。
    pub fn load_users(&self) -> Result<FtpUserRegistry> {
        let path = self.users_path();
        if !path.exists() {
            return Ok(FtpUserRegistry::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        serde_json::from_str(&raw).map_err(Error::Json)
    }

    fn save_users(&self, reg: &FtpUserRegistry) -> Result<()> {
        let raw = serde_json::to_string_pretty(reg)?;
        std::fs::write(self.users_path(), raw)?;
        Ok(())
    }

    /// 读取服务配置。
    pub fn config(&self) -> FtpdConfig {
        std::fs::read_to_string(self.config_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// 保存服务配置（运行中的内嵌/Pure-FTPd 服务需重启后生效）。
    pub fn save_config(&self, config: &FtpdConfig) -> Result<()> {
        let raw = serde_json::to_string_pretty(config)?;
        std::fs::create_dir_all(&self.cfg.data_dir)?;
        std::fs::write(self.config_path(), raw)?;
        Ok(())
    }

    /// 用户根目录（默认 `<数据目录>/ftp/<用户名>`）。
    pub fn user_home(&self, u: &FtpUser) -> PathBuf {
        match &u.home_dir {
            Some(h) if !h.trim().is_empty() => PathBuf::from(h),
            _ => self.cfg.data_dir.join("ftp").join(&u.username),
        }
    }

    // ---- 后端探测 ----

    /// 探测可用后端：Windows 恒为内嵌；Linux 找到 pure-ftpd 且同级有 pure-pw 时用 Pure-FTPd。
    pub fn detect_backend(&self) -> FtpdBackend {
        if cfg!(windows) {
            return FtpdBackend::Embedded;
        }
        let names = if cfg!(windows) { ["pure-ftpd.exe"] } else { ["pure-ftpd"] };
        let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .chain([PathBuf::from("/usr/sbin"), PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/sbin")])
            .collect();
        for dir in dirs {
            for name in names {
                let bin = dir.join(name);
                if bin.is_file() {
                    // pure-pw 必须在同级（虚拟用户管理依赖它）
                    let pw = if cfg!(windows) { dir.join("pure-pw.exe") } else { dir.join("pure-pw") };
                    if pw.is_file() {
                        return FtpdBackend::PureFtpd { binary: bin };
                    }
                }
            }
        }
        FtpdBackend::Embedded
    }

    /// 当前后端名称（展示用）。
    pub fn backend_name(&self) -> &'static str {
        match self.detect_backend() {
            FtpdBackend::Embedded => "内嵌 libunftp",
            FtpdBackend::PureFtpd { .. } => "Pure-FTPd",
        }
    }

    // ---- 生命周期 ----

    /// 启动 FTP 服务端（进程内运行，适用于桌面端/面板等常驻宿主），返回后端描述。
    ///
    /// CLI 等短生命周期宿主请用 [`FtpdManager::start_daemon`] 或 `ftpd run`。
    pub async fn start(&self) -> Result<&'static str> {
        let config = self.config();
        match self.detect_backend() {
            FtpdBackend::Embedded => {
                if probe_port(config.port).await {
                    return Err(Error::Other(format!("端口 {} 已被占用", config.port)));
                }
                std::fs::create_dir_all(self.cfg.data_dir.join("ftp"))?;
                let data_dir = self.cfg.data_dir.clone();
                let users_path = self.users_path();
                let authenticator = std::sync::Arc::new(RunphpAuthenticator { users_path: users_path.clone() });
                let provider = std::sync::Arc::new(RunphpUserProvider {
                    users_path,
                    data_dir: data_dir.clone(),
                });
                let factory = move || {
                    unftp_sbe_fs::Filesystem::new(data_dir.clone())
                        .unwrap_or_else(|e| panic!("FTP 根目录不可用: {e}"))
                };
                let server = libunftp::ServerBuilder::with_authenticator(
                    Box::new(factory),
                    authenticator,
                )
                .user_detail_provider(provider)
                .passive_ports(config.passive_from..=config.passive_to)
                .build()
                .map_err(|e| Error::Other(format!("FTP 服务端构建失败: {e}")))?;
                let handle = tokio::spawn(server.listen(format!("0.0.0.0:{}", config.port)));
                let mut guard = EMBEDDED.lock().await;
                if let Some(old) = guard.take() {
                    old.abort();
                }
                *guard = Some(handle);
                if !wait_port_ready(config.port, 5).await {
                    return Err(Error::Other(format!(
                        "内嵌 FTP 服务启动后端口 {} 未就绪",
                        config.port
                    )));
                }
                Ok("内嵌 libunftp")
            }
            FtpdBackend::PureFtpd { binary } => {
                self.sync_puredb(&binary).await?;
                let mut svc = ManagedService::new(ServiceKind::Ftp, "FTP 服务".into(), config.port);
                svc.id = FTP_SERVICE_ID.to_string();
                svc.source = ServiceSource::Portable;
                svc.binary_path = Some(binary);
                svc.extra_args = pure_ftpd_args(config.port, config.passive_from, config.passive_to, &self.pure_puredb());
                self.services.upsert(svc)?;
                self.services.start(FTP_SERVICE_ID).await?;
                if !wait_port_ready(config.port, 10).await {
                    let log = self.services.read_log(FTP_SERVICE_ID, 10).unwrap_or_default();
                    return Err(Error::Other(format!("Pure-FTPd 启动后端口 {} 未就绪。日志末尾：\n{log}", config.port)));
                }
                Ok("Pure-FTPd")
            }
        }
    }

    /// 前台运行 FTP 服务端直到被停止（`ftpd run` 宿主循环内调用 start 后阻塞）。
    pub async fn run_forever(&self) -> Result<()> {
        let backend = self.start().await?;
        tracing::info!("FTP 服务端（{backend}）前台运行中，等待中断…");
        match tokio::signal::ctrl_c().await {
            Ok(()) => tracing::info!("收到中断信号，停止 FTP 服务端"),
            Err(e) => tracing::warn!("等待中断信号失败: {e}"),
        }
        self.stop().await
    }

    /// 守护方式启动（CLI 场景）：Pure-FTPd 走子进程托管；
    /// 内嵌后端自派生分离的 `runphp ftpd run` 子进程（PID 落盘供 stop）。
    pub async fn start_daemon(&self) -> Result<&'static str> {
        if !matches!(self.detect_backend(), FtpdBackend::Embedded) {
            return self.start().await;
        }
        let config = self.config();
        if probe_port(config.port).await {
            return Err(Error::Other(format!("端口 {} 已被占用", config.port)));
        }
        let exe = std::env::current_exe()?;
        let mut cmd = tokio::process::Command::new(exe);
        cmd.args(["ftpd", "run", "--data-dir"])
            .arg(&self.cfg.data_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            // tokio::process::Command 在 Windows 上原生提供 creation_flags
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // 脱离终端会话，避免随宿主退出
            cmd.process_group(0);
        }
        let child = cmd
            .spawn()
            .map_err(|e| Error::Other(format!("派生 FTP 守护进程失败: {e}")))?;
        let pid = child.id();
        // 子进程已分离，句柄即刻丢弃
        drop(child);
        if let Some(pid) = pid {
            std::fs::create_dir_all(self.cfg.data_dir.join("ftp"))?;
            std::fs::write(self.embedded_pid_file(), pid.to_string())?;
        }
        if !wait_port_ready(config.port, 10).await {
            return Err(Error::Other("内嵌 FTP 守护进程启动后端口未就绪".into()));
        }
        Ok("内嵌 libunftp（守护进程）")
    }

    /// 内嵌守护进程的 PID 文件。
    fn embedded_pid_file(&self) -> PathBuf {
        self.cfg.data_dir.join("ftp").join("embedded.pid")
    }

    /// 停止 FTP 服务端（进程内实例、守护进程、Pure-FTPd 全部尽力停止）。
    pub async fn stop(&self) -> Result<()> {
        if let Some(handle) = EMBEDDED.lock().await.take() {
            handle.abort();
        }
        // 分离守护进程按 PID 文件终止
        let pid_file = self.embedded_pid_file();
        if pid_file.exists() {
            if let Ok(pid) = std::fs::read_to_string(&pid_file)
                .map_err(Error::Io)
                .and_then(|s| s.trim().parse::<u32>().map_err(|_| Error::Other("PID 解析失败".into())))
            {
                crate::caddy::kill_process(pid);
            }
            std::fs::remove_file(&pid_file).ok();
        }
        // Pure-FTPd 注册过则一并停止（未注册时忽略）
        let _ = self.services.stop(FTP_SERVICE_ID).await;
        Ok(())
    }

    /// 服务状态（控制端口探测）。
    pub async fn status(&self) -> bool {
        probe_port(self.config().port).await
    }

    // ---- 用户 CRUD ----

    /// 列出全部虚拟用户。
    pub fn list_users(&self) -> Result<Vec<FtpUser>> {
        Ok(self.load_users()?.users)
    }

    /// 新增用户（同名拒绝；自动创建默认根目录）。
    pub async fn add_user(&self, user: FtpUser) -> Result<()> {
        validate_username(&user.username)?;
        let mut reg = self.load_users()?;
        if reg.users.iter().any(|u| u.username == user.username) {
            return Err(Error::Config(format!("用户 {} 已存在", user.username)));
        }
        std::fs::create_dir_all(self.user_home(&user))?;
        reg.users.push(user);
        self.save_users(&reg)?;
        self.sync_users_to_backend().await
    }

    /// 更新用户（按 id，保留 created_at）。
    pub async fn update_user(&self, user: FtpUser) -> Result<()> {
        validate_username(&user.username)?;
        let mut reg = self.load_users()?;
        // 先做同名检查再取可变引用，避免借用冲突
        if reg
            .users
            .iter()
            .any(|u| u.id != user.id && u.username == user.username)
        {
            return Err(Error::Config(format!("用户 {} 已存在", user.username)));
        }
        let target = reg
            .users
            .iter_mut()
            .find(|u| u.id == user.id)
            .ok_or_else(|| Error::Other("FTP 用户不存在".into()))?;
        let mut user = user;
        user.created_at = target.created_at.clone();
        std::fs::create_dir_all(self.user_home(&user))?;
        *target = user;
        self.save_users(&reg)?;
        self.sync_users_to_backend().await
    }

    /// 删除用户（按 id）。
    pub async fn remove_user(&self, id: &str) -> Result<()> {
        let mut reg = self.load_users()?;
        reg.users.retain(|u| u.id != id);
        self.save_users(&reg)?;
        self.sync_users_to_backend().await
    }

    /// 用户变更后同步到 Pure-FTPd 后端（内嵌后端即时生效无需同步）。
    async fn sync_users_to_backend(&self) -> Result<()> {
        if let FtpdBackend::PureFtpd { binary } = self.detect_backend() {
            self.sync_puredb(&binary).await?;
        }
        Ok(())
    }

    /// 以 JSON 注册表为准源，全量重建 Pure-FTPd 的 passwd 与 puredb。
    async fn sync_puredb(&self, binary: &Path) -> Result<()> {
        let pure_pw = binary
            .parent()
            .map(|d| {
                if cfg!(windows) {
                    d.join("pure-pw.exe")
                } else {
                    d.join("pure-pw")
                }
            })
            .filter(|p| p.is_file())
            .ok_or_else(|| Error::Other("未找到 pure-pw 工具（需与 pure-ftpd 同目录）".into()))?;
        let passwd = self.pure_passwd();
        let puredb = self.pure_puredb();
        if let Some(parent) = passwd.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 全量重建：删除旧库后按注册表逐个写入
        std::fs::remove_file(&passwd).ok();
        std::fs::remove_file(&puredb).ok();
        let sys_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "nobody".to_string());
        for u in self.load_users()?.users.iter().filter(|u| u.enabled) {
            let home = self.user_home(u);
            std::fs::create_dir_all(&home)?;
            let mut child = tokio::process::Command::new(&pure_pw)
                .args([
                    "useradd",
                    &u.username,
                    "-f",
                    &passwd.to_string_lossy(),
                    "-u",
                    &sys_user,
                    "-d",
                    &home.to_string_lossy(),
                    "-m",
                ])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| Error::Other(format!("执行 pure-pw 失败: {e}")))?;
            // pure-pw 交互式提示密码两次
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin
                    .write_all(format!("{}\n{}\n", u.password, u.password).as_bytes())
                    .await;
            }
            let output = child
                .wait_with_output()
                .await
                .map_err(|e| Error::Other(format!("pure-pw 执行失败: {e}")))?;
            if !output.status.success() {
                return Err(Error::Other(format!(
                    "pure-pw 写入用户 {} 失败: {}",
                    u.username,
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
        }
        Ok(())
    }
}

/// Pure-FTPd 启动参数（chroot 全员 + 自动建主目录 + puredb 认证）。
fn pure_ftpd_args(port: u16, passive_from: u16, passive_to: u16, puredb: &Path) -> Vec<String> {
    vec![
        "-A".into(),
        "-j".into(),
        "-S".into(),
        format!("0.0.0.0:{port}"),
        "-p".into(),
        format!("{passive_from}:{passive_to}"),
        "-l".into(),
        format!("puredb:{}", puredb.display()),
    ]
}

/// 用户名校验：字母/数字/下划线/连字符，1-32 位。
fn validate_username(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(Error::Config(
            "用户名仅允许字母、数字、下划线、连字符，长度 1-32".into(),
        ))
    }
}

// ---- 内嵌后端（认证与用户详情，现读 JSON 即时生效） ----

/// 认证器：校验 ftp_users.json 中的用户名/密码。
#[derive(Debug)]
struct RunphpAuthenticator {
    users_path: PathBuf,
}

#[async_trait]
impl unftp_core::auth::Authenticator for RunphpAuthenticator {
    async fn authenticate(
        &self,
        username: &str,
        creds: &unftp_core::auth::Credentials,
    ) -> std::result::Result<unftp_core::auth::Principal, unftp_core::auth::AuthenticationError> {
        let reg: FtpUserRegistry = std::fs::read_to_string(&self.users_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let user = reg
            .users
            .iter()
            .find(|u| u.username == username && u.enabled)
            .ok_or(unftp_core::auth::AuthenticationError::BadUser)?;
        let pass = creds.password.as_deref().unwrap_or("");
        if user.password != pass {
            return Err(unftp_core::auth::AuthenticationError::BadPassword);
        }
        Ok(unftp_core::auth::Principal {
            username: username.to_string(),
        })
    }
}

/// 会话用户详情：携带根目录实现每用户 chroot。
#[derive(Debug, Clone)]
struct RunphpUser {
    username: String,
    home: PathBuf,
    enabled: bool,
}

impl std::fmt::Display for RunphpUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.username)
    }
}

impl unftp_core::auth::UserDetail for RunphpUser {
    fn account_enabled(&self) -> bool {
        self.enabled
    }

    fn home(&self) -> Option<&Path> {
        Some(&self.home)
    }
}

/// 用户详情提供者：Principal → 根目录（校验须位于数据目录内）。
#[derive(Debug)]
struct RunphpUserProvider {
    users_path: PathBuf,
    data_dir: PathBuf,
}

#[async_trait]
impl unftp_core::auth::UserDetailProvider for RunphpUserProvider {
    type User = RunphpUser;

    async fn provide_user_detail(
        &self,
        principal: &unftp_core::auth::Principal,
    ) -> std::result::Result<RunphpUser, unftp_core::auth::UserDetailError> {
        let reg: FtpUserRegistry = std::fs::read_to_string(&self.users_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let user = reg
            .users
            .iter()
            .find(|u| u.username == principal.username)
            .ok_or_else(|| unftp_core::auth::UserDetailError::UserNotFound {
                username: principal.username.clone(),
            })?;
        // 内嵌后端以数据目录为文件系统根（cap-std 能力约束），
        // 外部目录（如站点根不在数据目录内）需改用 Pure-FTPd 后端
        let home = if let Some(h) = &user.home_dir {
            PathBuf::from(h)
        } else {
            self.data_dir.join("ftp").join(&user.username)
        };
        if !home.starts_with(&self.data_dir) {
            return Err(unftp_core::auth::UserDetailError::Generic(
                "内嵌 FTP 服务端的用户目录须位于数据目录内；绑定外部目录请使用 Pure-FTPd 后端".into(),
            ));
        }
        Ok(RunphpUser {
            username: user.username.clone(),
            home,
            enabled: user.enabled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unftp_core::auth::Authenticator as _;

    fn mgr_at(dir: &Path) -> FtpdManager {
        let cfg = AppConfig {
            data_dir: dir.to_path_buf(),
            ..Default::default()
        };
        FtpdManager::new(cfg)
    }

    #[test]
    fn 用户注册表往返与校验() {
        let dir = std::env::temp_dir().join("runphp-ftpd-users-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = mgr_at(&dir);
        assert!(mgr.list_users().unwrap().is_empty());
        assert!(validate_username("web-1_x").is_ok());
        assert!(validate_username("bad name").is_err());
        assert!(validate_username("a".repeat(33).as_str()).is_err());

        let mut reg = FtpUserRegistry::default();
        reg.users.push(FtpUser::new("alice".into(), "pw".into()));
        mgr.save_users(&reg).unwrap();
        assert_eq!(mgr.list_users().unwrap().len(), 1);
        assert_eq!(mgr.list_users().unwrap()[0].username, "alice");
        // enabled 缺省兼容
        let raw = std::fs::read_to_string(mgr.users_path()).unwrap();
        assert!(raw.contains("\"username\": \"alice\""));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 配置默认值与保存() {
        let dir = std::env::temp_dir().join("runphp-ftpd-cfg-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = mgr_at(&dir);
        let c = mgr.config();
        assert_eq!(c.port, 21);
        assert_eq!(c.passive_from, 50000);
        assert!(!c.autostart);

        let mut c2 = c.clone();
        c2.port = 2121;
        c2.autostart = true;
        mgr.save_config(&c2).unwrap();
        let c3 = mgr.config();
        assert_eq!(c3.port, 2121);
        assert!(c3.autostart);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 用户根目录默认与自定义() {
        let dir = std::env::temp_dir().join("runphp-ftpd-home-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = mgr_at(&dir);
        let mut u = FtpUser::new("bob".into(), "pw".into());
        assert_eq!(
            mgr.user_home(&u),
            dir.join("ftp").join("bob")
        );
        u.home_dir = Some("/srv/custom".into());
        assert_eq!(mgr.user_home(&u), PathBuf::from("/srv/custom"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pure_ftpd_参数构造() {
        let args = pure_ftpd_args(2121, 50000, 50100, Path::new("/data/ftp/pureftpd.pdb"));
        assert!(args.contains(&"-A".to_string()));
        assert!(args.windows(2).any(|w| w[0] == "-S" && w[1] == "0.0.0.0:2121"));
        assert!(args.windows(2).any(|w| w[0] == "-p" && w[1] == "50000:50100"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-l" && w[1] == "puredb:/data/ftp/pureftpd.pdb")
        );
    }

    #[tokio::test]
    async fn 用户增改删与目录创建() {
        let dir = std::env::temp_dir().join("runphp-ftpd-crud-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = mgr_at(&dir);

        let u = FtpUser::new("carol".into(), "pw".into());
        mgr.add_user(u.clone()).await.unwrap();
        assert!(dir.join("ftp").join("carol").is_dir());

        // 同名拒绝
        let dup = FtpUser::new("carol".into(), "pw2".into());
        assert!(mgr.add_user(dup).await.is_err());

        // 更新
        let mut updated = mgr.list_users().unwrap()[0].clone();
        updated.password = "new".into();
        mgr.update_user(updated).await.unwrap();
        assert_eq!(mgr.list_users().unwrap()[0].password, "new");

        // 删除
        let id = mgr.list_users().unwrap()[0].id.clone();
        mgr.remove_user(&id).await.unwrap();
        assert!(mgr.list_users().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn 内嵌认证器校验用户与密码() {
        let dir = std::env::temp_dir().join("runphp-ftpd-auth-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = mgr_at(&dir);
        let mut reg = FtpUserRegistry::default();
        reg.users.push(FtpUser::new("dave".into(), "secret".into()));
        mgr.save_users(&reg).unwrap();

        let auth = RunphpAuthenticator {
            users_path: mgr.users_path(),
        };
        let mut creds: unftp_core::auth::Credentials = "secret".into();
        assert!(auth.authenticate("dave", &creds).await.is_ok());
        creds.password = Some("wrong".into());
        assert!(auth.authenticate("dave", &creds).await.is_err());
        assert!(auth.authenticate("nobody", &"secret".into()).await.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
