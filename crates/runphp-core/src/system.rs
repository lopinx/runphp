//! 主机系统信息：为仪表盘底部状态栏提供 CPU 架构 / 内存 / 硬盘 / 系统版本。

use serde::Serialize;
use sysinfo::{Disks, System};

/// 主机系统概要（容量字段单位为字节，由前端格式化展示）。
#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    /// CPU 架构，如 `x86_64` / `aarch64`。
    pub cpu_arch: String,
    /// 物理内存总量（字节）。
    pub memory_total: u64,
    /// 全部磁盘总容量（字节）。
    pub disk_total: u64,
    /// 全部磁盘可用容量（字节）。
    pub disk_free: u64,
    /// 系统名称与版本描述，如 `Windows 11 企业版 LTSC (10.0.26100)`。
    pub os: String,
}

/// 收集主机系统信息。
pub fn collect() -> SystemInfo {
    let mut sys = System::new();
    sys.refresh_memory();
    let (disk_total, disk_free) = Disks::new_with_refreshed_list()
        .iter()
        .fold((0u64, 0u64), |(t, f), d| {
            (t + d.total_space(), f + d.available_space())
        });
    SystemInfo {
        cpu_arch: std::env::consts::ARCH.to_string(),
        memory_total: sys.total_memory(),
        disk_total,
        disk_free,
        os: os_description(),
    }
}

/// 拼接系统名称与版本号；长名称已含版本号时不再重复。
fn os_description() -> String {
    let name = System::long_os_version()
        .or_else(System::name)
        .unwrap_or_else(|| "未知系统".to_string());
    match System::os_version().or_else(System::kernel_version) {
        Some(v) if !name.contains(&v) => format!("{name} ({v})"),
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 收集信息有效() {
        let info = collect();
        assert!(!info.cpu_arch.is_empty());
        assert!(!info.os.is_empty());
        assert!(info.memory_total > 0);
        assert!(info.disk_total >= info.disk_free);
    }
}
