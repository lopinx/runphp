# RunPHP

基于 [FrankenPHP](https://frankenphp.dev) 的 PHP 建站环境管理软件。

- **站点管理**：域名 / HTTPS / Worker 模式 / 实时日志
- **数据库管理**：内置 SQLite，管理已有 MySQL/MariaDB、PostgreSQL
- **Hosts 管理**：受管区块读写、备份、提权
- **多平台**：Windows 桌面、Linux 桌面（Tauri 2），无 UI 的 Linux 服务器（CLI + Web 面板）

## 技术栈

| 层 | 选型 |
|---|---|
| 核心逻辑 | Rust（`crates/runphp-core`） |
| 桌面壳 | Tauri 2 |
| 前端 | Vue 3 + Vite + TypeScript + Naive UI |
| 无头模式 | `crates/runphp-cli`（clap + axum） |

## 开发

```bash
# 安装前端依赖
npm install

# 桌面端开发
npm run tauri dev

# 仅前端开发
npm run dev

# 构建
npm run tauri build
```

## 目录结构

```
├── crates/
│   ├── runphp-core/   # 业务逻辑库（站点/运行时/数据库/hosts）
│   └── runphp-cli/    # 无头模式二进制（CLI + Web 面板）
├── src-tauri/         # Tauri 2 桌面壳
├── src/               # Vue 3 前端（桌面与面板共用）
└── docs/              # 中文文档
```

## 许可证

MIT