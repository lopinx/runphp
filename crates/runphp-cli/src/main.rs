//! RunPHP 命令行入口。
//!
//! 提供运行时管理、站点管理、启动停止等子命令，供无头 Linux 模式使用。
//! M2 阶段实现 runtime / site / run 三类命令。

use clap::{Parser, Subcommand};
use runphp_core::{caddy, AppConfig, RuntimeManager, Site};
use std::path::PathBuf;

/// RunPHP —— 基于 FrankenPHP 的 PHP 建站环境管理工具
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// 覆盖数据目录位置
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 显示版本信息
    Version,
    /// 运行时管理
    Runtime {
        #[command(subcommand)]
        action: RuntimeCmd,
    },
    /// 站点管理
    Site {
        #[command(subcommand)]
        action: SiteCmd,
    },
    /// Hosts 管理
    Hosts {
        #[command(subcommand)]
        action: HostsCmd,
    },
    /// 启动 FrankenPHP（前台运行）
    Run,
    /// 停止运行中的 FrankenPHP
    Stop,
    /// 热重载配置（不中断连接）
    Reload,
    /// 查询运行状态
    Status,
}

#[derive(Subcommand)]
enum RuntimeCmd {
    /// 列出已安装运行时
    List,
    /// 下载并安装指定版本
    Install { version: String },
}

#[derive(Subcommand)]
enum SiteCmd {
    /// 列出全部站点
    List,
    /// 新增站点
    Add {
        /// 站点名称
        name: String,
        /// 域名（可多个）
        #[arg(long, num_args = 1.., required = true)]
        domain: Vec<String>,
        /// 网站根目录
        #[arg(long)]
        root: PathBuf,
        /// 启用 HTTPS
        #[arg(long)]
        https: bool,
    },
    /// 删除站点
    Rm { id: String },
}

#[derive(Subcommand)]
enum HostsCmd {
    /// 列出受管区块内的 hosts 条目
    List,
    /// 同步全部站点域名到 hosts
    Sync,
    /// 显示提权命令（无写入权限时使用）
    Elevation,
}

fn load_cfg(data_dir: Option<PathBuf>) -> AppConfig {
    let dir = data_dir.unwrap_or_else(runphp_core::default_data_dir);
    AppConfig::load(&dir).unwrap_or_else(|e| {
        eprintln!("加载配置失败: {e}");
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "runphp=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = load_cfg(cli.data_dir.clone());

    match cli.command {
        Command::Version => {
            println!("RunPHP {}", env!("CARGO_PKG_VERSION"));
        }
        Command::Runtime { action } => match action {
            RuntimeCmd::List => {
                let mgr = RuntimeManager::new(cfg.clone());
                let installed = mgr.list_installed();
                if installed.is_empty() {
                    println!("尚未安装任何运行时。使用 `runphp runtime install <版本>` 安装。");
                } else {
                    for rt in installed {
                        let mark = if rt.is_default { "*" } else { " " };
                        println!("{} {}: {}", mark, rt.version, rt.path.display());
                    }
                }
            }
            RuntimeCmd::Install { version } => {
                let mgr = RuntimeManager::new(cfg.clone());
                println!("开始安装 FrankenPHP v{version}…");
                match mgr.install(&version, None).await {
                    Ok(p) => {
                        println!("安装成功: {}", p.display());
                        // 首次安装自动设为默认
                        if cfg.default_runtime_version.is_empty() {
                            let mut new_cfg = cfg.clone();
                            new_cfg.default_runtime_version = version.clone();
                            if let Err(e) = new_cfg.save() {
                                eprintln!("更新默认运行时配置失败: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("安装失败: {e}");
                        std::process::exit(1);
                    }
                }
            }
        },
        Command::Site { action } => match action {
            SiteCmd::List => {
                let reg = cfg.load_sites().unwrap_or_default();
                if reg.sites.is_empty() {
                    println!("暂无站点。使用 `runphp site add` 创建。");
                } else {
                    for s in &reg.sites {
                        println!("[{}] {} ({})", s.id, s.name, s.domains.join(", "));
                    }
                }
            }
            SiteCmd::Add {
                name,
                domain,
                root,
                https,
            } => {
                let mut reg = cfg.load_sites().unwrap_or_default();
                if let Err(e) = reg.validate(
                    &Site::new(name.clone(), domain.clone(), root.clone()),
                    None,
                ) {
                    eprintln!("校验失败: {e}");
                    std::process::exit(1);
                }
                let mut site = Site::new(name, domain, root);
                site.https = https;
                reg.add(site);
                if let Err(e) = cfg.save_sites(&reg) {
                    eprintln!("保存失败: {e}");
                    std::process::exit(1);
                }
                if let Err(e) = caddy::write_caddyfile(&cfg, &reg.sites) {
                    eprintln!("生成 Caddyfile 失败: {e}");
                    std::process::exit(1);
                }
                println!("站点创建成功，Caddyfile 已更新。");
            }
            SiteCmd::Rm { id } => {
                let mut reg = cfg.load_sites().unwrap_or_default();
                match reg.remove(&id) {
                    Some(s) => {
                        println!("已删除站点: {}", s.name);
                        let _ = cfg.save_sites(&reg);
                        let _ = caddy::write_caddyfile(&cfg, &reg.sites);
                    }
                    None => {
                        eprintln!("站点 {id} 不存在");
                        std::process::exit(1);
                    }
                }
            }
        },
        Command::Hosts { action } => {
            use runphp_core::hosts::{entries_from_sites, HostsManager};
            let hm = HostsManager::system();
            match action {
                HostsCmd::List => {
                    match hm.list_managed() {
                        Ok(entries) => {
                            if entries.is_empty() {
                                println!("受管区块为空。");
                            } else {
                                for e in &entries {
                                    let c = e.comment.as_deref().unwrap_or("");
                                    println!("{} {} {}", e.ip, e.host, c);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("读取 hosts 失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                HostsCmd::Sync => {
                    let reg = cfg.load_sites().unwrap_or_default();
                    let entries = entries_from_sites(&reg.sites);
                    if entries.is_empty() {
                        println!("无站点域名需要同步。");
                        return;
                    }
                    if !hm.check_writable() {
                        eprintln!("无写入 hosts 权限。请使用提权命令：");
                        println!("{}", hm.elevation_command("sync"));
                        std::process::exit(1);
                    }
                    match hm.sync(&entries) {
                        Ok(()) => println!("已同步 {} 条 hosts 记录。", entries.len()),
                        Err(e) => {
                            eprintln!("同步失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                HostsCmd::Elevation => {
                    println!("提权命令（复制到管理员终端执行）：");
                    println!("{}", hm.elevation_command("sync"));
                }
            }
        }
        Command::Run => {
            let mgr = RuntimeManager::new(cfg.clone());
            let rt = match mgr.resolve(None) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{e}");
                    eprintln!("请先 `runphp runtime install <版本>` 安装运行时。");
                    std::process::exit(1);
                }
            };
            println!("启动 FrankenPHP v{}…", rt.version);
            match caddy::start(&cfg, &rt.path).await {
                Ok((info, mut child)) => {
                    println!("已启动，PID={}，日志: {}", info.pid, info.log_path.display());
                    // 等待子进程退出或收到 Ctrl+C
                    tokio::select! {
                        status = child.wait() => {
                            if let Ok(s) = status {
                                println!("FrankenPHP 退出，状态: {s}");
                            }
                        }
                        _ = tokio::signal::ctrl_c() => {
                            println!("\n收到 Ctrl+C，停止服务…");
                            let _ = caddy::stop(&cfg).await;
                            // 兜底 kill
                            let _ = child.kill().await;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("启动失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        Command::Stop => {
            if let Err(e) = caddy::stop(&cfg).await {
                eprintln!("停止失败: {e}");
                std::process::exit(1);
            }
            println!("已发送停止请求。");
        }
        Command::Status => {
            if caddy::status().await {
                println!("运行中");
            } else {
                println!("未运行");
            }
        }
        Command::Reload => {
            // 先重新生成 Caddyfile，再热重载
            let reg = cfg.load_sites().unwrap_or_default();
            if let Err(e) = caddy::write_caddyfile(&cfg, &reg.sites) {
                eprintln!("生成 Caddyfile 失败: {e}");
                std::process::exit(1);
            }
            let mgr = RuntimeManager::new(cfg.clone());
            let rt = match mgr.resolve(None) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            };
            match caddy::reload(&cfg, &rt.path).await {
                Ok(()) => println!("热重载成功。"),
                Err(e) => {
                    eprintln!("热重载失败: {e}");
                    eprintln!("请确认 FrankenPHP 正在运行（runphp status）。");
                    std::process::exit(1);
                }
            }
        }
    }
}
