# AGENTS.md — RunPHP 工作区指令

> 面向未来 ZCode Agent 的简明工作区指南。修改本仓库前请先阅读。

## 项目简介

RunPHP 是基于 [FrankenPHP](https://frankenphp.dev) 的 PHP 建站环境管理软件。桌面端用 Tauri 2，无头 Linux 服务器用 CLI + axum Web 面板。功能包括：站点管理（域名/HTTPS/Worker/热重载）、数据库管理（内置 SQLite + 远程 MySQL/PG 连接）、hosts 管理。

## 目录结构

```
crates/runphp-core/   核心业务库（无 UI 依赖，三端共用）
  src/caddy.rs        Caddyfile 生成 + FrankenPHP 子进程启停 + 热重载 + 日志读取
  src/config.rs       配置与站点注册表持久化（JSON）
  src/runtime.rs      FrankenPHP 下载/多版本（GitHub Releases）
  src/site.rs         站点模型 + CRUD + 域名校验
  src/hosts.rs        hosts 受管区块读写 + 备份 + 提权
  src/db/sqlite.rs    rusqlite bundled 引擎 + 表结构浏览 + SQL 执行器
  src/db/remote.rs    mysql_async + tokio-postgres 连接管理 + 表浏览 + SQL 执行；MongoDB/Redis/Qdrant 仅连接测试
crates/runphp-cli/    无头二进制（clap CLI + axum 面板）
  src/main.rs         子命令分发（runtime/site/hosts/run/stop/reload/status/logs/panel/service-install）
  src/panel.rs        axum Web 面板（rust_embed 嵌入 dist/，Bearer token 鉴权）
src-tauri/            Tauri 2 桌面壳（薄封装 core，不含业务逻辑）
src/                  Vue 3 前端（桌面与面板共用同一构建产物）
  api/index.ts        适配层：运行时检测 window.__TAURI_INTERNALS__ 自动切换 invoke/fetch
  stores/app.ts       Pinia 状态管理
  views/              五个页面：仪表盘/站点/数据库/Hosts/设置
docs/                 中文文档（用户手册.md、架构设计.md）
```

## 构建与测试命令

```bash
# 前端
npm install              # 安装依赖（本机无 pnpm，只用 npm）
npm run dev              # Vite dev server（端口 1420）
npm run build            # vue-tsc 类型检查 + vite build → dist/
npx vue-tsc --noEmit     # 仅类型检查

# Rust
cargo build --workspace           # 全量编译
cargo test -p runphp-core         # 核心库单元测试（11 个）
cargo build -p runphp-cli         # 编译 CLI（target/debug/runphp[.exe]）
cargo check -p runphp-desktop     # 检查桌面端

# 桌面端开发
npm run tauri dev        # 启动 Tauri 窗口 + Vite 热更新

# CLI 运行
./target/debug/runphp runtime list
./target/debug/runphp site add <名称> --domain <域名> --root <目录>
./target/debug/runphp run          # 启动 FrankenPHP
./target/debug/runphp panel --port 9080  # Web 面板
```

## 架构边界与编辑规则

1. **业务逻辑只在 `runphp-core`**：桌面端 `src-tauri/lib.rs` 和 CLI `main.rs`/`panel.rs` 只做薄封装，不要在壳层写业务逻辑。
2. **前端一份代码两处运行**：`src/api/index.ts` 适配层按 `VITE_RUNPHP_MODE` 切换 invoke/fetch。新增 API 时在适配层加类型定义和函数，桌面端和面板端同时可用。
3. **Tauri command 与 REST API 同名同参**：新增功能时在 `src-tauri/lib.rs` 加 `#[tauri::command]`，同时在 `panel.rs` 加对应 axum 路由，保持两端一致。
4. **模块内 `Result` 别名**：`site.rs` 和 `hosts.rs` 内使用 `type Result<T> = crate::Result<T>;` 避免与 std::Result 冲突。
5. **Naive UI 全量注册**：`main.ts` 必须用 `import naive from "naive-ui"` 默认导出，不要用 `create()`（会导致 CSS 不注入、UI 显示纯文本）。

## 编码约定

- **全程中文**：注释、文档、commit 信息均中文。commit 使用中文前缀：`功能:`/`修复:`/`重构:`
- Rust 代码：模块级 `//!` 文档注释，函数级 `///`，中文描述
- 前端：Vue 3 `<script setup lang="ts">`，Naive UI 组件
- 路径处理：用 `PathBuf`，不拼字符串

## 环境踩坑（Windows / Git Bash）

- **无 pnpm**：本机只有 npm（v11），不要使用 pnpm 命令
- **esbuild 脚本**：npm 11 默认拦截，需 `npm approve-scripts esbuild && npm rebuild esbuild`
- **Windows GNU 目标**：`src-tauri/Cargo.toml` 的 `[lib]` 只用 `crate-type = ["rlib"]`，不要加 `cdylib`/`staticlib`（会触发 "export ordinal too large" 链接错误）
- **icon.ico 必须**：tauri-build 在 Windows 要求 `src-tauri/icons/icon.ico`，PNG 不够
- **端口 1420 残留**：`tauri dev` 异常退出后 vite 进程可能残留，需 `taskkill` 清理
- **tracing-subscriber**：用 `with_env_filter`（snake_case），不是 `with_envFilter`
- **rust_embed 路径**：`#[folder = "../../dist/"]` 相对于 crate 目录，不是 workspace 根
- **mysql_async**：用 `features = ["minimal-rust"]`，不要用 `rustls`（编译错误）

## 敏感区域

修改以下文件前请先阅读对应文档：
- `crates/runphp-core/src/caddy.rs` — Caddyfile 模板格式影响 FrankenPHP 配置解析
- `crates/runphp-core/src/runtime.rs` — 资产命名规则影响下载 URL 拼接
- `src-tauri/tauri.conf.json` — Tauri 构建配置
- `docs/架构设计.md` — 完整架构设计文档
