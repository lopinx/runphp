//! 桌面端可执行文件入口。

// Windows 发布版隐藏控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    runphp_desktop_lib::run()
}
