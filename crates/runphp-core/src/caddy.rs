//! Caddyfile 生成与 FrankenPHP 进程生命周期管理。
//!
//! FrankenPHP 本质是内置 PHP 的 Caddy，配置走 Caddyfile，支持 admin API
//! （默认 127.0.0.1:2019）热重载与 stop。

use crate::{config::AppConfig, site::Site, Error, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

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
        lines.push(format!(
            "    php_server {{\n        worker {} {}\n    }}",
            w.script, w.num
        ));
    } else {
        lines.push("    php_server".into());
    }

    lines.push("}".into());
    lines.join("\n") + "\n"
}

/// 对路径加引号（处理含空格的情况）。
fn quote_path(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    if s.contains(' ') {
        format!("{s:?}")
    } else {
        s.to_string()
    }
}

/// 根据全部站点生成完整 Caddyfile 文本。
pub fn generate_caddyfile(sites: &[Site], admin_addr: &str) -> String {
    let mut out = String::new();
    out.push_str("# 由 RunPHP 自动生成，请勿手动编辑\n");
    out.push_str(&global_block(admin_addr));
    out.push('\n');
    for s in sites {
        out.push_str(&site_block(s));
        out.push('\n');
    }
    out
}

/// 将 Caddyfile 写入配置指定路径。
pub fn write_caddyfile(cfg: &AppConfig, sites: &[Site]) -> Result<()> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let content = generate_caddyfile(sites, "127.0.0.1:2019");
    std::fs::write(cfg.caddyfile_path(), content)?;
    Ok(())
}

/// 进程句柄信息。
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub log_path: PathBuf,
}

/// 启动 FrankenPHP 子进程。
///
/// 返回子进程句柄；调用方必须 `await` 它以保持运行（否则因 `kill_on_drop`
/// 退出时子进程会被终止）。日志写入 `logs_dir()/frankenphp.log`。
pub async fn start(cfg: &AppConfig, binary: &std::path::Path) -> Result<(ProcessInfo, tokio::process::Child)> {
    let caddyfile = cfg.caddyfile_path();
    if !caddyfile.exists() {
        return Err(Error::Caddy(
            "Caddyfile 尚未生成，请先创建站点".into(),
        ));
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
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Caddy(format!("启动 FrankenPHP 失败: {e}")))?;

    let pid = child
        .id()
        .ok_or_else(|| Error::Caddy("无法获取子进程 PID".into()))?;

    // 记录 PID 供后续停止
    let pid_file = cfg.data_dir.join("frankenphp.pid");
    std::fs::write(&pid_file, pid.to_string())?;

    Ok((ProcessInfo { pid, log_path }, child))
}

/// 通过 admin API 停止运行中的 FrankenPHP。
pub async fn stop(cfg: &AppConfig) -> Result<()> {
    let url = "http://127.0.0.1:2019/stop";
    let client = reqwest::Client::new();
    let resp = client.post(url).send().await;
    // 忽略连接失败（可能进程已退出）
    if let Ok(r) = resp {
        if !r.status().is_success() {
            tracing::warn!("admin API /stop 返回非成功状态: {}", r.status());
        }
    }
    // 清理 PID 文件
    let pid_file = cfg.data_dir.join("frankenphp.pid");
    if pid_file.exists() {
        std::fs::remove_file(&pid_file).ok();
    }
    Ok(())
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
        let out = generate_caddyfile(&[s1, s2], "127.0.0.1:2019");
        assert!(out.contains("a.test"));
        assert!(out.contains("b.test"));
        assert!(out.contains("frankenphp"));
        assert!(out.starts_with("# 由 RunPHP 自动生成"));
    }
}
