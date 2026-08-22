//! FTP 管理：支持 FTP / SFTP / FTPS 三种协议的远程文件管理。
//!
//! - 连接档案（含密码/密钥路径）以 JSON 持久化于数据目录（`ftp_profiles.json`）
//! - SFTP 复用 russh 建立 SSH 连接后请求 sftp 子系统，认证方式与 tunnel.rs 一致
//! - FTP/FTPS 通过 suppaftp 异步客户端，FTPS 走 AUTH TLS 显式加密升级
//! - 统一 `FtpClient` 枚举包装三种连接句柄，对外提供一致的文件操作接口

use crate::Error;
use russh::client;
use russh::keys::key;
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use suppaftp::list::File as FtpFile;
use suppaftp::tokio::{AsyncFtpStream, AsyncRustlsConnector, AsyncRustlsFtpStream};
use suppaftp::types::FileType;
use suppaftp::FtpError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// FTP 协议类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FtpProtocol {
    /// 普通 FTP（明文）。
    Ftp,
    /// SFTP（基于 SSH）。
    Sftp,
    /// FTP over TLS（显式加密）。
    Ftps,
}

impl FtpProtocol {
    /// 默认端口。
    pub fn default_port(self) -> u16 {
        match self {
            FtpProtocol::Ftp | FtpProtocol::Ftps => 21,
            FtpProtocol::Sftp => 22,
        }
    }
}

/// FTP 连接档案。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpProfile {
    /// 唯一标识。
    pub id: String,
    /// 显示名称。
    pub name: String,
    /// 协议类型。
    pub protocol: FtpProtocol,
    /// 主机。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 用户名。
    pub username: String,
    /// 密码（FTP/FTPS/SFTP 密码认证使用，明文存储，本地工具用途）。
    #[serde(default)]
    pub password: String,
    /// SSH 私钥路径（仅 SFTP，与 password 二选一）。
    #[serde(default)]
    pub ssh_key: Option<String>,
    /// SSH 密码（仅 SFTP，与 ssh_key 二选一）。
    #[serde(default)]
    pub ssh_password: Option<String>,
    /// 限定作用范围目录（chroot 根）。
    /// 留空或 "/" 表示不限定；否则所有远程路径都会被归一化到此目录下，
    /// 任何试图通过 `..` 跳出此目录的请求都会被拒绝。
    #[serde(default)]
    pub root_dir: Option<String>,
    /// 创建时间。
    pub created_at: String,
}

impl FtpProfile {
    /// 创建空白档案。
    pub fn new(name: String, protocol: FtpProtocol, host: String, port: u16) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            protocol,
            host,
            port: if port > 0 { port } else { protocol.default_port() },
            username: String::new(),
            password: String::new(),
            ssh_key: None,
            ssh_password: None,
            root_dir: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// 返回归一化后的限定根目录：空值视为 `/`。
    fn effective_root(&self) -> String {
        match &self.root_dir {
            Some(r) if !r.trim().is_empty() => normalize_remote_path(r),
            _ => "/".to_string(),
        }
    }

    /// 将用户输入的远程路径归一化到限定根目录下，并拒绝越界访问。
    fn resolve_remote(&self, input: &str) -> Result<String, Error> {
        let root = self.effective_root();
        // 输入为空或根标记，直接返回限定根
        if input.is_empty() || input == "/" {
            return Ok(root.clone());
        }
        let combined = if input.starts_with('/') {
            // 绝对路径：挂到限定根下（忽略其前导 /）
            if root == "/" {
                input.to_string()
            } else {
                format!("{}{}", root.trim_end_matches('/'), input)
            }
        } else {
            format!("{}/{}", root.trim_end_matches('/'), input)
        };
        let normalized = normalize_remote_path(&combined);
        // 越界检查：归一化结果必须等于限定根或位于其下
        let in_scope = normalized == root
            || root == "/"
            || normalized.starts_with(&format!("{}/", root));
        if !in_scope {
            return Err(Error::Other(format!(
                "路径越界：{} 超出限定根目录 {}",
                input, root
            )));
        }
        Ok(normalized)
    }
}

/// 档案注册表（持久化为 `ftp_profiles.json`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FtpProfileRegistry {
    pub profiles: Vec<FtpProfile>,
}

impl FtpProfileRegistry {
    /// 按 id 查找档案。
    pub fn get(&self, id: &str) -> Option<&FtpProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// 按 id 查找可变引用。
    pub fn get_mut(&mut self, id: &str) -> Option<&mut FtpProfile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }
}

/// 远程文件条目。
#[derive(Debug, Clone, Serialize)]
pub struct FtpEntry {
    /// 名称。
    pub name: String,
    /// 是否为目录。
    pub is_dir: bool,
    /// 字节数（目录为 0）。
    pub size: u64,
    /// 修改时间（RFC3339，无法获取时为空串）。
    pub modified: String,
}

/// FTP 客户端统一句柄：包装三种协议的连接。
pub enum FtpClient {
    /// 普通 FTP。
    Ftp(AsyncFtpStream),
    /// FTPS（已升级为 TLS）。
    Ftps(AsyncRustlsFtpStream),
    /// SFTP（基于 russh-sftp）。
    Sftp(SftpSession),
}

/// 进度回调：(已传输字节, 总字节, 当前文件名)。总字节未知时为 0。
pub type ProgressFn<'a> = Option<&'a (dyn Fn(u64, u64, &str) + Send + Sync)>;

/// SFTP 客户端回调（空实现，仅满足 trait 要求）。
struct SshHandler;

#[async_trait::async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // 本地工具，不校验主机密钥（用户自行确认安全性）
        Ok(true)
    }
}

/// FTP 档案管理器。
pub struct FtpManager {
    /// 档案文件路径（`ftp_profiles.json`）。
    path: PathBuf,
}

impl FtpManager {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            path: data_dir.join("ftp_profiles.json"),
        }
    }

    pub fn load(&self) -> Result<FtpProfileRegistry, Error> {
        if !self.path.exists() {
            return Ok(FtpProfileRegistry::default());
        }
        let raw = std::fs::read_to_string(&self.path).map_err(Error::Io)?;
        serde_json::from_str(&raw).map_err(Error::Json)
    }

    pub fn save(&self, reg: &FtpProfileRegistry) -> Result<(), Error> {
        let raw = serde_json::to_string_pretty(reg).map_err(Error::Json)?;
        std::fs::write(&self.path, raw).map_err(Error::Io)?;
        Ok(())
    }

    pub fn list_profiles(&self) -> Result<Vec<FtpProfile>, Error> {
        Ok(self.load()?.profiles)
    }

    pub fn add_profile(&self, profile: FtpProfile) -> Result<(), Error> {
        let mut reg = self.load()?;
        reg.profiles.push(profile);
        self.save(&reg)
    }

    pub fn remove_profile(&self, id: &str) -> Result<(), Error> {
        let mut reg = self.load()?;
        reg.profiles.retain(|p| p.id != id);
        self.save(&reg)
    }

    /// 更新已有档案：按 id 查找并替换，保留原 created_at。
    pub fn update_profile(&self, profile: FtpProfile) -> Result<(), Error> {
        let mut reg = self.load()?;
        let target = reg
            .get_mut(&profile.id)
            .ok_or_else(|| Error::Other("FTP 连接档案不存在".into()))?;
        let mut profile = profile;
        profile.created_at = target.created_at.clone();
        *target = profile;
        self.save(&reg)
    }

    /// 根据 profile 建立连接，返回统一句柄。
    pub async fn connect(profile: &FtpProfile) -> Result<FtpClient, Error> {
        match profile.protocol {
            FtpProtocol::Ftp => {
                let mut ftp = AsyncFtpStream::connect((profile.host.as_str(), profile.port))
                    .await
                    .map_err(ftp_err)?;
                ftp.login(&profile.username, &profile.password)
                    .await
                    .map_err(ftp_err)?;
                Ok(FtpClient::Ftp(ftp))
            }
            FtpProtocol::Ftps => {
                // 用 AsyncRustlsFtpStream 连接（先明文 TCP，再 AUTH TLS 升级），
                // 这样 into_secure 的泛型约束 Stream = AsyncRustlsStream 可满足。
                let ftp = AsyncRustlsFtpStream::connect((profile.host.as_str(), profile.port))
                    .await
                    .map_err(ftp_err)?;
                let config = rustls_client_config();
                let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
                let domain = if profile.host.is_empty() {
                    "localhost"
                } else {
                    &profile.host
                };
                let sec = AsyncRustlsConnector::from(connector);
                let mut ftps = ftp.into_secure(sec, domain).await.map_err(ftp_err)?;
                ftps.login(&profile.username, &profile.password)
                    .await
                    .map_err(ftp_err)?;
                Ok(FtpClient::Ftps(ftps))
            }
            FtpProtocol::Sftp => {
                let session = connect_ssh(profile).await?;
                let channel = session
                    .channel_open_session()
                    .await
                    .map_err(|e| Error::Other(format!("SSH 开启通道失败: {e}")))?;
                channel
                    .request_subsystem(true, "sftp")
                    .await
                    .map_err(|e| Error::Other(format!("SSH 请求 sftp 子系统失败: {e}")))?;
                let stream = channel.into_stream();
                let sftp = SftpSession::new(stream)
                    .await
                    .map_err(|e| Error::Other(format!("SFTP 会话建立失败: {e}")))?;
                Ok(FtpClient::Sftp(sftp))
            }
        }
    }

    /// 测试连接（连接 + 列限定根目录）。
    pub async fn test_connection(profile: &FtpProfile) -> Result<String, Error> {
        let client = Self::connect(profile).await?;
        let root = profile.effective_root();
        let _ = Self::list_dir_with(client, &root).await?;
        Ok(format!("{:?} 连接成功", profile.protocol))
    }

    /// 列出远程目录内容。`path` 会被归一化到限定根目录下。
    pub async fn list_dir(profile: &FtpProfile, path: &str) -> Result<Vec<FtpEntry>, Error> {
        let client = Self::connect(profile).await?;
        let resolved = profile.resolve_remote(path)?;
        Self::list_dir_with(client, &resolved).await
    }

    async fn list_dir_with(
        client: FtpClient,
        path: &str,
    ) -> Result<Vec<FtpEntry>, Error> {
        match client {
            FtpClient::Ftp(mut ftp) => {
                ftp.cwd(path).await.map_err(ftp_err)?;
                let lines = ftp.list(None).await.map_err(ftp_err)?;
                Ok(parse_ftp_list(&lines))
            }
            FtpClient::Ftps(mut ftp) => {
                ftp.cwd(path).await.map_err(ftp_err)?;
                let lines = ftp.list(None).await.map_err(ftp_err)?;
                Ok(parse_ftp_list(&lines))
            }
            FtpClient::Sftp(sftp) => {
                let read = sftp.read_dir(path).await.map_err(sftp_err)?;
                let mut entries = Vec::new();
                for entry in read {
                    let meta = entry.metadata();
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|t| {
                            t.duration_since(std::time::UNIX_EPOCH)
                                .ok()
                                .map(|d| {
                                    chrono::DateTime::<chrono::Utc>::from_timestamp(
                                        d.as_secs() as i64,
                                        d.subsec_nanos(),
                                    )
                                    .map(|dt| dt.to_rfc3339())
                                    .unwrap_or_default()
                                })
                        })
                        .unwrap_or_default();
                    entries.push(FtpEntry {
                        name: entry.file_name(),
                        is_dir: meta.file_type().is_dir(),
                        size: meta.len(),
                        modified,
                    });
                }
                // 目录优先，再按名称排序
                entries.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                Ok(entries)
            }
        }
    }

    /// 上传本地文件到远程。
    ///
    /// `progress` 回调在每块写入后被调用，参数为 (已传输字节, 总字节, 文件名)。
    pub async fn upload(
        profile: &FtpProfile,
        local_path: &str,
        remote_path: &str,
        progress: ProgressFn<'_>,
    ) -> Result<(), Error> {
        let remote_path = profile.resolve_remote(remote_path)?;
        let file_name = std::path::Path::new(local_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let total = tokio::fs::metadata(local_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let client = Self::connect(profile).await?;
        Self::upload_with_client(client, local_path, &remote_path, &file_name, total, progress).await
    }

    /// 已解析远程路径 + 已建立连接的内部上传，供 `upload` 与 `upload_dir_inner` 复用。
    async fn upload_with_client(
        client: FtpClient,
        local_path: &str,
        remote_path: &str,
        file_name: &str,
        total: u64,
        progress: ProgressFn<'_>,
    ) -> Result<(), Error> {
        let mut transferred: u64 = 0;
        match client {
            FtpClient::Ftp(mut ftp) => {
                ftp.transfer_type(FileType::Binary).await.map_err(ftp_err)?;
                let mut stream = ftp.put_with_stream(remote_path).await.map_err(ftp_err)?;
                let mut file = tokio::fs::File::open(local_path)
                    .await
                    .map_err(Error::Io)?;
                let mut buf = [0u8; 8192];
                loop {
                    let n = file.read(&mut buf).await.map_err(Error::Io)?;
                    if n == 0 {
                        break;
                    }
                    stream.write_all(&buf[..n]).await.map_err(ftp_io_err)?;
                    transferred += n as u64;
                    if let Some(cb) = progress {
                        cb(transferred, total, file_name);
                    }
                }
                ftp.finalize_put_stream(stream).await.map_err(ftp_err)?;
            }
            FtpClient::Ftps(mut ftp) => {
                ftp.transfer_type(FileType::Binary).await.map_err(ftp_err)?;
                let mut stream = ftp.put_with_stream(remote_path).await.map_err(ftp_err)?;
                let mut file = tokio::fs::File::open(local_path)
                    .await
                    .map_err(Error::Io)?;
                let mut buf = [0u8; 8192];
                loop {
                    let n = file.read(&mut buf).await.map_err(Error::Io)?;
                    if n == 0 {
                        break;
                    }
                    stream.write_all(&buf[..n]).await.map_err(ftp_io_err)?;
                    transferred += n as u64;
                    if let Some(cb) = progress {
                        cb(transferred, total, file_name);
                    }
                }
                ftp.finalize_put_stream(stream).await.map_err(ftp_err)?;
            }
            FtpClient::Sftp(sftp) => {
                let mut file = tokio::fs::File::open(local_path)
                    .await
                    .map_err(Error::Io)?;
                let mut remote = sftp
                    .create(remote_path)
                    .await
                    .map_err(sftp_err)?;
                let mut buf = [0u8; 8192];
                loop {
                    let n = file.read(&mut buf).await.map_err(Error::Io)?;
                    if n == 0 {
                        break;
                    }
                    remote.write_all(&buf[..n]).await.map_err(sftp_err_io)?;
                    transferred += n as u64;
                    if let Some(cb) = progress {
                        cb(transferred, total, file_name);
                    }
                }
                remote.flush().await.map_err(sftp_err_io)?;
            }
        }
        Ok(())
    }

    /// 下载远程文件到本地。
    ///
    /// `progress` 回调在每块写入后被调用，参数为 (已传输字节, 总字节, 文件名)。
    pub async fn download(
        profile: &FtpProfile,
        remote_path: &str,
        local_path: &str,
        progress: ProgressFn<'_>,
    ) -> Result<(), Error> {
        let remote_path = profile.resolve_remote(remote_path)?;
        let file_name = std::path::Path::new(&remote_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let client = Self::connect(profile).await?;
        let mut transferred: u64 = 0;
        match client {
            FtpClient::Ftp(mut ftp) => {
                ftp.transfer_type(FileType::Binary).await.map_err(ftp_err)?;
                let total = ftp.size(&remote_path).await.unwrap_or(0) as u64;
                let stream = ftp.retr_as_stream(&remote_path).await.map_err(ftp_err)?;
                let mut stream = stream;
                let mut file = tokio::fs::File::create(local_path)
                    .await
                    .map_err(Error::Io)?;
                let mut buf = [0u8; 8192];
                loop {
                    let n = stream.read(&mut buf).await.map_err(ftp_io_err)?;
                    if n == 0 {
                        break;
                    }
                    file.write_all(&buf[..n]).await.map_err(Error::Io)?;
                    transferred += n as u64;
                    if let Some(cb) = progress {
                        cb(transferred, total, &file_name);
                    }
                }
                file.flush().await.map_err(Error::Io)?;
                ftp.finalize_retr_stream(stream).await.map_err(ftp_err)?;
            }
            FtpClient::Ftps(mut ftp) => {
                ftp.transfer_type(FileType::Binary).await.map_err(ftp_err)?;
                let total = ftp.size(&remote_path).await.unwrap_or(0) as u64;
                let stream = ftp.retr_as_stream(&remote_path).await.map_err(ftp_err)?;
                let mut stream = stream;
                let mut file = tokio::fs::File::create(local_path)
                    .await
                    .map_err(Error::Io)?;
                let mut buf = [0u8; 8192];
                loop {
                    let n = stream.read(&mut buf).await.map_err(ftp_io_err)?;
                    if n == 0 {
                        break;
                    }
                    file.write_all(&buf[..n]).await.map_err(Error::Io)?;
                    transferred += n as u64;
                    if let Some(cb) = progress {
                        cb(transferred, total, &file_name);
                    }
                }
                file.flush().await.map_err(Error::Io)?;
                ftp.finalize_retr_stream(stream).await.map_err(ftp_err)?;
            }
            FtpClient::Sftp(sftp) => {
                let total = sftp
                    .metadata(&remote_path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                let mut remote = sftp.open(&remote_path).await.map_err(sftp_err)?;
                let mut file = tokio::fs::File::create(local_path)
                    .await
                    .map_err(Error::Io)?;
                let mut buf = [0u8; 8192];
                loop {
                    let n = remote.read(&mut buf).await.map_err(sftp_err_io)?;
                    if n == 0 {
                        break;
                    }
                    file.write_all(&buf[..n]).await.map_err(Error::Io)?;
                    transferred += n as u64;
                    if let Some(cb) = progress {
                        cb(transferred, total, &file_name);
                    }
                }
                file.flush().await.map_err(Error::Io)?;
            }
        }
        Ok(())
    }

    /// 递归上传本地目录到远程。
    ///
    /// 会遍历 `local_dir` 下所有文件和子目录，在远程 `remote_dir` 下重建结构。
    /// `progress` 回调对每个文件的每块写入触发，参数为 (已传输字节, 当前文件总字节, 文件名)。
    pub async fn upload_dir(
        profile: &FtpProfile,
        local_dir: &str,
        remote_dir: &str,
        progress: ProgressFn<'_>,
    ) -> Result<(), Error> {
        let remote_dir = profile.resolve_remote(remote_dir)?;
        Self::upload_dir_inner(profile, std::path::Path::new(local_dir), &remote_dir, progress)
            .await
    }

    async fn upload_dir_inner(
        profile: &FtpProfile,
        local_dir: &std::path::Path,
        remote_dir: &str,
        progress: ProgressFn<'_>,
    ) -> Result<(), Error> {
        // 确保远程目录存在（忽略已存在错误）；路径已由调用方 resolve，这里直接建
        if let Ok(client) = Self::connect(profile).await {
            let _ = Self::make_dir_with_client(client, remote_dir).await;
        }

        let mut entries = tokio::fs::read_dir(local_dir)
            .await
            .map_err(Error::Io)?;
        while let Some(entry) = entries.next_entry().await.map_err(Error::Io)? {
            let name = entry.file_name().to_string_lossy().to_string();
            let local_path = entry.path();
            let remote_path = format!("{}/{}", remote_dir.trim_end_matches('/'), name);
            let file_type = entry.file_type().await.map_err(Error::Io)?;
            if file_type.is_dir() {
                Box::pin(Self::upload_dir_inner(profile, &local_path, &remote_path, progress))
                    .await?;
            } else if file_type.is_file() {
                let file_name = std::path::Path::new(&local_path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let total = tokio::fs::metadata(&local_path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                let client = Self::connect(profile).await?;
                Self::upload_with_client(
                    client,
                    &local_path.to_string_lossy(),
                    &remote_path,
                    &file_name,
                    total,
                    progress,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// 删除文件或目录。`path` 会被归一化到限定根目录下。
    pub async fn delete(profile: &FtpProfile, path: &str, is_dir: bool) -> Result<(), Error> {
        let path = profile.resolve_remote(path)?;
        let client = Self::connect(profile).await?;
        Self::delete_with_client(client, &path, is_dir).await
    }

    async fn delete_with_client(client: FtpClient, path: &str, is_dir: bool) -> Result<(), Error> {
        match client {
            FtpClient::Ftp(mut ftp) => {
                if is_dir {
                    ftp.rmdir(path).await.map_err(ftp_err)?;
                } else {
                    ftp.rm(path).await.map_err(ftp_err)?;
                }
            }
            FtpClient::Ftps(mut ftp) => {
                if is_dir {
                    ftp.rmdir(path).await.map_err(ftp_err)?;
                } else {
                    ftp.rm(path).await.map_err(ftp_err)?;
                }
            }
            FtpClient::Sftp(sftp) => {
                if is_dir {
                    sftp.remove_dir(path).await.map_err(sftp_err)?;
                } else {
                    sftp.remove_file(path).await.map_err(sftp_err)?;
                }
            }
        }
        Ok(())
    }

    /// 创建目录。`path` 会被归一化到限定根目录下。
    pub async fn make_dir(profile: &FtpProfile, path: &str) -> Result<(), Error> {
        let path = profile.resolve_remote(path)?;
        let client = Self::connect(profile).await?;
        Self::make_dir_with_client(client, &path).await
    }

    async fn make_dir_with_client(client: FtpClient, path: &str) -> Result<(), Error> {
        match client {
            FtpClient::Ftp(mut ftp) => {
                ftp.mkdir(path).await.map_err(ftp_err)?;
            }
            FtpClient::Ftps(mut ftp) => {
                ftp.mkdir(path).await.map_err(ftp_err)?;
            }
            FtpClient::Sftp(sftp) => {
                sftp.create_dir(path).await.map_err(sftp_err)?;
            }
        }
        Ok(())
    }

    /// 重命名文件或目录。`from`/`to` 会被归一化到限定根目录下。
    pub async fn rename(profile: &FtpProfile, from: &str, to: &str) -> Result<(), Error> {
        let from = profile.resolve_remote(from)?;
        let to = profile.resolve_remote(to)?;
        let client = Self::connect(profile).await?;
        match client {
            FtpClient::Ftp(mut ftp) => {
                ftp.rename(&from, &to).await.map_err(ftp_err)?;
            }
            FtpClient::Ftps(mut ftp) => {
                ftp.rename(&from, &to).await.map_err(ftp_err)?;
            }
            FtpClient::Sftp(sftp) => {
                sftp.rename(&from, &to).await.map_err(sftp_err)?;
            }
        }
        Ok(())
    }
}

/// 建立 SSH 连接并认证（SFTP 专用）。
///
/// 认证逻辑与 `db::tunnel::SshTunnel::open` 一致：优先密钥，其次密码。
/// 独立实现以避免改动已验证的隧道模块。
async fn connect_ssh(profile: &FtpProfile) -> Result<client::Handle<SshHandler>, Error> {
    let ssh_host = &profile.host;
    let ssh_port = profile.port;
    let ssh_user = if profile.username.is_empty() {
        "root"
    } else {
        &profile.username
    };

    let config = client::Config::default();
    let mut session = client::connect(
        Arc::new(config),
        format!("{ssh_host}:{ssh_port}"),
        SshHandler {},
    )
    .await
    .map_err(|e| Error::Other(format!("SSH 连接失败: {e}")))?;

    // 认证：优先密钥，其次密码
    let auth_ok = if let Some(key_path) = &profile.ssh_key {
        let key_pair = russh_keys::load_secret_key(key_path, None)
            .map_err(|e| Error::Other(format!("SSH 密钥加载失败: {e}")))?;
        session
            .authenticate_publickey(ssh_user, Arc::new(key_pair))
            .await
            .map_err(|e| Error::Other(format!("SSH 密钥认证失败: {e}")))?
    } else if let Some(password) = &profile.ssh_password {
        session
            .authenticate_password(ssh_user, password)
            .await
            .map_err(|e| Error::Other(format!("SSH 密码认证失败: {e}")))?
    } else if !profile.password.is_empty() {
        session
            .authenticate_password(ssh_user, &profile.password)
            .await
            .map_err(|e| Error::Other(format!("SSH 密码认证失败: {e}")))?
    } else {
        return Err(Error::Other("SFTP 需要配置密钥或密码".into()));
    };

    if !auth_ok {
        return Err(Error::Other("SSH 认证失败".into()));
    }
    Ok(session)
}

/// 构建 rustls 客户端配置（内置 CA + 跳过自签校验，本地工具场景）。
fn rustls_client_config() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// 解析 FTP LIST 命令返回的行列表。
fn parse_ftp_list(lines: &[String]) -> Vec<FtpEntry> {
    let mut entries = Vec::new();
    for line in lines {
        if let Ok(f) = FtpFile::from_str(line) {
            let modified = f
                .modified()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(
                        d.as_secs() as i64,
                        d.subsec_nanos(),
                    )
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
                })
                .unwrap_or_default();
            entries.push(FtpEntry {
                name: f.name().to_string(),
                is_dir: f.is_directory(),
                size: f.size() as u64,
                modified,
            });
        }
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// 通过 suppaftp 上传文件（STOR）——已内联至 upload，此处保留占位以维持模块可读性。
/// 实际上传逻辑见 `FtpManager::upload`。

/// 归一化远程路径：合并 `.`/`..` 段、折叠多余斜杠、保证以 `/` 开头。
///
/// 例：`/a/b/../c/./d` → `/a/c/d`，`a//b` → `/a/b`。
fn normalize_remote_path(input: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in input.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    format!("/{}", joined)
}

/// suppaftp 错误转核心错误。
fn ftp_err(e: FtpError) -> Error {
    Error::Other(format!("FTP 错误: {e}"))
}

/// suppaftp 流 IO 错误转核心错误。
fn ftp_io_err(e: std::io::Error) -> Error {
    Error::Other(format!("FTP 流错误: {e}"))
}

/// russh-sftp 错误转核心错误。
fn sftp_err(e: russh_sftp::client::error::Error) -> Error {
    Error::Other(format!("SFTP 错误: {e}"))
}

/// russh-sftp 包装的 io 错误转核心错误。
fn sftp_err_io(e: std::io::Error) -> Error {
    Error::Other(format!("SFTP IO 错误: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_serialization() {
        let p = FtpProfile::new("测试".into(), FtpProtocol::Sftp, "example.com".into(), 22);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"protocol\":\"sftp\""));
        let back: FtpProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.protocol, FtpProtocol::Sftp);
        assert_eq!(back.name, "测试");
    }

    #[test]
    fn protocol_lowercase() {
        for (proto, s) in [
            (FtpProtocol::Ftp, "ftp"),
            (FtpProtocol::Sftp, "sftp"),
            (FtpProtocol::Ftps, "ftps"),
        ] {
            let p = FtpProfile::new("x".into(), proto, "h".into(), 0);
            let json = serde_json::to_string(&p).unwrap();
            assert!(json.contains(&format!("\"protocol\":\"{s}\"")));
        }
    }

    #[test]
    fn default_port() {
        assert_eq!(FtpProtocol::Ftp.default_port(), 21);
        assert_eq!(FtpProtocol::Ftps.default_port(), 21);
        assert_eq!(FtpProtocol::Sftp.default_port(), 22);
    }

    #[test]
    fn registry_roundtrip() {
        let reg = FtpProfileRegistry {
            profiles: vec![
                FtpProfile::new("a".into(), FtpProtocol::Ftp, "h1".into(), 21),
                FtpProfile::new("b".into(), FtpProtocol::Sftp, "h2".into(), 22),
            ],
        };
        let json = serde_json::to_string(&reg).unwrap();
        let back: FtpProfileRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.profiles.len(), 2);
    }

    #[test]
    fn manager_crud() {
        let dir = std::env::temp_dir().join("runphp-ftp-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = FtpManager::new(&dir);
        assert!(mgr.list_profiles().unwrap().is_empty());
        let p = FtpProfile::new("测试".into(), FtpProtocol::Ftp, "h".into(), 21);
        mgr.add_profile(p).unwrap();
        assert_eq!(mgr.list_profiles().unwrap().len(), 1);
        let id = mgr.list_profiles().unwrap()[0].id.clone();
        mgr.remove_profile(&id).unwrap();
        assert!(mgr.list_profiles().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_profile_preserves_created_at() {
        let dir = std::env::temp_dir().join("runphp-ftp-update-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = FtpManager::new(&dir);
        let p = FtpProfile::new("原始".into(), FtpProtocol::Ftp, "h1".into(), 21);
        mgr.add_profile(p).unwrap();
        let id = mgr.list_profiles().unwrap()[0].id.clone();
        let original_created = mgr.list_profiles().unwrap()[0].created_at.clone();

        let mut updated = mgr.list_profiles().unwrap()[0].clone();
        updated.name = "改后".into();
        updated.host = "h2".into();
        mgr.update_profile(updated).unwrap();

        let after_reg = mgr.load().unwrap();
        let after = after_reg.get(&id).unwrap();
        assert_eq!(after.name, "改后");
        assert_eq!(after.host, "h2");
        assert_eq!(after.created_at, original_created);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_nonexistent_profile_errors() {
        let dir = std::env::temp_dir().join("runphp-ftp-missing-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = FtpManager::new(&dir);
        let p = FtpProfile::new("不存在".into(), FtpProtocol::Ftp, "h".into(), 21);
        assert!(mgr.update_profile(p).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn registry_get_by_id() {
        let mut reg = FtpProfileRegistry::default();
        let p = FtpProfile::new("x".into(), FtpProtocol::Sftp, "h".into(), 22);
        let id = p.id.clone();
        reg.profiles.push(p);
        assert!(reg.get(&id).is_some());
        assert!(reg.get("nope").is_none());
        reg.get_mut(&id).unwrap().name = "y".into();
        assert_eq!(reg.get(&id).unwrap().name, "y");
    }

    #[test]
    fn normalize_remote_path_basic() {
        assert_eq!(normalize_remote_path("/a/b/../c"), "/a/c");
        assert_eq!(normalize_remote_path("a//b/./c"), "/a/b/c");
        assert_eq!(normalize_remote_path("/a/../../b"), "/b");
        assert_eq!(normalize_remote_path(""), "/");
        assert_eq!(normalize_remote_path("/"), "/");
        assert_eq!(normalize_remote_path("/var/www/"), "/var/www");
    }

    #[test]
    fn resolve_remote_no_root() {
        // 无限定根目录时，绝对路径直接归一化
        let p = FtpProfile::new("x".into(), FtpProtocol::Ftp, "h".into(), 21);
        assert_eq!(p.resolve_remote("/a/b/../c").unwrap(), "/a/c");
        assert_eq!(p.resolve_remote("a//b").unwrap(), "/a/b");
        assert_eq!(p.resolve_remote("/").unwrap(), "/");
        assert_eq!(p.resolve_remote("").unwrap(), "/");
    }

    #[test]
    fn resolve_remote_with_root() {
        let mut p = FtpProfile::new("x".into(), FtpProtocol::Ftp, "h".into(), 21);
        p.root_dir = Some("/var/www".into());
        // 相对路径拼到根下
        assert_eq!(p.resolve_remote("site").unwrap(), "/var/www/site");
        // 绝对路径挂到根下
        assert_eq!(p.resolve_remote("/site/index.html").unwrap(), "/var/www/site/index.html");
        // 根标记返回限定根
        assert_eq!(p.resolve_remote("/").unwrap(), "/var/www");
        // 越界访问被拒绝
        assert!(p.resolve_remote("../etc/passwd").is_err());
        assert!(p.resolve_remote("/../etc").is_err());
        // 带点的路径归一化但不越界
        assert_eq!(p.resolve_remote("site/./sub/../").unwrap(), "/var/www/site");
    }

    #[test]
    fn resolve_remote_root_with_trailing_slash() {
        let mut p = FtpProfile::new("x".into(), FtpProtocol::Ftp, "h".into(), 21);
        p.root_dir = Some("/var/www/".into());
        assert_eq!(p.resolve_remote("site").unwrap(), "/var/www/site");
        assert_eq!(p.resolve_remote("/").unwrap(), "/var/www");
    }

    #[test]
    fn resolve_remote_root_normalize() {
        let mut p = FtpProfile::new("x".into(), FtpProtocol::Ftp, "h".into(), 21);
        p.root_dir = Some("/a/../var/www".into());
        assert_eq!(p.effective_root(), "/var/www");
    }

    #[test]
    fn resolve_remote_empty_root_string() {
        let mut p = FtpProfile::new("x".into(), FtpProtocol::Ftp, "h".into(), 21);
        p.root_dir = Some("  ".into());
        // 空白串视为不限定
        assert_eq!(p.resolve_remote("/a/b").unwrap(), "/a/b");
    }
}
