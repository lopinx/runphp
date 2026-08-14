//! 站点模型与 CRUD。

use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// 本模块内统一使用 crate 的 Result 别名。
type Result<T> = crate::Result<T>;

/// 站点 Worker 模式配置（用于 Laravel/Symfony 等常驻进程框架）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// 入口脚本相对路径，如 `public/index.php`。
    pub script: String,
    /// Worker 进程数。
    pub num: u32,
}

/// 单个站点配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Site {
    /// 唯一标识（uuid）。
    pub id: String,
    /// 站点名称（显示用）。
    pub name: String,
    /// 域名列表，如 `["mysite.test"]`。
    pub domains: Vec<String>,
    /// 监听端口（0 表示由 Caddy 自动分配 / 使用 80/443）。
    pub port: u16,
    /// 网站根目录绝对路径。
    pub root: PathBuf,
    /// 是否启用本地 HTTPS（Caddy `tls internal`）。
    pub https: bool,
    /// Worker 模式配置（None 则用普通 php_server）。
    pub worker: Option<WorkerConfig>,
    /// PHP ini 覆盖指令，每行一条如 `memory_limit = 256M`。
    pub php_ini: Vec<String>,
    /// 创建时间（RFC3339）。
    pub created_at: String,
    /// 更新时间（RFC3339）。
    pub updated_at: String,
}

impl Site {
    /// 创建新站点（生成 id 与时间戳）。
    pub fn new(name: String, domains: Vec<String>, root: PathBuf) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            domains,
            port: 0,
            root,
            https: false,
            worker: None,
            php_ini: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// 主域名（第一个），用于显示。
    pub fn primary_domain(&self) -> &str {
        self.domains.first().map(|s| s.as_str()).unwrap_or("")
    }

    /// 触摸更新时间。
    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
}

/// 站点集合管理（CRUD）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SiteRegistry {
    pub sites: Vec<Site>,
}

impl SiteRegistry {
    pub fn add(&mut self, mut site: Site) {
        site.touch();
        self.sites.push(site);
    }

    pub fn get(&self, id: &str) -> Option<&Site> {
        self.sites.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Site> {
        self.sites.iter_mut().find(|s| s.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<Site> {
        let pos = self.sites.iter().position(|s| s.id == id)?;
        Some(self.sites.remove(pos))
    }

    /// 校验：域名不重复、根目录有效。
    pub fn validate(&self, site: &Site, exclude_id: Option<&str>) -> Result<()> {
        if site.name.trim().is_empty() {
            return Err(Error::Config("站点名称不能为空".into()));
        }
        if site.domains.is_empty() {
            return Err(Error::Config("至少需要一个域名".into()));
        }
        // 域名唯一性
        for d in &site.domains {
            let dup = self.sites.iter().any(|s| {
                s.id != exclude_id.unwrap_or("") && s.domains.iter().any(|x| x == d)
            });
            if dup {
                return Err(Error::Config(format!("域名 {d} 已被其他站点占用")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 站点增删与域名校验() {
        let mut reg = SiteRegistry::default();
        let s1 = Site::new("测试站".into(), vec!["a.test".into()], PathBuf::from("/tmp/a"));
        reg.add(s1.clone());
        assert_eq!(reg.sites.len(), 1);

        // 重复域名应报错
        let s2 = Site::new("另一个".into(), vec!["a.test".into()], PathBuf::from("/tmp/b"));
        assert!(reg.validate(&s2, None).is_err());

        // 排除自身时可通过
        assert!(reg.validate(&s1, Some(&s1.id)).is_ok());

        // 删除
        let removed = reg.remove(&s1.id);
        assert!(removed.is_some());
        assert!(reg.sites.is_empty());
    }
}
