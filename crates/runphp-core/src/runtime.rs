//! FrankenPHP 运行时管理：下载（落到数据目录）、导入（仅注册引用，保留原路径）。
//!
//! 官方 Release 资产命名规则（v1.12.7 实测）：
//! - Linux x86_64 静态二进制: `frankenphp-linux-x86_64`
//! - Linux aarch64 静态二进制: `frankenphp-linux-aarch_64`
//! - Windows x86_64 zip（含 PHP 与扩展 dll）: `frankenphp-windows-x86_64.zip`
//!
//! 运行时来源两类：
//! - 下载安装（`downloaded`）：二进制复制到 `runtimes/<version>/frankenphp.exe`
//! - 导入本地（`imported`）：仅在 `runtimes/<version>/import_meta.json` 注册
//!   来源路径，二进制保持原位不动
//!
//! 列表聚合两类运行时，`path` 字段对用户透明——下载项指向托管副本，导入项指向原路径。

use crate::{config::AppConfig, Error, Result};
use std::path::{Path, PathBuf};

/// 导入元数据文件名（记录导入运行时的来源路径与版本号）。
const IMPORT_META_FILE: &str = "import_meta.json";

/// 已安装的本地运行时。
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledRuntime {
    /// 版本号，如 `1.12.7`。
    pub version: String,
    /// 二进制绝对路径（下载项为托管副本路径，导入项为原始来源路径）。
    pub path: PathBuf,
    /// 是否当前默认。
    pub is_default: bool,
    /// 导入来源路径（下载安装的运行时为 `None`）。
    pub imported_from: Option<PathBuf>,
}

/// 本地运行时导入结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportResult {
    /// 导入注册后的二进制路径（实际为原始来源路径，未复制）。
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
            "frankenphp-linux-aarch_64"
        } else {
            "frankenphp-linux-x86_64"
        }
    }

    /// 构造下载 URL（`{mirror}/{tag}/{asset}`）。
    pub fn download_url(&self, version: &str) -> String {
        format!("{}/v{}/{}", self.cfg.runtime_mirror, version, self.asset_name())
    }

    /// 运行时条目目录：`数据目录/runtimes/<version>`。
    ///
    /// 下载项存放二进制本身；导入项仅放元数据文件，二进制仍在原位置。
    /// 版本号仅允许字母、数字、点、连字符，防止路径穿越。
    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.cfg.runtimes_dir().join(sanitize_version(version))
    }

    /// 下载项的二进制路径（Windows 带 .exe）。
    pub fn binary_path(&self, version: &str) -> PathBuf {
        let dir = self.version_dir(version);
        if cfg!(target_os = "windows") {
            dir.join("frankenphp.exe")
        } else {
            dir.join("frankenphp")
        }
    }

    /// 列出全部运行时：聚合下载项与导入项。
    ///
    /// 下载项：`<version>/frankenphp[.exe]` 存在即视为已安装。
    /// 导入项：`<version>/import_meta.json` 存在即视为已注册（且来源文件必须仍存在）。
    pub fn list_installed(&self) -> Vec<InstalledRuntime> {
        let dir = self.cfg.runtimes_dir();
        let mut result = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return result,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let entry_dir = entry.path();
            // 跳过元数据文件本身（runtimes 目录根级散落的 import_meta.json）
            if !entry_dir.is_dir() {
                continue;
            }
            let is_default = name == self.cfg.default_runtime_version;
            // 优先识别导入项：有元数据且来源文件存在 → 注册成功
            if let Some(source) = self.read_import_meta(&name) {
                if source.is_file() {
                    result.push(InstalledRuntime {
                        version: name.clone(),
                        path: source.clone(),
                        is_default,
                        imported_from: Some(source),
                    });
                    continue;
                }
            }
            // 否则按下载项处理：二进制必须在
            let bin = self.binary_path(&name);
            if bin.exists() {
                result.push(InstalledRuntime {
                    version: name.clone(),
                    path: bin,
                    is_default,
                    imported_from: None,
                });
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

    /// 注册本地已有的 FrankenPHP 二进制（仅写入元数据，不复制文件）。
    ///
    /// 版本标签优先通过执行 `<binary> version` 解析；失败时回退为 `local`。
    /// 当该版本号已被占用（已下载或自其它位置导入）时，以来源目录名作后缀
    /// 生成独立版本号；同一来源重复注册会复用已有槽位。
    pub async fn import(&self, source: &Path) -> Result<ImportResult> {
        if !source.is_file() {
            return Err(Error::Runtime(format!(
                "文件不存在: {}",
                source.display()
            )));
        }
        let source = normalize_source(source);
        let label = detect_version_label(&source)
            .await
            .unwrap_or_else(|| "local".to_string());
        let version = self.find_import_slot(&label, &source);
        // 仅写入元数据；运行时条目目录可能不存在（如首次注册该槽位）
        let entry_dir = self.version_dir(&version);
        std::fs::create_dir_all(&entry_dir)?;
        self.write_import_meta(&version, &source)?;
        tracing::info!(
            "已注册本地运行时 {version}（复用原路径）: {}",
            source.display()
        );
        Ok(ImportResult {
            path: source,
            version,
        })
    }

    /// 为来源二进制寻找可用的运行时槽位。
    ///
    /// 优先级：
    /// 1. 已从同一来源注册过的槽位（可复用）
    /// 2. 空槽位（裸版本号）
    /// 3. 以来源目录名作后缀生成的槽位（避免与已有下载项冲突）
    fn find_import_slot(&self, label: &str, source: &Path) -> String {
        // 优先复用：先看裸版本号是否已被同一来源占用
        for cand in [label.to_string()] {
            if self.read_import_meta(&cand).as_ref() == Some(&source.to_path_buf()) {
                return cand;
            }
        }
        // 其次：以来源目录名作后缀生成新槽位；冲突则追加序号
        let suffix = source
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| sanitize_version(&n.to_string_lossy()))
            .filter(|s| !s.is_empty() && s != "unknown")
            .unwrap_or_else(|| "import".to_string());

        for i in 0..100 {
            let cand = if i == 0 {
                format!("{label}-{suffix}")
            } else {
                format!("{label}-{suffix}-{i}")
            };
            let entry_dir = self.version_dir(&cand);
            // 槽位空：可注册
            if !entry_dir.exists() {
                return cand;
            }
            // 槽位已被同一来源占用：复用
            if self.read_import_meta(&cand).as_ref() == Some(&source.to_path_buf()) {
                return cand;
            }
        }
        // 极端兜底
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

    /// 读取导入元数据；同时校验来源二进制是否仍存在；
    /// 来源丢失时清理元数据避免列表显示无效项。
    fn read_import_meta(&self, version: &str) -> Option<PathBuf> {
        let raw = std::fs::read_to_string(self.meta_path(version)).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let s = v.get("imported_from").and_then(|s| s.as_str())?;
        let path = PathBuf::from(s);
        if path.is_file() {
            Some(path)
        } else {
            // 来源已失效：清理元数据，避免下一次又读到死路径
            std::fs::remove_file(self.meta_path(version)).ok();
            None
        }
    }

    /// 写入导入元数据（含版本号，便于诊断；当前解析版本仍以 `binary version` 为准）。
    fn write_import_meta(&self, version: &str, source: &Path) -> Result<()> {
        let meta_path = self.meta_path(version);
        if let Some(parent) = meta_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::json!({
            "version": version,
            "imported_from": source.to_string_lossy(),
        })
        .to_string();
        std::fs::write(meta_path, raw)?;
        Ok(())
    }

    /// 下载并安装指定版本（落到数据目录）。
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
        // 模拟已下载的 1.12.7（无导入元数据）：裸槽位被下载项占据
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
        let src_dir = dir.join("portable-foo");
        std::fs::create_dir_all(&src_dir).unwrap();
        let source = src_dir.join("frankenphp.exe");
        std::fs::write(&source, b"x").unwrap();

        // 首次：裸槽位未被同源占用 → 直接生成带后缀槽位
        let slot1 = mgr.find_import_slot("1.12.7", &source);
        assert_eq!(slot1, "1.12.7-portable-foo");
        // 模拟已写入元数据
        mgr.write_import_meta(&slot1, &source).unwrap();
        // 再次导入同一来源：复用
        assert_eq!(mgr.find_import_slot("1.12.7", &source), slot1);
        // 同版本号的另一来源：槽位已被同源占用 → 生成新槽位
        let other_dir = dir.join("portable-bar");
        std::fs::create_dir_all(&other_dir).unwrap();
        let other = other_dir.join("frankenphp.exe");
        std::fs::write(&other, b"x").unwrap();
        assert_eq!(
            mgr.find_import_slot("1.12.7", &other),
            "1.12.7-portable-bar"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn 导入不复制二进制仅写元数据() {
        let dir = std::env::temp_dir().join("runphp-rt-no-copy-test");
        std::fs::remove_dir_all(&dir).ok();
        let src_dir = dir.join("portable");
        std::fs::create_dir_all(&src_dir).unwrap();
        #[cfg(windows)]
        let src_bin = src_dir.join("frankenphp.exe");
        #[cfg(not(windows))]
        let src_bin = src_dir.join("frankenphp");
        let original = b"original-bytes";
        std::fs::write(&src_bin, original).unwrap();

        let cfg = AppConfig {
            data_dir: dir.join("data"),
            ..Default::default()
        };
        let mgr = RuntimeManager::new(cfg);
        let result = mgr.import(&src_bin).await.unwrap();
        // path 必须是原始来源路径，未发生复制
        assert_eq!(result.path, normalize_source(&src_bin));
        // 源文件未被修改
        assert_eq!(std::fs::read(&src_bin).unwrap(), original);
        // 托管目录只有元数据，无二进制副本
        let meta_file = mgr.meta_path(&result.version);
        assert!(meta_file.is_file());
        let entry_dir = mgr.version_dir(&result.version);
        assert!(entry_dir.join("import_meta.json").is_file());
        assert!(!mgr.binary_path(&result.version).exists());

        // 列表正确暴露导入项
        let listed = mgr.list_installed();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, normalize_source(&src_bin));
        assert_eq!(
            listed[0].imported_from.as_ref().unwrap(),
            &normalize_source(&src_bin)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 来源丢失时清理元数据() {
        let dir = std::env::temp_dir().join("runphp-rt-stale-test");
        std::fs::remove_dir_all(&dir).ok();
        let cfg = AppConfig {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let mgr = RuntimeManager::new(cfg);
        // 写一份指向不存在路径的元数据
        mgr.write_import_meta("1.12.7", Path::new("Z:/nope/frankenphp.exe"))
            .unwrap();
        assert!(mgr.meta_path("1.12.7").is_file());
        // 读取时校验失败并自动清理
        assert!(mgr.read_import_meta("1.12.7").is_none());
        assert!(!mgr.meta_path("1.12.7").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
