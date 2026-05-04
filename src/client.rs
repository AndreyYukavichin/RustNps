use crate::config::load_client_config;
use crate::model::{ClientRuntimeConfig, LocalServer};
use crate::mux::MuxSession;
use crate::protocol::{
    ok, read_blob, read_message, write_blob, write_message, BridgeHello, Link, LinkKind,
    ServerMessage,
};
use crate::relay::{
    copy_bidirectional, http_header_value, parse_http_request_line, parse_http_status_code,
    read_http_head,
    wrap_client_transport, write_http_response, RelayStream,
};
use crate::{CORE_VERSION, VERSION};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
    let start_cmd = env::args().skip(1).collect::<Vec<_>>().join(" ");
    if args.command == "help" {
        print_npc_help();
        return Ok(());
    }
    if args.command == "version" {
        println!("RustNps npc version {VERSION}, core {CORE_VERSION}");
        return Ok(());
    }
    crate::logging::init_console_from_text(&args.console_log_level);
    if !start_cmd.is_empty() {
        crate::log_info!("npc", "start cmd:{start_cmd}");
    }
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
            let config = match load_client_config(&path) {
                Ok(config) => config,
                Err(err) => {
                    crate::log_error!("npc", "Config file {} loading error {}", path, err);
                    return Err(err);
                }
            };
            crate::log_info!("npc", "Loading configuration file {path} successfully");
            crate::log_info!(
                "npc",
                "the version of client is {VERSION}, the core version of client is {CORE_VERSION},tls enable is {}",
                config.common.tls_enable
            );
            send_config(&config)?;
            start_health_monitor(config.clone());
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
    start_health_monitor(config.clone());
    control_loop_with_reconnect(config)
}

fn start_health_monitor(config: ClientRuntimeConfig) {
    if config.healths.is_empty() {
        return;
    }

    let server_addr = config.common.server_addr.clone();
    let vkey = config.common.vkey.clone();
    let healths = config.healths.clone();
    crate::log_info!("npc", "health monitor enabled, rules={}", healths.len());

    thread::spawn(move || {
        let mut failure_counts: HashMap<usize, HashMap<String, u32>> = HashMap::new();
        let mut next_checks: HashMap<usize, Instant> = HashMap::new();

        loop {
            let now = Instant::now();
            let mut next_wait = Duration::from_secs(1);

            for (index, health) in healths.iter().enumerate() {
                if health.interval_secs == 0 || health.timeout_secs == 0 || health.max_failed == 0 {
                    continue;
                }

                let interval = Duration::from_secs(health.interval_secs.max(1));
                let next_check = next_checks.entry(index).or_insert(now);
                if now >= *next_check {
                    let per_health = failure_counts.entry(index).or_default();
                    for target in &health.targets {
                        let ok = probe_health_target(health, target);
                        let failures = per_health.entry(target.clone()).or_insert(0);
                        if ok {
                            if *failures >= health.max_failed {
                                if let Err(err) = report_health_change(&server_addr, &vkey, target, true) {
                                    crate::log_trace!(
                                        "npc",
                                        "health restore report failed vkey={} target={} err={}",
                                        vkey,
                                        target,
                                        err
                                    );
                                    continue;
                                }
                            }
                            *failures = 0;
                        } else {
                            *failures = failures.saturating_add(1);
                            if *failures % health.max_failed == 0 {
                                if let Err(err) = report_health_change(&server_addr, &vkey, target, false) {
                                    crate::log_trace!(
                                        "npc",
                                        "health remove report failed vkey={} target={} err={}",
                                        vkey,
                                        target,
                                        err
                                    );
                                }
                            }
                        }
                    }
                    *next_check = now + interval;
                }

                let wait = next_check.saturating_duration_since(now);
                if wait < next_wait {
                    next_wait = wait;
                }
            }

            thread::sleep(next_wait.max(Duration::from_millis(100)));
        }
    });
}

fn report_health_change(
    server_addr: &str,
    vkey: &str,
    target: &str,
    status: bool,
) -> io::Result<()> {
    let mut stream = TcpStream::connect(server_addr)?;
    let hello = BridgeHello::Health {
        vkey: vkey.to_string(),
        target: target.to_string(),
        status,
    };
    write_message(&mut stream, &hello)?;
    match read_message::<ServerMessage>(&mut stream)? {
        ServerMessage::Ok { .. } => Ok(()),
        ServerMessage::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected health response: {other:?}"),
        )),
    }
}

fn probe_health_target(health: &crate::model::HealthCheck, target: &str) -> bool {
    let timeout = Duration::from_secs(health.timeout_secs.max(1));
    match health.kind.to_ascii_lowercase().as_str() {
        "http" => probe_http_health_target(target, &health.http_url, timeout),
        _ => probe_tcp_health_target(target, timeout),
    }
}

fn probe_tcp_health_target(target: &str, timeout: Duration) -> bool {
    let Ok(mut addrs) = target.to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

fn probe_http_health_target(target: &str, http_url: &str, timeout: Duration) -> bool {
    let Ok(mut addrs) = target.to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let path = if http_url.starts_with('/') {
        http_url.to_string()
    } else {
        format!("/{http_url}")
    };
    let request = format!("GET {path} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::new();
    if stream.read_to_end(&mut response).is_err() {
        return false;
    }
    parse_http_status_code(&response) == Some(200)
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
                crate::log_info!("npc", "accpet error,the conn has closed");
                crate::log_info!(
                    "npc",
                    "Client closed! It will be reconnected in five seconds"
                );
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
        ServerMessage::Error { message } => {
            log_config_mode_server_error(&message);
            Err(classify_server_error(message))
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected config response: {other:?}"),
        )),
    }
}

fn log_config_mode_server_error(message: &str) {
    let lower = message.to_ascii_lowercase();
    if lower.contains("web_user") && lower.contains("occupied") {
        crate::log_error!("npc", "the web_user may have been occupied!");
        return;
    }
    if lower.contains("occupied")
        || lower.contains("already in use")
        || lower.contains("not allowed")
        || lower.contains("validation list")
        || lower.contains("allow")
    {
        crate::log_error!(
            "npc",
            "The server returned an error, which port or host may have been occupied or not allowed to open."
        );
        return;
    }
    crate::log_error!("npc", "{}", message);
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
            crate::log_error!("npc", "{}", message);
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
        "start vkey:{}", config.common.vkey
    );
    crate::log_info!(
        "npc",
        "Successful connection with server {}",
        config.common.server_addr
    );
    if mux_disabled() {
        crate::log_warn!("npc", "mux disabled by RUSTNPS_DISABLE_MUX, fallback to raw data links");
    } else if let Err(err) = ensure_mux_session(&config, &mux_session) {
        crate::log_warn!("npc", "mux session unavailable, fallback to raw data links: {err}");
    }

    loop {
        let msg: ServerMessage = read_message(&mut stream)?;
        match msg {
            ServerMessage::Open { link_id, link } => {
                crate::log_debug!(
                    "npc",
                    "control received open link_id={} kind={:?} target={} remote={}",
                    link_id,
                    link.kind,
                    link.target,
                    link.remote_addr
                );
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
    crate::log_debug!(
        "npc",
        "handle open begin link_id={} kind={:?} target={} remote={} mux_disabled={}",
        link_id,
        link.kind,
        link.target,
        link.remote_addr,
        mux_disabled()
    );
    match link.kind {
        LinkKind::Tcp | LinkKind::Secret | LinkKind::P2p => {
            crate::log_trace!(
                "npc",
                "new tcp connection with the goal of {}, remote address:{}",
                link.target,
                link.remote_addr
            );
        }
        LinkKind::Udp => {
            crate::log_trace!(
                "npc",
                "new udp5 connection with the goal of {}, remote address:{}",
                link.target,
                link.remote_addr
            );
        }
        LinkKind::Http => {
            crate::log_trace!(
                "npc",
                "http request, remote address:{}, target:{}",
                link.remote_addr,
                link.target
            );
        }
        LinkKind::File => {}
    }
    if let Ok(stream) = open_mux_stream(&config, &mux_session, link_id) {
        crate::log_debug!("npc", "handle open mux stream ready link_id={}", link_id);
        return serve_link(Box::new(stream), link);
    }

    crate::log_debug!(
        "npc",
        "handle open falling back to raw data link link_id={} server={}",
        link_id,
        config.common.server_addr
    );

    let mut stream = TcpStream::connect(&config.common.server_addr)?;
    let hello = BridgeHello::Data {
        vkey: config.common.vkey.clone(),
        link_id,
    };
    crate::log_debug!(
        "npc",
        "handle open sending data hello link_id={} vkey={}",
        link_id,
        config.common.vkey
    );
    write_message(&mut stream, &hello)?;
    match read_message::<ServerMessage>(&mut stream)? {
        ServerMessage::Ok { message } => {
            crate::log_debug!(
                "npc",
                "handle open data accepted link_id={} message={}",
                link_id,
                message
            );
            serve_link(Box::new(stream), link)
        }
        ServerMessage::Error { message } => Err(io::Error::new(io::ErrorKind::Other, message)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected data response: {other:?}"),
        )),
    }
}

fn serve_link(stream: Box<dyn RelayStream>, link: Link) -> io::Result<()> {
    let target_addr = link.target.clone();
    let stream = wrap_client_transport(stream, link.crypt, link.compress)?;
    match link.kind {
        LinkKind::Udp => serve_udp(stream, link),
        LinkKind::File => serve_file(stream, link),
        LinkKind::Http => {
            let mut stream = stream;
            crate::log_debug!(
                "npc",
                "serve_link http reading request head target={} remote={}",
                target_addr,
                link.remote_addr
            );
            let head = read_http_head(&mut stream, 64 * 1024)?;
            crate::log_debug!(
                "npc",
                "serve_link http request head read bytes={} target={} remote={}",
                head.len(),
                target_addr,
                link.remote_addr
            );
            if let Some((method, path, _)) = parse_http_request_line(&head) {
                let host = http_header_value(&head, "Host").unwrap_or_default();
                crate::log_trace!(
                    "npc",
                    "http request, method {}, host {}, url {}, remote address {}",
                    method,
                    host,
                    path,
                    link.remote_addr
                );
            }
            let mut target = match TcpStream::connect(&target_addr) {
                Ok(t) => t,
                Err(err) => {
                    crate::log_info!("npc", "new connect error ,the target {} refuse to connect", target_addr);
                    crate::log_error!("npc", "Accept server data error read tcp ->{}: {}, end this service", target_addr, err);
                    return Err(err);
                }
            };
            write_proxy_protocol_if_needed(&mut target, &link.proto_version, &link.remote_addr)?;
            crate::log_debug!(
                "npc",
                "serve_link http forwarding request head bytes={} target={} remote={}",
                head.len(),
                target_addr,
                link.remote_addr
            );
            if let Err(err) = target.write_all(&head) {
                crate::log_warn!(
                    "npc",
                    "serve_link http forward request head failed target={} remote={} err={}",
                    target_addr,
                    link.remote_addr,
                    err
                );
                return Err(err);
            }
            crate::log_debug!(
                "npc",
                "serve_link http request head forwarded target={} remote={} starting relay=true",
                target_addr,
                link.remote_addr
            );
            let result = copy_bidirectional(stream, target);
            match &result {
                Ok(()) => {
                    crate::log_debug!(
                        "npc",
                        "serve_link http relay finished target={} remote={} status=ok",
                        target_addr,
                        link.remote_addr
                    );
                }
                Err(err) => {
                    crate::log_warn!(
                        "npc",
                        "serve_link http relay finished target={} remote={} status=err err={}",
                        target_addr,
                        link.remote_addr,
                        err
                    );
                }
            }
            result
        }
        LinkKind::Tcp | LinkKind::Secret | LinkKind::P2p => {
            let target = match TcpStream::connect(&target_addr) {
                Ok(t) => t,
                Err(err) => {
                    crate::log_info!("npc", "new connect error ,the target {} refuse to connect", target_addr);
                    crate::log_error!("npc", "Accept server data error read tcp ->{}: {}, end this service", target_addr, err);
                    return Err(err);
                }
            };
            let mut target = target;
            write_proxy_protocol_if_needed(&mut target, &link.proto_version, &link.remote_addr)?;
            copy_bidirectional(stream, target)
        }
    }
}

fn write_proxy_protocol_if_needed(
    target: &mut TcpStream,
    proto_version: &str,
    remote_addr: &str,
) -> io::Result<()> {
    let version = proto_version.trim().to_ascii_uppercase();
    if version.is_empty() {
        return Ok(());
    }

    let remote = match remote_addr.parse::<SocketAddr>() {
        Ok(addr) => addr,
        Err(_) => return Ok(()),
    };
    let local = target.local_addr()?;

    match version.as_str() {
        "V1" => write_proxy_protocol_v1(target, remote, local),
        "V2" => write_proxy_protocol_v2(target, remote, local),
        _ => Ok(()),
    }
}

fn write_proxy_protocol_v1(
    target: &mut TcpStream,
    remote: SocketAddr,
    local: SocketAddr,
) -> io::Result<()> {
    let header = match (remote.ip(), local.ip()) {
        (IpAddr::V4(source_ip), IpAddr::V4(destination_ip)) => format!(
            "PROXY TCP4 {} {} {} {}\r\n",
            source_ip,
            destination_ip,
            remote.port(),
            local.port()
        ),
        (IpAddr::V6(source_ip), IpAddr::V6(destination_ip)) => format!(
            "PROXY TCP6 {} {} {} {}\r\n",
            source_ip,
            destination_ip,
            remote.port(),
            local.port()
        ),
        _ => return Ok(()),
    };
    target.write_all(header.as_bytes())
}

fn write_proxy_protocol_v2(
    target: &mut TcpStream,
    remote: SocketAddr,
    local: SocketAddr,
) -> io::Result<()> {
    const SIGNATURE: [u8; 12] = [0x0d, 0x0a, 0x0d, 0x0a, 0x00, 0x0d, 0x0a, 0x51, 0x55, 0x49, 0x54, 0x0a];

    let mut frame = Vec::with_capacity(52);
    frame.extend_from_slice(&SIGNATURE);
    frame.push(0x21);

    match (remote.ip(), local.ip()) {
        (IpAddr::V4(source_ip), IpAddr::V4(destination_ip)) => {
            frame.push(0x11);
            frame.extend_from_slice(&(12_u16).to_be_bytes());
            frame.extend_from_slice(&source_ip.octets());
            frame.extend_from_slice(&destination_ip.octets());
            frame.extend_from_slice(&remote.port().to_be_bytes());
            frame.extend_from_slice(&local.port().to_be_bytes());
        }
        (IpAddr::V6(source_ip), IpAddr::V6(destination_ip)) => {
            frame.push(0x21);
            frame.extend_from_slice(&(36_u16).to_be_bytes());
            frame.extend_from_slice(&source_ip.octets());
            frame.extend_from_slice(&destination_ip.octets());
            frame.extend_from_slice(&remote.port().to_be_bytes());
            frame.extend_from_slice(&local.port().to_be_bytes());
        }
        _ => return Ok(()),
    }

    target.write_all(&frame)
}

fn serve_udp(mut stream: Box<dyn RelayStream>, link: Link) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let packet = match read_blob(&mut stream) {
            Ok(packet) => packet,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(());
            }
            Err(err) => {
                crate::log_error!("npc", "read udp data from server error {}", err);
                return Err(err);
            }
        };
        if let Err(err) = socket.send_to(&packet, &link.target) {
            crate::log_error!("npc", "write data to remote {} error {}", link.target, err);
            return Err(err);
        }
        let (n, _) = match socket.recv_from(&mut buf) {
            Ok(result) => result,
            Err(err) => {
                crate::log_error!("npc", "read data from remote server error {}", err);
                return Err(err);
            }
        };
        if let Err(err) = write_blob(&mut stream, &buf[..n]) {
            crate::log_error!("npc", "write data to remote  error {}", err);
            return Err(err);
        }
    }
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
    if mux_disabled() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "mux disabled by RUSTNPS_DISABLE_MUX",
        ));
    }
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

fn mux_disabled() -> bool {
    matches!(
        std::env::var("RUSTNPS_DISABLE_MUX")
            .ok()
            .as_deref()
            .map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on")
    )
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

#[cfg(test)]
mod tests {
    use super::{report_health_change, write_proxy_protocol_v1, write_proxy_protocol_v2};
    use crate::protocol::{ok, read_message, write_message, BridgeHello};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    #[test]
    fn proxy_protocol_v1_writes_text_header() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            socket.read_to_end(&mut buf).unwrap();
            String::from_utf8(buf).unwrap()
        });

        let mut stream = TcpStream::connect(addr).unwrap();
        let remote = "127.0.0.1:43210".parse().unwrap();
        let local = stream.local_addr().unwrap();
        write_proxy_protocol_v1(&mut stream, remote, local).unwrap();
        stream.write_all(b"hello").unwrap();
        stream.flush().unwrap();
        drop(stream);

        let captured = server.join().unwrap();
        assert!(captured.starts_with("PROXY TCP4 127.0.0.1 127.0.0.1 43210 "));
        assert!(captured.ends_with("\r\nhello"));
    }

    #[test]
    fn proxy_protocol_v2_writes_binary_header() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            socket.read_to_end(&mut buf).unwrap();
            buf
        });

        let mut stream = TcpStream::connect(addr).unwrap();
        let remote = "127.0.0.1:43210".parse().unwrap();
        let local = stream.local_addr().unwrap();
        write_proxy_protocol_v2(&mut stream, remote, local).unwrap();
        stream.write_all(b"hello").unwrap();
        drop(stream);

        let captured = server.join().unwrap();
        assert_eq!(&captured[..12], b"\r\n\r\n\0\r\nQUIT\n");
        assert_eq!(captured[12], 0x21);
        assert_eq!(captured[13], 0x11);
        assert_eq!(u16::from_be_bytes([captured[14], captured[15]]), 12);
        assert_eq!(&captured[captured.len() - 5..], b"hello");
    }

    #[test]
    fn report_health_change_sends_health_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let hello: BridgeHello = read_message(&mut socket).unwrap();
            match hello {
                BridgeHello::Health { vkey, target, status } => {
                    assert_eq!(vkey, "health-vkey");
                    assert_eq!(target, "127.0.0.1:8081");
                    assert!(!status);
                    write_message(&mut socket, &ok("health accepted")).unwrap();
                }
                other => panic!("unexpected hello: {other:?}"),
            }
        });

        report_health_change(&addr.to_string(), "health-vkey", "127.0.0.1:8081", false).unwrap();
        server.join().unwrap();
    }
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
