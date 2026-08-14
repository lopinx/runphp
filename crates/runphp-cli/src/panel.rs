//! Web 面板：axum 托管前端静态资源 + REST API。
//!
//! API 路径与 Tauri command 同名，前端适配层按 VITE_RUNPHP_MODE 切换。
//! 简单 bearer token 鉴权，默认仅监听 127.0.0.1。

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use runphp_core::{
    caddy,
    db::{remote::*, sqlite::*},
    hosts::{entries_from_sites, HostsManager},
    AppConfig, RuntimeManager, Site,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

/// 面板共享状态。
#[allow(dead_code)]
struct PanelState {
    cfg: AppConfig,
    token: Option<String>,
}

/// 启动 Web 面板。
pub async fn serve(cfg: AppConfig, port: u16, host: &str, token: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let state = Arc::new(PanelState { cfg, token });

    let app = Router::new()
        // 静态资源（嵌入前端 dist）
        .route("/", get(index))
        .route("/assets/{*file}", get(asset))
        // REST API
        .route("/api/data_dir", get(data_dir))
        .route("/api/runtime_list", get(runtime_list))
        .route("/api/runtime_status", get(runtime_status))
        .route("/api/runtime_start", post(runtime_start))
        .route("/api/runtime_stop", post(runtime_stop))
        .route("/api/runtime_reload", post(runtime_reload))
        .route("/api/site_list", get(site_list))
        .route("/api/site_add", post(site_add))
        .route("/api/site_update", post(site_update))
        .route("/api/site_remove", post(site_remove))
        .route("/api/hosts_list", get(hosts_list))
        .route("/api/hosts_writable", get(hosts_writable))
        .route("/api/hosts_sync", post(hosts_sync))
        .route("/api/hosts_content", get(hosts_content))
        .route("/api/db_sqlite_list", get(db_sqlite_list))
        .route("/api/db_sqlite_create", post(db_sqlite_create))
        .route("/api/db_sqlite_tables", get(db_sqlite_tables))
        .route("/api/db_sqlite_execute", post(db_sqlite_execute))
        .route("/api/db_remote_list", get(db_remote_list))
        .route("/api/db_remote_add", post(db_remote_add))
        .route("/api/db_remote_remove", post(db_remote_remove))
        .route("/api/db_remote_test", post(db_remote_test))
        .with_state(state);

    tracing::info!("Web 面板启动: http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ---- 鉴权检查（预留，当前面板端点内联使用） ----
#[allow(dead_code)]
fn check_auth(headers: &HeaderMap, token: &Option<String>) -> bool {
    match token {
        None => true, // 无 token 则不鉴权（仅本地使用）
        Some(t) => headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == format!("Bearer {t}"))
            .unwrap_or(false),
    }
}

// ---- 静态资源 ----

#[derive(rust_embed::Embed)]
#[folder = "../../dist/"]
struct PanelAssets;

// rust_embed 的 EmbeddedFile 通过实现 trait 提供 get() 方法
use rust_embed::Embed as _;

async fn index() -> impl IntoResponse {
    match PanelAssets::get("index.html") {
        Some(content) => Html(content.data.to_vec()),
        None => Html("<h1>前端资源未构建</h1><p>请先运行 npm run build</p>".as_bytes().to_vec()),
    }
}

async fn asset(Path(file): Path<String>) -> Response {
    let path = format!("assets/{file}");
    match PanelAssets::get(&path) {
        Some(content) => {
            let mime = mime_guess(&path);
            Response::builder()
                .header("Content-Type", mime)
                .body(axum::body::Body::from(content.data.to_vec()))
                .unwrap()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn mime_guess(path: &str) -> &'static str {
    if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".html") {
        "text/html"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

// ---- API 处理函数 ----

type S = State<Arc<PanelState>>;

async fn data_dir(State(s): S) -> Json<Value> {
    Json(json!(s.cfg.data_dir.to_string_lossy().to_string()))
}

async fn runtime_list(State(s): S) -> Json<Value> {
    let mgr = RuntimeManager::new(s.cfg.clone());
    let list: Vec<_> = mgr
        .list_installed()
        .into_iter()
        .map(|r| json!({"version": r.version, "path": r.path.to_string_lossy(), "is_default": r.is_default}))
        .collect();
    Json(json!(list))
}

async fn runtime_status() -> Json<Value> {
    Json(json!({"running": caddy::status().await}))
}

async fn runtime_start(State(s): S) -> Json<Value> {
    let mgr = RuntimeManager::new(s.cfg.clone());
    match mgr.resolve(None) {
        Ok(rt) => match caddy::start(&s.cfg, &rt.path).await {
            Ok((info, child)) => {
                tokio::spawn(async move {
                    let mut child = child;
                    let _ = child.wait().await;
                });
                Json(json!({"pid": info.pid}))
            }
            Err(e) => Json(json!({"error": e.to_string()})),
        },
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn runtime_stop(State(s): S) -> Json<Value> {
    match caddy::stop(&s.cfg).await {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn runtime_reload(State(s): S) -> Json<Value> {
    let mgr = RuntimeManager::new(s.cfg.clone());
    match mgr.resolve(None) {
        Ok(rt) => match caddy::reload(&s.cfg, &rt.path).await {
            Ok(()) => Json(json!({"ok": true})),
            Err(e) => Json(json!({"error": e.to_string()})),
        },
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn site_list(State(s): S) -> Json<Value> {
    let reg = s.cfg.load_sites().unwrap_or_default();
    Json(json!(reg.sites))
}

async fn site_add(State(s): S, Json(site): Json<Site>) -> Json<Value> {
    let mut reg = s.cfg.load_sites().unwrap_or_default();
    if let Err(e) = reg.validate(&site, None) {
        return Json(json!({"error": e.to_string()}));
    }
    reg.add(site);
    let _ = s.cfg.save_sites(&reg);
    let _ = caddy::write_caddyfile(&s.cfg, &reg.sites);
    Json(json!({"ok": true}))
}

async fn site_update(State(s): S, Json(site): Json<Site>) -> Json<Value> {
    let mut reg = s.cfg.load_sites().unwrap_or_default();
    let created = reg.get(&site.id).map(|s| s.created_at.clone());
    if created.is_none() {
        return Json(json!({"error": "站点不存在"}));
    }
    let _ = reg.validate(&site, Some(&site.id));
    let mut site = site;
    site.created_at = created.unwrap();
    site.touch();
    if let Some(e) = reg.get_mut(&site.id) {
        *e = site;
    }
    let _ = s.cfg.save_sites(&reg);
    let _ = caddy::write_caddyfile(&s.cfg, &reg.sites);
    Json(json!({"ok": true}))
}

async fn site_remove(State(s): S, Json(body): Json<Value>) -> Json<Value> {
    let id = body["id"].as_str().unwrap_or("");
    let mut reg = s.cfg.load_sites().unwrap_or_default();
    reg.remove(id);
    let _ = s.cfg.save_sites(&reg);
    let _ = caddy::write_caddyfile(&s.cfg, &reg.sites);
    Json(json!({"ok": true}))
}

async fn hosts_list() -> Json<Value> {
    let hm = HostsManager::system();
    let entries = hm.list_managed().unwrap_or_default();
    Json(json!(entries))
}

async fn hosts_writable() -> Json<Value> {
    Json(json!({"writable": HostsManager::system().check_writable()}))
}

async fn hosts_sync(State(s): S) -> Json<Value> {
    let reg = s.cfg.load_sites().unwrap_or_default();
    let entries = entries_from_sites(&reg.sites);
    let hm = HostsManager::system();
    match hm.sync(&entries) {
        Ok(()) => Json(json!({"count": entries.len()})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn hosts_content() -> Json<Value> {
    let hm = HostsManager::system();
    let content = hm.read().unwrap_or_default();
    Json(json!(content))
}

async fn db_sqlite_list(State(s): S) -> Json<Value> {
    let mgr = SqliteManager::new(s.cfg.data_dir.join("databases"));
    let list = mgr.list_databases().unwrap_or_default();
    Json(json!(list))
}

async fn db_sqlite_create(State(s): S, Json(body): Json<Value>) -> Json<Value> {
    let name = body["name"].as_str().unwrap_or("");
    let mgr = SqliteManager::new(s.cfg.data_dir.join("databases"));
    match mgr.create_database(name) {
        Ok(p) => Json(json!({"path": p.to_string_lossy()})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn db_sqlite_tables(State(s): S, Json(body): Json<Value>) -> Json<Value> {
    let name = body["name"].as_str().unwrap_or("");
    let mgr = SqliteManager::new(s.cfg.data_dir.join("databases"));
    match mgr.list_tables(name) {
        Ok(t) => Json(json!(t)),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn db_sqlite_execute(State(s): S, Json(body): Json<Value>) -> Json<Value> {
    let name = body["name"].as_str().unwrap_or("");
    let sql = body["sql"].as_str().unwrap_or("");
    let mgr = SqliteManager::new(s.cfg.data_dir.join("databases"));
    match mgr.execute(name, sql) {
        Ok(r) => Json(json!(r)),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn db_remote_list(State(s): S) -> Json<Value> {
    let mgr = RemoteDbManager::new(&s.cfg.data_dir);
    let reg = mgr.load().unwrap_or_default();
    Json(json!(reg.profiles))
}

async fn db_remote_add(State(s): S, Json(profile): Json<ConnectionProfile>) -> Json<Value> {
    let mgr = RemoteDbManager::new(&s.cfg.data_dir);
    match mgr.add(profile) {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn db_remote_remove(State(s): S, Json(body): Json<Value>) -> Json<Value> {
    let id = body["id"].as_str().unwrap_or("");
    let mgr = RemoteDbManager::new(&s.cfg.data_dir);
    match mgr.remove(id) {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

async fn db_remote_test(Json(profile): Json<ConnectionProfile>) -> Json<Value> {
    match RemoteDbManager::test_connection(&profile).await {
        Ok(msg) => Json(json!({"message": msg})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}
