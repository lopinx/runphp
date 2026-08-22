//! FrankenPHP 运行时管理：下载、校验、多版本并存。
//!
//! 官方 Release 资产命名规则（v1.12.7 实测）：
//! - Linux x86_64 静态二进制: `frankenphp-linux-x86_64`
//! - Linux aarch64 静态二进制: `frankenphp-linux-aarch64`
//! - Windows x86_64 zip（含 PHP 与扩展 dll）: `frankenphp-windows-x86_64.zip`

use crate::{config::AppConfig, Error, Result};
use std::path::{Path, PathBuf};

/// 导入元数据文件名（记录导入运行时的来源路径）。
const IMPORT_META_FILE: &str = "import_meta.json";

/// 已安装的本地运行时。
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledRuntime {
    /// 版本号，如 `1.12.7`。
    pub version: String,
    /// 二进制绝对路径。
    pub path: PathBuf,
    /// 是否当前默认。
    pub is_default: bool,
    /// 导入来源路径（下载安装的运行时为 `None`）。
    pub imported_from: Option<PathBuf>,
}

/// 本地运行时导入结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportResult {
    /// 导入后的二进制路径。
    pub path: PathBuf,
    /// 版本标签。
    pub version: String,
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
                        imported_from: self.read_import_meta(&name),
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
        installed
            .into_iter()
            .next()
            .ok_or_else(|| Error::Runtime("运行时列表为空".into()))
    }

    /// 导入已有的本地 FrankenPHP 二进制到托管目录。
    ///
    /// 版本标签优先通过执行 `<binary> version` 解析；失败时回退为 `local`。
    /// 当该版本号已被占用（已下载或自其它位置导入）时，以来源目录名作后缀
    /// 生成独立版本号，确保每个本地二进制都能单独导入并切换默认。
    pub async fn import(&self, source: &Path) -> Result<ImportResult> {
        if !source.is_file() {
            return Err(Error::Runtime(format!(
                "文件不存在: {}",
                source.display()
            )));
        }
        // 规范化来源路径（剥离 Windows 的 \\?\ 前缀），保证与检测路径可比较
        let source = normalize_source(source);
        let label = detect_version_label(&source)
            .await
            .unwrap_or_else(|| "local".to_string());
        let version = self.find_import_slot(&label, &source);
        let bin_path = self.binary_path(&version);
        // 已从同一来源导入过该版本槽位，直接复用
        if bin_path.exists() {
            return Ok(ImportResult {
                path: bin_path,
                version,
            });
        }
        let dest_dir = self.version_dir(&version);
        std::fs::create_dir_all(&dest_dir)?;
        std::fs::copy(&source, &bin_path)?;
        set_executable(&bin_path)?;
        self.write_import_meta(&version, &source)?;
        tracing::info!("已导入本地运行时 {version}: {}", bin_path.display());
        Ok(ImportResult {
            path: bin_path,
            version,
        })
    }

    /// 为来源二进制寻找导入槽位：返回已从同一来源导入的版本号，
    /// 或生成一个不与现有运行时冲突的新版本号。
    fn find_import_slot(&self, label: &str, source: &Path) -> String {
        // 以来源目录名作后缀区分同版本号的不同二进制
        let suffix = source
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| sanitize_version(&n.to_string_lossy()))
            .filter(|s| !s.is_empty() && s != "unknown")
            .unwrap_or_else(|| "import".to_string());

        let mut candidates = vec![label.to_string(), format!("{label}-{suffix}")];
        for i in 2..10 {
            candidates.push(format!("{label}-{suffix}-{i}"));
        }
        for cand in candidates {
            if !self.binary_path(&cand).exists() {
                return cand;
            }
            // 槽位已被占用：若来自同一来源则复用
            if self
                .read_import_meta(&cand)
                .map(|p| p == source)
                .unwrap_or(false)
            {
                return cand;
            }
        }
        // 极端兜底：附加时间戳
        format!(
            "{label}-{suffix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        )
    }

    /// 导入元数据文件路径。
    fn meta_path(&self, version: &str) -> PathBuf {
        self.version_dir(version).join(IMPORT_META_FILE)
    }

    /// 读取导入来源路径元数据；下载的运行时返回 `None`。
    fn read_import_meta(&self, version: &str) -> Option<PathBuf> {
        let raw = std::fs::read_to_string(self.meta_path(version)).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("imported_from")
            .and_then(|s| s.as_str())
            .map(PathBuf::from)
    }

    /// 写入导入来源路径元数据。
    fn write_import_meta(&self, version: &str, source: &Path) -> Result<()> {
        let raw = serde_json::json!({ "imported_from": source.to_string_lossy() }).to_string();
        std::fs::write(self.meta_path(version), raw)?;
        Ok(())
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

        // 下载安装的运行时没有导入来源，清理该目录可能残留的导入元数据
        std::fs::remove_file(dest_dir.join(IMPORT_META_FILE)).ok();

        tracing::info!("运行时安装完成: {}", bin_path.display());
        Ok(bin_path.to_path_buf())
    }
}

/// GitHub Releases API 地址（用于拉取可安装版本列表）。
const RELEASES_API: &str =
    "https://api.github.com/repos/dunglas/frankenphp/releases?per_page=50";

/// 拉取 GitHub Releases 发布的可安装版本列表（跳过 draft，新版本在前）。
pub async fn available_versions() -> Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("RunPHP/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let resp = client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(Error::Runtime(format!(
            "获取版本列表失败: HTTP {}",
            resp.status()
        )));
    }
    let releases: Vec<serde_json::Value> = resp.json().await?;
    Ok(releases
        .into_iter()
        .filter(|r| !r.get("draft").and_then(|d| d.as_bool()).unwrap_or(false))
        .filter_map(|r| {
            r.get("tag_name")
                .and_then(|t| t.as_str())
                .map(|t| t.trim_start_matches('v').to_string())
        })
        .collect())
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

/// 规范化导入来源路径：canonicalize 后剥离 Windows `\\?\` 前缀，
/// 使其与检测返回的普通路径写法一致，便于前端比对。
fn normalize_source(path: &Path) -> PathBuf {
    let p = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(stripped) => PathBuf::from(stripped),
        None => p,
    }
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

/// 执行 `<binary> version` 并解析出版本标签（如 `1.12.7`），5 秒超时。
///
/// FrankenPHP 的版本输出形如 `frankenphp v1.12.7 ...`，取首个形如 `v?数字.数字` 的片段。
async fn detect_version_label(binary: &Path) -> Option<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(binary)
            .arg("version")
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // 提取首个 `v1.2.3` 或 `1.2.3` 形式的版本号
    let mut cur = String::new();
    let mut versions: Vec<String> = Vec::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            cur.push(ch);
        } else {
            if cur.contains('.') && cur.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                versions.push(cur.trim_matches('.').to_string());
            }
            cur.clear();
        }
    }
    if cur.contains('.') {
        versions.push(cur.trim_matches('.').to_string());
    }
    versions.into_iter().next()
}

/// 清理版本号，防止路径穿越。
///
/// 仅允许字母、数字、点、连字符。若清理后为空（如输入全为特殊字符），
/// 返回 `unknown` 作为兜底，避免版本目录退化为 runtimes 根目录。
fn sanitize_version(version: &str) -> String {
    let filtered: String = version
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '-')
        .collect();
    if filtered.is_empty() {
        "unknown".to_string()
    } else {
        filtered
    }
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

    #[test]
    fn 版本冲突时生成带后缀的导入槽位() {
        let dir = std::env::temp_dir().join("runphp-rt-slot-test");
        std::fs::remove_dir_all(&dir).ok();
        let cfg = AppConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let mgr = RuntimeManager::new(cfg);
        // 模拟已下载的 1.12.7（无导入元数据）
        std::fs::create_dir_all(mgr.version_dir("1.12.7")).unwrap();
        std::fs::write(mgr.binary_path("1.12.7"), b"x").unwrap();
        let slot = mgr.find_import_slot("1.12.7", Path::new("D:/FrankenPHP/frankenphp.exe"));
        assert_eq!(slot, "1.12.7-FrankenPHP");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 同一来源复用已导入槽位() {
        let dir = std::env::temp_dir().join("runphp-rt-reuse-test");
        std::fs::remove_dir_all(&dir).ok();
        let cfg = AppConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let mgr = RuntimeManager::new(cfg);
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let source = src_dir.join("frankenphp.exe");
        std::fs::write(&source, b"x").unwrap();

        // 首次：版本号未被占用，直接取裸版本号
        let slot1 = mgr.find_import_slot("1.12.7", &source);
        assert_eq!(slot1, "1.12.7");
        // 模拟导入完成：落地二进制 + 元数据
        std::fs::create_dir_all(mgr.version_dir(&slot1)).unwrap();
        std::fs::write(mgr.binary_path(&slot1), b"x").unwrap();
        mgr.write_import_meta(&slot1, &source).unwrap();
        // 再次导入同一来源：复用槽位
        assert_eq!(mgr.find_import_slot("1.12.7", &source), slot1);
        // 同版本号的另一来源：生成新槽位
        let other_dir = dir.join("other");
        std::fs::create_dir_all(&other_dir).unwrap();
        let other = other_dir.join("frankenphp.exe");
        std::fs::write(&other, b"x").unwrap();
        assert_eq!(mgr.find_import_slot("1.12.7", &other), "1.12.7-other");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn 导入记录来源元数据() {
        let dir = std::env::temp_dir().join("runphp-rt-import-meta-test");
        std::fs::remove_dir_all(&dir).ok();
        let src_dir = dir.join("portable");
        std::fs::create_dir_all(&src_dir).unwrap();
        #[cfg(windows)]
        let src_bin = src_dir.join("frankenphp.exe");
        #[cfg(not(windows))]
        let src_bin = src_dir.join("frankenphp");
        std::fs::write(&src_bin, b"fake").unwrap();

        let cfg = AppConfig {
            data_dir: dir.join("data"),
            ..Default::default()
        };
        let mgr = RuntimeManager::new(cfg);
        let result = mgr.import(&src_bin).await.unwrap();
        // 伪二进制解析不出版本 → 标签 local，首个空闲槽位即 "local"
        assert_eq!(result.version, "local");
        let listed = mgr.list_installed();
        assert_eq!(listed.len(), 1);
        let meta = listed[0].imported_from.clone().expect("应记录导入来源");
        let meta_s = meta.to_string_lossy().to_lowercase();
        assert!(meta_s.contains("portable"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
