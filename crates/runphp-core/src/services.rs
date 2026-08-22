//! 本地服务托管：数据库引擎 / FTP 服务端的进程生命周期管理。
//!
//! 两类服务来源：
//! - takeover（接管）：本机已安装的系统服务（Windows 服务 / systemd 单元），
//!   仅调用系统命令启停并用端口探测状态，不托管进程
//! - portable（便携托管）：RunPHP 以子进程方式托管外部二进制，
//!   数据落在 `services/<id>/data`，日志写入 `logs/services/<id>.log`，
//!   PID 写入 `services/<id>.pid` 供跨会话兜底终止
//!
//! 引擎差异（数据目录初始化、建库建用户）由 `db::service` 与 `ftpd` 模块负责，
//! 本模块只提供通用的注册表与进程托管。

use crate::{config::AppConfig, Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// 服务类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    Mysql,
    Mariadb,
    Postgresql,
    Redis,
    Ftp,
}

impl ServiceKind {
    /// 默认监听端口。
    pub fn default_port(self) -> u16 {
        match self {
            ServiceKind::Mysql | ServiceKind::Mariadb => 3306,
            ServiceKind::Postgresql => 5432,
            ServiceKind::Redis => 6379,
            ServiceKind::Ftp => 21,
        }
    }

    /// 显示名称。
    pub fn display_name(self) -> &'static str {
        match self {
            ServiceKind::Mysql => "MySQL",
            ServiceKind::Mariadb => "MariaDB",
            ServiceKind::Postgresql => "PostgreSQL",
            ServiceKind::Redis => "Redis",
            ServiceKind::Ftp => "FTP",
        }
    }
}

/// 服务来源。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceSource {
    /// 接管本机已安装的系统服务。
    Takeover,
    /// RunPHP 子进程托管便携二进制。
    Portable,
}

/// 受管服务定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedService {
    /// 唯一标识（uuid）。
    pub id: String,
    /// 服务类型。
    pub kind: ServiceKind,
    /// 显示名称。
    pub name: String,
    /// 来源。
    pub source: ServiceSource,
    /// 便携托管的二进制绝对路径（接管服务为 None）。
    #[serde(default)]
    pub binary_path: Option<PathBuf>,
    /// 监听端口。
    pub port: u16,
    /// 是否随应用自动启动。
    #[serde(default)]
    pub autostart: bool,
    /// 管理员用户名（MySQL/MariaDB/PostgreSQL 的 root 凭据；Redis/FTP 为空）。
    #[serde(default)]
    pub root_username: String,
    /// 管理员密码（明文存储，本地工具用途，与连接档案先例一致）。
    #[serde(default)]
    pub root_password: String,
    /// 接管服务对应的系统服务名（Windows 服务名或 systemd 单元名）。
    #[serde(default)]
    pub os_service_name: Option<String>,
    /// 附加启动参数（便携服务，逐条传入命令行）。
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// 创建时间（RFC3339）。
    pub created_at: String,
}

impl ManagedService {
    /// 创建新服务定义（生成 id 与时间戳）。
    pub fn new(kind: ServiceKind, name: String, port: u16) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            name,
            source: ServiceSource::Portable,
            binary_path: None,
            port: if port > 0 { port } else { kind.default_port() },
            autostart: false,
            root_username: String::new(),
            root_password: String::new(),
            os_service_name: None,
            extra_args: Vec::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// 服务注册表（持久化为 `services.json`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceRegistry {
    pub services: Vec<ManagedService>,
}

/// 服务运行状态。
#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    /// 端口是否可达（对接管与便携服务均有效）。
    pub running: bool,
    /// 便携托管的进程 PID（读取自 PID 文件，跨会话有效）。
    pub pid: Option<u32>,
}

/// 服务管理器：注册表 CRUD + 进程托管。
pub struct ServiceManager {
    cfg: AppConfig,
}

/// 全局托管的子进程表（服务 id → 进程句柄）。
static HOSTED: Mutex<Vec<(String, Child)>> = Mutex::const_new(Vec::new());

/// 在托管表中插入/替换服务进程句柄。
async fn hosted_insert(id: &str, child: Child) {
    let mut guard = HOSTED.lock().await;
    guard.retain(|(i, _)| i != id);
    guard.push((id.to_string(), child));
}

/// 从托管表取出并返回服务进程句柄。
async fn hosted_take(id: &str) -> Option<Child> {
    let mut guard = HOSTED.lock().await;
    let pos = guard.iter().position(|(i, _)| i == id)?;
    Some(guard.remove(pos).1)
}

impl ServiceManager {
    pub fn new(cfg: AppConfig) -> Self {
        Self { cfg }
    }

    /// 注册表文件路径：`数据目录/services.json`。
    fn registry_path(&self) -> PathBuf {
        self.cfg.data_dir.join("services.json")
    }

    /// 服务条目根目录：`数据目录/services/<id>`。
    pub fn service_dir(&self, id: &str) -> PathBuf {
        self.cfg.data_dir.join("services").join(id)
    }

    /// 便携服务数据目录：`数据目录/services/<id>/data`。
    pub fn service_data_dir(&self, id: &str) -> PathBuf {
        self.service_dir(id).join("data")
    }

    /// 服务日志路径：`数据目录/logs/services/<id>.log`。
    pub fn log_path(&self, id: &str) -> PathBuf {
        self.cfg.logs_dir().join("services").join(format!("{id}.log"))
    }

    /// PID 文件路径：`数据目录/services/<id>.pid`。
    fn pid_file(&self, id: &str) -> PathBuf {
        self.service_dir(id).join("service.pid")
    }

    pub fn load(&self) -> Result<ServiceRegistry> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(ServiceRegistry::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        serde_json::from_str(&raw).map_err(Error::Json)
    }

    pub fn save(&self, reg: &ServiceRegistry) -> Result<()> {
        std::fs::create_dir_all(&self.cfg.data_dir)?;
        let raw = serde_json::to_string_pretty(reg)?;
        std::fs::write(self.registry_path(), raw)?;
        Ok(())
    }

    /// 列出全部受管服务。
    pub fn list(&self) -> Result<Vec<ManagedService>> {
        Ok(self.load()?.services)
    }

    /// 按 id 查找服务。
    pub fn get(&self, id: &str) -> Result<ManagedService> {
        self.load()?
            .services
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::Other(format!("服务 {id} 不存在")))
    }

    /// 新增或按 id 覆盖保存服务（保留原 created_at）。
    pub fn upsert(&self, mut service: ManagedService) -> Result<()> {
        let mut reg = self.load()?;
        if let Some(existing) = reg.services.iter_mut().find(|s| s.id == service.id) {
            service.created_at = existing.created_at.clone();
            *existing = service;
        } else {
            reg.services.push(service);
        }
        self.save(&reg)
    }

    /// 删除服务注册（不动进程，调用方先行停止）。
    pub fn remove(&self, id: &str) -> Result<()> {
        let mut reg = self.load()?;
        reg.services.retain(|s| s.id != id);
        self.save(&reg)
    }

    /// 读取 PID 文件（便携托管专用）。
    fn read_pid(&self, id: &str) -> Option<u32> {
        std::fs::read_to_string(self.pid_file(id))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// 启动服务：便携托管走子进程，接管服务调用系统命令。
    pub async fn start(&self, id: &str) -> Result<()> {
        let svc = self.get(id)?;
        match svc.source {
            ServiceSource::Portable => {
                let binary = svc
                    .binary_path
                    .as_ref()
                    .filter(|p| p.is_file())
                    .ok_or_else(|| {
                        Error::Other(format!(
                            "二进制不存在，请检查服务路径: {}",
                            svc.binary_path
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default()
                        ))
                    })?;
                std::fs::create_dir_all(self.service_data_dir(id))?;
                let log_dir = self.log_path(id).parent().unwrap().to_path_buf();
                std::fs::create_dir_all(&log_dir)?;
                // 截断旧日志
                let log_file = std::fs::File::create(self.log_path(id))?;
                let stderr = log_file.try_clone()?;
                let mut cmd = Command::new(binary);
                cmd.args(portable_args(&svc, &self.service_data_dir(id)))
                    .stdout(Stdio::from(log_file))
                    .stderr(Stdio::from(stderr));
                let child = cmd
                    .spawn()
                    .map_err(|e| Error::Other(format!("启动 {} 失败: {e}", svc.kind.display_name())))?;
                let pid = child.id().unwrap_or(0);
                if pid > 0 {
                    std::fs::write(self.pid_file(id), pid.to_string())?;
                }
                hosted_insert(id, child).await;
                tracing::info!("已启动服务 {}（pid={pid}）", svc.name);
                Ok(())
            }
            ServiceSource::Takeover => {
                let name = svc.os_service_name.clone().ok_or_else(|| {
                    Error::Other("该服务未关联系统服务，无法通过 RunPHP 启动".into())
                })?;
                os_service_ctl(&name, true).await
            }
        }
    }

    /// 停止服务：便携托管先杀子进程、再按 PID 文件兜底；接管服务调用系统命令。
    pub async fn stop(&self, id: &str) -> Result<()> {
        let svc = self.get(id)?;
        match svc.source {
            ServiceSource::Portable => {
                if let Some(mut child) = hosted_take(id).await {
                    let _ = child.kill().await;
                } else if let Some(pid) = self.read_pid(id) {
                    // 跨会话兜底（上个会话遗留的进程）
                    crate::caddy::kill_process(pid);
                }
                std::fs::remove_file(self.pid_file(id)).ok();
                tracing::info!("已停止服务 {}", svc.name);
                Ok(())
            }
            ServiceSource::Takeover => {
                let name = svc.os_service_name.clone().ok_or_else(|| {
                    Error::Other("该服务未关联系统服务，无法通过 RunPHP 停止".into())
                })?;
                os_service_ctl(&name, false).await
            }
        }
    }

    /// 查询服务状态（端口探测 + PID 文件）。
    pub async fn status(&self, id: &str) -> Result<ServiceStatus> {
        let svc = self.get(id)?;
        Ok(ServiceStatus {
            running: probe_port(svc.port).await,
            pid: self.read_pid(id),
        })
    }

    /// 读取服务日志末尾若干行。
    pub fn read_log(&self, id: &str, tail_lines: usize) -> Result<String> {
        let path = self.log_path(id);
        if !path.exists() {
            return Ok(String::new());
        }
        let content = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(tail_lines);
        Ok(lines[start..].join("\n"))
    }
}

/// 构造便携服务的启动参数（引擎公共部分；引擎特有参数经 extra_args 附加）。
///
/// `data_dir` 为 ServiceManager 规划的 `services/<id>/data`。
fn portable_args(svc: &ManagedService, data_dir: &std::path::Path) -> Vec<String> {
    let dir = data_dir.to_string_lossy().to_string();
    let mut args: Vec<String> = match svc.kind {
        ServiceKind::Mysql | ServiceKind::Mariadb => vec![
            "--console".into(),
            "--datadir".into(),
            dir,
            "--port".into(),
            svc.port.to_string(),
        ],
        ServiceKind::Postgresql => vec![
            "-D".into(),
            dir,
            "-p".into(),
            svc.port.to_string(),
        ],
        ServiceKind::Redis => vec![
            "--port".into(),
            svc.port.to_string(),
            "--dir".into(),
            dir,
        ],
        // FTP 服务端参数由 ftpd 模块通过 extra_args 提供
        ServiceKind::Ftp => vec![],
    };
    args.extend(svc.extra_args.iter().cloned());
    args
}

/// 调用系统服务控制（Windows `sc` / Linux `systemctl`）。
async fn os_service_ctl(name: &str, start: bool) -> Result<()> {
    let action = if start { "start" } else { "stop" };
    let output = if cfg!(windows) {
        Command::new("sc").args([action, name]).output().await
    } else {
        Command::new("systemctl").args([action, name]).output().await
    };
    let output = output.map_err(|e| Error::Other(format!("调用系统服务控制失败: {e}")))?;
    if !output.status.success() {
        let msg = format!(
            "{}",
            String::from_utf8_lossy(if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            })
        )
        .trim()
        .to_string();
        return Err(Error::Other(format!(
            "系统服务 {name} {action} 失败: {msg}（接管启停可能需要管理员/root 权限，也可改用便携托管）"
        )));
    }
    Ok(())
}

/// 探测本机端口是否可达（500ms 超时）。
pub(crate) async fn probe_port(port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

/// 轮询等待端口就绪（数据库引擎启动需数秒到数十秒）。
pub(crate) async fn wait_port_ready(port: u16, timeout_secs: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if probe_port(port).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(kind: ServiceKind, id: &str, port: u16) -> ManagedService {
        let mut s = ManagedService::new(kind, format!("{kind:?}"), port);
        s.id = id.to_string();
        s
    }

    #[test]
    fn 注册表增改删往返() {
        let dir = std::env::temp_dir().join("runphp-svc-registry-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = AppConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let mgr = ServiceManager::new(cfg);
        assert!(mgr.list().unwrap().is_empty());

        let mut s = svc(ServiceKind::Mysql, "m1", 3306);
        mgr.upsert(s.clone()).unwrap();
        // 按 id 覆盖保存（更新端口）
        s.port = 3307;
        mgr.upsert(s).unwrap();
        assert_eq!(mgr.list().unwrap().len(), 1);
        assert_eq!(mgr.get("m1").unwrap().port, 3307);

        mgr.remove("m1").unwrap();
        assert!(mgr.list().unwrap().is_empty());
        assert!(mgr.get("m1").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 各引擎启动参数() {
        let data = PathBuf::from("/data/services/m1/data");
        let mut mysql = svc(ServiceKind::Mysql, "m1", 3306);
        mysql.binary_path = Some(PathBuf::from("/bin/mysqld"));
        let args = portable_args(&mysql, &data);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--datadir" && w[1] == data.to_string_lossy()));
        assert!(args.contains(&"--port".to_string()));
        assert!(args.contains(&"3306".to_string()));

        let pg = svc(ServiceKind::Postgresql, "p1", 5432);
        let args = portable_args(&pg, &data);
        assert!(args.windows(2).any(|w| w[0] == "-D" && w[1] == data.to_string_lossy()));

        let mut redis = svc(ServiceKind::Redis, "r1", 6379);
        redis.extra_args = vec! ["--requirepass".into(), "secret".into()];
        let args = portable_args(&redis, &data);
        assert!(args.contains(&"--requirepass".to_string()));
        assert!(args.contains(&"secret".to_string()));

        // FTP 服务端参数全部来自 extra_args
        let mut ftp = svc(ServiceKind::Ftp, "f1", 21);
        ftp.extra_args = vec!["-A".into(), "-j".into()];
        assert_eq!(portable_args(&ftp, &data), vec!["-A", "-j"]);
    }

    #[test]
    fn 默认端口() {
        assert_eq!(ServiceKind::Mysql.default_port(), 3306);
        assert_eq!(ServiceKind::Mariadb.default_port(), 3306);
        assert_eq!(ServiceKind::Postgresql.default_port(), 5432);
        assert_eq!(ServiceKind::Redis.default_port(), 6379);
        assert_eq!(ServiceKind::Ftp.default_port(), 21);
    }

    #[tokio::test]
    async fn 未注册服务启停报错() {
        let dir = std::env::temp_dir().join("runphp-svc-missing-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = AppConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let mgr = ServiceManager::new(cfg);
        assert!(mgr.start("nope").await.is_err());
        assert!(mgr.stop("nope").await.is_err());
        assert!(mgr.status("nope").await.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
