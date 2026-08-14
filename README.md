<div align="center">

# 🐘 RunPHP

### 基于 FrankenPHP 的 PHP 建站环境管理软件

桌面端 · 服务器端 · 一键下载运行时 · 站点/数据库/Hosts 全管理

[![Rust](https://img.shields.io/badge/Rust-1.97-orange.svg)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/Tauri-2.0-blue.svg)](https://tauri.app/)
[![Vue](https://img.shields.io/badge/Vue-3.5-brightgreen.svg)](https://vuejs.org/)
[![License](https://img.shields.io/badge/license-WTFPL-e40000.svg)](./LICENSE)

</div>

---

## ✨ 功能特性

| 功能 | 说明 |
|---|---|
| 🚀 **运行时管理** | 首次启动自动从 GitHub Releases 下载 FrankenPHP 二进制，支持多版本并存 |
| 🌐 **站点管理** | 域名绑定、本地 HTTPS（Caddy 内置 CA 自动签发）、Worker 模式（Laravel/Symfony 常驻进程） |
| 🔥 **热重载** | 站点变更后 `frankenphp reload` 热加载，不中断连接 |
| 🗄️ **数据库管理** | 内置 SQLite 引擎（建库/查询/SQL 编辑器）+ 连接管理已有 MySQL/MariaDB、PostgreSQL |
| 📋 **Hosts 管理** | 受管区块读写、自动备份、无权限时生成提权命令 |
| 🖥️ **多平台** | Windows 桌面、Linux 桌面（Tauri 2），无 UI 的 Linux 服务器（CLI + Web 面板） |
| 🇨🇳 **全程中文** | 界面、注释、文档、提交信息均中文 |

## 🛠️ 技术栈

| 层 | 技术 | 说明 |
|---|---|---|
| 核心逻辑 | **Rust** | `crates/runphp-core`，三端共用，无 UI 依赖 |
| 桌面壳 | **Tauri 2** | WebView2 (Windows) / webkit2gtk (Linux) |
| 前端 | **Vue 3 + Vite + TS + Naive UI** | 一份构建产物，桌面与面板复用 |
| 无头模式 | **clap + axum** | CLI 子命令 + 内建 Web 面板（rust_embed 嵌入前端） |
| SQLite | **rusqlite** (bundled) | 零外部依赖 |
| MySQL/PG | **mysql_async + tokio-postgres** | 仅连接管理已有实例 |

## 📦 快速开始

### 桌面端开发

```bash
# 安装依赖
npm install

# 启动桌面端（Tauri 窗口 + Vite 热更新）
npm run tauri dev

# 构建发布包
npm run tauri build
```

### 无头服务器模式

```bash
# 编译 CLI
cargo build -p runphp-cli --release

# 安装 FrankenPHP 运行时
./target/release/runphp runtime install 1.12.7

# 创建站点
./target/release/runphp site add mysite \
  --domain mysite.test \
  --root /var/www/mysite \
  --https

# 同步 hosts
./target/release/runphp hosts sync

# 启动服务
./target/release/runphp run

# 启动 Web 管理面板
./target/release/runphp panel --port 9080

# 安装为 systemd 服务
./target/release/runphp service install
```

## 📋 CLI 命令一览

| 命令 | 说明 |
|---|---|
| `runphp version` | 显示版本 |
| `runphp runtime list` | 列出已安装运行时 |
| `runphp runtime install <版本>` | 下载安装指定版本 |
| `runphp site list` | 列出全部站点 |
| `runphp site add <名称> --domain <域名> --root <目录> [--https]` | 新建站点 |
| `runphp site rm <id>` | 删除站点 |
| `runphp run` | 启动 FrankenPHP（前台） |
| `runphp stop` | 停止服务 |
| `runphp reload` | 热重载配置 |
| `runphp status` | 查询运行状态 |
| `runphp hosts list` | 列出受管 hosts 条目 |
| `runphp hosts sync` | 同步站点域名到 hosts |
| `runphp hosts elevation` | 显示提权命令 |
| `runphp panel --port <端口>` | 启动 Web 管理面板 |
| `runphp service install` | 生成 systemd 服务单元 |

## 📁 目录结构

```
runphp/
├── crates/
│   ├── runphp-core/          # 核心业务库（站点/运行时/数据库/hosts）
│   │   └── src/
│   │       ├── caddy.rs      # Caddyfile 生成 + 进程启停 + 热重载
│   │       ├── config.rs     # 配置持久化
│   │       ├── runtime.rs    # FrankenPHP 下载/多版本
│   │       ├── site.rs       # 站点模型与 CRUD
│   │       ├── hosts.rs      # hosts 受管区块读写
│   │       └── db/           # 数据库管理（SQLite + MySQL/PG）
│   └── runphp-cli/           # 无头二进制（CLI + Web 面板）
│       └── src/
│           ├── main.rs       # clap 子命令分发
│           └── panel.rs      # axum Web 面板
├── src-tauri/                # Tauri 2 桌面壳
├── src/                      # Vue 3 前端（桌面与面板共用）
│   ├── views/                # 五个页面
│   ├── api/                  # 适配层（invoke / fetch 双模式）
│   └── stores/               # Pinia 状态管理
└── docs/                     # 中文文档
    ├── 用户手册.md
    └── 架构设计.md
```

## 📂 数据目录

| 平台 | 路径 |
|---|---|
| Windows | `%APPDATA%\RunPHP` |
| Linux | `~/.local/share/runphp` |

包含：运行时分版本目录、站点根目录、日志、Caddyfile、配置 JSON、SQLite 数据库。

## 📖 文档

- [用户手册](docs/用户手册.md) — 安装、使用、常见问题
- [架构设计](docs/架构设计.md) — 模块职责、设计决策、数据流
- [AGENTS.md](AGENTS.md) — Agent 工作区指令

## 📜 许可证

```
        DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
                   Version 2, December 2004

 Copyright (C) 2026 RunPHP Contributors

 Everyone is permitted to copy and distribute verbatim or modified
 copies of this license document, and changing it is allowed as long
 as the name is changed.

           DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE
  TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION

  0. You just DO WHAT THE FUCK YOU WANT TO.
```

详细文本见 [LICENSE](./LICENSE) 文件。
