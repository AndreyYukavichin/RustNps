use crate::server::{self, Registry, WebSession};
use axum::{
    extract::{ConnectInfo, OriginalUri, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::services::ServeDir;

pub async fn start_web_manager(
    registry: Arc<Registry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr =
        format!("{}:{}", registry.server.web_ip, registry.server.web_port).parse()?;
    let base = registry.server.web_base_url.clone();
    let web_port = registry.server.web_port;
    let web_root = get_web_root();
    crate::log_debug!(
        "web",
        "Web manager using resources from: {}",
        web_root.display()
    );

    let router = Router::new()
        .route("/", get(handle_index))
        .route("/index/index", get(handle_index))
        .route("/login/index", get(handle_login))
        .route("/login/", get(handle_login))
        .route("/login/verify", post(handle_login_verify))
        .route("/captcha/", get(handle_captcha))
        .route(
            "/login/register",
            get(handle_register).post(handle_register_submit),
        )
        .route("/login/out", get(handle_logout))
        .route("/client/list", get(handle_client_list))
        .route("/client/list", post(handle_client_list_api))
        .route("/index/hostlist", get(handle_host_list))
        .route("/index/hostlist", post(handle_host_list_api))
        .route("/index/gettunnel", post(handle_tunnel_list_api))
        .route("/index/tcp", get(handle_tcp_list))
        .route("/index/udp", get(handle_udp_list))
        .route("/index/http", get(handle_http_list))
        .route("/index/socks5", get(handle_socks5_list))
        .route("/index/secret", get(handle_secret_list))
        .route("/index/p2p", get(handle_p2p_list))
        .route("/index/file", get(handle_file_list))
        .route("/index/all", get(handle_all_tunnels))
        .route("/client/add", get(handle_client_add))
        .route("/client/edit", get(handle_client_edit))
        .route("/index/add", get(handle_tunnel_add))
        .route("/index/edit", get(handle_tunnel_edit))
        .route("/index/addhost", get(handle_host_add))
        .route("/index/edithost", get(handle_host_edit))
        .route("/global/index", get(handle_global))
        .route("/api/dashboard", get(handle_dashboard_api))
        .route("/client/changestatus", post(handle_post_mutation))
        .route("/client/del", post(handle_post_mutation))
        .route("/client/add", post(handle_post_mutation))
        .route("/client/edit", post(handle_post_mutation))
        .route("/index/stop", post(handle_post_mutation))
        .route("/index/start", post(handle_post_mutation))
        .route("/index/del", post(handle_post_mutation))
        .route("/index/copy", post(handle_post_mutation))
        .route("/index/add", post(handle_post_mutation))
        .route("/index/edit", post(handle_post_mutation))
        .route("/index/hoststop", post(handle_post_mutation))
        .route("/index/hoststart", post(handle_post_mutation))
        .route("/index/delhost", post(handle_post_mutation))
        .route("/index/addhost", post(handle_post_mutation))
        .route("/index/edithost", post(handle_post_mutation))
        .route("/global/save", post(handle_post_mutation))
        .nest_service("/static", ServeDir::new(web_root.join("static")));

    let app = if !base.is_empty() && base != "/" {
        Router::new().nest(&base, router)
    } else {
        router
    }
    .with_state(registry);

    crate::log_info!("web", "web management start, access port is {}", web_port);
    crate::log_debug!("web", "Web manager listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

fn get_web_root() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("NPS_WEB_PATH") {
        return std::path::PathBuf::from(p);
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut curr = cwd.as_path();
        loop {
            let path = curr.join("web");
            if path.is_dir() && path.join("views").is_dir() {
                return path;
            }
            let rnp_path = curr.join("RustNps").join("web");
            if rnp_path.is_dir() && rnp_path.join("views").is_dir() {
                return rnp_path;
            }
            if let Some(parent) = curr.parent() {
                curr = parent;
            } else {
                break;
            }
        }
    }
    std::path::PathBuf::from("web")
}

fn current_session(registry: &Registry, jar: &CookieJar) -> Option<WebSession> {
    if registry.server.web_username.is_empty() {
        return Some(WebSession::admin(""));
    }
    if let Some(session) = jar.get("rustnps_session") {
        return registry
            .sessions
            .lock()
            .unwrap()
            .get(session.value())
            .cloned();
    }
    None
}

async fn handle_login(State(registry): State<Arc<Registry>>) -> Html<String> {
    Html(render_login(&registry, ""))
}

async fn handle_captcha(
    State(registry): State<Arc<Registry>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let token = params.get("token").cloned().unwrap_or_default();
    crate::log_trace!("web", "captcha request token={}", token);
    if let Some(svg) = server::captcha_svg(registry.as_ref(), &token) {
        ([("content-type", "image/svg+xml; charset=utf-8")], svg).into_response()
    } else {
        crate::log_warn!("web", "captcha request miss token={}", token);
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn handle_register(State(registry): State<Arc<Registry>>) -> Html<String> {
    if !registry.server.allow_user_register {
        crate::log_warn!("web", "register page denied, allow_user_register=false");
        return Html(render_login(&registry, "register is not allow"));
    }
    crate::log_trace!("web", "register page opened");
    Html(load_view(
        "register.html",
        &HashMap::from([("base".to_string(), registry.server.web_base_url.clone())]),
    ))
}

async fn handle_register_submit(
    State(registry): State<Arc<Registry>>,
    Form(params): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    crate::log_info!(
        "web",
        "register submit username={}",
        params.get("username").cloned().unwrap_or_default()
    );
    let res = server::register_web_user(&registry, &params);
    axum::Json(serde_json::from_str::<Value>(&res).unwrap())
}

async fn handle_login_verify(
    State(registry): State<Arc<Registry>>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    jar: CookieJar,
    Form(params): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let username = params.get("username").cloned().unwrap_or_default();
    let password = params.get("password").cloned().unwrap_or_default();
    crate::log_info!("web", "login verify username={} remote={}", username, remote_addr);

    if registry.server.open_captcha {
        let captcha_token = params.get("captcha_token").cloned().unwrap_or_default();
        let captcha = params.get("captcha").cloned().unwrap_or_default();
        if !server::verify_login_captcha(registry.as_ref(), &captcha_token, &captcha) {
            crate::log_warn!("web", "login captcha verify failed username={} remote={}", username, remote_addr);
            return (
                jar,
                axum::Json(serde_json::json!({
                    "status": 0,
                    "msg": "the verification code is wrong, please get it again and try again"
                })),
            );
        }
    }

    if let Some(session) = server::authenticate_web_user(&registry, &username, &password) {
        server::authorize_web_login_ip(registry.as_ref(), &remote_addr.to_string());
        let token = crate::relay::md5_hex(&format!(
            "{}:{}:{:?}:{}",
            username,
            password,
            std::time::SystemTime::now(),
            registry.next_link_id()
        ));
        registry
            .sessions
            .lock()
            .unwrap()
            .insert(token.clone(), session);
        let jar = jar.add(
            Cookie::build(("rustnps_session", token))
                .path("/")
                .http_only(true)
                .build(),
        );
        (
            jar,
            axum::Json(serde_json::json!({ "status": 1, "msg": "login success" })),
        )
    } else {
        let _ = password;
        crate::log_warn!("web", "login failed username={} remote={}", username, remote_addr);
        (
            jar,
            axum::Json(serde_json::json!({ "status": 0, "msg": "username or password incorrect" })),
        )
    }
}

async fn handle_logout(State(registry): State<Arc<Registry>>, jar: CookieJar) -> impl IntoResponse {
    if let Some(session) = jar.get("rustnps_session") {
        registry.sessions.lock().unwrap().remove(session.value());
    }
    let jar = jar.remove(Cookie::from("rustnps_session"));
    (
        jar,
        Redirect::to(&format!("{}/login/index", registry.server.web_base_url)),
    )
}

async fn handle_index(State(r): State<Arc<Registry>>, j: CookieJar) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return Redirect::to(&format!("{}/login/index", r.server.web_base_url)).into_response();
    };
    Html(render_layout(
        &r,
        &session,
        "index",
        "Dashboard",
        &render_dashboard(&r, &session),
    ))
    .into_response()
}

async fn handle_client_list(State(r): State<Arc<Registry>>, j: CookieJar) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return Redirect::to(&format!("{}/login/index", r.server.web_base_url)).into_response();
    };
    Html(render_layout(
        &r,
        &session,
        "client",
        "Client List",
        &load_view(
            "client_list.html",
            &HashMap::from([
                ("base".to_string(), r.server.web_base_url.clone()),
                (
                    "client_add_button".to_string(),
                    if session.is_admin {
                        format!(
                            r#"<a href="{}/client/add" class="btn btn-primary dim"><i class="fa fa-fw fa-lg fa-plus"></i> <span langtag="word-add"></span></a>"#,
                            r.server.web_base_url
                        )
                    } else {
                        String::new()
                    },
                ),
                (
                    "is_admin".to_string(),
                    if session.is_admin { "true" } else { "false" }.to_string(),
                ),
            ]),
        ),
    ))
    .into_response()
}

async fn handle_host_list(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return Redirect::to(&format!("{}/login/index", r.server.web_base_url)).into_response();
    };
    let client_id = scoped_client_id(&session, q.get("client_id").cloned());
    Html(render_layout(
        &r,
        &session,
        "host",
        "Host List",
        &load_view(
            "host_list.html",
            &HashMap::from([
                ("base".to_string(), r.server.web_base_url.clone()),
                ("client_id".to_string(), client_id),
            ]),
        ),
    ))
    .into_response()
}

fn scoped_client_id(session: &WebSession, requested: Option<String>) -> String {
    if session.is_admin {
        requested.unwrap_or_else(|| "0".to_string())
    } else {
        session.client_id.unwrap_or(0).to_string()
    }
}

fn scoped_params(
    session: &WebSession,
    mut params: HashMap<String, String>,
) -> HashMap<String, String> {
    if !session.is_admin {
        if let Some(client_id) = session.client_id {
            params.insert("client_id".to_string(), client_id.to_string());
        }
    }
    params
}

async fn handle_client_list_api(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Form(p): Form<HashMap<String, String>>,
) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let p = scoped_params(&session, p);
    axum::Json(serde_json::from_str::<Value>(&server::client_rows_json(&r, &p)).unwrap())
        .into_response()
}

async fn handle_host_list_api(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Form(p): Form<HashMap<String, String>>,
) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let p = scoped_params(&session, p);
    axum::Json(serde_json::from_str::<Value>(&server::host_rows_json(&r, &p)).unwrap())
        .into_response()
}

async fn handle_tunnel_list_api(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Form(p): Form<HashMap<String, String>>,
) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let p = scoped_params(&session, p);
    axum::Json(serde_json::from_str::<Value>(&server::tunnel_rows_json(&r, &p)).unwrap())
        .into_response()
}

async fn handle_dashboard_api(State(r): State<Arc<Registry>>, j: CookieJar) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    axum::Json(
        serde_json::from_str::<Value>(&server::dashboard_json_scoped(
            &r,
            scope_client_id(&session),
        ))
        .unwrap(),
    )
    .into_response()
}

fn scope_client_id(session: &WebSession) -> Option<u64> {
    if session.is_admin {
        None
    } else {
        session.client_id
    }
}

async fn handle_tcp_list(
    s: State<Arc<Registry>>,
    j: CookieJar,
    q: Query<HashMap<String, String>>,
) -> Response {
    handle_tunnel_page(s, j, q, "tcp", "TCP").await
}
async fn handle_udp_list(
    s: State<Arc<Registry>>,
    j: CookieJar,
    q: Query<HashMap<String, String>>,
) -> Response {
    handle_tunnel_page(s, j, q, "udp", "UDP").await
}
async fn handle_http_list(
    s: State<Arc<Registry>>,
    j: CookieJar,
    q: Query<HashMap<String, String>>,
) -> Response {
    handle_tunnel_page(s, j, q, "httpProxy", "HTTP Proxy").await
}
async fn handle_socks5_list(
    s: State<Arc<Registry>>,
    j: CookieJar,
    q: Query<HashMap<String, String>>,
) -> Response {
    handle_tunnel_page(s, j, q, "socks5", "SOCKS5").await
}
async fn handle_secret_list(
    s: State<Arc<Registry>>,
    j: CookieJar,
    q: Query<HashMap<String, String>>,
) -> Response {
    handle_tunnel_page(s, j, q, "secret", "Secret").await
}
async fn handle_p2p_list(
    s: State<Arc<Registry>>,
    j: CookieJar,
    q: Query<HashMap<String, String>>,
) -> Response {
    handle_tunnel_page(s, j, q, "p2p", "P2P").await
}
async fn handle_file_list(
    s: State<Arc<Registry>>,
    j: CookieJar,
    q: Query<HashMap<String, String>>,
) -> Response {
    handle_tunnel_page(s, j, q, "fileserver", "File Server").await
}
async fn handle_all_tunnels(
    s: State<Arc<Registry>>,
    j: CookieJar,
    q: Query<HashMap<String, String>>,
) -> Response {
    handle_tunnel_page(s, j, q, "all", "All Tunnels").await
}

async fn handle_tunnel_page(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Query(q): Query<HashMap<String, String>>,
    kind: &str,
    name: &str,
) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return Redirect::to(&format!("{}/login/index", r.server.web_base_url)).into_response();
    };
    let client_id = scoped_client_id(&session, q.get("client_id").cloned());
    Html(render_layout(
        &r,
        &session,
        kind,
        name,
        &load_view(
            "tunnel_list.html",
            &HashMap::from([
                ("base".to_string(), r.server.web_base_url.clone()),
                ("kind".to_string(), kind.to_string()),
                ("name".to_string(), name.to_string()),
                ("client_id".to_string(), client_id),
            ]),
        ),
    ))
    .into_response()
}

async fn handle_client_add(State(r): State<Arc<Registry>>, j: CookieJar) -> Response {
    render_form(r, j, "client", "clientadd", "/client/add", None, None).await
}
async fn handle_client_edit(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    render_form(
        r,
        j,
        "client",
        "clientedit",
        "/client/edit",
        q.get("id").and_then(|v| v.parse().ok()),
        None,
    )
    .await
}
async fn handle_tunnel_add(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let mut kind = q.get("type").cloned().unwrap_or_else(|| "tcp".to_string());
    if kind.eq_ignore_ascii_case("all") {
        kind = "tcp".to_string();
    }
    render_form(
        r,
        j,
        &kind,
        "add",
        "/index/add",
        None,
        q.get("client_id").and_then(|v| v.parse().ok()),
    )
    .await
}
async fn handle_tunnel_edit(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    render_form(
        r,
        j,
        "tcp",
        "edit",
        "/index/edit",
        q.get("id").and_then(|v| v.parse().ok()),
        None,
    )
    .await
}
async fn handle_host_add(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    render_form(
        r,
        j,
        "host",
        "hostadd",
        "/index/addhost",
        None,
        q.get("client_id").and_then(|v| v.parse().ok()),
    )
    .await
}
async fn handle_host_edit(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    render_form(
        r,
        j,
        "host",
        "hostedit",
        "/index/edithost",
        q.get("id").and_then(|v| v.parse().ok()),
        None,
    )
    .await
}
async fn handle_global(State(r): State<Arc<Registry>>, j: CookieJar) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return Redirect::to(&format!("{}/login/index", r.server.web_base_url)).into_response();
    };
    if !session.is_admin {
        return StatusCode::FORBIDDEN.into_response();
    }
    let global = r.global.lock().unwrap().clone();
    Html(render_layout(
        &r,
        &session,
        "global",
        "Global Config",
        &load_view(
            "global.html",
            &HashMap::from([
                ("base".to_string(), r.server.web_base_url.clone()),
                ("server_url".to_string(), html_escape(&global.server_url)),
                (
                    "global_black_ip_list".to_string(),
                    html_escape(&global.black_ip_list.join("\n")),
                ),
            ]),
        ),
    ))
    .into_response()
}

async fn render_form(
    registry: Arc<Registry>,
    jar: CookieJar,
    kind: &str,
    title_tag: &str,
    action: &str,
    id: Option<u64>,
    client_id: Option<u64>,
) -> Response {
    let Some(session) = current_session(&registry, &jar) else {
        return Redirect::to(&format!("{}/login/index", registry.server.web_base_url))
            .into_response();
    };
    let id = id.unwrap_or(0);
    if let Err(status) = authorize_form(&registry, &session, action, id) {
        return status.into_response();
    }
    let client_id = if session.is_admin {
        client_id.unwrap_or(0)
    } else {
        session.client_id.unwrap_or(0)
    };
    let mut vars = HashMap::from([
        ("base".to_string(), registry.server.web_base_url.clone()),
        ("title_tag".to_string(), title_tag.to_string()),
        ("action".to_string(), action.to_string()),
        (
            "id_input".to_string(),
            format!(r#"<input type="hidden" name="id" value="{id}">"#),
        ),
    ]);
    let fields = match kind {
        "client" => render_client_fields(&registry, &session, id),
        "host" => render_host_fields(&registry, &session, id, client_id),
        _ => render_tunnel_fields(&registry, &session, kind, id, client_id),
    };
    vars.insert("fields".to_string(), fields);
    Html(render_layout(
        &registry,
        &session,
        "client",
        title_tag,
        &load_view("form.html", &vars),
    ))
    .into_response()
}

fn authorize_form(
    registry: &Registry,
    session: &WebSession,
    action: &str,
    id: u64,
) -> Result<(), StatusCode> {
    if session.is_admin {
        return Ok(());
    }
    let Some(client_id) = session.client_id else {
        return Err(StatusCode::FORBIDDEN);
    };
    match action {
        "/client/add" => Err(StatusCode::FORBIDDEN),
        "/client/edit" => {
            if id == client_id {
                Ok(())
            } else {
                Err(StatusCode::FORBIDDEN)
            }
        }
        "/index/add" | "/index/addhost" => Ok(()),
        "/index/edit" => tunnel_belongs_to_client(registry, id, client_id)
            .then_some(())
            .ok_or(StatusCode::FORBIDDEN),
        "/index/edithost" => host_belongs_to_client(registry, id, client_id)
            .then_some(())
            .ok_or(StatusCode::FORBIDDEN),
        _ => Ok(()),
    }
}

fn tunnel_belongs_to_client(registry: &Registry, tunnel_id: u64, client_id: u64) -> bool {
    let clients = registry.clients.lock().unwrap();
    clients.values().any(|client| {
        client.id == client_id && client.tunnels.iter().any(|tunnel| tunnel.id == tunnel_id)
    })
}

fn host_belongs_to_client(registry: &Registry, host_id: u64, client_id: u64) -> bool {
    let clients = registry.clients.lock().unwrap();
    clients
        .values()
        .any(|client| client.id == client_id && client.hosts.iter().any(|host| host.id == host_id))
}

fn render_client_fields(registry: &Registry, session: &WebSession, id: u64) -> String {
    let current = {
        let clients = registry.clients.lock().unwrap();
        clients.values().find(|client| client.id == id).cloned()
    };
    let (vkey, remark, user, pass, web_user, web_pass, compress, crypt, rate_limit, flow_limit, max_conn, max_tunnel, config_conn_allow, ip_white, ip_white_pass, ip_white_list, black_ip_list) = current
        .as_ref()
        .map(|client| {
            let info = &client.common.client;
            (
                client.common.vkey.clone(),
                info.remark.clone(),
                info.basic_username.clone(),
                info.basic_password.clone(),
                info.web_username.clone(),
                info.web_password.clone(),
                info.compress,
                info.crypt,
                info.rate_limit_kb.to_string(),
                info.flow_limit_mb.to_string(),
                info.max_conn.to_string(),
                info.max_tunnel_num.to_string(),
                info.config_conn_allow,
                info.ip_white,
                info.ip_white_pass.clone(),
                info.ip_white_list.join("\n"),
                info.black_ip_list.join("\n"),
            )
        })
        .unwrap_or_else(|| {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                false,
                false,
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                true,
                false,
                String::new(),
                String::new(),
                String::new(),
            )
        });
    let compress_checked = if compress { "checked" } else { "" };
    let crypt_checked = if crypt { "checked" } else { "" };
    let config_conn_allow_yes = if config_conn_allow { "selected" } else { "" };
    let config_conn_allow_no = if config_conn_allow { "" } else { "selected" };
    let ip_white_yes = if ip_white { "selected" } else { "" };
    let ip_white_no = if ip_white { "" } else { "selected" };
    let health_summary = current
        .as_ref()
        .map(|client| render_client_health_summary(registry, client))
        .unwrap_or_default();
    let limit_fields = if session.is_admin {
        let mut html = String::new();
        if registry.server.allow_flow_limit {
            html.push_str(&format!(r#"<div class="form-group"><label langtag="word-flowlimit"></label><input class="form-control" name="flow_limit" value="{}"><span class="help-block m-b-none">MB</span></div>"#, html_escape(&flow_limit)));
        }
        if registry.server.allow_rate_limit {
            html.push_str(&format!(r#"<div class="form-group"><label langtag="word-ratelimit"></label><input class="form-control" name="rate_limit" value="{}"><span class="help-block m-b-none">KB/s</span></div>"#, html_escape(&rate_limit)));
        }
        if registry.server.allow_connection_num_limit {
            html.push_str(&format!(r#"<div class="form-group"><label langtag="word-maxconnections"></label><input class="form-control" name="max_conn" value="{}"></div>"#, html_escape(&max_conn)));
        }
        if registry.server.allow_tunnel_num_limit {
            html.push_str(&format!(r#"<div class="form-group"><label langtag="word-maxtunnels"></label><input class="form-control" name="max_tunnel" value="{}"></div>"#, html_escape(&max_tunnel)));
        }
        html
    } else {
        String::new()
    };
    let vkey_field = if session.is_admin {
        format!(
            r#"<div class="form-group"><label>VKey</label><input class="form-control" name="vkey" value="{}"></div>"#,
            html_escape(&vkey)
        )
    } else {
        format!(
            r#"<div class="form-group"><label>VKey</label><input class="form-control" value="{}" readonly></div>"#,
            html_escape(&vkey)
        )
    };
    let web_user_field = if session.is_admin || registry.server.allow_user_change_username {
        format!(
            r#"<div class="form-group"><label langtag="word-webusername"></label><input class="form-control" name="web_username" value="{}"></div>"#,
            html_escape(&web_user)
        )
    } else {
        format!(
            r#"<div class="form-group"><label langtag="word-webusername"></label><input class="form-control" value="{}" readonly></div>"#,
            html_escape(&web_user)
        )
    };
    format!(
        r#"{vkey_field}<div class="form-group"><label langtag="word-remark"></label><input class="form-control" name="remark" value="{remark}"></div>{health_summary}{limit_fields}<div class="form-group"><label langtag="word-basicusername"></label><input class="form-control" name="u" value="{user}"></div><div class="form-group"><label langtag="word-basicpassword"></label><input class="form-control" name="p" value="{pass}"></div>{web_user_field}<div class="form-group"><label langtag="word-webpassword"></label><input class="form-control" name="web_password" value="{web_pass}"></div><div class="form-group"><label langtag="word-connectbyconfig"></label><select class="form-control" name="config_conn_allow"><option value="1" {config_conn_allow_yes} langtag="word-yes"></option><option value="0" {config_conn_allow_no} langtag="word-no"></option></select></div><div class="form-group"><label>Compress</label><div><label><input type="checkbox" name="compress" value="1" {compress_checked}> snappy</label></div></div><div class="form-group"><label>Crypt</label><div><label><input type="checkbox" name="crypt" value="1" {crypt_checked}> TLS relay</label></div></div><div class="form-group"><label langtag="word-ipwhite"></label><select class="form-control" id="ipwhite" name="ipwhite"><option value="0" {ip_white_no} langtag="word-no"></option><option value="1" {ip_white_yes} langtag="word-yes"></option></select></div><div class="form-group" id="ip_white_pass_group"><label langtag="word-ipwhitepass"></label><input class="form-control" name="ipwhitepass" value="{ip_white_pass}"><span class="help-block m-b-none" langtag="info-ipwhitepass"></span></div><div class="form-group" id="ip_white_list_group"><label langtag="word-ipwhitelist"></label><textarea class="form-control" rows="4" name="ipwhitelist">{ip_white_list}</textarea></div><div class="form-group" id="black_ip_list_group"><label langtag="word-blackiplist"></label><textarea class="form-control" rows="4" name="blackiplist">{black_ip_list}</textarea></div><script>(function(){{function syncIpWhite(){{var enabled=$('#ipwhite').val()==='1';$('#ip_white_pass_group').toggle(enabled);$('#ip_white_list_group').toggle(enabled);$('#black_ip_list_group').toggle(!enabled);}}$('#ipwhite').on('change',syncIpWhite);syncIpWhite();}})();</script>"#,
        remark = html_escape(&remark),
        health_summary = health_summary,
        limit_fields = limit_fields,
        user = html_escape(&user),
        pass = html_escape(&pass),
        web_pass = html_escape(&web_pass),
        compress_checked = compress_checked,
        crypt_checked = crypt_checked,
        config_conn_allow_yes = config_conn_allow_yes,
        config_conn_allow_no = config_conn_allow_no,
        ip_white_yes = ip_white_yes,
        ip_white_no = ip_white_no,
        ip_white_pass = html_escape(&ip_white_pass),
        ip_white_list = html_escape(&ip_white_list),
        black_ip_list = html_escape(&black_ip_list),
    )
}

fn render_client_health_summary(registry: &Registry, client: &crate::model::ClientRuntimeConfig) -> String {
    if client.healths.is_empty() {
        return String::new();
    }

    let removed_targets = server::client_health_down_targets(registry, &client.common.vkey);
    let mut unique_targets = HashSet::new();
    let mut down_targets = Vec::new();
    let mut rules = String::new();

    for health in &client.healths {
        let mut rule_down = Vec::new();
        let mut rule_up = Vec::new();
        for target in &health.targets {
            unique_targets.insert(target.clone());
            if removed_targets.contains(target) {
                rule_down.push(target.clone());
                down_targets.push(target.clone());
            } else {
                rule_up.push(target.clone());
            }
        }
        rules.push_str(&format!(
            r#"<li><strong>{}</strong> {}s / timeout {}s / max_failed {} / down {} / up {}<br><span class="text-muted">{}</span></li>"#,
            html_escape(&health.remark),
            health.interval_secs,
            health.timeout_secs,
            health.max_failed,
            rule_down.len(),
            rule_up.len(),
            html_escape(&health.targets.join("\n")),
        ));
    }

    let state = if down_targets.is_empty() { "healthy" } else { "degraded" };
    let mut distinct_down = down_targets;
    distinct_down.sort();
    distinct_down.dedup();

    format!(
        r#"<div class="form-group"><label>Health Summary</label><div class="alert alert-info" style="margin-bottom:10px"><div><strong>Status:</strong> {state}</div><div><strong>Rules:</strong> {rule_count}</div><div><strong>Targets:</strong> {target_count}</div><div><strong>Down:</strong> {down_count}</div><div><strong>Down targets:</strong> {down_targets}</div></div><ul class="list-unstyled" style="margin-top:8px">{rules}</ul></div>"#,
        state = state,
        rule_count = client.healths.len(),
        target_count = unique_targets.len(),
        down_count = distinct_down.len(),
        down_targets = html_escape(&distinct_down.join("\n")),
        rules = rules,
    )
}

fn render_host_fields(registry: &Registry, session: &WebSession, id: u64, client_id: u64) -> String {
    let current = {
        let clients = registry.clients.lock().unwrap();
        clients
            .values()
            .flat_map(|client| client.hosts.iter())
            .find(|host| host.id == id)
            .cloned()
    };
    let host = current.unwrap_or_default();
    let scheme_all = selected(&host.scheme, "all");
    let scheme_http = selected(&host.scheme, "http");
    let scheme_https = selected(&host.scheme, "https");
    let local_proxy_yes = if host.target.local_proxy { "selected" } else { "" };
    let local_proxy_no = if host.target.local_proxy { "" } else { "selected" };
    let proto_v1 = selected(&host.proto_version, "V1");
    let proto_v2 = selected(&host.proto_version, "V2");
    let proto_empty = if host.proto_version.trim().is_empty() { "selected" } else { "" };
    let auto_https_yes = if host.auto_https { "selected" } else { "" };
    let auto_https_no = if host.auto_https { "" } else { "selected" };
    let client_field = render_client_binding_field(registry, session, client_id, Some(host.client_vkey.as_str()));
    let local_proxy_field = if registry.server.allow_local_proxy {
        format!(r#"<div class="form-group"><label langtag="word-proxytolocal"></label><select class="form-control" name="local_proxy"><option value="0" {local_proxy_no} langtag="word-no"></option><option value="1" {local_proxy_yes} langtag="word-yes"></option></select></div>"#)
    } else {
        String::new()
    };
    let proto_field = format!(r#"<div class="form-group"><label>Proxy Protocol Version</label><select class="form-control" name="proto_version"><option value="" {proto_empty}></option><option value="V1" {proto_v1}>V1</option><option value="V2" {proto_v2}>V2</option></select></div>"#);
    format!(
        r#"{client_field}<div class="form-group"><label langtag="word-remark"></label><input class="form-control" name="remark" value="{remark}"></div><div class="form-group"><label langtag="word-host"></label><input class="form-control" name="host" value="{host_name}"></div><div class="form-group"><label langtag="word-scheme"></label><select class="form-control" id="scheme_select" name="scheme"><option value="all" {scheme_all}>all</option><option value="http" {scheme_http}>http</option><option value="https" {scheme_https}>https</option></select></div><div class="form-group" id="auto_https_group"><label>Auto HTTPS (301)</label><select class="form-control" name="AutoHttps"><option value="0" {auto_https_no} langtag="word-no"></option><option value="1" {auto_https_yes} langtag="word-yes"></option></select></div><div class="form-group" id="cert_file_group"><label>Cert file</label><textarea class="form-control" name="cert_file_path" rows="6">{cert}</textarea></div><div class="form-group" id="key_file_group"><label>Key file</label><textarea class="form-control" name="key_file_path" rows="6">{key}</textarea></div><div class="form-group"><label langtag="word-location"></label><input class="form-control" name="location" value="{location}"></div>{local_proxy_field}{proto_field}<div class="form-group"><label langtag="word-target"></label><textarea class="form-control" rows="4" name="target">{target}</textarea></div><div class="form-group"><label langtag="word-requestheader"></label><textarea class="form-control" rows="4" name="header">{header}</textarea></div><div class="form-group"><label langtag="word-requesthost"></label><input class="form-control" name="hostchange" value="{host_change}"></div><script>(function(){{function syncScheme(){{var tls=$('#scheme_select').val()!=='http';$('#cert_file_group').toggle(tls);$('#key_file_group').toggle(tls);$('#auto_https_group').toggle(tls);}}$('#scheme_select').on('change',syncScheme);syncScheme();}})();</script>"#,
        client_field = client_field,
        target = html_escape(&host.target.target_str),
        location = html_escape(&host.location),
        remark = html_escape(&host.remark),
        host_change = html_escape(&host.host_change),
        header = html_escape(&host.header_change),
        local_proxy_field = local_proxy_field,
        proto_field = proto_field,
        auto_https_no = auto_https_no,
        auto_https_yes = auto_https_yes,
        host_name = html_escape(&host.host),
        cert = html_escape(&host.cert_file_path),
        key = html_escape(&host.key_file_path),
    )
}

fn render_tunnel_fields(registry: &Registry, session: &WebSession, kind: &str, id: u64, client_id: u64) -> String {
    let current = {
        let clients = registry.clients.lock().unwrap();
        clients
            .values()
            .flat_map(|client| client.tunnels.iter())
            .find(|tunnel| tunnel.id == id)
            .cloned()
    };
    let mut kind = current
        .as_ref()
        .map(|tunnel| tunnel.mode.clone())
        .filter(|mode| !mode.is_empty())
        .unwrap_or_else(|| kind.to_string());
    if kind.eq_ignore_ascii_case("all") {
        kind = "tcp".to_string();
    }
    let tunnel = current.unwrap_or_default();
    let port_value = if tunnel.ports.is_empty() {
        tunnel.server_port.to_string()
    } else {
        tunnel.ports.clone()
    };
    let client_field = render_client_binding_field(registry, session, client_id, Some(tunnel.client_vkey.as_str()));
    let local_proxy_yes = if tunnel.target.local_proxy { "selected" } else { "" };
    let local_proxy_no = if tunnel.target.local_proxy { "" } else { "selected" };
    let proto_v1 = selected(&tunnel.proto_version, "V1");
    let proto_v2 = selected(&tunnel.proto_version, "V2");
    let proto_empty = if tunnel.proto_version.trim().is_empty() { "selected" } else { "" };
    let server_ip_field = if registry.server.allow_multi_ip {
        format!(r#"<div class="form-group" id="server_ip_group"><label langtag="word-serverip"></label><input class="form-control" name="server_ip" value="{}"></div>"#, html_escape(&tunnel.server_ip))
    } else {
        String::new()
    };
    let local_proxy_field = if registry.server.allow_local_proxy {
        format!(r#"<div class="form-group"><label langtag="word-proxytolocal"></label><select class="form-control" name="local_proxy"><option value="0" {local_proxy_no} langtag="word-no"></option><option value="1" {local_proxy_yes} langtag="word-yes"></option></select></div>"#)
    } else {
        String::new()
    };
    let sel_tcp = selected(&kind, "tcp");
    let sel_udp = selected(&kind, "udp");
    let sel_http = selected(&kind, "httpProxy");
    let sel_socks5 = selected(&kind, "socks5");
    let sel_secret = selected(&kind, "secret");
    let sel_p2p = selected(&kind, "p2p");

    let mut fields = format!(
        r#"<div class="form-group"><label langtag="word-scheme"></label><select class="form-control" name="type" id="scheme_type"><option value="tcp" {sel_tcp} langtag="scheme-tcp"></option><option value="udp" {sel_udp} langtag="scheme-udp"></option><option value="httpProxy" {sel_http} langtag="scheme-httpproxy"></option><option value="socks5" {sel_socks5} langtag="scheme-socks5"></option><option value="secret" {sel_secret} langtag="scheme-secret"></option><option value="p2p" {sel_p2p} langtag="scheme-p2p"></option></select></div>{client_field}<div class="form-group"><label langtag="word-remark"></label><input class="form-control" name="remark" value="{remark}"></div>{server_ip_field}<div class="form-group" id="port_group"><label langtag="word-port"></label><input class="form-control" name="port" value="{port}" placeholder="留空自动生成"></div><div class="form-group" id="local_proxy_group"><label langtag="word-proxytolocal"></label><select class="form-control" name="local_proxy"><option value="0" {local_proxy_no} langtag="word-no"></option><option value="1" {local_proxy_yes} langtag="word-yes"></option></select></div><div class="form-group" id="target_group"><label langtag="word-target"></label><textarea class="form-control" rows="4" name="target">{target}</textarea></div><div class="form-group" id="password_group"><label langtag="word-identificationkey"></label><input class="form-control" name="password" value="{password}"></div><div class="form-group" id="local_path_group"><label>Local path</label><input class="form-control" name="local_path" value="{local_path}"></div><div class="form-group" id="strip_pre_group"><label>Strip prefix</label><input class="form-control" name="strip_pre" value="{strip_pre}"></div><div class="form-group" id="proto_group"><label>Proxy Protocol Version</label><select class="form-control" name="proto_version"><option value="" {proto_empty}></option><option value="V1" {proto_v1}>V1</option><option value="V2" {proto_v2}>V2</option></select></div><script>(function(){{function sync(){{var mode=$('#scheme_type').val();$('#local_proxy_group').toggle(mode==='tcp'||mode==='udp');$('#target_group').toggle(mode!=='socks5'&&mode!=='httpProxy');$('#password_group').toggle(mode==='secret'||mode==='p2p');$('#local_path_group').toggle(mode==='file');$('#strip_pre_group').toggle(mode==='file');$('#proto_group').toggle(mode==='tcp');$('#server_ip_group').toggle(mode!=='socks5'&&mode!=='httpProxy');}}$('#scheme_type').on('change',sync);sync();}})();</script>"#,
        sel_tcp = sel_tcp,
        sel_udp = sel_udp,
        sel_http = sel_http,
        sel_socks5 = sel_socks5,
        sel_secret = sel_secret,
        sel_p2p = sel_p2p,
        client_field = client_field,
        server_ip_field = server_ip_field,
        port = html_escape(&port_value),
        local_proxy_no = local_proxy_no,
        local_proxy_yes = local_proxy_yes,
        target = html_escape(&tunnel.target.target_str),
        remark = html_escape(&tunnel.remark),
        password = html_escape(&tunnel.password),
        local_path = html_escape(&tunnel.local_path),
        strip_pre = html_escape(&tunnel.strip_pre),
        proto_empty = proto_empty,
        proto_v1 = proto_v1,
        proto_v2 = proto_v2,
    );
    fields
}

fn render_client_binding_field(
    registry: &Registry,
    session: &WebSession,
    client_id: u64,
    current_vkey: Option<&str>,
) -> String {
    if !session.is_admin {
        let resolved = session.client_id.unwrap_or(client_id);
        return format!(r#"<input type="hidden" name="client_id" value="{resolved}">"#);
    }

    let clients = registry.clients.lock().unwrap();
    let mut items: Vec<_> = clients.values().collect();
    items.sort_by_key(|client| client.id);
    let resolved_id = if client_id != 0 {
        client_id
    } else {
        current_vkey
            .and_then(|vkey| items.iter().find(|client| client.common.vkey == vkey).map(|client| client.id))
            .unwrap_or(0)
    };
    let mut options = String::new();
    for client in items {
        let selected = if client.id == resolved_id { "selected" } else { "" };
        options.push_str(&format!(
            r#"<option value="{}" {}>{}-{} </option>"#,
            client.id,
            selected,
            client.id,
            html_escape(&client.common.client.remark)
        ));
    }
    format!(r#"<div class="form-group"><label langtag="word-clientid"></label><select class="form-control" name="client_id">{options}</select></div>"#)
}

fn selected(current: &str, expected: &str) -> &'static str {
    if current.eq_ignore_ascii_case(expected) {
        "selected"
    } else {
        ""
    }
}

async fn handle_post_mutation(
    State(r): State<Arc<Registry>>,
    j: CookieJar,
    OriginalUri(uri): OriginalUri,
    Form(mut p): Form<HashMap<String, String>>,
) -> Response {
    let Some(session) = current_session(&r, &j) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let path = uri.path().to_string();
    let base = &r.server.web_base_url;
    let clean_path = if !base.is_empty() && path.starts_with(base) {
        &path[base.len()..]
    } else {
        &path
    };
    crate::log_info!("web", "mutation request path={} params={}", clean_path, p.len());
    if let Err(message) = authorize_mutation(&r, &session, clean_path, &mut p) {
        crate::log_warn!("web", "mutation denied path={} reason={}", clean_path, message);
        return axum::Json(serde_json::json!({"status":0, "msg":message})).into_response();
    }
    let res = match clean_path {
        "/client/changestatus" => server::mutate_client_status(&r, &p),
        "/client/del" => server::mutate_client_delete(&r, &p),
        "/client/add" => server::mutate_client_add(&r, &p),
        "/client/edit" => server::mutate_client_edit(&r, &p),
        "/index/stop" => server::mutate_tunnel_status(&r, &p, false),
        "/index/start" => server::mutate_tunnel_status(&r, &p, true),
        "/index/del" => server::mutate_tunnel_delete(&r, &p),
        "/index/copy" => server::mutate_tunnel_copy(&r, &p),
        "/index/add" => server::mutate_tunnel_add(&r, &p),
        "/index/edit" => server::mutate_tunnel_edit(&r, &p),
        "/index/hoststop" => server::mutate_host_status(&r, &p, true),
        "/index/hoststart" => server::mutate_host_status(&r, &p, false),
        "/index/delhost" => server::mutate_host_delete(&r, &p),
        "/index/addhost" => server::mutate_host_add(&r, &p),
        "/index/edithost" => server::mutate_host_edit(&r, &p),
        "/global/save" => server::mutate_global_save(&r, &p),
        _ => {
            crate::log_warn!("web", "Unhandled mutation path: {}", clean_path);
            serde_json::json!({"status":0, "msg":"unsupported"}).to_string()
        }
    };
    crate::log_info!("web", "mutation response path={} body={}", clean_path, res);
    axum::Json(serde_json::from_str::<Value>(&res).unwrap()).into_response()
}

fn authorize_mutation(
    registry: &Registry,
    session: &WebSession,
    path: &str,
    params: &mut HashMap<String, String>,
) -> Result<(), &'static str> {
    if session.is_admin {
        return Ok(());
    }
    let Some(client_id) = session.client_id else {
        return Err("permission denied");
    };

    match path {
        "/client/add" | "/client/del" | "/client/changestatus" | "/global/save" => {
            Err("permission denied")
        }
        "/client/edit" => {
            let id = params
                .get("id")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            if id != client_id {
                return Err("permission denied");
            }
            params.remove("vkey");
            if !registry.server.allow_user_change_username {
                params.remove("web_username");
            }
            Ok(())
        }
        "/index/add" | "/index/addhost" => {
            params.insert("client_id".to_string(), client_id.to_string());
            Ok(())
        }
        "/index/stop" | "/index/start" | "/index/del" | "/index/copy" | "/index/edit" => {
            let id = params
                .get("id")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            if tunnel_belongs_to_client(registry, id, client_id) {
                params.insert("client_id".to_string(), client_id.to_string());
                Ok(())
            } else {
                Err("permission denied")
            }
        }
        "/index/hoststop" | "/index/hoststart" | "/index/delhost" | "/index/edithost" => {
            let id = params
                .get("id")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            if host_belongs_to_client(registry, id, client_id) {
                params.insert("client_id".to_string(), client_id.to_string());
                Ok(())
            } else {
                Err("permission denied")
            }
        }
        _ => Ok(()),
    }
}

fn load_view(name: &str, vars: &HashMap<String, String>) -> String {
    let path = get_web_root().join("views").join(name);
    let mut content = fs::read_to_string(&path)
        .unwrap_or_else(|_| format!("View {} not found at {}", name, path.display()));
    for (k, v) in vars {
        content = content.replace(&format!("{{{{{}}}}}", k), v);
    }
    content
}

fn render_login(r: &Registry, message: &str) -> String {
    let register_link = if r.server.allow_user_register {
        format!(
            r#"<p class="text-muted text-center"><small langtag="info-noaccount"></small></p><a class="btn btn-sm btn-white btn-block" href="{}/login/register" langtag="word-register"></a>"#,
            r.server.web_base_url
        )
    } else {
        String::new()
    };
    let captcha_block = server::login_captcha_block(r);
    load_view(
        "login.html",
        &HashMap::from([
            ("base".to_string(), r.server.web_base_url.clone()),
            ("message".to_string(), message.to_string()),
            ("register_link".to_string(), register_link),
            ("captcha_block".to_string(), captcha_block),
        ]),
    )
}

fn render_layout(
    r: &Registry,
    session: &WebSession,
    menu: &str,
    title: &str,
    body: &str,
) -> String {
    let base = &r.server.web_base_url;
    let mut nav = String::new();
    let items = [
        ("index", "/", "fa-tachometer-alt", "word-dashboard"),
        ("client", "/client/list", "fa-desktop", "word-client"),
        ("host", "/index/hostlist", "fa-globe", "scheme-host"),
        ("tcp", "/index/tcp", "fa-retweet", "scheme-tcp"),
        ("udp", "/index/udp", "fa-random", "scheme-udp"),
        ("http", "/index/http", "fa-server", "scheme-httpproxy"),
        ("socks5", "/index/socks5", "fa-layer-group", "scheme-socks5"),
        ("secret", "/index/secret", "fa-low-vision", "scheme-secret"),
        ("p2p", "/index/p2p", "fa-exchange-alt", "scheme-p2p"),
        ("file", "/index/file", "fa-briefcase", "scheme-file"),
        ("global", "/global/index", "fa-cog", "word-globalparam"),
    ];
    for (key, href, icon, tag) in items {
        if !session.is_admin && key == "global" {
            continue;
        }
        let active = if key == menu { "active" } else { "" };
        // Force refresh by adding a cache-busting timestamp or just using a standard navigation that forces reload
        nav.push_str(&format!(
            r#"<li class="{active}"><a href="{base}{href}"><i class="fa {icon} fa-lg"></i><span class="nav-label" langtag="{tag}"></span></a></li>"#
        ));
    }
    load_view(
        "layout.html",
        &HashMap::from([
            ("base".to_string(), base.clone()),
            ("title".to_string(), title.to_string()),
            ("nav".to_string(), nav),
            ("body".to_string(), body.to_string()),
            ("username".to_string(), html_escape(&session.username)),
        ]),
    )
}

fn render_dashboard(r: &Registry, session: &WebSession) -> String {
    load_view(
        "dashboard.html",
        &HashMap::from([(
            "data".to_string(),
            html_escape(&server::dashboard_json_scoped(r, scope_client_id(session))),
        )]),
    )
}

fn html_escape(s: &str) -> String {
    s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&#39;")
}
