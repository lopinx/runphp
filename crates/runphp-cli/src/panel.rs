//! Web 面板：axum 托管前端静态资源 + REST API。
//!
//! 契约与 Tauri command 完全对齐：
//! - 全部 API 为 POST（前端适配层统一 POST JSON）
//! - 请求体为 Tauri invoke 的参数对象（如 `{"site": {...}}`）
//! - 成功时直接返回与 Tauri command 相同形状的 JSON（数组/布尔/数字/字符串）；
//!   失败返回非 2xx + 错误文本
//! - 设置 token 时，`/api/*` 走 Bearer 鉴权

use axum::{
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
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
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

/// 面板共享状态。
struct PanelState {
    cfg: AppConfig,
    token: Option<String>,
}

/// 启动 Web 面板。
pub async fn serve(
    cfg: AppConfig,
    port: u16,
    host: &str,
    token: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let has_token = token.is_some();
    let state = Arc::new(PanelState { cfg, token });

    // REST API（与 Tauri command 同名同参）
    let api = Router::new()
        .route("/data_dir", post(data_dir))
        .route("/runtime_list", post(runtime_list))
        .route("/runtime_install", post(runtime_install))
        .route("/runtime_start", post(runtime_start))
        .route("/runtime_stop", post(runtime_stop))
        .route("/runtime_reload", post(runtime_reload))
        .route("/runtime_status", post(runtime_status))
        .route("/runtime_set_default", post(runtime_set_default))
        .route("/logs_read", post(logs_read))
        .route("/site_list", post(site_list))
        .route("/site_add", post(site_add))
        .route("/site_update", post(site_update))
        .route("/site_remove", post(site_remove))
        .route("/hosts_list", post(hosts_list))
        .route("/hosts_writable", post(hosts_writable))
        .route("/hosts_sync", post(hosts_sync))
        .route("/hosts_content", post(hosts_content))
        .route("/hosts_elevation", post(hosts_elevation))
        .route("/db_sqlite_list", post(db_sqlite_list))
        .route("/db_sqlite_create", post(db_sqlite_create))
        .route("/db_sqlite_delete", post(db_sqlite_delete))
        .route("/db_sqlite_tables", post(db_sqlite_tables))
        .route("/db_sqlite_query_table", post(db_sqlite_query_table))
        .route("/db_sqlite_execute", post(db_sqlite_execute))
        .route("/db_remote_list", post(db_remote_list))
        .route("/db_remote_add", post(db_remote_add))
        .route("/db_remote_remove", post(db_remote_remove))
        .route("/db_remote_test", post(db_remote_test));

    // 设置 token 时对 API 启用 Bearer 鉴权
    let api = if has_token {
        api.layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
    } else {
        api
    };

    let app = Router::new()
        // 静态资源（嵌入前端 dist）
        .route("/", get(index))
        .route("/assets/{*file}", get(asset))
        .nest("/api", api)
        // 允许跨域（用户可能从不同端口/域名访问面板 API）
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(state);

    tracing::info!("Web 面板启动: http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Bearer token 鉴权中间件。
async fn auth_middleware(
    State(s): State<Arc<PanelState>>,
    headers: HeaderMap,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let ok = match &s.token {
        None => true,
        Some(t) => headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == format!("Bearer {t}"))
            .unwrap_or(false),
    };
    if !ok {
        return (StatusCode::UNAUTHORIZED, "未授权：缺少或错误的 Bearer token").into_response();
    }
    next.run(req).await
}

// ---- 静态资源 ----

#[derive(rust_embed::Embed)]
#[folder = "../../dist/"]
struct PanelAssets;

async fn index() -> impl IntoResponse {
    match PanelAssets::get("index.html") {
        Some(content) => Html(content.data.to_vec()),
        None => Html("<h1>前端资源未构建</h1><p>请先运行 npm run build</p>".as_bytes().to_vec()),
    }
}

async fn asset(axum::extract::Path(file): axum::extract::Path<String>) -> Response {
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

// ---- 请求体结构（与 Tauri invoke 参数名对齐） ----

#[derive(Deserialize)]
struct VersionReq {
    version: String,
}
#[derive(Deserialize)]
struct SiteReq {
    site: Site,
}
#[derive(Deserialize)]
struct IdReq {
    id: String,
}
#[derive(Deserialize)]
struct NameReq {
    name: String,
}
#[derive(Deserialize)]
struct TablesReq {
    name: String,
}
#[derive(Deserialize)]
struct QueryTableReq {
    name: String,
    table: String,
    limit: Option<i64>,
    offset: Option<i64>,
}
#[derive(Deserialize)]
struct ExecuteReq {
    name: String,
    sql: String,
}
#[derive(Deserialize)]
struct ProfileReq {
    profile: ConnectionProfile,
}

type S = State<Arc<PanelState>>;

fn sqlite_mgr(cfg: &AppConfig) -> SqliteManager {
    SqliteManager::new(cfg.data_dir.join("databases"))
}

// ---- API 处理函数 ----

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

async fn runtime_install(State(s): S, Json(req): Json<VersionReq>) -> Response {
    let mgr = RuntimeManager::new(s.cfg.clone());
    match mgr.install(&req.version, None).await {
        Ok(p) => {
            // 首次安装自动设为默认（与 CLI 行为一致）
            if s.cfg.default_runtime_version.is_empty() {
                let mut new_cfg = s.cfg.clone();
                new_cfg.default_runtime_version = req.version.clone();
                let _ = new_cfg.save();
            }
            Json(json!(p.to_string_lossy().to_string())).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn runtime_start(State(s): S) -> Response {
    let mgr = RuntimeManager::new(s.cfg.clone());
    let rt = match mgr.resolve(None) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match caddy::start(&s.cfg, &rt.path).await {
        Ok((info, child)) => {
            tokio::spawn(async move {
                let mut child = child;
                let _ = child.wait().await;
            });
            Json(json!(info.pid)).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn runtime_stop(State(s): S) -> Response {
    match caddy::stop(&s.cfg).await {
        Ok(()) => Json(Value::Null).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn runtime_reload(State(s): S) -> Response {
    let mgr = RuntimeManager::new(s.cfg.clone());
    let rt = match mgr.resolve(None) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match caddy::reload(&s.cfg, &rt.path).await {
        Ok(()) => Json(Value::Null).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn runtime_status() -> Json<Value> {
    Json(json!(caddy::status().await))
}

async fn runtime_set_default(State(s): S, Json(req): Json<VersionReq>) -> Response {
    let mgr = RuntimeManager::new(s.cfg.clone());
    if !mgr.list_installed().iter().any(|r| r.version == req.version) {
        return (
            StatusCode::BAD_REQUEST,
            format!("运行时 {} 未安装", req.version),
        )
            .into_response();
    }
    let mut cfg = s.cfg.clone();
    cfg.default_runtime_version = req.version;
    match cfg.save() {
        Ok(()) => Json(Value::Null).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn logs_read(State(s): S, Json(req): Json<Value>) -> Response {
    let lines = req
        .get("lines")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(200);
    match caddy::read_log(&s.cfg, lines) {
        Ok(t) => Json(json!(t)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn site_list(State(s): S) -> Json<Value> {
    let reg = s.cfg.load_sites().unwrap_or_default();
    Json(json!(reg.sites))
}

async fn site_add(State(s): S, Json(req): Json<SiteReq>) -> Response {
    let mut site = req.site;
    let mut reg = s.cfg.load_sites().unwrap_or_default();
    if let Err(e) = reg.validate(&site, None) {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    site.touch();
    reg.add(site);
    if let Err(e) = s.cfg.save_sites(&reg) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = caddy::write_caddyfile(&s.cfg, &reg.sites) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    Json(Value::Null).into_response()
}

async fn site_update(State(s): S, Json(req): Json<SiteReq>) -> Response {
    let mut site = req.site;
    let mut reg = s.cfg.load_sites().unwrap_or_default();
    let created_at = match reg.get(&site.id).map(|x| x.created_at.clone()) {
        Some(c) => c,
        None => return (StatusCode::BAD_REQUEST, "站点不存在".to_string()).into_response(),
    };
    if let Err(e) = reg.validate(&site, Some(&site.id)) {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    site.created_at = created_at;
    site.touch();
    if let Some(existing) = reg.get_mut(&site.id) {
        *existing = site;
    }
    if let Err(e) = s.cfg.save_sites(&reg) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = caddy::write_caddyfile(&s.cfg, &reg.sites) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    Json(Value::Null).into_response()
}

async fn site_remove(State(s): S, Json(req): Json<IdReq>) -> Response {
    let mut reg = s.cfg.load_sites().unwrap_or_default();
    if reg.remove(&req.id).is_none() {
        return (StatusCode::BAD_REQUEST, "站点不存在".to_string()).into_response();
    }
    if let Err(e) = s.cfg.save_sites(&reg) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = caddy::write_caddyfile(&s.cfg, &reg.sites) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    Json(Value::Null).into_response()
}

async fn hosts_list() -> Json<Value> {
    let hm = HostsManager::system();
    let entries = hm.list_managed().unwrap_or_default();
    Json(json!(entries))
}

async fn hosts_writable() -> Json<Value> {
    Json(json!(HostsManager::system().check_writable()))
}

async fn hosts_sync(State(s): S) -> Response {
    let reg = s.cfg.load_sites().unwrap_or_default();
    let entries = entries_from_sites(&reg.sites);
    let count = entries.len();
    let hm = HostsManager::system();
    match hm.sync(&entries) {
        Ok(()) => Json(json!(count)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn hosts_content() -> Json<Value> {
    let hm = HostsManager::system();
    let content = hm.read().unwrap_or_default();
    Json(json!(content))
}

async fn hosts_elevation() -> Json<Value> {
    Json(json!(HostsManager::system().elevation_command("sync")))
}

async fn db_sqlite_list(State(s): S) -> Json<Value> {
    let list = sqlite_mgr(&s.cfg).list_databases().unwrap_or_default();
    Json(json!(list))
}

async fn db_sqlite_create(State(s): S, Json(req): Json<NameReq>) -> Response {
    match sqlite_mgr(&s.cfg).create_database(&req.name) {
        Ok(p) => Json(json!(p.to_string_lossy().to_string())).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn db_sqlite_delete(State(s): S, Json(req): Json<NameReq>) -> Response {
    match sqlite_mgr(&s.cfg).delete_database(&req.name) {
        Ok(()) => Json(Value::Null).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn db_sqlite_tables(State(s): S, Json(req): Json<TablesReq>) -> Response {
    match sqlite_mgr(&s.cfg).list_tables(&req.name) {
        Ok(t) => Json(json!(t)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn db_sqlite_query_table(State(s): S, Json(req): Json<QueryTableReq>) -> Response {
    let limit = req.limit.unwrap_or(100);
    let offset = req.offset.unwrap_or(0);
    match sqlite_mgr(&s.cfg).query_table(&req.name, &req.table, limit, offset) {
        Ok(r) => Json(json!(r)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn db_sqlite_execute(State(s): S, Json(req): Json<ExecuteReq>) -> Response {
    match sqlite_mgr(&s.cfg).execute(&req.name, &req.sql) {
        Ok(r) => Json(json!(r)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn db_remote_list(State(s): S) -> Json<Value> {
    let mgr = RemoteDbManager::new(&s.cfg.data_dir);
    let reg = mgr.load().unwrap_or_default();
    Json(json!(reg.profiles))
}

async fn db_remote_add(State(s): S, Json(req): Json<ProfileReq>) -> Response {
    let mgr = RemoteDbManager::new(&s.cfg.data_dir);
    match mgr.add(req.profile) {
        Ok(()) => Json(Value::Null).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn db_remote_remove(State(s): S, Json(req): Json<IdReq>) -> Response {
    let mgr = RemoteDbManager::new(&s.cfg.data_dir);
    match mgr.remove(&req.id) {
        Ok(()) => Json(Value::Null).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn db_remote_test(State(_): State<Arc<PanelState>>, Json(req): Json<ProfileReq>) -> Response {
    match RemoteDbManager::test_connection(&req.profile).await {
        Ok(msg) => Json(json!(msg)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}
