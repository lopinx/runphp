//! 文件系统浏览：为 UI 目录选择器提供目录列举能力。

use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// 单个目录条目。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DirEntry {
    /// 目录名。
    pub name: String,
    /// 绝对路径。
    pub path: String,
}

/// 目录浏览结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DirListing {
    /// 当前目录；空串表示根列表（Windows 盘符列表）。
    pub current: String,
    /// 上级目录；空串表示回到根列表，null 表示已在顶层。
    pub parent: Option<String>,
    /// 子目录列表（按名称排序）。
    pub dirs: Vec<DirEntry>,
    /// 当前目录下的文件列表（按名称排序）。
    pub files: Vec<DirEntry>,
}

/// 浏览目录：`None` 或空串返回根列表（Windows 盘符 / Unix 根目录）。
pub fn browse(path: Option<&str>) -> Result<DirListing> {
    match path.map(str::trim).filter(|s| !s.is_empty()) {
        None => roots(),
        Some(p) => list(p),
    }
}

/// 根列表：Windows 枚举存在的盘符，Unix 直接列根目录。
fn roots() -> Result<DirListing> {
    if cfg!(windows) {
        let dirs = (b'A'..=b'Z')
            .map(|c| format!("{}:\\", c as char))
            .filter(|p| Path::new(p).exists())
            .map(|p| DirEntry {
                name: p.clone(),
                path: p,
            })
            .collect();
        Ok(DirListing {
            current: String::new(),
            parent: None,
            dirs,
            files: Vec::new(),
        })
    } else {
        let mut listing = list("/")?;
        listing.parent = None;
        Ok(listing)
    }
}

/// 列出指定目录的子目录和文件。
fn list(p: &str) -> Result<DirListing> {
    let base = PathBuf::from(p);
    if !base.is_dir() {
        return Err(Error::Runtime(format!("路径不是目录: {p}")));
    }
    let mut dirs: Vec<DirEntry> = Vec::new();
    let mut files: Vec<DirEntry> = Vec::new();
    for entry in std::fs::read_dir(&base)?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let entry = DirEntry { name, path: path.to_string_lossy().to_string() };
        if is_dir {
            dirs.push(entry);
        } else {
            files.push(entry);
        }
    }
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    // Windows 盘符根目录的上级为根列表（空串），std 的 parent 返回 None
    let parent = base
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .or_else(|| Some(String::new()));
    Ok(DirListing {
        current: base.to_string_lossy().to_string(),
        parent,
        dirs,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 列举子目录与上级() {
        let dir = std::env::temp_dir().join("runphp-fs-test");
        let sub = dir.join("sub dir");
        std::fs::create_dir_all(&sub).unwrap();
        let listing = browse(Some(dir.to_str().unwrap())).unwrap();
        assert_eq!(listing.dirs.len(), 1);
        assert_eq!(listing.dirs[0].name, "sub dir");
        assert!(listing.parent.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 根列表可用() {
        let listing = browse(None).unwrap();
        assert!(listing.current.is_empty());
        if cfg!(windows) {
            assert!(listing.dirs.iter().any(|d| d.path.starts_with("C:\\")));
        } else {
            assert!(!listing.dirs.is_empty());
        }
    }
}