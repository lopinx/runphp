//! RunPHP 命令行入口。
//!
//! M1 阶段仅提供 `version` 占位；M6 阶段补充完整的站点/运行时/面板子命令。

use clap::Parser;

/// RunPHP —— 基于 FrankenPHP 的 PHP 建站环境管理工具
#[derive(Parser)]
#[command(version, about, long_about = None)]
enum Cli {
    /// 显示版本信息
    Version,
}

fn main() {
    let cli = Cli::parse();
    match cli {
        Cli::Version => {
            println!("RunPHP {}", env!("CARGO_PKG_VERSION"));
        }
    }
}
