//! RunPHP 命令行入口。
//!
//! 提供运行时管理、站点管理、hosts 管理、启动停止等子命令，
//! 以及内建 Web 管理面板，供无头 Linux 服务器使用。

mod panel;

use clap::{Parser, Subcommand};
use runphp_core::{
    caddy, ftp::FtpProfile, AppConfig, RuntimeManager, Site,
};
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
    /// FTP 管理（FTP/SFTP/FTPS）
    Ftp {
        #[command(subcommand)]
        action: FtpCmd,
    },
    /// 启动 FrankenPHP（前台运行）
    Run,
    /// 停止运行中的 FrankenPHP
    Stop,
    /// 热重载配置（不中断连接）
    Reload,
    /// 查询运行状态
    Status,
    /// 显示运行时日志末尾
    Logs {
        /// 显示行数
        #[arg(long, default_value = "100")]
        lines: usize,
    },
    /// 启动 Web 管理面板
    Panel {
        /// 监听端口
        #[arg(long, default_value = "9080")]
        port: u16,
        /// 监听地址（默认 127.0.0.1，0.0.0.0 允许外部）
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// 鉴权 token（不设则不鉴权）
        #[arg(long)]
        token: Option<String>,
    },
    /// 安装 systemd 服务（Linux）
    ServiceInstall,
}

#[derive(Subcommand)]
enum RuntimeCmd {
    /// 列出已安装运行时
    List,
    /// 下载并安装指定版本
    Install { version: String },
    /// 设置默认运行时版本
    Default { version: String },
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

#[derive(Subcommand)]
enum FtpCmd {
    /// 列出全部 FTP 连接档案
    List,
    /// 新增 FTP 连接档案
    Add {
        /// 显示名称
        name: String,
        /// 协议（ftp/sftp/ftps）
        #[arg(long, default_value = "ftp")]
        protocol: String,
        /// 主机
        #[arg(long)]
        host: String,
        /// 端口（默认按协议选择）
        #[arg(long)]
        port: Option<u16>,
        /// 用户名
        #[arg(long, default_value = "anonymous")]
        username: String,
        /// 密码
        #[arg(long, default_value = "")]
        password: String,
        /// SSH 私钥路径（仅 SFTP）
        #[arg(long)]
        key: Option<String>,
    },
    /// 删除 FTP 连接档案
    Rm {
        id: String,
    },
    /// 测试 FTP 连接
    Test {
        id: String,
    },
    /// 列出远程目录内容
    Ls {
        /// 档案 id
        id: String,
        /// 远程路径（默认 "/"）
        #[arg(long, default_value = "/")]
        path: String,
    },
    /// 在远程创建目录
    Mkdir {
        id: String,
        /// 远程目录路径
        #[arg(long)]
        path: String,
    },
    /// 上传本地文件到远程
    Upload {
        id: String,
        /// 本地文件路径
        #[arg(long)]
        local: PathBuf,
        /// 远程目标路径
        #[arg(long)]
        remote: String,
    },
    /// 下载远程文件到本地
    Download {
        id: String,
        /// 远程文件路径
        #[arg(long)]
        remote: String,
        /// 本地保存路径
        #[arg(long)]
        local: PathBuf,
    },
    /// 递归上传本地目录到远程
    UploadDir {
        id: String,
        /// 本地目录路径
        #[arg(long)]
        local: PathBuf,
        /// 远程目标目录
        #[arg(long)]
        remote: String,
    },
    /// 删除远程文件或目录
    Rmfile {
        id: String,
        /// 远程路径
        #[arg(long)]
        path: String,
        /// 目标为目录
        #[arg(long)]
        dir: bool,
    },
    /// 重命名远程文件或目录
    Rename {
        id: String,
        /// 原路径
        #[arg(long)]
        from: String,
        /// 新路径
        #[arg(long)]
        to: String,
    },
}

/// 按档案 id 查找 profile，不存在则报错退出。
fn profile_by_id(mgr: &runphp_core::ftp::FtpManager, id: &str) -> FtpProfile {
    match mgr.list_profiles() {
        Ok(list) => list.into_iter().find(|p| p.id == id).unwrap_or_else(|| {
            eprintln!("档案 {id} 不存在");
            std::process::exit(1);
        }),
        Err(e) => {
            eprintln!("读取档案失败: {e}");
            std::process::exit(1);
        }
    }
}

/// 字节数转人类可读字符串（如 1.2 MB）。
fn fmt_size(n: u64) -> String {
    if n == 0 {
        return "—".into();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, UNITS[0])
    } else if v >= 100.0 {
        format!("{:.0} {}", v, UNITS[i])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

/// 终端进度显示：覆盖当前行显示文件名、已传/总量、百分比。
fn print_progress(transferred: u64, total: u64, file: &str) {
    use std::io::Write;
    let pct = if total > 0 {
        (transferred * 100 / total).min(100)
    } else {
        0
    };
    let line = if total > 0 {
        format!("\r{file}  {pct}%  {}/{}", fmt_size(transferred), fmt_size(total))
    } else {
        format!("\r{file}  {}", fmt_size(transferred))
    };
    print!("{line}");
    std::io::stdout().flush().ok();
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
            RuntimeCmd::Default { version } => {
                let mgr = RuntimeManager::new(cfg.clone());
                if !mgr.list_installed().iter().any(|r| r.version == version) {
                    eprintln!("运行时 {version} 未安装");
                    std::process::exit(1);
                }
                let mut new_cfg = cfg.clone();
                new_cfg.default_runtime_version = version.clone();
                match new_cfg.save() {
                    Ok(()) => println!("已将默认运行时设置为 {version}。"),
                    Err(e) => {
                        eprintln!("保存失败: {e}");
                        std::process::exit(1);
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
                        if let Err(e) = cfg.save_sites(&reg) {
                            eprintln!("保存失败: {e}");
                            std::process::exit(1);
                        }
                        if let Err(e) = caddy::write_caddyfile(&cfg, &reg.sites) {
                            eprintln!("生成 Caddyfile 失败: {e}");
                            std::process::exit(1);
                        }
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
        Command::Ftp { action } => {
            use runphp_core::ftp::{FtpManager, FtpProtocol};
            let mgr = FtpManager::new(&cfg.data_dir);
            match action {
                FtpCmd::List => {
                    let profiles = mgr.list_profiles().unwrap_or_default();
                    if profiles.is_empty() {
                        println!("暂无 FTP 连接档案。使用 `runphp ftp add` 创建。");
                    } else {
                        for p in &profiles {
                            println!("[{}] {} ({:?}) {}:{}", p.id, p.name, p.protocol, p.host, p.port);
                        }
                    }
                }
                FtpCmd::Add {
                    name,
                    protocol,
                    host,
                    port,
                    username,
                    password,
                    key,
                } => {
                    let proto = match protocol.to_lowercase().as_str() {
                        "sftp" => FtpProtocol::Sftp,
                        "ftps" => FtpProtocol::Ftps,
                        _ => FtpProtocol::Ftp,
                    };
                    let p = FtpProfile {
                        username: username.clone(),
                        password: password.clone(),
                        ssh_key: key.clone(),
                        ssh_password: None,
                        ..FtpProfile::new(name, proto, host, port.unwrap_or(0))
                    };
                    match mgr.add_profile(p) {
                        Ok(()) => println!("FTP 连接档案已保存。"),
                        Err(e) => {
                            eprintln!("保存失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                FtpCmd::Rm { id } => match mgr.remove_profile(&id) {
                    Ok(()) => println!("已删除 FTP 连接档案: {id}"),
                    Err(e) => {
                        eprintln!("删除失败: {e}");
                        std::process::exit(1);
                    }
                },
                FtpCmd::Test { id } => {
                    let profiles = mgr.list_profiles().unwrap_or_default();
                    let p = match profiles.iter().find(|p| p.id == id) {
                        Some(p) => p.clone(),
                        None => {
                            eprintln!("档案 {id} 不存在");
                            std::process::exit(1);
                        }
                    };
                    println!("测试连接 {:?} {}:{}…", p.protocol, p.host, p.port);
                    match FtpManager::test_connection(&p).await {
                        Ok(msg) => println!("{msg}"),
                        Err(e) => {
                            eprintln!("连接失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                FtpCmd::Ls { id, path } => {
                    let p = profile_by_id(&mgr, &id);
                    match FtpManager::list_dir(&p, &path).await {
                        Ok(entries) => {
                            if entries.is_empty() {
                                println!("（空目录）");
                            }
                            for e in &entries {
                                let kind = if e.is_dir { "目录" } else { "文件" };
                                println!("{kind}\t{}\t{}", e.name, fmt_size(e.size));
                            }
                            println!("共 {} 项", entries.len());
                        }
                        Err(e) => {
                            eprintln!("列目录失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                FtpCmd::Mkdir { id, path } => {
                    let p = profile_by_id(&mgr, &id);
                    match FtpManager::make_dir(&p, &path).await {
                        Ok(()) => println!("已创建目录: {path}"),
                        Err(e) => {
                            eprintln!("创建目录失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                FtpCmd::Upload { id, local, remote } => {
                    let p = profile_by_id(&mgr, &id);
                    println!("上传 {local:?} → {remote} …");
                    match FtpManager::upload(
                        &p,
                        &local.to_string_lossy(),
                        &remote,
                        Some(&|d, t, f| print_progress(d, t, f)),
                    )
                    .await
                    {
                        Ok(()) => { println!(); println!("上传完成"); }
                        Err(e) => {
                            eprintln!("\n上传失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                FtpCmd::Download { id, remote, local } => {
                    let p = profile_by_id(&mgr, &id);
                    println!("下载 {remote} → {local:?} …");
                    match FtpManager::download(
                        &p,
                        &remote,
                        &local.to_string_lossy(),
                        Some(&|d, t, f| print_progress(d, t, f)),
                    )
                    .await
                    {
                        Ok(()) => { println!(); println!("下载完成"); }
                        Err(e) => {
                            eprintln!("\n下载失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                FtpCmd::UploadDir { id, local, remote } => {
                    let p = profile_by_id(&mgr, &id);
                    println!("递归上传 {local:?} → {remote} …");
                    match FtpManager::upload_dir(
                        &p,
                        &local.to_string_lossy(),
                        &remote,
                        Some(&|d, t, f| print_progress(d, t, f)),
                    )
                    .await
                    {
                        Ok(()) => { println!(); println!("目录上传完成"); }
                        Err(e) => {
                            eprintln!("\n上传失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                FtpCmd::Rmfile { id, path, dir } => {
                    let p = profile_by_id(&mgr, &id);
                    match FtpManager::delete(&p, &path, dir).await {
                        Ok(()) => println!("已删除: {path}"),
                        Err(e) => {
                            eprintln!("删除失败: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                FtpCmd::Rename { id, from, to } => {
                    let p = profile_by_id(&mgr, &id);
                    match FtpManager::rename(&p, &from, &to).await {
                        Ok(()) => println!("已重命名: {from} → {to}"),
                        Err(e) => {
                            eprintln!("重命名失败: {e}");
                            std::process::exit(1);
                        }
                    }
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
                Ok(info) => {
                    println!("已启动，PID={}，日志: {}", info.pid, info.log_path.display());
                    // CLI 前台运行：等待 Ctrl+C 后停止
                    // 进程崩溃自动重启由 caddy 模块的监控任务负责
                    tokio::signal::ctrl_c()
                        .await
                        .ok();
                    println!("\n收到 Ctrl+C，停止服务…");
                    let _ = caddy::stop(&cfg).await;
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
        Command::Logs { lines } => match caddy::read_log(&cfg, lines) {
            Ok(t) if t.is_empty() => println!("日志为空或尚未启动。"),
            Ok(t) => println!("{t}"),
            Err(e) => {
                eprintln!("读取日志失败: {e}");
                std::process::exit(1);
            }
        },
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
        Command::Panel { port, host, token } => {
            println!("启动 RunPHP Web 管理面板…");
            if let Err(e) = panel::serve(cfg, port, &host, token).await {
                eprintln!("面板启动失败: {e}");
                std::process::exit(1);
            }
        }
        Command::ServiceInstall => {
            install_systemd_service(&cfg).await;
        }
    }
}

/// 生成并安装 systemd 服务单元（仅 Linux）。
async fn install_systemd_service(cfg: &AppConfig) {
    if !cfg!(target_os = "linux") {
        eprintln!("systemd 服务安装仅支持 Linux。");
        std::process::exit(1);
    }

    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "runphp".to_string());

    let unit = format!(
        r#"[Unit]
Description=RunPHP PHP 建站环境管理服务
After=network.target

[Service]
Type=simple
ExecStart={exe} run
WorkingDirectory={data_dir}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
"#,
        data_dir = cfg.data_dir.display()
    );

    let unit_path = "/etc/systemd/system/runphp.service";
    let tmp_path = "/tmp/runphp.service";

    if let Err(e) = std::fs::write(tmp_path, &unit) {
        eprintln!("写入临时文件失败: {e}");
        std::process::exit(1);
    }

    println!("已生成 systemd 单元文件: {tmp_path}");
    println!("请以 root 权限执行以下命令完成安装:\n");
    println!("  sudo cp {tmp_path} {unit_path}");
    println!("  sudo systemctl daemon-reload");
    println!("  sudo systemctl enable runphp");
    println!("  sudo systemctl start runphp");
    println!("\n查看日志: sudo journalctl -u runphp -f");
}
