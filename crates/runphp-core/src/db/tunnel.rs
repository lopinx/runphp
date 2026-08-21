//! SSH 隧道：通过 SSH 端口转发安全连接远程数据库。
//!
//! 当连接档案配置了 SSH 主机时，先建立 SSH 隧道（本地端口转发），
//! 然后数据库连接走本地隧道端口，实现加密传输。
//! 隧道在 SshTunnel 被 drop 时通过 AbortHandle 中止转发任务，
//! listener 关闭后 accept 循环退出，session Arc 归零后 SSH 连接断开。

use crate::db::remote::ConnectionProfile;
use crate::Error;
use russh::client;
use russh::keys::key;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// SSH 隧道句柄。
///
/// drop 时通过 AbortHandle 中止端口转发任务，
/// listener 随任务结束关闭，session Arc 归零后 SSH 连接自然断开。
pub struct SshTunnel {
    /// 本地监听端口。
    local_port: u16,
    /// 端口转发任务的 AbortHandle，drop 时中止任务。
    forward_task: AbortHandle,
}

/// SSH 客户端回调（空实现，仅满足 trait 要求）。
struct ClientHandler;

#[async_trait::async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // 本地工具，不校验主机密钥（用户自行确认安全性）
        Ok(true)
    }
}

impl SshTunnel {
    /// 根据 ConnectionProfile 建立 SSH 隧道。
    ///
    /// 若 profile 未配置 SSH，返回 None（调用方直接走 TCP）。
    /// 成功时返回隧道实例，调用方通过 `local_port()` 获取本地端口。
    pub async fn open(profile: &ConnectionProfile) -> Result<Option<Self>, Error> {
        if !profile.ssh_enabled() {
            return Ok(None);
        }

        let ssh_host = profile.ssh_host.as_ref().unwrap();
        let ssh_port = profile.ssh_port.unwrap_or(22).max(1);
        let ssh_user = profile.ssh_user.as_deref().unwrap_or("root");

        let config = client::Config::default();
        let mut session = client::connect(
            Arc::new(config),
            format!("{ssh_host}:{ssh_port}"),
            ClientHandler {},
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
        } else {
            return Err(Error::Other("SSH 需要配置密钥或密码".into()));
        };

        if !auth_ok {
            return Err(Error::Other("SSH 认证失败".into()));
        }

        // 分配本地随机端口
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| Error::Other(format!("绑定本地端口失败: {e}")))?;
        let local_port = listener
            .local_addr()
            .map_err(|e| Error::Other(format!("获取本地端口失败: {e}")))?
            .port();

        // 启动端口转发：本地端口 → SSH → 数据库 host:port
        let db_host = profile.host.clone();
        let db_port = profile.port;
        let session_arc = Arc::new(session);
        let forward_task = tokio::spawn(async move {
            loop {
                let (mut local_stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let tunnel_host = db_host.clone();
                let session_ref = session_arc.clone();
                tokio::spawn(async move {
                    let mut channel = match session_ref
                        .channel_open_direct_tcpip(
                            &tunnel_host,
                            db_port as u32,
                            "127.0.0.1",
                            0,
                        )
                        .await
                    {
                        Ok(c) => c,
                        Err(_) => return,
                    };
                    // 双向数据转发
                    let mut buf = [0u8; 8192];
                    loop {
                        tokio::select! {
                            n = local_stream.read(&mut buf) => {
                                match n {
                                    Ok(0) | Err(_) => {
                                        let _ = channel.eof().await;
                                        break;
                                    }
                                    Ok(n) => {
                                        if channel.data(&buf[..n]).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            msg = channel.wait() => {
                                match msg {
                                    Some(russh::ChannelMsg::Data { data }) => {
                                        if local_stream.write_all(&data).await.is_err() {
                                            break;
                                        }
                                    }
                                    Some(russh::ChannelMsg::Eof)
                                    | Some(russh::ChannelMsg::Close)
                                    | None => break,
                                    _ => {}
                                }
                            }
                        }
                    }
                });
            }
        })
        .abort_handle();

        tracing::info!(
            "SSH 隧道已建立: 127.0.0.1:{local_port} → {ssh_host}:{ssh_port} → {}:{db_port}",
            profile.host
        );

        Ok(Some(Self {
            local_port,
            forward_task,
        }))
    }

    /// 获取本地隧道端口。
    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        // 中止端口转发任务，listener 随之关闭
        self.forward_task.abort();
    }
}
