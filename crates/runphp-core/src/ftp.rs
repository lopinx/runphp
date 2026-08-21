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
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// 档案注册表（持久化为 `ftp_profiles.json`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FtpProfileRegistry {
    pub profiles: Vec<FtpProfile>,
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

    /// 测试连接（连接 + 列根目录）。
    pub async fn test_connection(profile: &FtpProfile) -> Result<String, Error> {
        let client = Self::connect(profile).await?;
        let _ = Self::list_dir_with(client, "/").await?;
        Ok(format!("{:?} 连接成功", profile.protocol))
    }

    /// 列出远程目录内容。
    pub async fn list_dir(profile: &FtpProfile, path: &str) -> Result<Vec<FtpEntry>, Error> {
        let client = Self::connect(profile).await?;
        Self::list_dir_with(client, path).await
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
    pub async fn upload(
        profile: &FtpProfile,
        local_path: &str,
        remote_path: &str,
    ) -> Result<(), Error> {
        let client = Self::connect(profile).await?;
        match client {
            FtpClient::Ftp(mut ftp) => {
                ftp.transfer_type(FileType::Binary).await.map_err(ftp_err)?;
                let mut file = tokio::fs::File::open(local_path)
                    .await
                    .map_err(Error::Io)?;
                ftp.put_file(remote_path, &mut file)
                    .await
                    .map_err(ftp_err)?;
            }
            FtpClient::Ftps(mut ftp) => {
                ftp.transfer_type(FileType::Binary).await.map_err(ftp_err)?;
                let mut file = tokio::fs::File::open(local_path)
                    .await
                    .map_err(Error::Io)?;
                ftp.put_file(remote_path, &mut file)
                    .await
                    .map_err(ftp_err)?;
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
                }
                remote.flush().await.map_err(sftp_err_io)?;
            }
        }
        Ok(())
    }

    /// 下载远程文件到本地。
    pub async fn download(
        profile: &FtpProfile,
        remote_path: &str,
        local_path: &str,
    ) -> Result<(), Error> {
        let client = Self::connect(profile).await?;
        match client {
            FtpClient::Ftp(mut ftp) => {
                ftp.transfer_type(FileType::Binary).await.map_err(ftp_err)?;
                let stream = ftp.retr_as_stream(remote_path).await.map_err(ftp_err)?;
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
                }
                file.flush().await.map_err(Error::Io)?;
                ftp.finalize_retr_stream(stream).await.map_err(ftp_err)?;
            }
            FtpClient::Ftps(mut ftp) => {
                ftp.transfer_type(FileType::Binary).await.map_err(ftp_err)?;
                let stream = ftp.retr_as_stream(remote_path).await.map_err(ftp_err)?;
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
                }
                file.flush().await.map_err(Error::Io)?;
                ftp.finalize_retr_stream(stream).await.map_err(ftp_err)?;
            }
            FtpClient::Sftp(sftp) => {
                let mut remote = sftp.open(remote_path).await.map_err(sftp_err)?;
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
                }
                file.flush().await.map_err(Error::Io)?;
            }
        }
        Ok(())
    }

    /// 删除文件或目录。
    pub async fn delete(profile: &FtpProfile, path: &str, is_dir: bool) -> Result<(), Error> {
        let client = Self::connect(profile).await?;
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

    /// 创建目录。
    pub async fn make_dir(profile: &FtpProfile, path: &str) -> Result<(), Error> {
        let client = Self::connect(profile).await?;
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

    /// 重命名文件或目录。
    pub async fn rename(profile: &FtpProfile, from: &str, to: &str) -> Result<(), Error> {
        let client = Self::connect(profile).await?;
        match client {
            FtpClient::Ftp(mut ftp) => {
                ftp.rename(from, to).await.map_err(ftp_err)?;
            }
            FtpClient::Ftps(mut ftp) => {
                ftp.rename(from, to).await.map_err(ftp_err)?;
            }
            FtpClient::Sftp(sftp) => {
                sftp.rename(from, to).await.map_err(sftp_err)?;
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
}
