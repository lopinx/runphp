//! FrankenPHP 运行时管理：下载、校验、多版本并存。
//!
//! 官方 Release 资产命名规则（v1.12.7 实测）：
//! - Linux x86_64 静态二进制: `frankenphp-linux-x86_64`
//! - Linux aarch64 静态二进制: `frankenphp-linux-aarch64`
//! - Windows x86_64 zip（含 PHP 与扩展 dll）: `frankenphp-windows-x86_64.zip`

use crate::{config::AppConfig, Error, Result};
use std::path::{Path, PathBuf};

/// 已安装的本地运行时。
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledRuntime {
    /// 版本号，如 `1.12.7`。
    pub version: String,
    /// 二进制绝对路径。
    pub path: PathBuf,
    /// 是否当前默认。
    pub is_default: bool,
}

/// 运行时管理器。
pub struct RuntimeManager {
    cfg: AppConfig,
}

impl RuntimeManager {
    pub fn new(cfg: AppConfig) -> Self {
        Self { cfg }
    }

    /// 对应当前平台的 GitHub Release 资产文件名。
    pub fn asset_name(&self) -> &'static str {
        if cfg!(target_os = "windows") {
            "frankenphp-windows-x86_64.zip"
        } else if cfg!(target_arch = "aarch64") {
            "frankenphp-linux-aarch64"
        } else {
            "frankenphp-linux-x86_64"
        }
    }

    /// 构造下载 URL（`{mirror}/{tag}/{asset}`）。
    pub fn download_url(&self, version: &str) -> String {
        format!("{}/v{}/{}", self.cfg.runtime_mirror, version, self.asset_name())
    }

    /// 运行时版本目录：`数据目录/runtimes/<version>`。
    ///
    /// 版本号仅允许字母、数字、点、连字符，防止路径穿越。
    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.cfg.runtimes_dir().join(sanitize_version(version))
    }

    /// 该版本二进制路径（Windows 带 .exe）。
    pub fn binary_path(&self, version: &str) -> PathBuf {
        let dir = self.version_dir(version);
        if cfg!(target_os = "windows") {
            dir.join("frankenphp.exe")
        } else {
            dir.join("frankenphp")
        }
    }

    /// 列出已安装的运行时版本。
    pub fn list_installed(&self) -> Vec<InstalledRuntime> {
        let dir = self.cfg.runtimes_dir();
        let mut result = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let bin = self.binary_path(&name);
                if bin.exists() {
                    result.push(InstalledRuntime {
                        version: name.clone(),
                        path: bin,
                        is_default: name == self.cfg.default_runtime_version,
                    });
                }
            }
        }
        result.sort_by(|a, b| b.version.cmp(&a.version));
        result
    }

    /// 查找指定版本；未指定则取默认或最新。
    pub fn resolve(&self, version: Option<&str>) -> Result<InstalledRuntime> {
        let installed = self.list_installed();
        if installed.is_empty() {
            return Err(Error::Runtime("尚未安装任何运行时".into()));
        }
        if let Some(v) = version {
            if !v.is_empty() {
                return installed
                    .into_iter()
                    .find(|r| r.version == v)
                    .ok_or_else(|| Error::Runtime(format!("运行时 {v} 未安装")));
            }
        }
        // 优先默认
        if let Some(d) = installed.iter().find(|r| r.is_default) {
            return Ok(d.clone());
        }
        Ok(installed.into_iter().next().unwrap())
    }

    /// 下载并安装指定版本。
    ///
    /// `on_progress` 回调接收 `(已下载字节数, 总字节数)`，用于 UI 进度条。
    pub async fn install(
        &self,
        version: &str,
        on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<PathBuf> {
        let url = self.download_url(version);
        let dest_dir = self.version_dir(version);
        std::fs::create_dir_all(&dest_dir)?;

        // 下载到临时文件
        let tmp = dest_dir.join("download.tmp");
        let bin_path = self.binary_path(version);

        // 执行下载与解压，失败时清理临时文件
        let result = self.do_download_and_extract(&url, &tmp, &bin_path, &dest_dir, on_progress).await;
        if result.is_err() {
            // 清理下载失败残留
            std::fs::remove_file(&tmp).ok();
        }
        result
    }

    /// 内部：下载、解压、设置可执行位的实际逻辑。
    async fn do_download_and_extract(
        &self,
        url: &str,
        tmp: &Path,
        bin_path: &Path,
        dest_dir: &Path,
        on_progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<PathBuf> {
        tracing::info!("开始下载运行时: {url}");
        let client = reqwest::Client::builder()
            .user_agent(concat!("RunPHP/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let resp = client.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(Error::Runtime(format!(
                "下载失败: HTTP {}",
                resp.status()
            )));
        }
        let total = resp.content_length().unwrap_or(0);

        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut file = std::fs::File::create(tmp)?;
        let mut downloaded: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            std::io::Write::write_all(&mut file, &chunk)?;
            downloaded += chunk.len() as u64;
            if let Some(cb) = &on_progress {
                cb(downloaded, total);
            }
        }
        drop(file);

        // Windows: 解压 zip；Linux: 直接落地并赋予可执行位
        if cfg!(target_os = "windows") {
            tracing::info!("解压 Windows zip 到 {}", dest_dir.display());
            extract_zip(tmp, dest_dir)?;
            std::fs::remove_file(tmp).ok();
        } else {
            // 临时文件即二进制，重命名
            std::fs::rename(tmp, bin_path)?;
            set_executable(bin_path)?;
        }

        if !bin_path.exists() {
            return Err(Error::Runtime(
                "下载完成但未找到二进制文件，请检查资产结构".into(),
            ));
        }

        tracing::info!("运行时安装完成: {}", bin_path.display());
        Ok(bin_path.to_path_buf())
    }
}

/// 解压 zip 到目标目录。
fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let outpath = match entry.enclosed_name() {
            Some(p) => dest.join(p),
            None => continue,
        };
        if entry.is_dir() {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut entry, &mut outfile)?;
        }
    }
    Ok(())
}

/// 设置文件可执行位（Linux/macOS）。
fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// 清理版本号，防止路径穿越。
/// 仅允许字母、数字、点、连字符。
fn sanitize_version(version: &str) -> String {
    version
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '-')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 资产名与路径() {
        let cfg = AppConfig::default();
        let mgr = RuntimeManager::new(cfg);
        let name = mgr.asset_name();
        assert!(!name.is_empty());
        let url = mgr.download_url("1.12.7");
        assert!(url.contains("1.12.7"));
        assert!(url.ends_with(name));
    }

    #[test]
    fn 未安装时报错() {
        let cfg = AppConfig {
            data_dir: std::env::temp_dir().join("runphp-rt-test-nonexist"),
            ..Default::default()
        };
        let mgr = RuntimeManager::new(cfg);
        assert!(mgr.resolve(None).is_err());
    }
}
