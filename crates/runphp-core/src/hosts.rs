//! hosts 文件管理：受管区块读写、备份、提权。
//!
//! 以成对标记注释维护受管区块，区块外内容只读不改：
//! ```text
//! # >>> RunPHP 托管开始（勿手动编辑本区块）>>>
//! 127.0.0.1 mysite.test
//! # <<< RunPHP 托管结束 <<<
//! ```

use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// 本模块内统一使用 crate 的 Result 别名。
type Result<T> = crate::Result<T>;

/// 受管区块开始标记。
const MARK_BEGIN: &str = "# >>> RunPHP 托管开始（勿手动编辑本区块）>>>";
/// 受管区块结束标记。
const MARK_END: &str = "# <<< RunPHP 托管结束 <<<";

/// 单条 hosts 映射。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct HostEntry {
    /// IP 地址，如 `127.0.0.1`。
    pub ip: String,
    /// 主机名，如 `mysite.test`。
    pub host: String,
    /// 可选注释。
    pub comment: Option<String>,
}

/// 系统 hosts 文件路径。
pub fn system_hosts_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

/// 解析 hosts 文件内容，提取受管区块内的条目。
///
/// 返回 `(受管区块条目, 区块外原文)`。
pub fn parse(content: &str) -> (Vec<HostEntry>, String) {
    let mut in_block = false;
    let mut managed = Vec::new();
    let mut outside = String::new();

    for line in content.lines() {
        if line.trim() == MARK_BEGIN {
            in_block = true;
            continue;
        }
        if line.trim() == MARK_END {
            in_block = false;
            continue;
        }
        if in_block {
            if let Some(entry) = parse_line(line) {
                managed.push(entry);
            }
        } else {
            outside.push_str(line);
            outside.push('\n');
        }
    }
    (managed, outside)
}

/// 解析单行 hosts 条目。
fn parse_line(line: &str) -> Option<HostEntry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // 格式: IP host [# comment]
    // 使用 split_whitespace 正确处理连续空格和制表符
    let mut parts = line.split_whitespace();
    let ip = parts.next()?;
    let host = parts.next()?;
    // 剩余部分可能包含注释（以 # 开头）或额外字段
    let rest: Vec<&str> = parts.collect();
    let comment_raw = if rest.is_empty() {
        None
    } else {
        // 将剩余部分合并，去掉注释前导 #
        let joined = rest.join(" ");
        let stripped = joined.trim_start_matches('#').trim();
        if stripped.is_empty() { None } else { Some(stripped.to_string()) }
    };
    Some(HostEntry {
        ip: ip.to_string(),
        host: host.to_string(),
        comment: comment_raw,
    })
}

/// 将受管条目和区块外内容合成完整 hosts 文件。
pub fn assemble(managed: &[HostEntry], outside: &str) -> String {
    let mut out = String::new();
    // 保留区块外内容
    let outside = outside.trim_end_matches('\n');
    if !outside.is_empty() {
        out.push_str(outside);
        out.push('\n');
    }

    if !managed.is_empty() {
        out.push('\n');
        out.push_str(MARK_BEGIN);
        out.push('\n');
        for e in managed {
            let line = if let Some(c) = &e.comment {
                format!("{} {} # {}", e.ip, e.host, c)
            } else {
                format!("{} {}", e.ip, e.host)
            };
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str(MARK_END);
        out.push('\n');
    }
    out
}

/// hosts 管理器：负责读写系统 hosts 文件。
pub struct HostsManager {
    path: PathBuf,
}

impl HostsManager {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// 使用系统默认路径。
    pub fn system() -> Self {
        Self::new(system_hosts_path())
    }

    /// 读取当前 hosts 文件内容。
    pub fn read(&self) -> Result<String> {
        if !self.path.exists() {
            return Ok(String::new());
        }
        std::fs::read_to_string(&self.path).map_err(Error::Io)
    }

    /// 列出受管区块内的全部条目。
    pub fn list_managed(&self) -> Result<Vec<HostEntry>> {
        let content = self.read()?;
        Ok(parse(&content).0)
    }

    /// 同步受管条目：用新列表替换整个受管区块。
    ///
    /// 写入前自动备份到 `hosts.runphp.bak.<时间戳>`。
    /// 若无写入权限，返回需要提权的错误。
    pub fn sync(&self, entries: &[HostEntry]) -> Result<()> {
        let content = self.read()?;
        let (_, outside) = parse(&content);

        // 备份
        self.backup(&content)?;

        // 合成新内容
        let new_content = assemble(entries, &outside);

        // 写入
        std::fs::write(&self.path, new_content).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::Config("无写入 hosts 权限，需要管理员/提权。请使用提权命令或手动写入。".to_string())
            } else {
                Error::Io(e)
            }
        })?;
        Ok(())
    }

    /// 添加单条（去重）。
    pub fn add(&self, entry: HostEntry) -> Result<()> {
        let mut entries = self.list_managed()?;
        if !entries.contains(&entry) {
            entries.push(entry);
        }
        self.sync(&entries)
    }

    /// 删除匹配主机的条目。
    pub fn remove_by_host(&self, host: &str) -> Result<()> {
        let mut entries = self.list_managed()?;
        entries.retain(|e| e.host != host);
        self.sync(&entries)
    }

    /// 备份当前 hosts 文件。
    fn backup(&self, content: &str) -> Result<()> {
        if content.is_empty() {
            return Ok(());
        }
        let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let bak_path = self.path.with_file_name(format!("hosts.runphp.bak.{ts}"));
        std::fs::write(&bak_path, content).map_err(Error::Io)?;
        tracing::info!("hosts 已备份到 {}", bak_path.display());
        Ok(())
    }

    /// 检测是否有写入权限。
    pub fn check_writable(&self) -> bool {
        let test = self.path.with_extension("runphp_write_test");
        match std::fs::write(&test, b"test") {
            Ok(_) => {
                std::fs::remove_file(&test).ok();
                true
            }
            Err(_) => false,
        }
    }

    /// 生成提权命令（供用户复制执行）。
    ///
    /// Windows 返回 PowerShell Start-Process -Verb RunAs 命令；
    /// Linux 返回 sudo 命令。
    pub fn elevation_command(&self, action: &str) -> String {
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "runphp".to_string());
        if cfg!(windows) {
            format!(
                "Start-Process -Verb RunAs -FilePath '{exe}' -ArgumentList 'hosts','{action}'"
            )
        } else {
            format!("sudo {exe} hosts {action}")
        }
    }
}

/// 根据站点列表生成 hosts 条目（全部指向 127.0.0.1）。
pub fn entries_from_sites(sites: &[crate::site::Site]) -> Vec<HostEntry> {
    let mut entries = Vec::new();
    for s in sites {
        for d in &s.domains {
            entries.push(HostEntry {
                ip: "127.0.0.1".to_string(),
                host: d.clone(),
                comment: Some(format!("RunPHP:{}", s.name)),
            });
        }
    }
    // 去重：使用 sort + dedup 确保全局去重（dedup 仅去相邻重复）
    entries.sort_by(|a, b| a.host.cmp(&b.host));
    entries.dedup_by(|a, b| a.host == b.host);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析与合成往返() {
        let content = format!(
            "127.0.0.1 localhost\n\
             10.0.0.1 other.test # 其他\n\
             \n\
             {MARK_BEGIN}\n\
             127.0.0.1 a.test\n\
             127.0.0.1 b.test # 测试\n\
             {MARK_END}\n"
        );
        let (managed, outside) = parse(&content);
        assert_eq!(managed.len(), 2);
        assert_eq!(managed[0].host, "a.test");
        assert_eq!(managed[1].comment.as_deref(), Some("测试"));
        assert!(outside.contains("localhost"));
        assert!(outside.contains("other.test"));

        let reassembled = assemble(&managed, &outside);
        let (m2, _) = parse(&reassembled);
        assert_eq!(m2, managed);
    }

    #[test]
    fn 无受管区块时解析为空() {
        let content = "127.0.0.1 localhost\n";
        let (managed, outside) = parse(content);
        assert!(managed.is_empty());
        assert!(outside.contains("localhost"));
    }

    #[test]
    fn 从站点生成条目() {
        use crate::site::Site;
        use std::path::PathBuf;
        let s = Site::new(
            "测试".into(),
            vec!["a.test".into(), "b.test".into()],
            PathBuf::from("/tmp"),
        );
        let entries = entries_from_sites(&[s]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ip, "127.0.0.1");
    }

    #[test]
    fn 连续空格hosts行正确解析() {
        // 之前 splitn(3, char::is_whitespace) 对连续空格会产生空字符串段，
        // 导致 host 为空、整行被跳过。改用 split_whitespace 后修复。
        let entry = parse_line("127.0.0.1   multiple.test   # 注释");
        assert!(entry.is_some(), "连续空格行不应被跳过");
        let e = entry.unwrap();
        assert_eq!(e.ip, "127.0.0.1");
        assert_eq!(e.host, "multiple.test");
        assert_eq!(e.comment.as_deref(), Some("注释"));

        // 制表符分隔也应正常工作
        let entry2 = parse_line("10.0.0.1\ttab.test");
        assert!(entry2.is_some());
        assert_eq!(entry2.unwrap().host, "tab.test");
    }
}
