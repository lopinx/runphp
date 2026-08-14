//! Tauri 2 桌面壳：薄封装 runphp-core，不含业务逻辑。

/// 示例命令：返回问候语（M1 骨架验证用，后续由真实命令替换）。
#[tauri::command]
fn greet(name: &str) -> String {
    format!("你好，{name}！欢迎使用 RunPHP。")
}

/// 返回当前数据目录路径。
#[tauri::command]
fn data_dir() -> String {
    runphp_core::default_data_dir().to_string_lossy().to_string()
}

/// 库入口：由桌面二进制与移动端共用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet, data_dir])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用时出错");
}
