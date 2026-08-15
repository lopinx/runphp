//! 应用配置与状态持久化（数据目录下 JSON 文件）。

use crate::{site::SiteRegistry, Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 配置文件在数据目录下的相对位置。
const CONFIG_FILE: &str = "config.json";

/// 站点注册表文件名。
const SITES_FILE: &str = "sites.json";

/// 应用全局配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 数据目录（运行时、站点、日志、元数据的根）。
    pub data_dir: PathBuf,
    /// FrankenPHP 下载镜像基址（默认官方 GitHub Releases）。
    pub runtime_mirror: String,
    /// 默认运行时版本（站点未指定时使用）。
    pub default_runtime_version: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            data_dir: crate::default_data_dir(),
            runtime_mirror: "https://github.com/dunglas/frankenphp/releases/download".to_string(),
            default_runtime_version: String::new(),
        }
    }
}

impl AppConfig {
    /// 加载配置；不存在时返回默认值（不落盘）。
    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(CONFIG_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        let mut cfg: AppConfig = serde_json::from_str(&raw)
            .map_err(|e| Error::Config(format!("配置文件解析失败: {e}")))?;
        // 数据目录以实际位置为准（防止配置被复制到别处）
        cfg.data_dir = data_dir.to_path_buf();
        Ok(cfg)
    }

    /// 保存配置到数据目录（原子写入：先写临时文件再 rename）。
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        let path = self.data_dir.join(CONFIG_FILE);
        let raw = serde_json::to_string_pretty(self)?;
        atomic_write(&path, &raw)?;
        Ok(())
    }

    /// 运行时二进制存放目录：`数据目录/runtimes`。
    pub fn runtimes_dir(&self) -> PathBuf {
        self.data_dir.join("runtimes")
    }

    /// 站点根目录默认位置：`数据目录/sites`。
    pub fn sites_dir(&self) -> PathBuf {
        self.data_dir.join("sites")
    }

    /// 日志目录：`数据目录/logs`。
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// 生成的 Caddyfile 路径。
    pub fn caddyfile_path(&self) -> PathBuf {
        self.data_dir.join("Caddyfile")
    }

    /// 站点注册表文件路径。
    pub fn sites_file_path(&self) -> PathBuf {
        self.data_dir.join(SITES_FILE)
    }

    /// 加载站点注册表；不存在时返回空。
    pub fn load_sites(&self) -> Result<SiteRegistry> {
        let path = self.sites_file_path();
        if !path.exists() {
            return Ok(SiteRegistry::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        let reg: SiteRegistry = serde_json::from_str(&raw)
            .map_err(|e| Error::Config(format!("站点注册表解析失败: {e}")))?;
        Ok(reg)
    }

    /// 保存站点注册表（原子写入：先写临时文件再 rename）。
    pub fn save_sites(&self, reg: &SiteRegistry) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        let raw = serde_json::to_string_pretty(reg)?;
        atomic_write(&self.sites_file_path(), &raw)?;
        Ok(())
    }
}

/// 原子写入：先写入临时文件，再 rename 覆盖目标文件。
/// 防止写入过程中崩溃导致文件损坏。
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
mod tests {
    use super::*;

    #[test]
    fn 配置保存与加载往返() {
        let dir = std::env::temp_dir().join("runphp-config-test");
        let mut cfg = AppConfig::default();
        cfg.data_dir = dir.clone();
        cfg.save().unwrap();
        let loaded = AppConfig::load(&dir).unwrap();
        assert_eq!(loaded.data_dir, dir);
        std::fs::remove_dir_all(&dir).ok();
    }
}
