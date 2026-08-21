//! Caddyfile 生成与 FrankenPHP 进程生命周期管理。
//!
//! FrankenPHP 本质是内置 PHP 的 Caddy，配置走 Caddyfile，支持 admin API
//! （默认 127.0.0.1:2019）热重载与 stop。

use crate::{config::AppConfig, site::Site, Error, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;

/// 生成的 Caddyfile 全局选项块。
fn global_block(admin_addr: &str) -> String {
    format!("{{\n    frankenphp\n    admin {admin_addr}\n}}\n")
}

/// 单个站点的 Caddyfile 片段。
fn site_block(site: &Site) -> String {
    let mut lines = Vec::new();

    // 地址行：域名列表 + 端口（0 表示由 Caddy 自动）
    let mut addrs = site.domains.clone();
    if site.port > 0 {
        addrs = addrs
            .iter()
            .map(|d| format!("{d}:{}", site.port))
            .collect();
    }
    lines.push(format!("{} {{", addrs.join(", ")));
    lines.push(format!("    root * {}", quote_path(&site.root)));
    lines.push("    encode gzip".into());

    if site.https {
        lines.push("    tls internal".into());
    }

    // PHP 处理：worker 模式或普通模式
    if let Some(w) = &site.worker {
        let script = quote_path(&PathBuf::from(&w.script));
        lines.push(format!(
            "    php_server {{\n        worker {} {}\n    }}",
            script, w.num
        ));
    } else {
        lines.push("    php_server".into());
    }

    lines.push("}".into());
    lines.join("\n") + "\n"
}

/// 以 Caddyfile 语法规则对路径加引号转义。
///
/// Caddyfile 中含空格或特殊字符的路径需用双引号包裹，
/// 路径内的双引号需用 `\"` 转义，反斜杠保持原样（不做额外转义）。
fn quote_path(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    if s.contains(' ') || s.contains('"') {
        let escaped = s.replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// 根据全部站点生成完整 Caddyfile 文本（末尾自动附加 Adminer 站点块）。
///
/// `adminer_root` 为 Adminer PHP 文件所在目录的绝对路径。
pub fn generate_caddyfile(sites: &[Site], admin_addr: &str, adminer_root: &std::path::Path) -> String {
    let mut out = String::new();
    out.push_str("# 由 RunPHP 自动生成，请勿手动编辑\n");
    out.push_str(&global_block(admin_addr));
    out.push('\n');
    for s in sites {
        out.push_str(&site_block(s));
        out.push('\n');
    }
    // Adminer 内置站点（固定端口，无需域名/hosts）
    out.push_str(&format!(
        ":{} {{\n    root * {}\n    encode gzip\n    php_server\n}}\n",
        crate::adminer::ADMINER_PORT,
        quote_path(adminer_root)
    ));
    out
}

/// 将 Caddyfile 写入配置指定路径。
pub fn write_caddyfile(cfg: &AppConfig, sites: &[Site]) -> Result<()> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let adminer_root = cfg.data_dir.join("adminer");
    let content = generate_caddyfile(sites, "127.0.0.1:2019", &adminer_root);
    std::fs::write(cfg.caddyfile_path(), content)?;
    Ok(())
}

/// 写入 Caddyfile 并在 FrankenPHP 运行中时自动热重载。
///
/// 用于站点增删改后无需手动重启：写完配置后探测 admin API，
/// 可达则执行 reload，不可达则跳过（进程未启动或已停止）。
pub async fn write_and_reload(
    cfg: &AppConfig,
    sites: &[Site],
    binary: &std::path::Path,
) -> Result<()> {
    write_caddyfile(cfg, sites)?;
    if status().await {
        if let Err(e) = reload(cfg, binary).await {
            tracing::warn!("自动热重载失败（配置已写入磁盘）: {e}");
        }
    }
    Ok(())
}

/// 进程句柄信息。
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub log_path: PathBuf,
}

/// 模块级进程监控状态：持有子进程句柄与重启参数。
///
/// 通过 `tokio::sync::Mutex` 保护，`start` 时存入、`stop` 时清空。
/// 监控任务在子进程意外退出时自动重启（最多 5 次/30 秒）。
#[allow(dead_code)]
struct Supervisor {
    /// 子进程句柄（去掉 kill_on_drop，由 supervisor 管理生命周期）。
    child: tokio::process::Child,
    /// 二进制路径（重启时使用）。
    binary: PathBuf,
    /// 配置快照（重启时使用）。
    cfg: AppConfig,
    /// 主动停止标志：true 表示用户调用了 stop，不重启。
    stopping: Arc<AtomicBool>,
}

/// 全局唯一的 supervisor 实例。
static SUPERVISOR: Mutex<Option<Supervisor>> = Mutex::const_new(None);

/// 自动重启参数：30 秒内最多 5 次，超过则放弃。
const RESTART_MAX: u32 = 5;
const RESTART_WINDOW_SECS: u64 = 30;
const RESTART_DELAY_SECS: u64 = 2;

/// 启动 FrankenPHP 子进程，并注册崩溃自动重启监控。
///
/// 监控任务在后台等待子进程退出：若为意外退出（非用户主动 stop），
/// 间隔 2 秒后自动重启，30 秒窗口内最多重试 5 次。
pub async fn start(cfg: &AppConfig, binary: &std::path::Path) -> Result<ProcessInfo> {
    let caddyfile = cfg.caddyfile_path();
    // Caddyfile 不存在时自动生成一份空配置（含 Adminer 站点块），
    // 允许无站点状态下启动 FrankenPHP 以使用 Adminer 数据库管理。
    if !caddyfile.exists() {
        write_caddyfile(cfg, &[])?;
    }
    let log_path = cfg.logs_dir().join("frankenphp.log");
    std::fs::create_dir_all(cfg.logs_dir())?;

    // 截断旧日志
    let log_file = std::fs::File::create(&log_path)?;
    let stderr = log_file.try_clone()?;

    let child = Command::new(binary)
        .arg("run")
        .arg("--config")
        .arg(&caddyfile)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| Error::Caddy(format!("启动 FrankenPHP 失败: {e}")))?;

    let pid = child
        .id()
        .ok_or_else(|| Error::Caddy("无法获取子进程 PID".into()))?;

    // 记录 PID 供后续停止
    let pid_file = cfg.data_dir.join("frankenphp.pid");
    std::fs::write(&pid_file, pid.to_string())?;

    // 存入 supervisor 并启动监控任务
    let stopping = Arc::new(AtomicBool::new(false));
    let mut guard = SUPERVISOR.lock().await;
    // 若已有旧 supervisor，先清理（替换场景）
    if let Some(old) = guard.take() {
        drop(old);
    }
    *guard = Some(Supervisor {
        child,
        binary: binary.to_path_buf(),
        cfg: cfg.clone(),
        stopping: stopping.clone(),
    });
    drop(guard);

    // 启动崩溃监控任务
    spawn_monitor(stopping, binary.to_path_buf(), cfg.clone(), log_path.clone());

    Ok(ProcessInfo { pid, log_path })
}

/// 崩溃监控任务：等待子进程退出，意外退出时自动重启。
fn spawn_monitor(
    stopping: Arc<AtomicBool>,
    binary: PathBuf,
    cfg: AppConfig,
    log_path: PathBuf,
) {
    tokio::spawn(async move {
        let mut restart_count: u32 = 0;
        let window_start = std::time::Instant::now();

        loop {
            // 等待当前子进程退出
            {
                let mut guard = SUPERVISOR.lock().await;
                if let Some(sup) = guard.as_mut() {
                    let _ = sup.child.wait().await;
                } else {
                    // supervisor 已被 stop 清空，退出监控
                    return;
                }
            }

            // 检查是否为主动停止
            if stopping.load(Ordering::SeqCst) {
                tracing::info!("FrankenPHP 主动停止，不重启");
                return;
            }

            // 意外退出，尝试重启
            let elapsed = window_start.elapsed().as_secs();
            if elapsed >= RESTART_WINDOW_SECS {
                restart_count = 0;
            }
            restart_count += 1;
            if restart_count > RESTART_MAX {
                tracing::error!(
                    "FrankenPHP 在 {RESTART_WINDOW_SECS} 秒内崩溃 {restart_count} 次，放弃自动重启"
                );
                // 清空 supervisor
                let mut guard = SUPERVISOR.lock().await;
                *guard = None;
                let pid_file = cfg.data_dir.join("frankenphp.pid");
                std::fs::remove_file(&pid_file).ok();
                return;
            }

            tracing::warn!(
                "FrankenPHP 意外退出，{RESTART_DELAY_SECS} 秒后自动重启（第 {restart_count}/{RESTART_MAX} 次）"
            );
            tokio::time::sleep(std::time::Duration::from_secs(RESTART_DELAY_SECS)).await;

            // 再次检查是否在此期间被主动停止
            if stopping.load(Ordering::SeqCst) {
                return;
            }

            // 重新启动
            let caddyfile = cfg.caddyfile_path();
            let log_file = match std::fs::File::create(&log_path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!("重启时创建日志文件失败: {e}");
                    return;
                }
            };
            let stderr = log_file.try_clone().ok();
            let mut cmd = Command::new(&binary);
            cmd.arg("run")
                .arg("--config")
                .arg(&caddyfile)
                .stdout(Stdio::from(log_file));
            if let Some(err) = stderr {
                cmd.stderr(Stdio::from(err));
            }
            let child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("自动重启失败: {e}");
                    return;
                }
            };

            let pid = child.id().unwrap_or(0);
            if pid > 0 {
                let pid_file = cfg.data_dir.join("frankenphp.pid");
                std::fs::write(&pid_file, pid.to_string()).ok();
            }

            let mut guard = SUPERVISOR.lock().await;
            *guard = Some(Supervisor {
                child,
                binary: binary.clone(),
                cfg: cfg.clone(),
                stopping: stopping.clone(),
            });
            drop(guard);
        }
    });
}

/// 通过 admin API 停止运行中的 FrankenPHP。
///
/// 先设置 supervisor 的停止标志（避免崩溃监控任务自动重启），
/// 然后尝试 admin API `/stop`，兜底读取 PID 文件 kill。
pub async fn stop(cfg: &AppConfig) -> Result<()> {
    let pid_file = cfg.data_dir.join("frankenphp.pid");

    // 标记为主动停止，阻止监控任务重启
    {
        let guard = SUPERVISOR.lock().await;
        if let Some(sup) = guard.as_ref() {
            sup.stopping.store(true, Ordering::SeqCst);
        }
    }

    // 尝试 admin API 停止
    let client = reqwest::Client::new();
    let resp = client.post("http://127.0.0.1:2019/stop").send().await;
    if let Ok(r) = resp {
        if !r.status().is_success() {
            tracing::warn!("admin API /stop 返回非成功状态: {}", r.status());
        }
    }

    // 读取 PID 并兜底 kill（进程可能未响应 admin API）
    if pid_file.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                kill_process(pid);
            }
        }
        std::fs::remove_file(&pid_file).ok();
    }

    // 清空 supervisor（丢弃子进程句柄）
    let mut guard = SUPERVISOR.lock().await;
    *guard = None;
    Ok(())
}

/// 跨平台终止进程。
#[cfg(unix)]
fn kill_process(pid: u32) {
    // SIGTERM
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// 跨平台终止进程（Windows）。
#[cfg(not(unix))]
fn kill_process(pid: u32) {
    use std::process::Command;
    // taskkill /F /PID <pid> /T 终止进程树
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string(), "/T"])
        .output();
}

/// 通过子进程 `frankenphp reload --config` 热重载配置（不中断连接）。
///
/// 相比直接调用 admin API `/load`，子进程方式会自动完成
/// Caddyfile → JSON 适配，更可靠。
pub async fn reload(cfg: &AppConfig, binary: &std::path::Path) -> Result<()> {
    let caddyfile = cfg.caddyfile_path();
    let output = tokio::process::Command::new(binary)
        .arg("reload")
        .arg("--config")
        .arg(&caddyfile)
        .output()
        .await
        .map_err(|e| Error::Caddy(format!("执行 reload 命令失败: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Caddy(format!(
            "热重载失败: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

/// 读取运行时日志末尾若干行（供仪表盘/日志页展示）。
pub fn read_log(cfg: &AppConfig, tail_lines: usize) -> Result<String> {
    let log_path = cfg.logs_dir().join("frankenphp.log");
    if !log_path.exists() {
        return Ok(String::new());
    }
    let content = std::fs::read_to_string(&log_path)?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(tail_lines);
    Ok(lines[start..].join("\n"))
}

/// 查询运行状态（admin API 可达即视为运行中）。
pub async fn status() -> bool {
    reqwest::Client::new()
        .get("http://127.0.0.1:2019/config/")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site::Site;
    use std::path::PathBuf;

    #[test]
    fn 生成单站点_caddyfile() {
        let mut site = Site::new(
            "测试".into(),
            vec!["a.test".into()],
            PathBuf::from("/var/www/a"),
        );
        site.worker = Some(crate::site::WorkerConfig {
            script: "public/index.php".into(),
            num: 4,
        });
        site.https = true;
        let s = site_block(&site);
        assert!(s.contains("a.test"));
        assert!(s.contains("tls internal"));
        assert!(s.contains("worker public/index.php 4"));
    }

    #[test]
    fn 多站点全量生成() {
        let s1 = Site::new("a".into(), vec!["a.test".into()], PathBuf::from("/a"));
        let s2 = Site::new("b".into(), vec!["b.test".into()], PathBuf::from("/b"));
        let out = generate_caddyfile(&[s1, s2], "127.0.0.1:2019", std::path::Path::new("/tmp/adminer"));
        assert!(out.contains("a.test"));
        assert!(out.contains("b.test"));
        assert!(out.contains("frankenphp"));
        assert!(out.starts_with("# 由 RunPHP 自动生成"));
    }
}
