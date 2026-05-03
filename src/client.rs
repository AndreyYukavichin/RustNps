use crate::config::load_client_config;
use crate::model::{ClientRuntimeConfig, LocalServer};
use crate::mux::MuxSession;
use crate::protocol::{
    ok, read_blob, read_message, write_blob, write_message, BridgeHello, Link, LinkKind,
    ServerMessage,
};
use crate::relay::{
    copy_bidirectional, read_http_head, wrap_client_transport, write_http_response, RelayStream,
};
use crate::{CORE_VERSION, VERSION};
use std::env;
use std::fs;
use std::io;
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
struct Args {
    server: String,
    vkey: String,
    config: String,
    conn_type: String,
    password: String,
    target: String,
    local_port: u16,
    local_type: String,
    register_hours: u32,
    command: String,
    tls_enable: bool,
    console_log_level: String,
    log_path: String,
}

pub fn entry() -> io::Result<()> {
    let args = parse_args();
    if args.command == "help" {
        print_npc_help();
        return Ok(());
    }
    if args.command == "version" {
        println!("RustNps npc version {VERSION}, core {CORE_VERSION}");
        return Ok(());
    }
    crate::logging::init_console_from_text(&args.console_log_level);
    if !args.log_path.is_empty() {
        crate::log_info!(
            "npc",
            "log_path is configured as {}, console output remains enabled in RustNps",
            args.log_path
        );
    }
    if args.command == "register" {
        return register_ip(&args);
    }
    if args.command == "status" || args.command == "nat" {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "npc {} is not implemented in RustNps yet; run npc --help for supported commands",
                args.command
            ),
        ));
    }
    run(args)
}

fn run(args: Args) -> io::Result<()> {
    if !args.password.is_empty() {
        crate::log_info!(
            "npc",
            "start local {} visitor, server={}, local_port={}, target={}",
            args.local_type,
            args.server,
            args.local_port,
            args.target
        );
        let local = LocalServer {
            kind: if args.local_type.is_empty() {
                "p2p".to_string()
            } else {
                args.local_type.clone()
            },
            ip: "127.0.0.1".to_string(),
            port: args.local_port,
            password: args.password.clone(),
            target: args.target.clone(),
        };
        start_local_server(args.server.clone(), local);
        park_forever();
        return Ok(());
    }

    if !args.config.is_empty() || args.server.is_empty() || args.vkey.is_empty() {
        let path = if args.config.is_empty() {
            default_client_conf()
        } else {
            args.config.clone()
        };
        if Path::new(&path).exists() {
            let config = load_client_config(&path)?;
            crate::log_info!("npc", "Loading configuration file {path} successfully");
            crate::log_info!(
                "npc",
                "the version of client is {VERSION}, the core version of client is {CORE_VERSION},tls enable is {}",
                config.common.tls_enable
            );
            send_config(&config)?;
            for local in config.local_servers.clone() {
                start_local_server(config.common.server_addr.clone(), local);
            }
            control_loop_with_reconnect(config)?;
            return Ok(());
        }
    }

    if args.server.is_empty() || args.vkey.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing -server/-vkey or -config",
        ));
    }

    let mut config = ClientRuntimeConfig::default();
    config.common.server_addr = args.server;
    config.common.vkey = args.vkey.clone();
    config.common.conn_type = if args.conn_type.is_empty() {
        "tcp".to_string()
    } else {
        args.conn_type
    };
    config.common.tls_enable = args.tls_enable;
    config.common.client.verify_key = args.vkey;
    crate::log_info!(
        "npc",
        "the version of client is {VERSION}, the core version of client is {CORE_VERSION},tls enable is {}",
        config.common.tls_enable
    );
    control_loop_with_reconnect(config)
}

fn control_loop_with_reconnect(config: ClientRuntimeConfig) -> io::Result<()> {
    loop {
        match control_loop(config.clone()) {
            Ok(()) => return Ok(()),
            Err(err) => {

                if !config.common.auto_reconnection {
                    crate::log_error!("npc", "The connection server failed, error {err}");
                    return Err(err);
                }
                crate::log_error!(
                    "npc",
                    "The connection server failed and will be reconnected in five seconds, error {err}"
                );
                crate::log_info!("npc", "Reconnecting...");
                thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

fn send_config(config: &ClientRuntimeConfig) -> io::Result<()> {
    let mut stream = TcpStream::connect(&config.common.server_addr)?;
    let hello = BridgeHello::Config {
        vkey: config.common.vkey.clone(),
        version: VERSION.to_string(),
        core_version: CORE_VERSION.to_string(),
        config: config.clone(),
    };
    write_message(&mut stream, &hello)?;
    match read_message::<ServerMessage>(&mut stream)? {
        ServerMessage::Ok { message } => {
            crate::log_info!("npc", "config accepted: {message}");
            if config.common.client.web_username.is_empty()
                || config.common.client.web_password.is_empty()
            {
                crate::log_notice!(
                    "npc",
                    "web access login username:user password:{}",
                    config.common.vkey
                );
            } else {
                crate::log_notice!(
                    "npc",
                    "web access login username:{} password:{}",
                    config.common.client.web_username,
                    config.common.client.web_password
                );
            }
            Ok(())
        }
        ServerMessage::Error { message } => Err(classify_server_error(message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected config response: {other:?}"),
        )),
    }
}

fn control_loop(config: ClientRuntimeConfig) -> io::Result<()> {
    let mut stream = TcpStream::connect(&config.common.server_addr)?;
    let mux_session = Arc::new(Mutex::new(None));
    let hello = BridgeHello::Control {
        vkey: config.common.vkey.clone(),
        version: VERSION.to_string(),
        core_version: CORE_VERSION.to_string(),
    };
    write_message(&mut stream, &hello)?;
    match read_message::<ServerMessage>(&mut stream)? {
        ServerMessage::Ok { .. } => {}
        ServerMessage::Error { message } => {
            return Err(classify_server_error(message))
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected control response: {other:?}"),
            ))
        }
    }
    crate::log_info!(
        "npc",
        "Successful connection with server {}",
        config.common.server_addr
    );
    if let Err(err) = ensure_mux_session(&config, &mux_session) {
        crate::log_warn!("npc", "mux session unavailable, fallback to raw data links: {err}");
    }

    loop {
        let msg: ServerMessage = read_message(&mut stream)?;
        match msg {
            ServerMessage::Open { link_id, link } => {
                let config = config.clone();
                let mux_session = Arc::clone(&mux_session);
                thread::spawn(move || {
                    if let Err(err) = handle_open(config, Arc::clone(&mux_session), link_id, link) {
                        crate::log_warn!("npc", "open link failed: {err}");
                    }
                });
            }
            ServerMessage::Ping => {
                write_message(&mut stream, &ok("pong"))?;
            }
            ServerMessage::Stop { reason } => {
                return Err(classify_server_error(reason));
            }
            ServerMessage::Ok { .. } | ServerMessage::Error { .. } => {}
        }
    }
}

fn classify_server_error(message: String) -> io::Error {
    if is_invalid_vkey_message(&message) {
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Invalid verification key, connection failed, please reconfigure the vkey parameter.",
        );
    }
    if is_fatal_server_message(&message) {
        return io::Error::new(io::ErrorKind::PermissionDenied, message);
    }
    io::Error::new(io::ErrorKind::Other, message)
}

fn is_invalid_vkey_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("invalid verification key")
        || lower.contains("validation key")
        || lower.contains("key incorrect")
        || message.contains("密钥错误")
}

fn is_fatal_server_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    is_invalid_vkey_message(message)
        || lower.contains("client disabled")
        || lower.contains("client deleted")
        || lower.contains("config connection is disabled")
}

fn handle_open(
    config: ClientRuntimeConfig,
    mux_session: Arc<Mutex<Option<Arc<MuxSession>>>>,
    link_id: u64,
    link: Link,
) -> io::Result<()> {
    if let Ok(stream) = open_mux_stream(&config, &mux_session, link_id) {
        return serve_link(Box::new(stream), link);
    }

    let mut stream = TcpStream::connect(&config.common.server_addr)?;
    let hello = BridgeHello::Data {
        vkey: config.common.vkey.clone(),
        link_id,
    };
    write_message(&mut stream, &hello)?;
    match read_message::<ServerMessage>(&mut stream)? {
        ServerMessage::Ok { .. } => serve_link(Box::new(stream), link),
        ServerMessage::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected data response: {other:?}"),
        )),
    }
}

fn serve_link(stream: Box<dyn RelayStream>, link: Link) -> io::Result<()> {
    let stream = wrap_client_transport(stream, link.crypt, link.compress)?;
    match link.kind {
        LinkKind::Udp => serve_udp(stream, link),
        LinkKind::File => serve_file(stream, link),
        LinkKind::Tcp | LinkKind::Http | LinkKind::Secret | LinkKind::P2p => {
            let target = TcpStream::connect(&link.target)?;
            copy_bidirectional(stream, target)
        }
    }
}

fn serve_udp(mut stream: Box<dyn RelayStream>, link: Link) -> io::Result<()> {
    let packet = read_blob(&mut stream)?;
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(10)))?;
    socket.send_to(&packet, &link.target)?;
    let mut buf = vec![0_u8; 64 * 1024];
    let (n, _) = socket.recv_from(&mut buf)?;
    write_blob(&mut stream, &buf[..n])
}

fn serve_file(mut stream: Box<dyn RelayStream>, link: Link) -> io::Result<()> {
    let Some(file) = link.file else {
        return write_http_response(
            &mut stream,
            "500 Internal Server Error",
            "text/plain",
            b"missing file route",
        );
    };

    let head = read_http_head(&mut stream, 32 * 1024)?;
    let path = parse_request_path(&head).unwrap_or_else(|| "/".to_string());
    let path = path
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_start_matches('/')
        .to_string();
    let strip = file.strip_pre.trim_start_matches('/');
    let relative = if !strip.is_empty() && path.starts_with(strip) {
        path[strip.len()..].trim_start_matches('/').to_string()
    } else {
        path
    };
    let root = PathBuf::from(&file.local_path);
    let Some(full_path) = safe_join(&root, &relative) else {
        return write_http_response(&mut stream, "403 Forbidden", "text/plain", b"forbidden");
    };

    if full_path.is_dir() {
        return serve_dir(stream.as_mut(), &root, &full_path);
    }

    match fs::read(&full_path) {
        Ok(bytes) => write_http_response(
            &mut stream,
            "200 OK",
            content_type(&full_path),
            bytes.as_slice(),
        ),
        Err(_) => write_http_response(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

fn serve_dir(stream: &mut dyn RelayStream, root: &Path, dir: &Path) -> io::Result<()> {
    let mut body = String::from("<!doctype html><meta charset=\"utf-8\"><ul>");
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        body.push_str(&format!(
            "<li><a href=\"/{}\">{}</a></li>",
            html_escape(&rel.to_string_lossy()),
            html_escape(&name)
        ));
    }
    body.push_str("</ul>");
    write_http_response(
        stream,
        "200 OK",
        "text/html; charset=utf-8",
        body.as_bytes(),
    )
}

fn parse_request_path(head: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    let first = text.lines().next()?;
    first.split_whitespace().nth(1).map(ToOwned::to_owned)
}

fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    Some(root.join(clean))
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn ensure_mux_session(
    config: &ClientRuntimeConfig,
    holder: &Arc<Mutex<Option<Arc<MuxSession>>>>,
) -> io::Result<Arc<MuxSession>> {
    if let Some(session) = holder.lock().unwrap().as_ref().cloned() {
        if !session.is_closed() {
            return Ok(session);
        }
    }
    let mut stream = TcpStream::connect(&config.common.server_addr)?;
    let hello = BridgeHello::Mux {
        vkey: config.common.vkey.clone(),
        version: VERSION.to_string(),
        core_version: CORE_VERSION.to_string(),
    };
    write_message(&mut stream, &hello)?;
    match read_message::<ServerMessage>(&mut stream)? {
        ServerMessage::Ok { .. } => {
            let session = MuxSession::new(stream)?;
            holder.lock().unwrap().replace(Arc::clone(&session));
            Ok(session)
        }
        ServerMessage::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected mux response: {other:?}"),
        )),
    }
}

fn open_mux_stream(
    config: &ClientRuntimeConfig,
    holder: &Arc<Mutex<Option<Arc<MuxSession>>>>,
    link_id: u64,
) -> io::Result<crate::mux::MuxStream> {
    let session = ensure_mux_session(config, holder)?;
    match session.open_stream(link_id) {
        Ok(stream) => Ok(stream),
        Err(err) => {
            holder.lock().unwrap().take();
            Err(err)
        }
    }
}

fn start_local_server(server_addr: String, local: LocalServer) {
    if local.port == 0 || local.password.is_empty() {
        crate::log_warn!(
            "npc",
            "skip local {} server with empty port/password",
            local.kind
        );
        return;
    }
    thread::spawn(move || {
        let bind = format!("{}:{}", empty_to(&local.ip, "127.0.0.1"), local.port);
        let listener = match TcpListener::bind(&bind) {
            Ok(listener) => listener,
            Err(err) => {
                crate::log_error!("npc", "local {} bind {bind} failed: {err}", local.kind);
                return;
            }
        };
        crate::log_info!(
            "npc",
            "successful start-up of local {} monitoring, port {}",
            local.kind,
            local.port
        );
        crate::log_debug!("npc", "local {} visitor listening on {bind}", local.kind);
        for incoming in listener.incoming() {
            let inbound = match incoming {
                Ok(s) => s,
                Err(err) => {
                    crate::log_warn!("npc", "local accept failed: {err}");
                    continue;
                }
            };
            let server_addr = server_addr.clone();
            let local = local.clone();
            thread::spawn(move || {
                if let Err(err) = connect_secret_visitor(server_addr, local, inbound) {
                    crate::log_warn!("npc", "local visitor failed: {err}");
                }
            });
        }
    });
}

fn connect_secret_visitor(
    server_addr: String,
    local: LocalServer,
    inbound: TcpStream,
) -> io::Result<()> {
    let mut bridge = TcpStream::connect(server_addr)?;
    let hello = if local.kind.eq_ignore_ascii_case("p2p") {
        BridgeHello::P2pVisitor {
            password: local.password,
            target: local.target,
        }
    } else {
        BridgeHello::SecretVisitor {
            password: local.password,
            target: local.target,
        }
    };
    write_message(&mut bridge, &hello)?;
    match read_message::<ServerMessage>(&mut bridge)? {
        ServerMessage::Ok { .. } => copy_bidirectional(inbound, bridge),
        ServerMessage::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected visitor response: {other:?}"),
        )),
    }
}

fn register_ip(args: &Args) -> io::Result<()> {
    let mut stream = TcpStream::connect(&args.server)?;
    let hello = BridgeHello::RegisterIp {
        vkey: args.vkey.clone(),
        hours: args.register_hours,
    };
    write_message(&mut stream, &hello)?;
    match read_message::<ServerMessage>(&mut stream)? {
        ServerMessage::Ok { message } => {
            crate::log_info!(
                "npc",
                "Successful ip registration for local public network, the validity period is {} hours. server said: {message}",
                args.register_hours
            );
            Ok(())
        }
        ServerMessage::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected register response: {other:?}"),
        )),
    }
}

fn parse_args() -> Args {
    let raw: Vec<String> = env::args().skip(1).collect();
    let mut args = Args {
        conn_type: "tcp".to_string(),
        local_port: 2000,
        local_type: "p2p".to_string(),
        register_hours: 2,
        console_log_level: "info".to_string(),
        ..Args::default()
    };

    if raw
        .iter()
        .any(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        args.command = "help".to_string();
        return args;
    }
    if raw.iter().any(|a| a == "-version" || a == "--version") {
        args.command = "version".to_string();
        return args;
    }
    if let Some(first) = raw.first() {
        if matches!(first.as_str(), "register" | "status" | "nat") {
            args.command = first.clone();
        }
    }

    for (idx, item) in raw.iter().enumerate() {
        let next = raw.get(idx + 1).cloned().unwrap_or_default();
        assign_arg(item, "-server", &next, &mut args.server);
        assign_arg(item, "--server", &next, &mut args.server);
        assign_arg(item, "-vkey", &next, &mut args.vkey);
        assign_arg(item, "--vkey", &next, &mut args.vkey);
        assign_arg(item, "-config", &next, &mut args.config);
        assign_arg(item, "--config", &next, &mut args.config);
        assign_arg(item, "-type", &next, &mut args.conn_type);
        assign_arg(item, "--type", &next, &mut args.conn_type);
        assign_arg(item, "-password", &next, &mut args.password);
        assign_arg(item, "--password", &next, &mut args.password);
        assign_arg(item, "-target", &next, &mut args.target);
        assign_arg(item, "--target", &next, &mut args.target);
        assign_arg(item, "-local_type", &next, &mut args.local_type);
        assign_arg(item, "--local-type", &next, &mut args.local_type);
        assign_arg(item, "-console_log_level", &next, &mut args.console_log_level);
        assign_arg(item, "--console-log-level", &next, &mut args.console_log_level);
        assign_arg(item, "-log_level", &next, &mut args.console_log_level);
        assign_arg(item, "--log-level", &next, &mut args.console_log_level);
        assign_arg(item, "-log_path", &next, &mut args.log_path);
        assign_arg(item, "--log-path", &next, &mut args.log_path);

        if let Some(value) =
            value_of(item, "-local_port").or_else(|| value_of(item, "--local-port"))
        {
            args.local_port = value.parse().unwrap_or(args.local_port);
        }
        if item == "-local_port" || item == "--local-port" {
            args.local_port = next.parse().unwrap_or(args.local_port);
        }
        if let Some(value) = value_of(item, "-time").or_else(|| value_of(item, "--time")) {
            args.register_hours = value.parse().unwrap_or(args.register_hours);
        }
        if item == "-time" || item == "--time" {
            args.register_hours = next.parse().unwrap_or(args.register_hours);
        }
        if let Some(value) =
            value_of(item, "-tls_enable").or_else(|| value_of(item, "--tls-enable"))
        {
            args.tls_enable = parse_bool(&value);
        }
        if item == "-tls_enable" || item == "--tls-enable" {
            args.tls_enable = next.is_empty() || next.starts_with('-') || parse_bool(&next);
        }
        if item == "-debug" || item == "--debug" {
            args.console_log_level = if next.is_empty() || next.starts_with('-') || parse_bool(&next)
            {
                "debug".to_string()
            } else {
                "info".to_string()
            };
        }
        if let Some(value) = value_of(item, "-debug").or_else(|| value_of(item, "--debug")) {
            args.console_log_level = if parse_bool(&value) {
                "debug".to_string()
            } else {
                "info".to_string()
            };
        }
    }
    args
}

fn assign_arg(item: &str, name: &str, next: &str, slot: &mut String) {
    if let Some(value) = value_of(item, name) {
        *slot = value;
    } else if item == name {
        *slot = next.to_string();
    }
}

fn value_of(item: &str, name: &str) -> Option<String> {
    item.strip_prefix(&format!("{name}="))
        .map(ToOwned::to_owned)
}

fn parse_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

fn print_npc_help() {
    println!(
        r#"RustNps npc {VERSION} (core {CORE_VERSION})

Usage:
  npc [options]
  npc register [options]
  npc -h | --help
  npc -version | --version

Connection options:
  -server <ip:port>       Server bridge address, for example 127.0.0.1:8024
  -vkey <key>             Client verify key from the nps web panel
  -config <file>          Path to npc.conf. Default: conf/npc.conf
  -type <tcp|kcp>         Bridge connection type. Current RustNps runtime supports tcp
  -tls_enable=true        Enable TLS bridge connection flag in config/CLI metadata
    --console-log-level <info|debug>
                                                    Console log level. Default: info.
                                                    debug prints detailed troubleshooting logs.
  -log_path <file>        Reserved for service/file logging compatibility
    -debug=<true|false>     Legacy alias for --console-log-level debug/info

Visitor / secret / p2p options:
  -password <password>    Start local visitor mode with a secret/p2p password
  -target <addr>          Optional visitor target
  -local_port <port>      Local visitor listen port. Default: 2000
  -local_type <p2p|secret>  Local visitor type. Default: p2p

Commands:
  register                Register current public IP with server ip_limit
    -server <ip:port> -vkey <key> [-time <hours>]
  status, nat             Go npc compatibility commands, not implemented yet in RustNps

Examples:
  npc -server 127.0.0.1:8024 -vkey 123
  npc -config conf/npc.conf
    npc --console-log-level debug -server 127.0.0.1:8024 -vkey 123
  npc register -server 127.0.0.1:8024 -vkey 123 -time 2
  npc -server 127.0.0.1:8024 -password ssh2 -local_type secret -local_port 2001"#
    );
}

fn empty_to<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.trim().is_empty() {
        default
    } else {
        value
    }
}

fn default_client_conf() -> String {
    for candidate in ["conf/npc.conf", "../nps/conf/npc.conf", "nps/conf/npc.conf"] {
        if fs::metadata(candidate).is_ok() {
            return candidate.to_string();
        }
    }
    "conf/npc.conf".to_string()
}

fn park_forever() {
    loop {
        thread::park();
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
