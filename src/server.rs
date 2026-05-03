use crate::config::{expand_ports, expand_targets, load_server_config};
use crate::model::{
    ClientRuntimeConfig, CommonConfig, FlowCounter, GlobalConfig, Host, ServerConfig, Target,
    Tunnel,
};
use crate::mux::{MuxSession, MuxTrafficPolicy};
use crate::protocol::{
    error, ok, read_blob, read_message, write_blob, write_message, BridgeHello, FileServe, Link,
    LinkKind, ServerMessage,
};
use crate::relay::{
    check_http_basic_auth, copy_bidirectional, copy_bidirectional_legacy, md5_hex,
    http_header_value, parse_http_host_path, parse_http_request_line, parse_http_status_code,
    parse_http_target, read_http_head, rewrite_http_request_head, wrap_server_transport,
    write_http_response, RelayPolicy, RelayRead, RelayStream, RelayWrite,
};
use crate::socks5::accept_socks5;
use crate::store::{PersistentState, PersistentStore};
use crate::tls::{build_server_config, handshake as tls_handshake, sniff_sni};
use crate::{CORE_VERSION, VERSION};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};
use sysinfo::{Networks, System};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub time: String,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub cpu: f32,
    pub virtual_mem: u64,
    pub total_mem: u64,
    pub swap_mem: u64,
    pub tcp: usize,
    pub udp: usize,
    pub io_send: u64,
    pub io_recv: u64,
}

#[derive(Default)]
struct HttpResponseCache {
    entries: HashMap<String, Vec<u8>>,
    order: VecDeque<String>,
}

impl HttpResponseCache {
    fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: String, value: Vec<u8>, limit: usize) {
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), value);
            self.order.retain(|item| item != &key);
            self.order.push_back(key);
            return;
        }
        self.entries.insert(key.clone(), value);
        self.order.push_back(key);
        if limit == 0 {
            return;
        }
        while self.order.len() > limit {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

#[derive(Clone)]
pub struct ControlHandle {
    pub tx: Sender<ServerMessage>,
    pub version: String,
    pub connected_at: SystemTime,
    pub remote_addr: String,
}

#[derive(Debug, Clone)]
pub struct WebSession {
    pub is_admin: bool,
    pub client_id: Option<u64>,
    pub username: String,
}

#[derive(Debug, Clone)]
struct CaptchaEntry {
    code: String,
    expires_at: SystemTime,
}

impl WebSession {
    pub fn admin(username: impl Into<String>) -> Self {
        Self {
            is_admin: true,
            client_id: None,
            username: username.into(),
        }
    }

    pub fn client(client_id: u64, username: impl Into<String>) -> Self {
        Self {
            is_admin: false,
            client_id: Some(client_id),
            username: username.into(),
        }
    }
}

pub struct Registry {
    pub server: ServerConfig,
    pub controls: Mutex<HashMap<String, ControlHandle>>,
    pub mux_sessions: Mutex<HashMap<String, Arc<MuxSession>>>,
    listener_shutdowns: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    tunnel_live_conns: Mutex<HashMap<u64, HashMap<u64, TcpStream>>>,
    pub pending: Mutex<HashMap<u64, Sender<Box<dyn RelayStream>>>>,
    pub clients: Mutex<HashMap<String, ClientRuntimeConfig>>,
    pub secrets: Mutex<HashMap<String, Tunnel>>,
    pub listeners: Mutex<HashSet<String>>,
    pub cursors: Mutex<HashMap<String, usize>>,
    pub sessions: Mutex<HashMap<String, WebSession>>,
    pub global: Mutex<GlobalConfig>,
    captcha_store: Mutex<HashMap<String, CaptchaEntry>>,
    http_cache: Mutex<HttpResponseCache>,
    active_conn_counts: Mutex<HashMap<String, usize>>,
    authorized_ips: Mutex<HashMap<String, SystemTime>>,
    client_flow_counters: Mutex<HashMap<String, Arc<Mutex<FlowCounter>>>>,
    pub store: PersistentStore,
    pub system_history: Mutex<VecDeque<SystemSnapshot>>,
    pub seq: AtomicU64,
}

impl Registry {
    fn new(server: ServerConfig, store: PersistentStore) -> Self {
        Self {
            server,
            controls: Mutex::new(HashMap::new()),
            mux_sessions: Mutex::new(HashMap::new()),
            listener_shutdowns: Mutex::new(HashMap::new()),
            tunnel_live_conns: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            clients: Mutex::new(HashMap::new()),
            secrets: Mutex::new(HashMap::new()),
            listeners: Mutex::new(HashSet::new()),
            cursors: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            global: Mutex::new(GlobalConfig::default()),
            captcha_store: Mutex::new(HashMap::new()),
            http_cache: Mutex::new(HttpResponseCache::default()),
            active_conn_counts: Mutex::new(HashMap::new()),
            authorized_ips: Mutex::new(HashMap::new()),
            client_flow_counters: Mutex::new(HashMap::new()),
            store,
            system_history: Mutex::new(VecDeque::with_capacity(10)),
            seq: AtomicU64::new(1),
        }
    }

    pub fn next_link_id(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    pub fn observe_id(&self, id: u64) {
        let mut current = self.seq.load(Ordering::Relaxed);
        while current <= id {
            match self.seq.compare_exchange_weak(
                current,
                id + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
    }
}

pub fn entry() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        print_nps_help();
        return Ok(());
    }
    if args
        .iter()
        .any(|arg| arg == "-version" || arg == "--version")
    {
        println!("RustNps nps version {VERSION}, core {CORE_VERSION}");
        return Ok(());
    }

    let conf_path = arg_value(&args, "-conf_path")
        .or_else(|| arg_value(&args, "--conf-path"))
        .unwrap_or_else(default_server_conf);
    let mut config_missing = false;
    let mut config = if Path::new(&conf_path).exists() {
        load_server_config(&conf_path)?
    } else {
        config_missing = true;
        ServerConfig::default()
    };
    let console_log_level = arg_value(&args, "--console-log-level")
        .or_else(|| arg_value(&args, "-console_log_level"))
        .or_else(|| arg_value(&args, "-log_level"))
        .or_else(|| arg_value(&args, "--log-level"))
        .unwrap_or_else(|| config.log_level.clone());
    config.log_level = console_log_level;
    if let Some(path) = arg_value(&args, "-log_path").or_else(|| arg_value(&args, "--log-path")) {
        config.log_path = path;
    }
    crate::logging::init_console_from_text(&config.log_level);
    if config_missing {
        crate::log_warn!(
            "nps",
            "config {conf_path} not found, using built-in defaults"
        );
    }
    crate::log_info!("nps", "the config path is: {conf_path}");
    if !config.log_path.is_empty() {
        crate::log_info!(
            "nps",
            "log_path is configured as {}, console output remains enabled in RustNps",
            config.log_path
        );
    }
    if std::env::var("NPS_WEB_PATH").is_err() {
        if let Some(web_root) = resolve_web_root_from_conf_path(Path::new(&conf_path)) {
            std::env::set_var("NPS_WEB_PATH", web_root);
        }
    }
    run_with_store(config, PersistentStore::from_conf_path(&conf_path))
}

fn start_system_monitor(registry: Arc<Registry>) {
    let mut sys = System::new_all();
    let mut networks = Networks::new_with_refreshed_list();
    let mut last_send: u64 = 0;
    let mut last_recv: u64 = 0;
    let mut first_run = true;

    loop {
        sys.refresh_all();
        networks.refresh();

        let cpu = sys.global_cpu_info().cpu_usage();
        let virtual_mem = sys.used_memory() / 1024 / 1024; // MB
        let total_mem = sys.total_memory() / 1024 / 1024; // MB
        let swap_mem = sys.used_swap() / 1024 / 1024; // MB
        let load = System::load_average();

        let mut current_send: u64 = 0;
        let mut current_recv: u64 = 0;
        for (_, data) in &networks {
            current_send += data.total_transmitted();
            current_recv += data.total_received();
        }

        let (io_send, io_recv) = if first_run {
            first_run = false;
            (0, 0)
        } else {
            // Delta over 3 seconds
            (
                (current_send.saturating_sub(last_send)) / 3,
                (current_recv.saturating_sub(last_recv)) / 3,
            )
        };

        last_send = current_send;
        last_recv = current_recv;

        let (tcp_est, udp_est) = {
            let clients = registry.clients.lock().unwrap();
            let mut t = 0;
            let mut u = 0;
            for c in clients.values() {
                t += c.tunnels.iter().filter(|tun| tun.mode == "tcp").count();
                u += c.tunnels.iter().filter(|tun| tun.mode == "udp").count();
            }
            (t, u)
        };

        let snapshot = SystemSnapshot {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            load1: load.one,
            load5: load.five,
            load15: load.fifteen,
            cpu,
            virtual_mem,
            total_mem,
            swap_mem,
            tcp: tcp_est,
            udp: udp_est,
            io_send,
            io_recv,
        };

        {
            let mut history = registry.system_history.lock().unwrap();
            if history.len() >= 10 {
                history.pop_front();
            }
            history.push_back(snapshot);
        }

        thread::sleep(Duration::from_secs(3));
    }
}

fn load_persistent_state(registry: &Arc<Registry>) -> io::Result<()> {
    let state = registry.store.load()?;
    install_persistent_state(registry, state);
    Ok(())
}

fn install_persistent_state(registry: &Arc<Registry>, state: PersistentState) {
    {
        let mut global = registry.global.lock().unwrap();
        *global = state.global;
    }

    let mut clients_by_vkey = HashMap::new();
    for mut client in state.clients {
        let vkey = if client.common.vkey.is_empty() {
            client.common.client.verify_key.clone()
        } else {
            client.common.vkey.clone()
        };
        if vkey.is_empty() {
            continue;
        }
        if client.id == 0 {
            client.id = registry.next_link_id();
        }
        client.common.vkey = vkey.clone();
        client.common.client.verify_key = vkey.clone();
        client.no_store = false;
        registry.observe_id(client.id);
        clients_by_vkey.insert(vkey, client);
    }

    for mut tunnel in state.tunnels {
        if tunnel.client_vkey.is_empty() {
            continue;
        }
        if tunnel.id == 0 {
            tunnel.id = registry.next_link_id();
        }
        tunnel.no_store = false;
        registry.observe_id(tunnel.id);
        if let Some(client) = clients_by_vkey.get_mut(&tunnel.client_vkey) {
            client.tunnels.push(tunnel);
        }
    }

    for mut host in state.hosts {
        if host.client_vkey.is_empty() {
            continue;
        }
        if host.id == 0 {
            host.id = registry.next_link_id();
        }
        host.no_store = false;
        registry.observe_id(host.id);
        if let Some(client) = clients_by_vkey.get_mut(&host.client_vkey) {
            client.hosts.push(host);
        }
    }

    let mut to_start = Vec::new();
    {
        let mut clients = registry.clients.lock().unwrap();
        for (vkey, client) in clients_by_vkey {
            to_start.extend(expand_runtime_tunnels(&client));
            clients.insert(vkey, client);
        }
    }

    for tunnel in to_start {
        start_tunnel_task(Arc::clone(registry), tunnel);
    }

    crate::log_info!(
        "nps",
        "Persistent state loaded from {}",
        registry.store.conf_dir().display()
    );
}

pub fn run(config: ServerConfig) -> io::Result<()> {
    run_with_store(config, PersistentStore::new("conf"))
}

pub fn run_with_store(config: ServerConfig, store: PersistentStore) -> io::Result<()> {
    crate::log_info!(
        "nps",
        "the version of server is {VERSION} ,allow client core version to be {CORE_VERSION},tls enable is {}",
        config.tls_enable
    );
    if !config.log_path.is_empty() {
        crate::log_info!("nps", "log path is {}", config.log_path);
    }
    let registry = Arc::new(Registry::new(config.clone(), store));
    load_persistent_state(&registry)?;

    // Start system monitor
    {
        let registry_for_sys = Arc::clone(&registry);
        thread::spawn(move || {
            start_system_monitor(registry_for_sys);
        });
    }

    if config.web_port > 0 {
        let registry_for_web = Arc::clone(&registry);
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(err) = crate::web::start_web_manager(registry_for_web).await {
                    crate::log_error!("nps", "web manager stopped: {err}");
                }
            });
        });
    }

    if config.http_proxy_port > 0 {
        let registry_for_http = Arc::clone(&registry);
        thread::spawn(move || {
            if let Err(err) = start_http_host_listener(registry_for_http) {
                crate::log_error!("nps", "http host proxy stopped: {err}");
            }
        });
    }

    if config.https_proxy_port > 0 {
        let registry_for_https = Arc::clone(&registry);
        thread::spawn(move || {
            if let Err(err) = start_https_host_listener(registry_for_https) {
                crate::log_error!("nps", "https host proxy stopped: {err}");
            }
        });
    }

    let bridge_addr = format!("{}:{}", config.bridge_ip, config.bridge_port);
    let listener = TcpListener::bind(&bridge_addr)?;
    crate::log_info!(
        "nps",
        "server start, the bridge type is {}, the bridge port is {}",
        config.bridge_type,
        config.bridge_port
    );
    crate::log_debug!("nps", "server bridge listening on {bridge_addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let registry = Arc::clone(&registry);
                thread::spawn(move || {
                    if let Err(err) = handle_bridge_conn(stream, registry) {
                        crate::log_warn!("nps", "bridge connection error: {err}");
                    }
                });
            }
            Err(err) => crate::log_warn!("nps", "bridge accept error: {err}"),
        }
    }
    Ok(())
}

fn handle_bridge_conn(mut stream: TcpStream, registry: Arc<Registry>) -> io::Result<()> {
    let remote_addr = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_default();
    let hello: BridgeHello = read_message(&mut stream)?;
    match hello {
        BridgeHello::Control {
            vkey,
            version,
            core_version,
        } => {
            if let Err(err) = validate_client_handshake(&registry, &vkey, &remote_addr, false) {
                write_message(&mut stream, &error(err.to_string()))?;
                return Ok(());
            }
            let client_id = registry.clients.lock().unwrap().get(&vkey).map(|c| c.id).unwrap_or(0);
            crate::log_info!(
                "nps",
                "clientId {} connection succeeded, address:{} ",
                client_id, remote_addr
            );
            crate::log_debug!(
                "nps",
                "client {} online, version={}, core={}",
                vkey,
                version,
                core_version
            );
            write_message(&mut stream, &ok("control accepted"))?;
            let (tx, rx) = mpsc::channel::<ServerMessage>();
            registry.controls.lock().unwrap().insert(
                vkey.clone(),
                ControlHandle {
                    tx,
                    version,
                    connected_at: SystemTime::now(),
                    remote_addr: remote_addr.clone(),
                },
            );
            update_client_last_online_addr(&registry, &vkey, &remote_addr);
            for msg in rx {
                if let Err(err) = write_message(&mut stream, &msg) {
                    crate::log_warn!("nps", "control write to {vkey} failed: {err}");
                    break;
                }
            }
            registry.controls.lock().unwrap().remove(&vkey);
            registry.mux_sessions.lock().unwrap().remove(&vkey);
            crate::log_info!("nps", "the client {vkey} closed");
        }
        BridgeHello::Mux {
            vkey,
            version,
            core_version,
        } => {
            if let Err(err) = validate_client_handshake(&registry, &vkey, &remote_addr, false) {
                write_message(&mut stream, &error(err.to_string()))?;
                return Ok(());
            }
            let client_id = registry.clients.lock().unwrap().get(&vkey).map(|c| c.id).unwrap_or(0);
            crate::log_info!(
                "nps",
                "clientId {} connection succeeded, address:{} ",
                client_id, remote_addr
            );
            crate::log_debug!(
                "nps",
                "client {} mux online, version={}, core={}",
                vkey,
                version,
                core_version
            );
            write_message(&mut stream, &ok("mux accepted"))?;
            let session = MuxSession::new_with_policy(
                stream,
                MuxTrafficPolicy {
                    rate_limit_kb: if registry.server.allow_rate_limit {
                        registry
                            .clients
                            .lock()
                            .unwrap()
                            .get(&vkey)
                            .map(|client| client.common.client.rate_limit_kb)
                            .unwrap_or(0)
                    } else {
                        0
                    },
                    flow_limit_mb: if registry.server.allow_flow_limit {
                        registry
                            .clients
                            .lock()
                            .unwrap()
                            .get(&vkey)
                            .map(|client| client.common.client.flow_limit_mb)
                            .unwrap_or(0)
                    } else {
                        0
                    },
                    flow_counter: Some(client_flow_counter(&registry, &vkey)),
                },
            )?;
            registry
                .mux_sessions
                .lock()
                .unwrap()
                .insert(vkey.clone(), Arc::clone(&session));
            let registry = Arc::clone(&registry);
            thread::spawn(move || mux_accept_loop(registry, vkey, session));
        }
        BridgeHello::Config {
            vkey,
            version,
            core_version,
            mut config,
        } => {
            if let Err(err) = validate_client_handshake(&registry, &vkey, &remote_addr, true) {
                write_message(&mut stream, &error(err.to_string()))?;
                return Ok(());
            }
            let client_id = config.id.max(registry.clients.lock().unwrap().get(&vkey).map(|c| c.id).unwrap_or(0));
            crate::log_info!(
                "nps",
                "clientId {} connection succeeded, address:{} ",
                client_id, remote_addr
            );
            crate::log_debug!(
                "nps",
                "client {} config online, version={}, core={}",
                vkey,
                version,
                core_version
            );
            if config.common.vkey.is_empty() {
                config.common.vkey = vkey.clone();
            }
            config.common.client.verify_key = config.common.vkey.clone();
            install_client_config(Arc::clone(&registry), config)?;
            update_client_last_online_addr(&registry, &vkey, &remote_addr);
            write_message(&mut stream, &ok("config accepted"))?;
        }
        BridgeHello::Data { vkey: _, link_id } => {
            crate::log_trace!(
                "nps",
                "new data connection, link_id={link_id}, address:{remote_addr}"
            );
            let tx = registry.pending.lock().unwrap().remove(&link_id);
            if let Some(tx) = tx {
                write_message(&mut stream, &ok("data accepted"))?;
                let _ = tx.send(Box::new(stream));
            } else {
                write_message(&mut stream, &error("unknown link id"))?;
            }
        }
        BridgeHello::SecretVisitor { password, target }
        | BridgeHello::P2pVisitor { password, target } => {
            handle_secret_visitor(stream, registry, &password, &target)?;
        }
        BridgeHello::RegisterIp { vkey, hours } => {
            ensure_ip_registration_vkey(&registry, &vkey)?;
            authorize_ip(registry.as_ref(), &remote_addr, hours);
            crate::log_notice!(
                "nps",
                "Successful ip registration request, vkey={vkey}, address:{remote_addr}, validity={hours} hours"
            );
            write_message(&mut stream, &ok("ip registered"))?;
        }
    }
    Ok(())
}

fn install_client_config(
    registry: Arc<Registry>,
    mut config: ClientRuntimeConfig,
) -> io::Result<()> {
    let vkey = config.common.vkey.clone();
    if vkey.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty vkey"));
    }

    for host in &mut config.hosts {
        if host.id == 0 {
            host.id = registry.next_link_id();
        }
        registry.observe_id(host.id);
        host.client_vkey = vkey.clone();
    }
    for tunnel in &mut config.tunnels {
        if tunnel.id == 0 {
            tunnel.id = registry.next_link_id();
        }
        validate_tunnel_ports(&registry.server, tunnel)?;
        registry.observe_id(tunnel.id);
        tunnel.client_vkey = vkey.clone();
    }

    let existing = registry.clients.lock().unwrap().get(&vkey).cloned();
    if let Some(existing) = existing {
        if config.no_store && !existing.common.client.config_conn_allow {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "config connection is disabled for this client",
            ));
        }
        config.id = existing.id;
        config.no_store = existing.no_store;
        config.no_display = existing.no_display;
        config.create_time = existing.create_time;
        config.last_online_time = now_text();
        config
            .hosts
            .extend(existing.hosts.into_iter().filter(|h| !h.no_store));
        config
            .tunnels
            .extend(existing.tunnels.into_iter().filter(|t| !t.no_store));
    } else {
        config.id = registry.next_link_id();
        config.no_store = true;
        config.last_online_time = now_text();
    }
    ensure_client_tunnel_limit(&registry.server, &config.common.client, tunnel_count(&config))?;
    registry.observe_id(config.id);

    let expanded = expand_runtime_tunnels(&config);
    {
        let mut clients = registry.clients.lock().unwrap();
        let mut normalized = config.clone();
        normalized.tunnels = expanded.clone();
        clients.insert(vkey.clone(), normalized);
    }

    for tunnel in expanded {
        start_tunnel_task(Arc::clone(&registry), tunnel);
    }
    Ok(())
}

fn expand_runtime_tunnels(config: &ClientRuntimeConfig) -> Vec<Tunnel> {
    let mut out = Vec::new();
    for tunnel in &config.tunnels {
        let mode = tunnel.mode.to_ascii_lowercase();
        if mode == "secret" || mode == "p2p" {
            out.push(tunnel.clone());
            continue;
        }

        let ports = expand_ports(&tunnel.ports);
        let targets = expand_targets(&tunnel.target_addr, &tunnel.target.target_str);
        if ports.is_empty() {
            out.push(tunnel.clone());
            continue;
        }

        for (idx, port) in ports.iter().copied().enumerate() {
            let mut clone = tunnel.clone();
            clone.server_port = port;
            if ports.len() > 1 {
                clone.remark = format!("{}_{}", tunnel.remark, port);
                if let Some(target) = targets.get(idx) {
                    clone.target.target_str = target.clone();
                }
            }
            out.push(clone);
        }
    }
    out
}

fn start_tunnel_task(registry: Arc<Registry>, tunnel: Tunnel) {
    if !tunnel.status {
        return;
    }
    if let Err(err) = validate_tunnel_ports(&registry.server, &tunnel) {
        let client_id = registry
            .clients
            .lock()
            .unwrap()
            .get(&tunnel.client_vkey)
            .map(|client| client.id)
            .unwrap_or(0);
        crate::log_error!(
            "nps",
            "clientId {} taskId {} start error {}",
            client_id,
            tunnel.id,
            err
        );
        return;
    }
    let mode = tunnel.mode.to_ascii_lowercase();
    if mode == "secret" || mode == "p2p" {
        if !tunnel.password.is_empty() {
            registry
                .secrets
                .lock()
                .unwrap()
                .insert(tunnel.password.clone(), tunnel.clone());
            registry
                .secrets
                .lock()
                .unwrap()
                .insert(md5_hex(&tunnel.password), tunnel.clone());
            crate::log_info!("nps", "secret task {} start ", tunnel.remark);
        }
        return;
    }

    let Some(listener_key) = listener_key(&tunnel) else {
        crate::log_warn!(
            "nps",
            "skip tunnel {} with empty listen port",
            tunnel.remark
        );
        return;
    };
    {
        let mut listeners = registry.listeners.lock().unwrap();
        if !listeners.insert(listener_key.clone()) {
            return;
        }
    }
    let shutdown = Arc::new(AtomicBool::new(false));
    registry
        .listener_shutdowns
        .lock()
        .unwrap()
        .insert(tunnel.id, Arc::clone(&shutdown));

    thread::spawn(move || {
        let tunnel_id = tunnel.id;
        let port = tunnel.server_port;
        let registry_for_cleanup = Arc::clone(&registry);
        let client_id = registry_for_cleanup
            .clients
            .lock()
            .unwrap()
            .get(&tunnel.client_vkey)
            .map(|client| client.id)
            .unwrap_or(0);
        let result = match mode.as_str() {
            "tcp" | "tcptrans" | "file" => start_tcp_listener(registry, tunnel, shutdown),
            "httpproxy" => start_http_proxy_listener(registry, tunnel, shutdown),
            "socks5" => start_socks5_listener(registry, tunnel, shutdown),
            "udp" => start_udp_listener(registry, tunnel, shutdown),
            other => {
                crate::log_error!(
                    "nps",
                    "Incorrect startup mode {other}, tunnel={}",
                    tunnel.remark
                );
                Ok(())
            }
        };
        if let Err(err) = result {
            if matches!(
                err.kind(),
                io::ErrorKind::AddrInUse | io::ErrorKind::AddrNotAvailable | io::ErrorKind::PermissionDenied
            ) {
                crate::log_error!("nps", "taskId {} start error port {} open failed", tunnel_id, port);
            } else {
                crate::log_error!("nps", "clientId {} taskId {} start error {}", client_id, tunnel_id, err);
            }
        }
        registry_for_cleanup
            .listeners
            .lock()
            .unwrap()
            .remove(&listener_key);
        registry_for_cleanup
            .listener_shutdowns
            .lock()
            .unwrap()
            .remove(&tunnel_id);
    });
}

fn start_active_tunnels_for_client(registry: &Arc<Registry>, config: &ClientRuntimeConfig) {
    if !config.common.client.status {
        return;
    }
    for tunnel in &config.tunnels {
        if tunnel.status && tunnel.run_status {
            start_tunnel_task(Arc::clone(registry), tunnel.clone());
        }
    }
}

fn stop_tunnel_runtime(registry: &Arc<Registry>, tunnel: &Tunnel) {
    crate::log_info!("nps", "stop server id {}", tunnel.id);
    if let Some(flag) = registry
        .listener_shutdowns
        .lock()
        .unwrap()
        .remove(&tunnel.id)
    {
        flag.store(true, Ordering::SeqCst);
    }
    disconnect_tunnel_live_conns(registry, tunnel.id);
    if let Some(key) = listener_key(tunnel) {
        for _ in 0..20 {
            if !registry.listeners.lock().unwrap().contains(&key) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    let client_id = registry
        .clients
        .lock()
        .unwrap()
        .get(&tunnel.client_vkey)
        .map(|client| client.id)
        .unwrap_or(0);
    crate::log_info!(
        "nps",
        "close port {},remark {},client id {},task id {}",
        tunnel.server_port,
        tunnel.remark,
        client_id,
        tunnel.id
    );
}

fn stop_tunnel_runtimes(registry: &Arc<Registry>, tunnels: &[Tunnel]) {
    for tunnel in tunnels {
        stop_tunnel_runtime(registry, tunnel);
    }
}

fn register_tunnel_live_conn(
    registry: &Arc<Registry>,
    tunnel_id: u64,
    stream: &TcpStream,
) -> Option<u64> {
    let cloned = stream.try_clone().ok()?;
    let conn_id = registry.next_link_id();
    registry
        .tunnel_live_conns
        .lock()
        .unwrap()
        .entry(tunnel_id)
        .or_default()
        .insert(conn_id, cloned);
    Some(conn_id)
}

fn unregister_tunnel_live_conn(registry: &Arc<Registry>, tunnel_id: u64, conn_id: Option<u64>) {
    let Some(conn_id) = conn_id else {
        return;
    };
    let mut all = registry.tunnel_live_conns.lock().unwrap();
    if let Some(group) = all.get_mut(&tunnel_id) {
        group.remove(&conn_id);
        if group.is_empty() {
            all.remove(&tunnel_id);
        }
    }
}

fn disconnect_tunnel_live_conns(registry: &Arc<Registry>, tunnel_id: u64) {
    let conns = registry
        .tunnel_live_conns
        .lock()
        .unwrap()
        .remove(&tunnel_id)
        .unwrap_or_default();
    for (_, stream) in conns {
        let _ = stream.shutdown(Shutdown::Both);
    }
}

struct TunnelConnGuard {
    registry: Arc<Registry>,
    tunnel_id: u64,
    conn_id: Option<u64>,
}

impl TunnelConnGuard {
    fn new(registry: Arc<Registry>, tunnel_id: u64, conn_id: Option<u64>) -> Self {
        Self {
            registry,
            tunnel_id,
            conn_id,
        }
    }
}

impl Drop for TunnelConnGuard {
    fn drop(&mut self) {
        unregister_tunnel_live_conn(&self.registry, self.tunnel_id, self.conn_id);
    }
}

fn terminate_client_connection(registry: &Arc<Registry>, vkey: &str, reason: &str) {
    let handle = registry.controls.lock().unwrap().remove(vkey);
    registry.mux_sessions.lock().unwrap().remove(vkey);
    registry.active_conn_counts.lock().unwrap().remove(vkey);
    if let Some(handle) = handle {
        let _ = handle.tx.send(ServerMessage::Stop {
            reason: reason.to_string(),
        });
    }
}

fn rebuild_secret_registry(registry: &Arc<Registry>) {
    let mut next = HashMap::new();
    let clients = registry.clients.lock().unwrap();
    for client in clients.values() {
        if !client.common.client.status {
            continue;
        }
        for tunnel in &client.tunnels {
            let mode = tunnel.mode.to_ascii_lowercase();
            if !(mode == "secret" || mode == "p2p") || !tunnel.status || !tunnel.run_status {
                continue;
            }
            if tunnel.password.is_empty() {
                continue;
            }
            next.insert(tunnel.password.clone(), tunnel.clone());
            next.insert(md5_hex(&tunnel.password), tunnel.clone());
        }
    }
    *registry.secrets.lock().unwrap() = next;
}

fn listener_key(tunnel: &Tunnel) -> Option<String> {
    if tunnel.server_port == 0 {
        return None;
    }
    Some(format!(
        "{}:{}:{}",
        tunnel.mode.to_ascii_lowercase(),
        tunnel.server_ip,
        tunnel.server_port
    ))
}

fn start_tcp_listener(
    registry: Arc<Registry>,
    tunnel: Tunnel,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    let addr = bind_addr(&tunnel);
    let listener = TcpListener::bind(&addr).map_err(|err| {
        crate::log_error!("nps", "taskId {} start error port {} open failed", tunnel.id, tunnel.server_port);
        err
    })?;
    listener.set_nonblocking(true)?;
    crate::log_info!(
        "nps",
        "tunnel task {} start mode:{} port {}",
        tunnel.remark,
        tunnel.mode,
        tunnel.server_port
    );
    crate::log_debug!(
        "nps",
        "{} tunnel {} listening on {addr}",
        tunnel.mode,
        tunnel.remark
    );
    let cursor_key = format!("{}:{}", tunnel.client_vkey, tunnel.remark);

    loop {
        if shutdown.load(Ordering::SeqCst) || !tunnel_is_active(&registry, tunnel.id) {
            break;
        }
        let mut inbound = match listener.accept() {
            Ok((s, _)) => s,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(err) => {
                crate::log_warn!("nps", "client connection accept failed on {addr}: {err}");
                continue;
            }
        };
        let registry = Arc::clone(&registry);
        let tunnel = tunnel.clone();
        let cursor_key = cursor_key.clone();
        let tracked_conn_id = register_tunnel_live_conn(&registry, tunnel.id, &inbound);
        thread::spawn(move || {
            let _conn_guard = TunnelConnGuard::new(Arc::clone(&registry), tunnel.id, tracked_conn_id);
            if !tunnel_is_active(&registry, tunnel.id) {
                return;
            }
            let remote = inbound
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            crate::log_info!(
                "nps",
                "new tcp connection, local port {}, client {}, remote address {}",
                tunnel.server_port,
                tunnel.client_vkey,
                remote
            );
            let Some(target) = select_target(&registry, &cursor_key, &tunnel) else {
                let _ = write_http_response(
                    &mut inbound,
                    "502 Bad Gateway",
                    "text/plain",
                    b"no target",
                );
                return;
            };
            let kind = if tunnel.mode.eq_ignore_ascii_case("file") {
                LinkKind::File
            } else {
                LinkKind::Tcp
            };
            let (crypt, compress) = client_transport_flags(
                &registry,
                &tunnel.client_vkey,
                tunnel.mode.eq_ignore_ascii_case("file"),
            );
            let link = Link {
                kind,
                target,
                remote_addr: remote,
                crypt,
                compress,
                local_proxy: tunnel.target.local_proxy,
                proto_version: tunnel.proto_version.clone(),
                file: if tunnel.mode.eq_ignore_ascii_case("file") {
                    Some(FileServe {
                        local_path: tunnel.local_path.clone(),
                        strip_pre: tunnel.strip_pre.clone(),
                    })
                } else {
                    None
                },
            };
            match request_client_stream(&registry, &tunnel.client_vkey, link) {
                Ok(client_stream) => {
                    let _ = copy_bidirectional(inbound, client_stream);
                }
                Err(err) => crate::log_warn!("nps", "request client stream failed: {err}"),
            }
        });
    }
    Ok(())
}

fn start_http_proxy_listener(
    registry: Arc<Registry>,
    tunnel: Tunnel,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    let addr = bind_addr(&tunnel);
    let listener = TcpListener::bind(&addr).map_err(|err| {
        crate::log_error!("nps", "taskId {} start error port {} open failed", tunnel.id, tunnel.server_port);
        err
    })?;
    listener.set_nonblocking(true)?;
    crate::log_info!(
        "nps",
        "tunnel task {} start mode:httpProxy port {}",
        tunnel.remark,
        tunnel.server_port
    );
    crate::log_debug!("nps", "http proxy {} listening on {addr}", tunnel.remark);
    loop {
        if shutdown.load(Ordering::SeqCst) || !tunnel_is_active(&registry, tunnel.id) {
            break;
        }
        let mut inbound = match listener.accept() {
            Ok((s, _)) => s,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(err) => {
                crate::log_warn!("nps", "http proxy accept failed: {err}");
                continue;
            }
        };
        let registry = Arc::clone(&registry);
        let tunnel = tunnel.clone();
        let tracked_conn_id = register_tunnel_live_conn(&registry, tunnel.id, &inbound);
        thread::spawn(move || {
            let _conn_guard = TunnelConnGuard::new(Arc::clone(&registry), tunnel.id, tracked_conn_id);
            if !tunnel_is_active(&registry, tunnel.id) {
                return;
            }
            let remote = inbound
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let head = match read_http_head(&mut inbound, 64 * 1024) {
                Ok(h) => h,
                Err(err) => {
                    crate::log_warn!("nps", "bad http proxy request: {err}");
                    return;
                }
            };
            let Some((target, is_connect)) = parse_http_target(&head) else {
                let _ = write_http_response(
                    &mut inbound,
                    "400 Bad Request",
                    "text/plain",
                    b"missing host",
                );
                return;
            };
            if let Some((method, path, _)) = parse_http_request_line(&head) {
                let host = http_header_value(&head, "Host").unwrap_or_else(|| target.clone());
                crate::log_info!(
                    "nps",
                    "http request, method {}, host {}, url {}, remote address {}, target {}",
                    method,
                    host,
                    path,
                    remote,
                    target
                );
            }
            if is_connect {
                let _ = inbound.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
            }
            let (crypt, compress) = client_transport_flags(&registry, &tunnel.client_vkey, false);
            let link = Link {
                kind: LinkKind::Http,
                target,
                remote_addr: remote,
                crypt,
                compress,
                local_proxy: tunnel.target.local_proxy,
                proto_version: String::new(),
                file: None,
            };
            match request_client_stream(&registry, &tunnel.client_vkey, link) {
                Ok(mut client_stream) => {
                    if !is_connect {
                        let _ = client_stream.write_all(&head);
                    }
                    let _ = copy_bidirectional(inbound, client_stream);
                }
                Err(err) => crate::log_warn!("nps", "http proxy stream failed: {err}"),
            }
        });
    }
    Ok(())
}

fn start_socks5_listener(
    registry: Arc<Registry>,
    tunnel: Tunnel,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    let addr = bind_addr(&tunnel);
    let listener = TcpListener::bind(&addr).map_err(|err| {
        crate::log_error!("nps", "taskId {} start error port {} open failed", tunnel.id, tunnel.server_port);
        err
    })?;
    listener.set_nonblocking(true)?;
    crate::log_info!(
        "nps",
        "tunnel task {} start mode:socks5 port {}",
        tunnel.remark,
        tunnel.server_port
    );
    crate::log_debug!("nps", "socks5 {} listening on {addr}", tunnel.remark);
    loop {
        if shutdown.load(Ordering::SeqCst) || !tunnel_is_active(&registry, tunnel.id) {
            break;
        }
        let mut inbound = match listener.accept() {
            Ok((s, _)) => s,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(err) => {
                crate::log_warn!("nps", "socks5 accept failed: {err}");
                continue;
            }
        };
        let registry = Arc::clone(&registry);
        let tunnel = tunnel.clone();
        let tracked_conn_id = register_tunnel_live_conn(&registry, tunnel.id, &inbound);
        thread::spawn(move || {
            let _conn_guard = TunnelConnGuard::new(Arc::clone(&registry), tunnel.id, tracked_conn_id);
            if !tunnel_is_active(&registry, tunnel.id) {
                return;
            }
            let remote = inbound
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let client_id = registry
                .clients
                .lock()
                .unwrap()
                .get(&tunnel.client_vkey)
                .map(|client| client.id)
                .unwrap_or(0);
            crate::log_trace!(
                "nps",
                "New socks5 connection,client {},remote address {}",
                client_id,
                remote
            );
            let target = match accept_socks5(&mut inbound) {
                Ok(target) => target,
                Err(err) => {
                    match err.kind() {
                        io::ErrorKind::InvalidData => {
                            if err.to_string().contains("not socks5") {
                                crate::log_warn!("nps", "only support socks5, request from: {}", remote);
                            } else {
                                crate::log_warn!("nps", "wrong method");
                            }
                        }
                        io::ErrorKind::PermissionDenied => {
                            crate::log_warn!("nps", "Validation failed: {}", err);
                        }
                        _ => {
                            crate::log_warn!("nps", "negotiation err {}", err);
                        }
                    }
                    return;
                }
            };
            let (crypt, compress) = client_transport_flags(&registry, &tunnel.client_vkey, false);
            let link = Link {
                kind: LinkKind::Tcp,
                target,
                remote_addr: remote,
                crypt,
                compress,
                local_proxy: tunnel.target.local_proxy,
                proto_version: String::new(),
                file: None,
            };
            match request_client_stream(&registry, &tunnel.client_vkey, link) {
                Ok(client_stream) => {
                    let _ = copy_bidirectional(inbound, client_stream);
                }
                Err(err) => {
                    crate::log_warn!(
                        "nps",
                        "client id {}, task id {}, error {}, when socks5 connection",
                        client_id,
                        tunnel.id,
                        err
                    );
                }
            }
        });
    }
    Ok(())
}

fn start_udp_listener(
    registry: Arc<Registry>,
    tunnel: Tunnel,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    let addr = bind_addr(&tunnel);
    let socket = Arc::new(UdpSocket::bind(&addr).map_err(|err| {
        crate::log_error!("nps", "taskId {} start error port {} open failed", tunnel.id, tunnel.server_port);
        err
    })?);
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    crate::log_info!(
        "nps",
        "tunnel task {} start mode:udp port {}",
        tunnel.remark,
        tunnel.server_port
    );
    crate::log_debug!("nps", "udp tunnel {} listening on {addr}", tunnel.remark);
    let cursor_key = format!("{}:{}", tunnel.client_vkey, tunnel.remark);
    let workers: Arc<Mutex<HashMap<String, Sender<Vec<u8>>>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut buf = vec![0_u8; 64 * 1024];

    loop {
        if shutdown.load(Ordering::SeqCst) || !tunnel_is_active(&registry, tunnel.id) {
            break;
        }
        let (n, peer) = match socket.recv_from(&mut buf) {
            Ok(value) => value,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(err) => return Err(err),
        };
        let packet = buf[..n].to_vec();
        let peer_key = peer.to_string();
        if let Some(tx) = workers.lock().unwrap().get(&peer_key).cloned() {
            let _ = tx.send(packet);
            continue;
        }

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let _ = tx.send(packet);
        workers
            .lock()
            .unwrap()
            .insert(peer_key.clone(), tx.clone());

        let registry = Arc::clone(&registry);
        let tunnel = tunnel.clone();
        let socket = Arc::clone(&socket);
        let cursor_key = cursor_key.clone();
        let workers = Arc::clone(&workers);
        let shutdown = Arc::clone(&shutdown);
        thread::spawn(move || {
            let _cleanup = UdpWorkerCleanup {
                workers,
                key: peer_key.clone(),
            };
            if shutdown.load(Ordering::SeqCst) || !tunnel_is_active(&registry, tunnel.id) {
                return;
            }
            let client_id = registry
                .clients
                .lock()
                .unwrap()
                .get(&tunnel.client_vkey)
                .map(|client| client.id)
                .unwrap_or(0);
            crate::log_trace!(
                "nps",
                "New udp connection,client {},remote address {}",
                client_id,
                peer
            );
            let Some(target) = select_target(&registry, &cursor_key, &tunnel) else {
                return;
            };
            let (crypt, compress) = client_transport_flags(&registry, &tunnel.client_vkey, false);
            let link = Link {
                kind: LinkKind::Udp,
                target,
                remote_addr: peer.to_string(),
                crypt,
                compress,
                local_proxy: tunnel.target.local_proxy,
                proto_version: String::new(),
                file: None,
            };
            match request_client_stream(&registry, &tunnel.client_vkey, link) {
                Ok(mut client_stream) => {
                    loop {
                        if shutdown.load(Ordering::SeqCst) || !tunnel_is_active(&registry, tunnel.id)
                        {
                            break;
                        }
                        let packet = match rx.recv_timeout(Duration::from_millis(200)) {
                            Ok(packet) => packet,
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => break,
                        };
                        if write_blob(&mut client_stream, &packet).is_err() {
                            break;
                        }
                        match read_blob(&mut client_stream) {
                            Ok(reply) => {
                                let _ = socket.send_to(&reply, peer);
                            }
                            Err(_) => break,
                        }
                    }
                }
                Err(err) => crate::log_warn!(
                    "nps",
                    "client id {}, task id {},error {}, when udp connection",
                    client_id,
                    tunnel.id,
                    err
                ),
            }
        });
    }
    workers.lock().unwrap().clear();
    Ok(())
}

struct UdpWorkerCleanup {
    workers: Arc<Mutex<HashMap<String, Sender<Vec<u8>>>>>,
    key: String,
}

impl Drop for UdpWorkerCleanup {
    fn drop(&mut self) {
        self.workers.lock().unwrap().remove(&self.key);
    }
}

fn start_http_host_listener(registry: Arc<Registry>) -> io::Result<()> {
    let addr = format!(
        "{}:{}",
        registry.server.http_proxy_ip, registry.server.http_proxy_port
    );
    let listener = TcpListener::bind(&addr)?;
    crate::log_info!(
        "nps",
        "start http listener, port is {}",
        registry.server.http_proxy_port
    );
    crate::log_debug!("nps", "http host proxy listening on {addr}");
    for incoming in listener.incoming() {
        let mut inbound = match incoming {
            Ok(s) => s,
            Err(err) => {
                crate::log_warn!("nps", "http host accept failed: {err}");
                continue;
            }
        };
        let registry = Arc::clone(&registry);
        thread::spawn(move || {
            let remote = inbound
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let head = match read_http_head(&mut inbound, 64 * 1024) {
                Ok(h) => h,
                Err(err) => {
                    crate::log_warn!("nps", "bad host proxy request: {err}");
                    return;
                }
            };
            let Some((host, path)) = parse_http_host_path(&head) else {
                crate::log_notice!("nps", "the url  {} can't be parsed!", String::from_utf8_lossy(&head));
                let _ = write_http_response(
                    &mut inbound,
                    "400 Bad Request",
                    "text/plain",
                    b"missing host",
                );
                return;
            };
            let Some((route, target)) = find_host_route(&registry, &host, &path, "http") else {
                let _ = write_custom_404(&mut inbound, &registry);
                return;
            };
            match prepare_host_proxy_request(&registry, &route, &remote, &head, "http") {
                Ok((prepared_head, cache_key)) => {
                    if let Err(err) = proxy_host_request(
                        &registry,
                        inbound,
                        route,
                        remote,
                        target,
                        prepared_head,
                        cache_key,
                    ) {
                        crate::log_warn!("nps", "host proxy request failed: {err}");
                    }
                }
                Err(HostProxyError::Unauthorized) => {
                    let _ = write_basic_auth_required(&mut inbound);
                }
                Err(HostProxyError::InvalidRequest) => {
                    crate::log_notice!("nps", "the url {} can't be parsed!", host);
                    let _ = write_http_response(
                        &mut inbound,
                        "400 Bad Request",
                        "text/plain",
                        b"bad request",
                    );
                }
            }
        });
    }
    Ok(())
}

fn start_https_host_listener(registry: Arc<Registry>) -> io::Result<()> {
    let addr = format!(
        "{}:{}",
        registry.server.http_proxy_ip, registry.server.https_proxy_port
    );
    let listener = TcpListener::bind(&addr).map_err(|err| {
        crate::log_error!("nps", "https listener start error port {} open failed", registry.server.https_proxy_port);
        err
    })?;
    crate::log_info!(
        "nps",
        "start https listener, port is {}",
        registry.server.https_proxy_port
    );
    crate::log_debug!("nps", "https host proxy listening on {addr}");
    for incoming in listener.incoming() {
        let inbound = match incoming {
            Ok(s) => s,
            Err(err) => {
                crate::log_warn!("nps", "https host accept failed: {err}");
                continue;
            }
        };
        let registry = Arc::clone(&registry);
        thread::spawn(move || {
            if let Err(err) = handle_https_host_conn(inbound, registry) {
                crate::log_warn!("nps", "https host connection failed: {err}");
            }
        });
    }
    Ok(())
}

fn handle_https_host_conn(inbound: TcpStream, registry: Arc<Registry>) -> io::Result<()> {
    let remote = inbound
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
    let server_name = sniff_sni(&inbound)?.unwrap_or_default();

    if registry.server.https_just_proxy {
        let Some((route, target)) = find_host_route(&registry, &server_name, "/", "https") else {
            crate::log_notice!("nps", "the url {} can't be parsed!", server_name);
            return Err(io::Error::new(io::ErrorKind::NotFound, "https host route not found"));
        };
        let client_id = registry
            .clients
            .lock()
            .unwrap()
            .get(&route.client_vkey)
            .map(|client| client.id)
            .unwrap_or(0);
        crate::log_trace!(
            "nps",
            "new https connection,clientId {},host {},remote address {}",
            client_id,
            route.host,
            remote
        );
        let (crypt, compress) = client_transport_flags(&registry, &route.client_vkey, false);
        let link = Link {
            kind: LinkKind::Tcp,
            target,
            remote_addr: remote,
            crypt,
            compress,
            local_proxy: route.target.local_proxy,
            proto_version: String::new(),
            file: None,
        };
        let client_stream = request_client_stream(&registry, &route.client_vkey, link)?;
        return copy_bidirectional(inbound, client_stream);
    }

    let tls_config = build_tls_config_for_registry(&registry)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no https certificates configured"))?;
    let mut tls_stream = tls_handshake(inbound, tls_config)?;
    let head = read_http_head(&mut tls_stream, 64 * 1024)?;
    let Some((host, path)) = parse_http_host_path(&head) else {
        crate::log_notice!("nps", "the url {} can't be parsed!", server_name);
        let _ = write_http_response(
            &mut tls_stream,
            "400 Bad Request",
            "text/plain",
            b"missing host",
        );
        return Ok(());
    };
    let Some((route, target)) = find_host_route(&registry, &host, &path, "https") else {
        let _ = write_custom_404(&mut tls_stream, &registry);
        return Ok(());
    };
    let client_id = registry
        .clients
        .lock()
        .unwrap()
        .get(&route.client_vkey)
        .map(|client| client.id)
        .unwrap_or(0);
    crate::log_trace!(
        "nps",
        "new https connection,clientId {},host {},remote address {}",
        client_id,
        route.host,
        remote
    );
    match prepare_host_proxy_request(&registry, &route, &remote, &head, "https") {
        Ok((prepared_head, cache_key)) => {
            if let Err(err) = proxy_host_request(
                &registry,
                tls_stream,
                route,
                remote,
                target,
                prepared_head,
                cache_key,
            ) {
                crate::log_warn!("nps", "https host proxy request failed: {err}");
            }
        }
        Err(HostProxyError::Unauthorized) => {
            let _ = write_basic_auth_required(&mut tls_stream);
        }
        Err(HostProxyError::InvalidRequest) => {
            crate::log_notice!("nps", "the url {} can't be parsed!", host);
        }
    }
    Ok(())
}

fn handle_secret_visitor(
    mut visitor: TcpStream,
    registry: Arc<Registry>,
    password: &str,
    visitor_target: &str,
) -> io::Result<()> {
    let tunnel = {
        let secrets = registry.secrets.lock().unwrap();
        secrets
            .get(password)
            .or_else(|| secrets.get(&md5_hex(password)))
            .cloned()
    };
    let Some(tunnel) = tunnel else {
        write_message(&mut visitor, &error("secret not found"))?;
        return Ok(());
    };
    let target = if visitor_target.trim().is_empty() {
        tunnel
            .target
            .pick(0)
            .map(|(target, _)| target)
            .unwrap_or_else(|| tunnel.target.target_str.clone())
    } else {
        visitor_target.to_string()
    };
    let (crypt, compress) = client_transport_flags(&registry, &tunnel.client_vkey, false);
    let link = Link {
        kind: if tunnel.mode.eq_ignore_ascii_case("p2p") {
            LinkKind::P2p
        } else {
            LinkKind::Secret
        },
        target,
        remote_addr: visitor
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default(),
        crypt,
        compress,
        local_proxy: tunnel.target.local_proxy,
        proto_version: String::new(),
        file: None,
    };
    match request_client_stream(&registry, &tunnel.client_vkey, link) {
        Ok(client_stream) => {
            write_message(&mut visitor, &ok("secret accepted"))?;
            let _ = copy_bidirectional(visitor, client_stream);
        }
        Err(err) => {
            write_message(&mut visitor, &error(format!("provider offline: {err}")))?;
        }
    }
    Ok(())
}

fn request_client_stream(
    registry: &Arc<Registry>,
    vkey: &str,
    link: Link,
) -> io::Result<Box<dyn RelayStream>> {
    validate_client_access(registry, vkey, &link.remote_addr)?;
    let slot_reserved = acquire_client_connection_slot(registry, vkey)?;
    let link_crypt = link.crypt;
    let link_compress = link.compress;
    let control = {
        let controls = registry.controls.lock().unwrap();
        controls.get(vkey).cloned()
    }
    .ok_or_else(|| {
        if slot_reserved {
            release_client_connection_slot(registry, vkey);
        }
        io::Error::new(io::ErrorKind::NotConnected, "client is offline")
    })?;

    let link_id = registry.next_link_id();
    let (tx, rx) = mpsc::channel();
    registry.pending.lock().unwrap().insert(link_id, tx);

    if control
        .tx
        .send(ServerMessage::Open { link_id, link })
        .is_err()
    {
        if slot_reserved {
            release_client_connection_slot(registry, vkey);
        }
        registry.pending.lock().unwrap().remove(&link_id);
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "control channel closed",
        ));
    }

    match rx.recv_timeout(Duration::from_secs(
        registry.server.disconnect_timeout.max(10),
    )) {
        Ok(stream) => {
            let is_mux_stream = stream.as_ref().as_any().is::<crate::mux::MuxStream>();
            let wrapped = wrap_server_transport(
                stream,
                link_crypt,
                link_compress,
                if is_mux_stream {
                    RelayPolicy::default()
                } else {
                    client_relay_policy(registry, vkey)
                },
            )?;
            Ok(Box::new(CountedRelayStream::new(
                wrapped,
                Arc::clone(registry),
                vkey.to_string(),
            )))
        }
        Err(err) => {
            crate::log_warn!(
                "nps",
                "client stream timeout vkey={} link_id={} err={}",
                vkey,
                link_id,
                err
            );
            if slot_reserved {
                release_client_connection_slot(registry, vkey);
            }
            registry.pending.lock().unwrap().remove(&link_id);
            Err(io::Error::new(io::ErrorKind::TimedOut, err))
        }
    }
}

struct CountedRelayStream {
    inner: Box<dyn RelayStream>,
    guard: Arc<ConnectionSlotGuard>,
}

struct ConnectionSlotGuard {
    registry: Arc<Registry>,
    vkey: String,
}

impl CountedRelayStream {
    fn new(inner: Box<dyn RelayStream>, registry: Arc<Registry>, vkey: String) -> Self {
        Self {
            inner,
            guard: Arc::new(ConnectionSlotGuard { registry, vkey }),
        }
    }
}

impl Drop for ConnectionSlotGuard {
    fn drop(&mut self) {
        release_client_connection_slot(&self.registry, &self.vkey);
    }
}

impl Read for CountedRelayStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for CountedRelayStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl RelayStream for CountedRelayStream {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        let Self { inner, guard } = *self;
        let (reader, writer) = inner.into_io_halves()?;
        Ok((
            Box::new(CountedReadHalf {
                inner: reader,
                _guard: Arc::clone(&guard),
            }),
            Box::new(CountedWriteHalf {
                inner: writer,
                _guard: guard,
            }),
        ))
    }
}

struct CountedReadHalf {
    inner: Box<dyn RelayRead>,
    _guard: Arc<ConnectionSlotGuard>,
}

impl Read for CountedReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

struct CountedWriteHalf {
    inner: Box<dyn RelayWrite>,
    _guard: Arc<ConnectionSlotGuard>,
}

impl Write for CountedWriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn acquire_client_connection_slot(registry: &Arc<Registry>, vkey: &str) -> io::Result<bool> {
    if !registry.server.allow_connection_num_limit {
        return Ok(false);
    }
    let max_conn = {
        let clients = registry.clients.lock().unwrap();
        clients
            .get(vkey)
            .map(|client| client.common.client.max_conn)
            .unwrap_or(0)
    };
    if max_conn == 0 {
        return Ok(false);
    }

    let mut counts = registry.active_conn_counts.lock().unwrap();
    let current = counts.get(vkey).copied().unwrap_or(0);
    if current >= max_conn {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "client connection count exceeds limit",
        ));
    }
    counts.insert(vkey.to_string(), current + 1);
    Ok(true)
}

fn release_client_connection_slot(registry: &Arc<Registry>, vkey: &str) {
    let mut counts = registry.active_conn_counts.lock().unwrap();
    match counts.get(vkey).copied() {
        Some(0) | None => {}
        Some(1) => {
            counts.remove(vkey);
        }
        Some(current) => {
            counts.insert(vkey.to_string(), current - 1);
        }
    }
}

fn ensure_client_tunnel_limit(
    server: &ServerConfig,
    client: &crate::model::ClientInfo,
    tunnel_count: usize,
) -> io::Result<()> {
    if !server.allow_tunnel_num_limit || client.max_tunnel_num == 0 {
        return Ok(());
    }
    if tunnel_count >= client.max_tunnel_num {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the number of tunnels exceeds the limit",
        ));
    }
    Ok(())
}

fn tunnel_count(config: &ClientRuntimeConfig) -> usize {
    config.tunnels.len() + config.hosts.len()
}

fn validate_tunnel_ports(server: &ServerConfig, tunnel: &Tunnel) -> io::Result<()> {
    let mode = tunnel.mode.trim().to_ascii_lowercase();
    if mode == "secret" || mode == "p2p" || server.allow_ports.trim().is_empty() {
        return Ok(());
    }
    let mut ports = expand_ports(&tunnel.ports);
    if ports.is_empty() && tunnel.server_port > 0 {
        ports.push(tunnel.server_port);
    }
    for port in ports {
        if !port_allowed(&server.allow_ports, port) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("port {port} is not in allow_ports"),
            ));
        }
    }
    Ok(())
}

fn ensure_tunnel_server_port(server: &ServerConfig, tunnel: &mut Tunnel) -> io::Result<()> {
    if tunnel.server_port == 0 {
        tunnel.server_port = generate_server_port(server, &tunnel.mode, &tunnel.server_ip)?;
        tunnel.ports = tunnel.server_port.to_string();
    } else if tunnel.ports.trim().is_empty() {
        tunnel.ports = tunnel.server_port.to_string();
    }
    Ok(())
}

fn generate_server_port(server: &ServerConfig, mode: &str, server_ip: &str) -> io::Result<u16> {
    let bind_ip = if server_ip.trim().is_empty() {
        "0.0.0.0"
    } else {
        server_ip
    };
    let mode_lc = mode.to_ascii_lowercase();

    if !server.allow_ports.trim().is_empty() {
        for port in expand_ports(&server.allow_ports) {
            if port == 0 {
                continue;
            }
            let available = if mode_lc == "udp" {
                UdpSocket::bind((bind_ip, port)).is_ok()
            } else {
                TcpListener::bind((bind_ip, port)).is_ok()
            };
            if available {
                return Ok(port);
            }
        }
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no available server port found in allow_ports",
        ));
    }

    let port = if mode_lc == "udp" {
        UdpSocket::bind((bind_ip, 0))?.local_addr()?.port()
    } else {
        TcpListener::bind((bind_ip, 0))?.local_addr()?.port()
    };
    if port < 1024 {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "generated server port is invalid",
        ));
    }
    Ok(port)
}

fn port_allowed(allow_ports: &str, port: u16) -> bool {
    allow_ports
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .any(|item| {
            if let Some((start, end)) = item.split_once('-') {
                let Ok(start) = start.trim().parse::<u16>() else {
                    return false;
                };
                let Ok(end) = end.trim().parse::<u16>() else {
                    return false;
                };
                let (lo, hi) = if start <= end {
                    (start, end)
                } else {
                    (end, start)
                };
                (lo..=hi).contains(&port)
            } else {
                item.parse::<u16>() == Ok(port)
            }
        })
}

fn mux_accept_loop(registry: Arc<Registry>, vkey: String, session: Arc<MuxSession>) {
    while let Ok(stream) = session.accept() {
        let tx = registry.pending.lock().unwrap().remove(&stream.id());
        if let Some(tx) = tx {
            let _ = tx.send(Box::new(stream) as Box<dyn RelayStream>);
        }
    }
    registry.mux_sessions.lock().unwrap().remove(&vkey);
}

fn validate_client_access(registry: &Arc<Registry>, vkey: &str, remote: &str) -> io::Result<()> {
    let ip = remote_ip(remote);
    if ip.is_empty() {
        return Ok(());
    }
    if registry.server.ip_limit && !is_ip_authorized(registry.as_ref(), &ip) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the ip {ip} is not in the validation list"),
        ));
    }
    if client_flow_limit_reached(registry, vkey) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "traffic exceeded",
        ));
    }
    {
        let global = registry.global.lock().unwrap();
        if ip_in_list(&ip, &global.black_ip_list) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "remote ip is in global blacklist",
            ));
        }
    }

    let clients = registry.clients.lock().unwrap();
    let Some(client) = clients.get(vkey) else {
        return Err(io::Error::new(io::ErrorKind::NotFound, "client not found"));
    };
    let info = &client.common.client;
    if !info.status {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "client disabled",
        ));
    }
    if client.no_store && !info.config_conn_allow {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "config connection is disabled for this client",
        ));
    }
    if ip_in_list(&ip, &info.black_ip_list) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "remote ip is in client blacklist",
        ));
    }
    if info.ip_white && !ip_in_list(&ip, &info.ip_white_list) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "remote ip is not in client whitelist",
        ));
    }
    Ok(())
}

fn validate_client_handshake(
    registry: &Arc<Registry>,
    vkey: &str,
    remote: &str,
    require_config_conn_allow: bool,
) -> io::Result<()> {
    if vkey.trim().is_empty() {
        crate::log_info!("nps", "Current client connection validation error, close this client:{}", remote);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid verification key",
        ));
    }
    if require_config_conn_allow && vkey == registry.server.public_vkey {
        return Ok(());
    }

    let ip = remote_ip(remote);
    if registry.server.ip_limit && !ip.is_empty() && !is_ip_authorized(registry.as_ref(), &ip) {
        crate::log_info!("nps", "The current ip {} is not allowed to connect", ip);
        crate::log_info!("nps", "Current client connection validation error, close this client:{}", remote);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the ip {ip} is not in the validation list"),
        ));
    }
    {
        let global = registry.global.lock().unwrap();
        if !ip.is_empty() && ip_in_list(&ip, &global.black_ip_list) {
            crate::log_info!("nps", "Current client connection validation error, close this client:{}", remote);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "remote ip is in global blacklist",
            ));
        }
    }

    let clients = registry.clients.lock().unwrap();
    let Some(client) = clients.get(vkey) else {
        crate::log_info!("nps", "Current client connection validation error, close this client:{}", remote);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid verification key",
        ));
    };
    let info = &client.common.client;
    if !info.status {
        crate::log_info!("nps", "Current client connection validation error, close this client:{}", remote);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "client disabled",
        ));
    }
    if require_config_conn_allow && !info.config_conn_allow {
        crate::log_info!("nps", "Current client connection validation error, close this client:{}", remote);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "config connection is disabled for this client",
        ));
    }
    if !ip.is_empty() && ip_in_list(&ip, &info.black_ip_list) {
        crate::log_info!("nps", "Current client connection validation error, close this client:{}", remote);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "remote ip is in client blacklist",
        ));
    }
    if !ip.is_empty() && info.ip_white && !ip_in_list(&ip, &info.ip_white_list) {
        crate::log_info!("nps", "Current client connection validation error, close this client:{}", remote);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "remote ip is not in client whitelist",
        ));
    }
    Ok(())
}

fn remote_ip(remote: &str) -> String {
    if let Ok(addr) = remote.parse::<std::net::SocketAddr>() {
        return addr.ip().to_string();
    }
    strip_host_port(remote)
}

fn ensure_ip_registration_vkey(registry: &Registry, vkey: &str) -> io::Result<()> {
    if vkey.trim().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty vkey"));
    }
    if vkey == registry.server.public_vkey {
        return Ok(());
    }
    let clients = registry.clients.lock().unwrap();
    if clients.contains_key(vkey) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "client not found for ip registration",
        ))
    }
}

fn client_transport_flags(registry: &Arc<Registry>, vkey: &str, disable_transport: bool) -> (bool, bool) {
    if disable_transport {
        return (false, false);
    }
    let clients = registry.clients.lock().unwrap();
    let Some(client) = clients.get(vkey) else {
        return (false, false);
    };
    let info = &client.common.client;
    (info.crypt, !info.crypt && info.compress)
}

fn client_flow_counter(registry: &Arc<Registry>, vkey: &str) -> Arc<Mutex<FlowCounter>> {
    let mut counters = registry.client_flow_counters.lock().unwrap();
    counters
        .entry(vkey.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(FlowCounter::default())))
        .clone()
}

fn client_relay_policy(registry: &Arc<Registry>, vkey: &str) -> RelayPolicy {
    let clients = registry.clients.lock().unwrap();
    let Some(client) = clients.get(vkey) else {
        return RelayPolicy::default();
    };
    RelayPolicy {
        rate_limit_kb: if registry.server.allow_rate_limit {
            client.common.client.rate_limit_kb
        } else {
            0
        },
        flow_limit_mb: if registry.server.allow_flow_limit {
            client.common.client.flow_limit_mb
        } else {
            0
        },
        flow_counter: Some(client_flow_counter(registry, vkey)),
    }
}

fn client_flow_limit_reached(registry: &Arc<Registry>, vkey: &str) -> bool {
    if !registry.server.allow_flow_limit {
        return false;
    }
    let limit_mb = {
        let clients = registry.clients.lock().unwrap();
        clients
            .get(vkey)
            .map(|client| client.common.client.flow_limit_mb)
            .unwrap_or(0)
    };
    if limit_mb == 0 {
        return false;
    }
    let total = {
        let counter = client_flow_counter(registry, vkey);
        let counter = counter.lock().unwrap();
        counter.inlet_flow.saturating_add(counter.export_flow)
    };
    total >= limit_mb.saturating_mul(1024 * 1024)
}

fn client_flow_snapshot(registry: &Registry, vkey: &str) -> FlowCounter {
    registry
        .client_flow_counters
        .lock()
        .unwrap()
        .get(vkey)
        .map(|counter| counter.lock().unwrap().clone())
        .unwrap_or_default()
}

fn client_now_conn(registry: &Registry, vkey: &str) -> usize {
    registry
        .active_conn_counts
        .lock()
        .unwrap()
        .get(vkey)
        .copied()
        .unwrap_or(0)
}

pub fn authorize_ip(registry: &Registry, remote: &str, hours: u32) {
    let ip = remote_ip(remote);
    if ip.is_empty() {
        return;
    }
    let expires_at = SystemTime::now() + Duration::from_secs(u64::from(hours.max(1)) * 3600);
    registry.authorized_ips.lock().unwrap().insert(ip, expires_at);
}

fn is_ip_authorized(registry: &Registry, ip: &str) -> bool {
    let now = SystemTime::now();
    let mut ips = registry.authorized_ips.lock().unwrap();
    ips.retain(|_, expires_at| *expires_at > now);
    ips.get(ip).map(|expires_at| *expires_at > now).unwrap_or(false)
}

fn ip_in_list(ip: &str, items: &[String]) -> bool {
    items.iter().any(|item| item.trim() == ip)
}

fn select_target(registry: &Arc<Registry>, key: &str, tunnel: &Tunnel) -> Option<String> {
    let mut cursors = registry.cursors.lock().unwrap();
    let cursor = *cursors.get(key).unwrap_or(&0);
    let (target, next) = tunnel.target.pick(cursor)?;
    cursors.insert(key.to_string(), next);
    Some(target)
}

fn tunnel_is_active(registry: &Arc<Registry>, id: u64) -> bool {
    let clients = registry.clients.lock().unwrap();
    for config in clients.values() {
        if let Some(tunnel) = config.tunnels.iter().find(|tunnel| tunnel.id == id) {
            return config.common.client.status && tunnel.status && tunnel.run_status;
        }
    }
    false
}

fn bind_addr(tunnel: &Tunnel) -> String {
    let ip = if tunnel.server_ip.trim().is_empty() {
        "0.0.0.0"
    } else {
        tunnel.server_ip.trim()
    };
    format!("{ip}:{}", tunnel.server_port)
}

fn find_host_route(
    registry: &Arc<Registry>,
    host_header: &str,
    path: &str,
    scheme: &str,
) -> Option<(Host, String)> {
    let host = strip_host_port(host_header);
    let clients = registry.clients.lock().unwrap();
    let mut best: Option<(Host, String)> = None;

    for config in clients.values() {
        if !config.common.client.status {
            continue;
        }
        for route in &config.hosts {
            if route.is_close {
                continue;
            }
            if !route_supports_scheme(route, scheme) {
                continue;
            }
            if !host_matches(&route.host, &host) {
                continue;
            }
            if !path.starts_with(&route.location) {
                continue;
            }
            let Some((target, _)) = route.target.pick(0) else {
                continue;
            };
            let replace = best
                .as_ref()
                .map(|(old, _)| route.location.len() > old.location.len())
                .unwrap_or(true);
            if replace {
                best = Some((route.clone(), target));
            }
        }
    }
    best
}

fn build_tls_config_for_registry(registry: &Arc<Registry>) -> io::Result<Option<Arc<rustls::ServerConfig>>> {
    let clients = registry.clients.lock().unwrap();
    let mut hosts = Vec::new();
    for config in clients.values() {
        if !config.common.client.status {
            continue;
        }
        for route in &config.hosts {
            if route.is_close || !route_supports_scheme(route, "https") {
                continue;
            }
            hosts.push(route.clone());
        }
    }
    build_server_config(&hosts)
}

enum HostProxyError {
    Unauthorized,
    InvalidRequest,
}

fn prepare_host_proxy_request(
    registry: &Arc<Registry>,
    route: &Host,
    remote: &str,
    head: &[u8],
    scheme: &str,
) -> Result<(Vec<u8>, Option<String>), HostProxyError> {
    let client_auth = {
        let clients = registry.clients.lock().unwrap();
        clients
            .get(&route.client_vkey)
            .map(|client| {
                (
                    client.common.client.basic_username.clone(),
                    client.common.client.basic_password.clone(),
                )
            })
    };
    if let Some((user, pass)) = client_auth {
        if !user.is_empty() && !pass.is_empty() && !check_http_basic_auth(head, &user, &pass) {
            crate::log_warn!("nps", "auth error unauthorized {}", remote);
            return Err(HostProxyError::Unauthorized);
        }
    }

    let (method, path, _) = parse_http_request_line(head).ok_or_else(|| {
        crate::log_notice!(
            "nps",
            "the url {} can't be parsed!, host {}, remote address {}",
            route.host,
            route.host,
            remote
        );
        HostProxyError::InvalidRequest
    })?;
    let cache_key = cache_key_for_request(registry, scheme, route, &method, &path);
    let prepared = rewrite_http_request_head(
        head,
        Some(if route.host_change.trim().is_empty() {
            &route.host
        } else {
            &route.host_change
        }),
        &route.header_change,
        remote,
        registry.server.http_add_origin_header,
        cache_key.is_some(),
    );
    Ok((prepared, cache_key))
}

fn proxy_host_request<S>(
    registry: &Arc<Registry>,
    mut inbound: S,
    route: Host,
    remote: String,
    target: String,
    head: Vec<u8>,
    cache_key: Option<String>,
) -> io::Result<()>
where
    S: Read + Write + Send + 'static,
{
    if let Some((method, path, _)) = parse_http_request_line(&head) {
        crate::log_info!(
            "nps",
            "http request, method {}, host {}, url {}, remote address {}, target {}",
            method,
            route.host,
            path,
            remote,
            target
        );
    }

    if route.auto_https && registry.server.https_proxy_port > 0 {
        let redirect = format!(
            "https://{}:{}{}",
            strip_host_port(&route.host),
            registry.server.https_proxy_port,
            route.location
        );
        let body = format!(
            "<html><body><a href=\"{redirect}\">Moved Permanently</a></body></html>"
        );
        let header = format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: {redirect}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        inbound.write_all(header.as_bytes())?;
        inbound.write_all(body.as_bytes())?;
        inbound.flush()?;
        return Ok(());
    }

    if let Some(key) = cache_key.as_ref() {
        if let Some(cached) = registry.http_cache.lock().unwrap().get(key) {
            inbound.write_all(&cached)?;
            inbound.flush()?;
            return Ok(());
        }
    }

    let (crypt, compress) = client_transport_flags(registry, &route.client_vkey, false);
    let target_for_log = target.clone();
    let link = Link {
        kind: LinkKind::Http,
        target,
        remote_addr: remote,
        crypt,
        compress,
        local_proxy: route.target.local_proxy,
        proto_version: String::new(),
        file: None,
    };
    let mut client_stream = match request_client_stream(registry, &route.client_vkey, link) {
        Ok(stream) => stream,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            let client_id = registry
                .clients
                .lock()
                .unwrap()
                .get(&route.client_vkey)
                .map(|client| client.id)
                .unwrap_or(0);
            crate::log_warn!(
                "nps",
                "client id {}, host id {}, error {}, when https connection",
                client_id,
                route.id,
                err
            );
            write_custom_404(&mut inbound, registry)?;
            return Ok(());
        }
        Err(err) => {
            crate::log_notice!("nps", "connect to target {} error {}", target_for_log, err);
            write_http_response(
                &mut inbound,
                "502 Bad Gateway",
                "text/plain",
                b"client offline",
            )?;
            return Err(err);
        }
    };
    client_stream.write_all(&head)?;

    if let Some(key) = cache_key {
        return proxy_and_cache_http_response(registry, inbound, client_stream, key);
    }
    copy_bidirectional_legacy(inbound, client_stream)
}

fn proxy_and_cache_http_response<S>(
    registry: &Arc<Registry>,
    mut inbound: S,
    mut client_stream: Box<dyn RelayStream>,
    cache_key: String,
) -> io::Result<()>
where
    S: Read + Write + Send + 'static,
{
    let head = read_http_head(&mut client_stream, 64 * 1024)?;
    let mut response = head.clone();
    let mut body = Vec::new();
    client_stream.read_to_end(&mut body)?;
    response.extend_from_slice(&body);

    inbound.write_all(&response)?;
    inbound.flush()?;

    if parse_http_status_code(&head) == Some(200) && response.len() <= 2 * 1024 * 1024 {
        let limit = registry.server.http_cache_length;
        registry
            .http_cache
            .lock()
            .unwrap()
            .insert(cache_key, response, limit);
    }
    Ok(())
}

fn cache_key_for_request(
    registry: &Arc<Registry>,
    scheme: &str,
    route: &Host,
    method: &str,
    path: &str,
) -> Option<String> {
    if !registry.server.http_cache || !method.eq_ignore_ascii_case("GET") || !is_cacheable_path(path)
    {
        return None;
    }
    Some(format!("{scheme}|{}|{path}", route.host.to_ascii_lowercase()))
}

fn is_cacheable_path(path: &str) -> bool {
    let plain = path.split('?').next().unwrap_or(path).trim();
    let ext = plain.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "css"
            | "js"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "svg"
            | "ico"
            | "webp"
            | "woff"
            | "woff2"
            | "ttf"
            | "map"
            | "txt"
            | "html"
            | "htm"
    )
}

fn write_custom_404<W: Write + ?Sized>(stream: &mut W, registry: &Registry) -> io::Result<()> {
    let body = load_error_page(registry).unwrap_or_else(|_| b"nps 404".to_vec());
    write_http_response(stream, "404 Not Found", "text/html; charset=utf-8", &body)
}

fn write_basic_auth_required<W: Write + ?Sized>(stream: &mut W) -> io::Result<()> {
    let body = b"401 Unauthorized";
    let header = format!(
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nWWW-Authenticate: Basic realm=\"easyProxy\"\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn load_error_page(registry: &Registry) -> io::Result<Vec<u8>> {
    let mut candidates = Vec::new();
    candidates.push(
        registry
            .store
            .conf_dir()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("web")
            .join("static")
            .join("page")
            .join("error.html"),
    );
    candidates.push(Path::new("web").join("static").join("page").join("error.html"));
    candidates.push(
        Path::new("RustNps")
            .join("web")
            .join("static")
            .join("page")
            .join("error.html"),
    );
    for candidate in candidates {
        if let Ok(bytes) = fs::read(&candidate) {
            return Ok(bytes);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "error page not found",
    ))
}

fn route_supports_scheme(route: &Host, scheme: &str) -> bool {
    match route.scheme.trim().to_ascii_lowercase().as_str() {
        "" | "all" => true,
        "http" => scheme.eq_ignore_ascii_case("http"),
        "https" => scheme.eq_ignore_ascii_case("https"),
        other => other.eq_ignore_ascii_case(scheme),
    }
}

fn host_matches(rule: &str, host: &str) -> bool {
    let rule = strip_host_port(rule).to_ascii_lowercase();
    let host = strip_host_port(host).to_ascii_lowercase();
    if rule == host {
        return true;
    }
    if let Some(suffix) = rule.strip_prefix("*.") {
        return host.ends_with(suffix);
    }
    false
}

fn strip_host_port(host: &str) -> String {
    let clean = host.trim().trim_end_matches('.');
    if clean.starts_with('[') {
        return clean.to_string();
    }
    match clean.rsplit_once(':') {
        Some((name, port)) if port.parse::<u16>().is_ok() => name.to_string(),
        _ => clean.to_string(),
    }
}

pub fn dashboard_json(registry: &Registry) -> String {
    dashboard_json_scoped(registry, None)
}

pub fn dashboard_json_scoped(registry: &Registry, scope_client_id: Option<u64>) -> String {
    let clients = registry.clients.lock().unwrap();
    let controls = registry.controls.lock().unwrap();
    let scoped_clients: Vec<_> = clients
        .values()
        .filter(|client| scope_client_id.map(|id| client.id == id).unwrap_or(true))
        .collect();
    let host_count: usize = scoped_clients.iter().map(|c| c.hosts.len()).sum();
    let tunnel_count: usize = scoped_clients.iter().map(|c| c.tunnels.len()).sum();
    let tcp_count = scoped_clients
        .iter()
        .flat_map(|c| c.tunnels.iter())
        .filter(|t| t.mode.eq_ignore_ascii_case("tcp"))
        .count();
    let udp_count = scoped_clients
        .iter()
        .flat_map(|c| c.tunnels.iter())
        .filter(|t| t.mode.eq_ignore_ascii_case("udp"))
        .count();
    let socks_count = scoped_clients
        .iter()
        .flat_map(|c| c.tunnels.iter())
        .filter(|t| t.mode.eq_ignore_ascii_case("socks5"))
        .count();
    let http_count = scoped_clients
        .iter()
        .flat_map(|c| c.tunnels.iter())
        .filter(|t| t.mode.eq_ignore_ascii_case("httpProxy"))
        .count();
    let secret_count = scoped_clients
        .iter()
        .flat_map(|c| c.tunnels.iter())
        .filter(|t| t.mode.eq_ignore_ascii_case("secret"))
        .count();
    let p2p_count = scoped_clients
        .iter()
        .flat_map(|c| c.tunnels.iter())
        .filter(|t| t.mode.eq_ignore_ascii_case("p2p"))
        .count();
    let online_clients: Vec<_> = controls
        .iter()
        .filter(|(vkey, _)| {
            clients
                .get(*vkey)
                .map(|client| scope_client_id.map(|id| client.id == id).unwrap_or(true))
                .unwrap_or(false)
        })
        .map(|(vkey, handle)| {
            serde_json::json!({
                "vkey": vkey,
                "version": &handle.version,
                "connectedSeconds": handle.connected_at.elapsed().unwrap_or_default().as_secs(),
            })
        })
        .collect();
    let (inlet_flow_count, export_flow_count) = scoped_clients.iter().fold((0_u64, 0_u64), |acc, client| {
        let flow = client_flow_snapshot(registry, &client.common.vkey);
        (
            acc.0.saturating_add(flow.inlet_flow),
            acc.1.saturating_add(flow.export_flow),
        )
    });

    let history = registry.system_history.lock().unwrap();
    let latest = history.back().cloned().unwrap_or_default();

    let mut data = serde_json::json!({
        "version": VERSION,
        "coreVersion": CORE_VERSION,
        "clientCount": scoped_clients.len(),
        "clientOnlineCount": online_clients.len(),
        "onlineClients": online_clients,
        "hostCount": host_count,
        "tunnelCount": tunnel_count,
        "tcpC": tcp_count,
        "udpCount": udp_count,
        "socks5Count": socks_count,
        "httpProxyCount": http_count,
        "secretCount": secret_count,
        "p2pCount": p2p_count,
        "inletFlowCount": inlet_flow_count,
        "exportFlowCount": export_flow_count,
        "tcpCount": tcp_count,
        "cpu": latest.cpu,
        "virtual_mem": latest.virtual_mem,
        "total_mem": latest.total_mem,
        "load": serde_json::to_string(&[latest.load1, latest.load5, latest.load15]).unwrap(),
        "tcp": latest.tcp,
        "udp": latest.udp,
        "io_send": latest.io_send,
        "io_recv": latest.io_recv,
        "bridgeType": &registry.server.bridge_type,
        "httpProxyPort": registry.server.http_proxy_port,
        "httpsProxyPort": registry.server.https_proxy_port,
        "ipLimit": registry.server.ip_limit,
        "flowStoreInterval": 0,
        "p2pPort": registry.server.p2p_port,
        "serverIp": registry.server.p2p_ip,
        "logLevel": 6,
    });

    let obj = data.as_object_mut().unwrap();
    for i in 1..=10 {
        let snapshot = history.get(i - 1).cloned().unwrap_or_default();
        obj.insert(format!("sys{}", i), serde_json::to_value(snapshot).unwrap());
    }

    serde_json::to_string(&data).unwrap()
}

pub fn param(params: &HashMap<String, String>, key: &str) -> String {
    params.get(key).cloned().unwrap_or_default()
}

pub fn param_bool(params: &HashMap<String, String>, key: &str) -> bool {
    matches!(
        param(params, key).to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn param_any(params: &HashMap<String, String>, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| {
            let value = param(params, key);
            (!value.is_empty()).then_some(value)
        })
        .unwrap_or_default()
}

fn ajax(status: u8, msg: &str) -> String {
    serde_json::json!({ "status": status, "msg": msg }).to_string()
}

fn ajax_with_id(status: u8, msg: &str, id: u64) -> String {
    serde_json::json!({ "status": status, "msg": msg, "id": id }).to_string()
}

pub fn authenticate_web_user(
    registry: &Arc<Registry>,
    username: &str,
    password: &str,
) -> Option<WebSession> {
    if username == registry.server.web_username && password == registry.server.web_password {
        return Some(WebSession::admin(username.to_string()));
    }
    if !registry.server.allow_user_login {
        return None;
    }

    let clients = registry.clients.lock().unwrap();
    clients.values().find_map(|client| {
        let info = &client.common.client;
        if !info.status || client.no_display {
            return None;
        }
        let default_user_login = info.web_username.is_empty()
            && info.web_password.is_empty()
            && username == "user"
            && password == client.common.vkey;
        let explicit_user_login = !info.web_username.is_empty()
            && info.web_username == username
            && info.web_password == password;
        if default_user_login || explicit_user_login {
            Some(WebSession::client(client.id, username.to_string()))
        } else {
            None
        }
    })
}

pub fn login_captcha_block(registry: &Registry) -> String {
    if !registry.server.open_captcha {
        return String::new();
    }
    let Some(token) = issue_login_captcha(registry) else {
        return String::new();
    };
    let base = &registry.server.web_base_url;
    format!(
        r#"<div class="form-group" style="display:flex;"><input type="hidden" name="captcha_token" value="{token}"><input class="form-control" name="captcha" placeholder="captcha" required langtag="word-captcha" style="border-top-right-radius:0;border-bottom-right-radius:0;border-right:0;"><img src="{base}/captcha/?token={token}" onclick="fetch(location.href).then(r=>r.text()).then(h=>{{let m=h.match(/name=\x22captcha_token\x22 value=\x22([^\x22]+)\x22/);if(m){{let p=this.previousElementSibling;p.value='';p.previousElementSibling.value=m[1];this.src='{base}/captcha/?token='+m[1]}}}})" alt="captcha" style="height:34px; cursor:pointer; border:1px solid #e5e6e7; border-radius:0 4px 4px 0; background:#fff;" title="refresh captcha" /></div>"#
    )
}

pub fn captcha_svg(registry: &Registry, token: &str) -> Option<String> {
    let now = SystemTime::now();
    let mut store = registry.captcha_store.lock().unwrap();
    let (code, expires_at) = store.get(token).map(|entry| (entry.code.clone(), entry.expires_at))?;
    if expires_at <= now {
        store.remove(token);
        return None;
    }
    Some(render_captcha_svg(&code))
}

pub fn verify_login_captcha(registry: &Registry, token: &str, answer: &str) -> bool {
    if !registry.server.open_captcha {
        return true;
    }
    let token = token.trim();
    let answer = answer.trim();
    if token.is_empty() || answer.is_empty() {
        return false;
    }
    let mut store = registry.captcha_store.lock().unwrap();
    let Some(entry) = store.remove(token) else {
        return false;
    };
    if entry.expires_at <= SystemTime::now() {
        return false;
    }
    entry.code.eq_ignore_ascii_case(answer)
}

fn issue_login_captcha(registry: &Registry) -> Option<String> {
    let token = md5_hex(&format!("{:?}:{}", SystemTime::now(), registry.next_link_id()));
    let code = captcha_code_from_token(&token);
    let expires_at = SystemTime::now()
        .checked_add(Duration::from_secs(300))
        .unwrap_or(SystemTime::now());
    registry.captcha_store.lock().unwrap().insert(
        token.clone(),
        CaptchaEntry { code, expires_at },
    );
    Some(token)
}

fn captcha_code_from_token(token: &str) -> String {
    let digest = md5_hex(token).to_uppercase();
    digest.chars().take(5).collect()
}

fn render_captcha_svg(code: &str) -> String {
    let digest = md5_hex(code);
    let mut seeded = digest.bytes();
    let width = 140;
    let height = 44;
    let mut noise = String::new();
    for _ in 0..5 {
        let x1 = next_seed(&mut seeded) % width;
        let y1 = next_seed(&mut seeded) % height;
        let x2 = next_seed(&mut seeded) % width;
        let y2 = next_seed(&mut seeded) % height;
        noise.push_str(&format!(r##"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#c8c8c8" stroke-width="1" />"##));
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect x="0" y="0" width="{width}" height="{height}" rx="4" fill="#f8fafc" stroke="#d6d8de"/>{noise}<text x="50%" y="58%" dominant-baseline="middle" text-anchor="middle" font-family="Arial, Helvetica, sans-serif" font-size="24" font-weight="700" fill="#1f2937" letter-spacing="4">{code}</text></svg>"##
    )
}

fn next_seed(seed: &mut impl Iterator<Item = u8>) -> u32 {
    seed.next().unwrap_or(0) as u32
}

pub fn authorize_web_login_ip(registry: &Registry, remote: &str) {
    authorize_ip(registry, remote, 2);
}

pub fn register_web_user(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    if !registry.server.allow_user_register {
        return ajax(0, "register is not allow");
    }
    let username = param(params, "username");
    let password = param(params, "password");
    if username.is_empty() || password.is_empty() || username == registry.server.web_username {
        return ajax(0, "please check your input");
    }

    {
        let clients = registry.clients.lock().unwrap();
        if clients
            .values()
            .any(|client| client.common.client.web_username == username)
        {
            return ajax(0, "web login username duplicate, please reset");
        }
    }

    let id = registry.next_link_id();
    let mut vkey = format!("user{id}");
    {
        let clients = registry.clients.lock().unwrap();
        while clients.contains_key(&vkey) {
            vkey = format!("user{}", registry.next_link_id());
        }
    }

    let mut config = ClientRuntimeConfig::default();
    config.id = id;
    config.no_store = false;
    config.create_time = now_text();
    config.common = CommonConfig::default();
    config.common.vkey = vkey.clone();
    config.common.client.verify_key = vkey.clone();
    config.common.client.status = true;
    config.common.client.web_username = username;
    config.common.client.web_password = password;
    config.common.client.config_conn_allow = false;

    registry.clients.lock().unwrap().insert(vkey, config);
    persist_ajax(registry, "register success")
}

pub fn client_rows_json(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let rows = client_rows(registry);
    let search = param(params, "search").to_ascii_lowercase();
    let client_id = param(params, "client_id").parse::<u64>().unwrap_or(0);
    let filtered: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            (client_id == 0 || row["Id"].as_u64().unwrap_or(0) == client_id)
                && (search.is_empty()
                    || row["Remark"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&search)
                    || row["VerifyKey"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&search))
        })
        .collect();
    let total = filtered.len();
    serde_json::json!({ "rows": page_slice(filtered, params), "total": total }).to_string()
}

pub fn tunnel_rows_json(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let rows = tunnel_rows(registry);
    let type_filter = param(params, "type");
    let client_id = param(params, "client_id").parse::<u64>().unwrap_or(0);
    let search = param(params, "search").to_ascii_lowercase();
    let filtered: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            (type_filter.is_empty() || row["Mode"].as_str().unwrap_or("") == type_filter)
                && (client_id == 0 || row["Client"]["Id"].as_u64().unwrap_or(0) == client_id)
                && (search.is_empty()
                    || row["Remark"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&search)
                    || row["Target"]["TargetStr"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&search))
        })
        .collect();
    let total = filtered.len();
    serde_json::json!({ "rows": page_slice(filtered, params), "total": total }).to_string()
}

pub fn host_rows_json(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let rows = host_rows(registry);
    let client_id = param(params, "client_id").parse::<u64>().unwrap_or(0);
    let search = param(params, "search").to_ascii_lowercase();
    let filtered: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            (client_id == 0 || row["Client"]["Id"].as_u64().unwrap_or(0) == client_id)
                && (search.is_empty()
                    || row["Host"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&search)
                    || row["Remark"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&search))
        })
        .collect();
    let total = filtered.len();
    serde_json::json!({ "rows": page_slice(filtered, params), "total": total }).to_string()
}

fn page_slice(
    rows: Vec<serde_json::Value>,
    params: &HashMap<String, String>,
) -> Vec<serde_json::Value> {
    let offset = param(params, "offset").parse::<usize>().unwrap_or(0);
    let limit = param(params, "limit")
        .parse::<usize>()
        .unwrap_or(rows.len().max(1));
    rows.into_iter().skip(offset).take(limit).collect()
}

pub fn client_rows(registry: &Arc<Registry>) -> Vec<serde_json::Value> {
    let clients = registry.clients.lock().unwrap();
    let controls = registry.controls.lock().unwrap();
    let mut keys: Vec<_> = clients.keys().cloned().collect();
    keys.sort();
    keys.into_iter()
        .filter_map(|vkey| {
            let config = clients.get(&vkey)?;
            let info = &config.common.client;
            let online = controls.contains_key(&vkey);
            let version = controls
                .get(&vkey)
                .map(|h| h.version.clone())
                .unwrap_or_default();
            let addr = controls
                .get(&vkey)
                .map(|h| h.remote_addr.clone())
                .filter(|addr| !addr.is_empty())
                .or_else(|| {
                    let addr = config.last_online_addr.clone();
                    if addr.is_empty() { None } else { Some(addr) }
                })
                .unwrap_or_default();
            let id = config.id;
            let flow = client_flow_snapshot(registry, &vkey);
            Some(serde_json::json!({
                "Id": id,
                "Remark": info.remark,
                "Version": version,
                "VerifyKey": vkey,
                "Addr": addr,
                "Cnf": { "U": info.basic_username, "P": info.basic_password, "Compress": info.compress, "Crypt": info.crypt },
                "Flow": { "InletFlow": flow.inlet_flow, "ExportFlow": flow.export_flow, "FlowLimit": info.flow_limit_mb },
                "Rate": { "NowRate": 0 },
                "RateLimit": info.rate_limit_kb,
                "MaxConn": info.max_conn,
                "NowConn": client_now_conn(registry, &vkey),
                "Status": info.status,
                "IsConnect": online,
                "NoStore": config.no_store,
                "NoDisplay": config.no_display,
                "MaxTunnelNum": info.max_tunnel_num,
                "WebUserName": info.web_username,
                "WebPassword": info.web_password,
                "ConfigConnAllow": info.config_conn_allow,
                "BlackIpList": info.black_ip_list,
                "IpWhite": info.ip_white,
                "IpWhitePass": info.ip_white_pass,
                "IpWhiteList": info.ip_white_list,
                "CreateTime": config.create_time,
                "LastOnlineTime": config.last_online_time,
                "LastOnlineAddr": config.last_online_addr,
                "Command": { "Server": format!("{}:{}", registry.server.bridge_ip, registry.server.bridge_port), "Host": "127.0.0.1" }
            }))
        })
        .collect()
}

fn update_client_last_online_addr(registry: &Arc<Registry>, vkey: &str, addr: &str) {
    if addr.is_empty() {
        return;
    }
    let mut clients = registry.clients.lock().unwrap();
    if let Some(client) = clients.get_mut(vkey) {
        client.last_online_addr = addr.to_string();
    }
}

pub fn tunnel_rows(registry: &Arc<Registry>) -> Vec<serde_json::Value> {
    let clients = registry.clients.lock().unwrap();
    let controls = registry.controls.lock().unwrap();
    let mut keys: Vec<_> = clients.keys().cloned().collect();
    keys.sort();
    let mut rows = Vec::new();
    for vkey in keys.into_iter() {
        let Some(config) = clients.get(&vkey) else {
            continue;
        };
        let client_json = client_json(
            registry,
            config.id,
            &vkey,
            config,
            controls.contains_key(&vkey),
            controls
                .get(&vkey)
                .map(|h| h.version.as_str())
                .unwrap_or(""),
        );
        for tunnel in &config.tunnels {
            rows.push(serde_json::json!({
                "Id": tunnel.id,
                "ClientId": config.id,
                "Remark": tunnel.remark,
                "Mode": if tunnel.mode.eq_ignore_ascii_case("all") { "tcp" } else { tunnel.mode.as_str() },
                "Port": tunnel.server_port,
                "Ports": tunnel.ports,
                "ServerIp": tunnel.server_ip,
                "Target": { "TargetStr": tunnel.target.target_str, "LocalProxy": tunnel.target.local_proxy },
                "Password": tunnel.password,
                "Status": tunnel.status,
                "RunStatus": tunnel.run_status,
                "LocalPath": tunnel.local_path,
                "StripPre": tunnel.strip_pre,
                "ProtoVersion": tunnel.proto_version,
                "Flow": { "InletFlow": 0, "ExportFlow": 0, "FlowLimit": 0 },
                "Client": client_json,
                "Command": { "Server": format!("{}:{}", registry.server.bridge_ip, registry.server.bridge_port), "Host": "127.0.0.1" }
            }));
        }
    }
    rows
}

pub fn host_rows(registry: &Arc<Registry>) -> Vec<serde_json::Value> {
    let clients = registry.clients.lock().unwrap();
    let controls = registry.controls.lock().unwrap();
    let mut keys: Vec<_> = clients.keys().cloned().collect();
    keys.sort();
    let mut rows = Vec::new();
    for vkey in keys.into_iter() {
        let Some(config) = clients.get(&vkey) else {
            continue;
        };
        let client_json = client_json(
            registry,
            config.id,
            &vkey,
            config,
            controls.contains_key(&vkey),
            controls
                .get(&vkey)
                .map(|h| h.version.as_str())
                .unwrap_or(""),
        );
        for host in &config.hosts {
            rows.push(serde_json::json!({
                "Id": host.id,
                "ClientId": config.id,
                "Remark": host.remark,
                "Host": host.host,
                "Scheme": host.scheme,
                "Target": { "TargetStr": host.target.target_str, "LocalProxy": host.target.local_proxy },
                "Location": host.location,
                "HeaderChange": host.header_change,
                "HostChange": host.host_change,
                "IsClose": host.is_close,
                "CertFilePath": host.cert_file_path,
                "KeyFilePath": host.key_file_path,
                "Flow": { "InletFlow": 0, "ExportFlow": 0, "FlowLimit": 0 },
                "Client": client_json
            }));
        }
    }
    rows
}

fn client_json(
    registry: &Registry,
    id: u64,
    vkey: &str,
    config: &ClientRuntimeConfig,
    online: bool,
    version: &str,
) -> serde_json::Value {
    let info = &config.common.client;
    let flow = client_flow_snapshot(registry, vkey);
    serde_json::json!({
        "Id": id,
        "Remark": info.remark,
        "Version": version,
        "VerifyKey": vkey,
        "Status": info.status,
        "IsConnect": online,
        "Cnf": { "U": info.basic_username, "P": info.basic_password, "Compress": info.compress, "Crypt": info.crypt },
        "Flow": { "InletFlow": flow.inlet_flow, "ExportFlow": flow.export_flow, "FlowLimit": info.flow_limit_mb }
    })
}

pub fn mutate_client_status(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let status = param_bool(params, "status");
    let changed = {
        let mut clients = registry.clients.lock().unwrap();
        if let Some(vkey) = client_vkey_by_id(&clients, id) {
            if let Some(config) = clients.get_mut(&vkey) {
                let previous = config.clone();
                config.common.client.status = status;
                Some((vkey, previous, config.clone()))
            } else {
                None
            }
        } else {
            None
        }
    };
    if let Some((vkey, previous, current)) = changed {
        crate::log_info!(
            "web",
            "client status mutation id={} vkey={} status={}",
            current.id,
            vkey,
            status
        );
        stop_tunnel_runtimes(registry, &previous.tunnels);
        if status {
            start_active_tunnels_for_client(registry, &current);
        } else {
            terminate_client_connection(registry, &vkey, "client disabled");
        }
        rebuild_secret_registry(registry);
        persist_ajax(registry, "modified success")
    } else {
        ajax(0, "modified fail")
    }
}

pub fn mutate_client_delete(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let deleted = {
        let mut clients = registry.clients.lock().unwrap();
        if let Some(vkey) = client_vkey_by_id(&clients, id) {
            clients.remove(&vkey).map(|config| (vkey, config))
        } else {
            None
        }
    };
    if let Some((vkey, removed)) = deleted {
        crate::log_info!("web", "client delete mutation id={} vkey={}", removed.id, vkey);
        stop_tunnel_runtimes(registry, &removed.tunnels);
        terminate_client_connection(registry, &vkey, "client deleted");
        rebuild_secret_registry(registry);
        persist_ajax(registry, "delete success")
    } else {
        ajax(0, "delete error")
    }
}

pub fn mutate_client_add(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let mut config = ClientRuntimeConfig::default();
    config.common = CommonConfig::default();
    let vkey = {
        let supplied = param(params, "vkey");
        if supplied.is_empty() {
            format!("web{}", registry.next_link_id())
        } else {
            supplied
        }
    };
    let web_username = param(params, "web_username");
    {
        let clients = registry.clients.lock().unwrap();
        if clients.contains_key(&vkey) {
            return ajax(0, "Vkey duplicate, please reset");
        }
        if !web_username.is_empty()
            && clients
                .values()
                .any(|client| client.common.client.web_username == web_username)
        {
            return ajax(0, "web login username duplicate, please reset");
        }
    }
    config.common.vkey = vkey.clone();
    config.common.client.verify_key = vkey.clone();
    apply_client_form(&mut config.common.client, params);
    config.common.client.web_username = web_username;
    config.common.client.web_password = param(params, "web_password");
    let id = registry.next_link_id();
    config.id = id;
    config.no_store = false;
    config.create_time = now_text();
    crate::log_info!("web", "client add mutation id={} vkey={}", id, config.common.vkey);
    registry.clients.lock().unwrap().insert(vkey, config);
    match persist_registry(registry) {
        Ok(()) => ajax_with_id(1, "add success", id),
        Err(err) => ajax(0, &format!("added but save failed: {err}")),
    }
}

pub fn mutate_client_edit(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let edited = {
        let mut clients = registry.clients.lock().unwrap();
        let Some(vkey) = client_vkey_by_id(&clients, id) else {
            return ajax(0, "client ID not found");
        };
        let new_vkey = {
            let candidate = param(params, "vkey");
            if candidate.is_empty() {
                vkey.clone()
            } else {
                candidate
            }
        };
        if new_vkey != vkey && clients.contains_key(&new_vkey) {
            return ajax(0, "Vkey duplicate, please reset");
        }
        let web_username = params.get("web_username").cloned();
        if web_username
            .as_ref()
            .map(|value| !value.is_empty())
            .unwrap_or(false)
            && clients.values().any(|client| {
                client.id != id
                    && Some(client.common.client.web_username.as_str())
                        == web_username.as_ref().map(String::as_str)
            })
        {
            return ajax(0, "web login username duplicate, please reset");
        }

        let Some(mut config) = clients.remove(&vkey) else {
            return ajax(0, "client ID not found");
        };
        let previous = config.clone();
        config.common.vkey = new_vkey.clone();
        config.common.client.verify_key = new_vkey.clone();
        apply_client_form(&mut config.common.client, params);
        if let Some(web_username) = web_username {
            config.common.client.web_username = web_username;
        }
        config.common.client.web_password = param(params, "web_password");
        for tunnel in &mut config.tunnels {
            tunnel.client_vkey = new_vkey.clone();
        }
        for host in &mut config.hosts {
            host.client_vkey = new_vkey.clone();
        }
        let current = config.clone();
        clients.insert(new_vkey, config);
        Some((vkey, previous, current))
    };
    if let Some((old_vkey, previous, current)) = edited {
        crate::log_info!(
            "web",
            "client edit mutation id={} old_vkey={} new_vkey={}",
            current.id,
            old_vkey,
            current.common.vkey
        );
        if old_vkey != current.common.vkey || previous.common.client.status != current.common.client.status {
            stop_tunnel_runtimes(registry, &previous.tunnels);
            terminate_client_connection(
                registry,
                &old_vkey,
                if old_vkey != current.common.vkey {
                    "invalid verification key"
                } else {
                    "client disabled"
                },
            );
            if current.common.client.status {
                start_active_tunnels_for_client(registry, &current);
            }
        }
        rebuild_secret_registry(registry);
        persist_ajax(registry, "save success")
    } else {
        ajax(0, "client ID not found")
    }
}

fn apply_client_form(info: &mut crate::model::ClientInfo, params: &HashMap<String, String>) {
    info.remark = param(params, "remark");
    info.basic_username = param(params, "u");
    info.basic_password = param(params, "p");
    info.compress = param_bool(params, "compress");
    info.crypt = param_bool(params, "crypt");
    info.rate_limit_kb = param(params, "rate_limit").parse::<u64>().unwrap_or(0);
    info.flow_limit_mb = param(params, "flow_limit").parse::<u64>().unwrap_or(0);
    info.max_conn = param(params, "max_conn").parse::<usize>().unwrap_or(0);
    info.max_tunnel_num = param(params, "max_tunnel").parse::<usize>().unwrap_or(0);
    info.ip_white = param_bool(params, "ipwhite");
    info.ip_white_pass = param_any(params, &["ipwhitepass", "ip_white_pass"]);
    info.ip_white_list = param_any(params, &["ipwhitelist", "ip_white_list"])
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    info.black_ip_list = param_any(params, &["blackiplist", "black_ip_list"])
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if let Some(config_conn_allow) = params.get("config_conn_allow") {
        info.config_conn_allow = matches!(
            config_conn_allow.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }
}

pub fn mutate_tunnel_status(
    registry: &Arc<Registry>,
    params: &HashMap<String, String>,
    status: bool,
) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let modified = {
        let mut clients = registry.clients.lock().unwrap();
        let mut result = None;
        for config in clients.values_mut() {
            if let Some(tunnel) = config.tunnels.iter_mut().find(|t| t.id == id) {
                let previous = tunnel.clone();
                tunnel.status = status;
                tunnel.run_status = status;
                result = Some((previous, tunnel.clone()));
                break;
            }
        }
        result
    };
    if let Some((previous, current)) = modified {
        crate::log_info!(
            "web",
            "tunnel status mutation id={} client_vkey={} status={}",
            current.id,
            current.client_vkey,
            status
        );
        stop_tunnel_runtime(registry, &previous);
        if status {
            start_tunnel_task(Arc::clone(registry), current);
        }
        rebuild_secret_registry(registry);
        persist_ajax(
            registry,
            if status {
                "start success"
            } else {
                "stop success"
            },
        )
    } else {
        ajax(0, if status { "start error" } else { "stop error" })
    }
}

pub fn mutate_tunnel_delete(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let deleted = {
        let mut clients = registry.clients.lock().unwrap();
        let mut result = None;
        for config in clients.values_mut() {
            if let Some(index) = config.tunnels.iter().position(|t| t.id == id) {
                result = Some(config.tunnels.remove(index));
                break;
            }
        }
        result
    };
    if let Some(tunnel) = deleted {
        crate::log_info!(
            "web",
            "tunnel delete mutation id={} client_vkey={} remark={}",
            tunnel.id,
            tunnel.client_vkey,
            tunnel.remark
        );
        stop_tunnel_runtime(registry, &tunnel);
        rebuild_secret_registry(registry);
        persist_ajax(registry, "delete success")
    } else {
        ajax(0, "delete error")
    }
}

pub fn mutate_tunnel_copy(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let mut to_start = None;
    {
        let mut clients = registry.clients.lock().unwrap();
        for config in clients.values_mut() {
            if let Some(tunnel) = config.tunnels.iter().find(|t| t.id == id).cloned() {
                if let Err(err) = ensure_client_tunnel_limit(
                    &registry.server,
                    &config.common.client,
                    tunnel_count(config),
                ) {
                    return ajax(0, &err.to_string());
                }
                let mut clone = tunnel;
                clone.id = registry.next_link_id();
                clone.server_port = 0;
                clone.ports.clear();
                if let Err(err) = ensure_tunnel_server_port(&registry.server, &mut clone) {
                    return ajax(0, &err.to_string());
                }
                clone.status = false;
                clone.run_status = false;
                clone.no_store = false;
                config.tunnels.push(clone.clone());
                to_start = Some(clone);
                break;
            }
        }
    }
    if let Some(tunnel) = to_start {
        crate::log_info!(
            "web",
            "tunnel copy mutation id={} client_vkey={} remark={}",
            tunnel.id,
            tunnel.client_vkey,
            tunnel.remark
        );
        if tunnel.server_port > 0 {
            start_tunnel_task(Arc::clone(registry), tunnel);
        }
        persist_ajax(registry, "add success")
    } else {
        ajax(0, "copy error")
    }
}

pub fn mutate_tunnel_add(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let client_id = param(params, "client_id").parse::<u64>().unwrap_or(1);
    let mut tunnel = Tunnel::default();
    tunnel.id = registry.next_link_id();
    tunnel.mode = param(params, "type");
    if tunnel.mode.is_empty() {
        tunnel.mode = "tcp".to_string();
    }
    tunnel.ports = param_any(params, &["ports", "port"]);
    tunnel.server_port = expand_ports(&tunnel.ports).into_iter().next().unwrap_or(0);
    tunnel.server_ip = param_any(params, &["server_ip", "serverIp"]);
    if tunnel.server_ip.is_empty() {
        tunnel.server_ip = "0.0.0.0".to_string();
    }
    tunnel.target = Target {
        target_str: param(params, "target"),
        local_proxy: param_bool(params, "local_proxy"),
    };
    tunnel.remark = param(params, "remark");
    tunnel.password = param(params, "password");
    tunnel.local_path = param(params, "local_path");
    tunnel.strip_pre = param(params, "strip_pre");
    tunnel.proto_version = param_any(params, &["proto_version", "proxy_protocol"]);
    tunnel.status = true;
    tunnel.run_status = true;
    tunnel.no_store = false;
    if let Err(err) = ensure_tunnel_server_port(&registry.server, &mut tunnel) {
        return ajax(0, &err.to_string());
    }
    if let Err(err) = validate_tunnel_ports(&registry.server, &tunnel) {
        return ajax(0, &err.to_string());
    }
    let mut should_start = None;
    {
        let mut clients = registry.clients.lock().unwrap();
        if let Some(vkey) = client_vkey_by_id(&clients, client_id) {
            tunnel.client_vkey = vkey.clone();
            if let Some(config) = clients.get_mut(&vkey) {
                if let Err(err) = ensure_client_tunnel_limit(
                    &registry.server,
                    &config.common.client,
                    tunnel_count(config),
                ) {
                    return ajax(0, &err.to_string());
                }
                config.tunnels.push(tunnel.clone());
                should_start = Some(tunnel.clone());
            }
        }
    }
    if let Some(tunnel) = should_start {
        crate::log_info!(
            "web",
            "tunnel add mutation id={} client_vkey={} mode={} port={} remark={}",
            tunnel.id,
            tunnel.client_vkey,
            tunnel.mode,
            tunnel.server_port,
            tunnel.remark
        );
        start_tunnel_task(Arc::clone(registry), tunnel.clone());
        rebuild_secret_registry(registry);
        match persist_registry(registry) {
            Ok(()) => ajax_with_id(1, "add success", tunnel.id),
            Err(err) => ajax(0, &format!("added but save failed: {err}")),
        }
    } else {
        ajax(0, "add error the client can not be found")
    }
}

pub fn mutate_tunnel_edit(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let modified = {
        let mut clients = registry.clients.lock().unwrap();
        let mut result = None;
        for config in clients.values_mut() {
            if let Some(tunnel) = config.tunnels.iter_mut().find(|t| t.id == id) {
                let previous = tunnel.clone();
                let ports = param_any(params, &["ports", "port"]);
                let requested_port = expand_ports(&ports).into_iter().next().unwrap_or(0);
                if requested_port != tunnel.server_port {
                    tunnel.ports = ports;
                    tunnel.server_port = requested_port;
                }
                let server_ip = param_any(params, &["server_ip", "serverIp"]);
                if !server_ip.is_empty() {
                    tunnel.server_ip = server_ip;
                }
                let mode = param(params, "type");
                if !mode.is_empty() {
                    tunnel.mode = mode;
                }
                tunnel.target.target_str = param(params, "target");
                tunnel.target.local_proxy = param_bool(params, "local_proxy");
                tunnel.remark = param(params, "remark");
                tunnel.password = param(params, "password");
                tunnel.local_path = param(params, "local_path");
                tunnel.strip_pre = param(params, "strip_pre");
                tunnel.proto_version = param_any(params, &["proto_version", "proxy_protocol"]);
                tunnel.no_store = false;
                if let Err(err) = ensure_tunnel_server_port(&registry.server, tunnel) {
                    return ajax(0, &err.to_string());
                }
                if let Err(err) = validate_tunnel_ports(&registry.server, tunnel) {
                    return ajax(0, &err.to_string());
                }
                result = Some((previous, tunnel.clone()));
                break;
            }
        }
        result
    };
    if let Some((previous, current)) = modified {
        crate::log_info!(
            "web",
            "tunnel edit mutation id={} client_vkey={} mode={} port={} remark={}",
            current.id,
            current.client_vkey,
            current.mode,
            current.server_port,
            current.remark
        );
        stop_tunnel_runtime(registry, &previous);
        if current.status {
            start_tunnel_task(Arc::clone(registry), current);
        }
        rebuild_secret_registry(registry);
        persist_ajax(registry, "modified success")
    } else {
        ajax(0, "modified error")
    }
}

pub fn mutate_host_status(
    registry: &Arc<Registry>,
    params: &HashMap<String, String>,
    is_close: bool,
) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let modified = {
        let mut clients = registry.clients.lock().unwrap();
        let mut found = false;
        for config in clients.values_mut() {
            if let Some(host) = config.hosts.iter_mut().find(|h| h.id == id) {
                host.is_close = is_close;
                found = true;
                break;
            }
        }
        found
    };
    if modified {
        crate::log_info!("web", "host status mutation id={} is_close={}", id, is_close);
        persist_ajax(
            registry,
            if is_close {
                "stop success"
            } else {
                "start success"
            },
        )
    } else {
        ajax(
            0,
            if is_close {
                "stop error"
            } else {
                "start error"
            },
        )
    }
}

pub fn mutate_host_delete(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let deleted = {
        let mut clients = registry.clients.lock().unwrap();
        let mut removed = false;
        for config in clients.values_mut() {
            let before = config.hosts.len();
            config.hosts.retain(|h| h.id != id);
            if config.hosts.len() != before {
                removed = true;
                break;
            }
        }
        removed
    };
    if deleted {
        crate::log_info!("web", "host delete mutation id={}", id);
        persist_ajax(registry, "delete success")
    } else {
        ajax(0, "delete error")
    }
}

pub fn mutate_host_add(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let client_id = param(params, "client_id").parse::<u64>().unwrap_or(1);
    let mut host = Host::default();
    host.id = registry.next_link_id();
    host.no_store = false;
    fill_host_from_params(&mut host, params);
    let added_id = host.id;
    let added = {
        let mut clients = registry.clients.lock().unwrap();
        if let Some(vkey) = client_vkey_by_id(&clients, client_id) {
            host.client_vkey = vkey.clone();
            if let Some(config) = clients.get_mut(&vkey) {
                if let Err(err) = ensure_client_tunnel_limit(
                    &registry.server,
                    &config.common.client,
                    tunnel_count(config),
                ) {
                    return ajax(0, &err.to_string());
                }
                if config
                    .hosts
                    .iter()
                    .any(|item| item.host == host.host && item.location == host.location)
                {
                    return ajax(0, "host has exist");
                }
                config.hosts.push(host.clone());
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if added {
        crate::log_info!(
            "web",
            "host add mutation id={} client_id={} host={} location={}",
            added_id,
            client_id,
            host.host,
            host.location
        );
        match persist_registry(registry) {
            Ok(()) => ajax_with_id(1, "add success", added_id),
            Err(err) => ajax(0, &format!("added but save failed: {err}")),
        }
    } else {
        ajax(0, "add error the client can not be found")
    }
}

pub fn mutate_host_edit(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    let id = param(params, "id").parse::<u64>().unwrap_or(0);
    let modified = {
        let mut clients = registry.clients.lock().unwrap();
        let mut found = false;
        for config in clients.values_mut() {
            if let Some(index) = config.hosts.iter().position(|h| h.id == id) {
                let mut updated = config.hosts[index].clone();
                fill_host_from_params(&mut updated, params);
                updated.no_store = false;
                if config.hosts.iter().any(|item| {
                    item.id != id && item.host == updated.host && item.location == updated.location
                }) {
                    return ajax(0, "host has exist");
                }
                config.hosts[index] = updated;
                found = true;
                break;
            }
        }
        found
    };
    if modified {
        crate::log_info!("web", "host edit mutation id={}", id);
        persist_ajax(registry, "modified success")
    } else {
        ajax(0, "modified error")
    }
}

pub fn mutate_global_save(registry: &Arc<Registry>, params: &HashMap<String, String>) -> String {
    {
        let mut global = registry.global.lock().unwrap();
        global.server_url = param_any(params, &["serverUrl", "server_url"]);
        global.black_ip_list = param_any(params, &["globalBlackIpList", "black_ip_list"])
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    crate::log_info!("web", "global save mutation");
    persist_ajax(registry, "save success")
}

fn fill_host_from_params(host: &mut Host, params: &HashMap<String, String>) {
    host.host = param(params, "host");
    host.target.target_str = param(params, "target");
    host.target.local_proxy = param_bool(params, "local_proxy");
    host.header_change = param(params, "header");
    host.host_change = param(params, "hostchange");
    host.remark = param(params, "remark");
    host.cert_file_path = param_any(params, &["cert_file_path", "certFilePath"]);
    host.key_file_path = param_any(params, &["key_file_path", "keyFilePath"]);
    host.auto_https = param_bool_any(params, &["auto_https", "AutoHttps"]);
    host.location = {
        let location = param(params, "location");
        if location.is_empty() {
            "/".to_string()
        } else if location.starts_with('/') {
            location
        } else {
            format!("/{location}")
        }
    };
    host.scheme = {
        let scheme = param(params, "scheme");
        if scheme.is_empty() {
            "all".to_string()
        } else {
            scheme
        }
    };
}

fn param_bool_any(params: &HashMap<String, String>, keys: &[&str]) -> bool {
    keys.iter().any(|key| param_bool(params, key))
}

fn client_vkey_by_id(clients: &HashMap<String, ClientRuntimeConfig>, id: u64) -> Option<String> {
    clients
        .iter()
        .find_map(|(vkey, config)| (config.id == id).then(|| vkey.clone()))
}

fn persist_registry(registry: &Registry) -> io::Result<()> {
    let clients = registry.clients.lock().unwrap();
    let global = registry.global.lock().unwrap();
    registry.store.save_all(&clients, &global)
}

fn persist_ajax(registry: &Registry, success_msg: &str) -> String {
    match persist_registry(registry) {
        Ok(()) => ajax(1, success_msg),
        Err(err) => ajax(0, &format!("modified but save failed: {err}")),
    }
}

fn now_text() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    for (idx, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
        if arg == name {
            if let Some(value) = args.get(idx + 1) {
                return Some(value.clone());
            }
        }
    }
    None
}

fn print_nps_help() {
    println!(
        r#"RustNps nps {VERSION} (core {CORE_VERSION})

Usage:
  nps [options]
  nps -h | --help
  nps -version | --version

Options:
  -conf_path <file>        Path to nps.conf. Default: conf/nps.conf
  --conf-path <file>       Same as -conf_path
    --console-log-level <info|debug>
                                                     Console log level. Default: info.
                                                     debug prints detailed troubleshooting logs.
  -log_path <file>         Reserved for service/file logging compatibility
  -version, --version      Show version information
  -h, --help               Show this help

Common nps.conf keys:
  bridge_ip / bridge_port        Server-client bridge listen address
  web_ip / web_port              Web management listen address
  web_username / web_password    Administrator login
  http_proxy_ip / http_proxy_port  HTTP host proxy listener
  allow_user_login/register/change_username  Multi-user login and registration switches

Examples:
  nps -conf_path conf/nps.conf
    nps --conf-path conf/nps.conf --console-log-level debug

Go nps service commands such as install/start/stop/restart are not implemented in
this Rust reconstruction yet; run the binary directly or manage it with your OS
service manager."#
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::fs;
    use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};

    #[test]
    fn host_mutations_persist_to_store_files() {
        let root = temp_test_dir("rustnps-host-persist");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));
        install_test_client(&registry, 101, "client-a", true);

        let add = HashMap::from([
            ("client_id".to_string(), "101".to_string()),
            ("host".to_string(), "example.com".to_string()),
            ("target".to_string(), "127.0.0.1:8080".to_string()),
            ("location".to_string(), "/api".to_string()),
            ("scheme".to_string(), "https".to_string()),
            ("cert_file_path".to_string(), "CERT".to_string()),
            ("key_file_path".to_string(), "KEY".to_string()),
        ]);
        assert!(mutate_host_add(&registry, &add).contains("add success"));

        let state = registry.store.load().unwrap();
        assert_eq!(state.hosts.len(), 1);
        assert_eq!(state.hosts[0].host, "example.com");
        assert_eq!(state.hosts[0].scheme, "https");
        assert_eq!(state.hosts[0].cert_file_path, "CERT");

        let host_id = state.hosts[0].id;
        let edit = HashMap::from([
            ("id".to_string(), host_id.to_string()),
            ("host".to_string(), "example.com".to_string()),
            ("target".to_string(), "127.0.0.1:9090".to_string()),
            ("location".to_string(), "/v2".to_string()),
            ("scheme".to_string(), "all".to_string()),
            ("cert_file_path".to_string(), "CERT2".to_string()),
            ("key_file_path".to_string(), "KEY2".to_string()),
        ]);
        assert!(mutate_host_edit(&registry, &edit).contains("modified success"));

        let state = registry.store.load().unwrap();
        assert_eq!(state.hosts.len(), 1);
        assert_eq!(state.hosts[0].location, "/v2");
        assert_eq!(state.hosts[0].target.target_str, "127.0.0.1:9090");
        assert_eq!(state.hosts[0].cert_file_path, "CERT2");

        let delete = HashMap::from([("id".to_string(), host_id.to_string())]);
        assert!(mutate_host_delete(&registry, &delete).contains("delete success"));

        let state = registry.store.load().unwrap();
        assert!(state.hosts.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn host_status_and_delete_take_effect_in_runtime_route_lookup() {
        let root = temp_test_dir("rustnps-host-runtime");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));
        install_test_client(&registry, 111, "host-runtime", false);

        let add = HashMap::from([
            ("client_id".to_string(), "111".to_string()),
            ("host".to_string(), "example.com".to_string()),
            ("target".to_string(), "127.0.0.1:8080".to_string()),
            ("location".to_string(), "/api".to_string()),
            ("scheme".to_string(), "http".to_string()),
        ]);
        assert!(mutate_host_add(&registry, &add).contains("add success"));

        let host_id = {
            let clients = registry.clients.lock().unwrap();
            clients
                .get("host-runtime")
                .and_then(|client| client.hosts.first())
                .map(|host| host.id)
                .unwrap()
        };

        let route = find_host_route(&registry, "example.com", "/api/users", "http").unwrap();
        assert_eq!(route.0.id, host_id);
        assert_eq!(route.1, "127.0.0.1:8080");

        let stop = HashMap::from([("id".to_string(), host_id.to_string())]);
        assert!(mutate_host_status(&registry, &stop, true).contains("stop success"));
        assert!(find_host_route(&registry, "example.com", "/api/users", "http").is_none());

        let start = HashMap::from([("id".to_string(), host_id.to_string())]);
        assert!(mutate_host_status(&registry, &start, false).contains("start success"));
        let route = find_host_route(&registry, "example.com", "/api/users", "http").unwrap();
        assert_eq!(route.0.id, host_id);

        let delete = HashMap::from([("id".to_string(), host_id.to_string())]);
        assert!(mutate_host_delete(&registry, &delete).contains("delete success"));
        assert!(find_host_route(&registry, "example.com", "/api/users", "http").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tunnel_mutations_persist_to_store_files() {
        let root = temp_test_dir("rustnps-tunnel-persist");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));
        install_test_client(&registry, 202, "client-b", true);

        let add = HashMap::from([
            ("client_id".to_string(), "202".to_string()),
            ("type".to_string(), "tcp".to_string()),
            ("port".to_string(), "0".to_string()),
            ("target".to_string(), "127.0.0.1:22".to_string()),
            ("remark".to_string(), "ssh".to_string()),
        ]);
        assert!(mutate_tunnel_add(&registry, &add).contains("add success"));

        let state = registry.store.load().unwrap();
        assert_eq!(state.tunnels.len(), 1);
        assert_eq!(state.tunnels[0].target.target_str, "127.0.0.1:22");
        let tunnel_id = state.tunnels[0].id;

        let edit = HashMap::from([
            ("id".to_string(), tunnel_id.to_string()),
            ("type".to_string(), "udp".to_string()),
            ("port".to_string(), "0".to_string()),
            ("target".to_string(), "127.0.0.1:53".to_string()),
            ("remark".to_string(), "dns".to_string()),
        ]);
        assert!(mutate_tunnel_edit(&registry, &edit).contains("modified success"));

        let state = registry.store.load().unwrap();
        assert_eq!(state.tunnels.len(), 1);
        assert_eq!(state.tunnels[0].mode, "udp");
        assert_eq!(state.tunnels[0].remark, "dns");
        assert_eq!(state.tunnels[0].target.target_str, "127.0.0.1:53");

        let delete = HashMap::from([("id".to_string(), tunnel_id.to_string())]);
        assert!(mutate_tunnel_delete(&registry, &delete).contains("delete success"));

        let state = registry.store.load().unwrap();
        assert!(state.tunnels.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validate_client_handshake_rejects_unknown_vkey() {
        let root = temp_test_dir("rustnps-handshake-invalid");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));

        let err = validate_client_handshake(&registry, "missing-vkey", "203.0.113.10:1234", false)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("invalid verification key"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn disabling_client_terminates_online_control() {
        let root = temp_test_dir("rustnps-disable-client");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));
        install_test_client(&registry, 303, "client-c", false);

        let (tx, rx) = std::sync::mpsc::channel();
        registry.controls.lock().unwrap().insert(
            "client-c".to_string(),
            ControlHandle {
                tx,
                version: "test".to_string(),
                connected_at: SystemTime::now(),
                remote_addr: "127.0.0.1:1234".to_string(),
            },
        );

        let status = HashMap::from([
            ("id".to_string(), "303".to_string()),
            ("status".to_string(), "0".to_string()),
        ]);
        assert!(mutate_client_status(&registry, &status).contains("modified success"));
        let stop = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        match stop {
            ServerMessage::Stop { reason } => assert!(reason.contains("disabled")),
            other => panic!("unexpected control message: {other:?}"),
        }
        assert!(!registry.controls.lock().unwrap().contains_key("client-c"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rotating_client_vkey_terminates_old_online_connection() {
        let root = temp_test_dir("rustnps-rotate-vkey");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));
        install_test_client(&registry, 304, "old-vkey", false);

        let (tx, rx) = std::sync::mpsc::channel();
        registry.controls.lock().unwrap().insert(
            "old-vkey".to_string(),
            ControlHandle {
                tx,
                version: "test".to_string(),
                connected_at: SystemTime::now(),
                remote_addr: "127.0.0.1:1234".to_string(),
            },
        );

        let edit = HashMap::from([
            ("id".to_string(), "304".to_string()),
            ("vkey".to_string(), "new-vkey".to_string()),
            ("web_password".to_string(), "pass".to_string()),
        ]);
        assert!(mutate_client_edit(&registry, &edit).contains("save success"));

        let stop = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        match stop {
            ServerMessage::Stop { reason } => {
                assert!(reason.contains("invalid verification key"));
            }
            other => panic!("unexpected control message: {other:?}"),
        }

        let clients = registry.clients.lock().unwrap();
        assert!(clients.contains_key("new-vkey"));
        assert!(!clients.contains_key("old-vkey"));
        drop(clients);
        assert!(!registry.controls.lock().unwrap().contains_key("old-vkey"));
        assert!(
            validate_client_handshake(&registry, "old-vkey", "127.0.0.1:1234", false).is_err()
        );
        validate_client_handshake(&registry, "new-vkey", "127.0.0.1:1234", false).unwrap();

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tunnel_edit_switches_listener_from_old_port_to_new_port() {
        let root = temp_test_dir("rustnps-tunnel-hot-switch");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));
        install_test_client(&registry, 404, "switch-vkey", false);

        let old_port = free_tcp_port();
        let mut new_port = free_tcp_port();
        if new_port == old_port {
            new_port = free_tcp_port();
        }

        let add = HashMap::from([
            ("client_id".to_string(), "404".to_string()),
            ("type".to_string(), "tcp".to_string()),
            ("port".to_string(), old_port.to_string()),
            ("target".to_string(), "127.0.0.1:22".to_string()),
            ("remark".to_string(), "switch-test".to_string()),
        ]);
        assert!(mutate_tunnel_add(&registry, &add).contains("add success"));
        assert!(wait_until_port_accepting(old_port));

        let tunnel_id = {
            let clients = registry.clients.lock().unwrap();
            clients
                .get("switch-vkey")
                .and_then(|client| client.tunnels.first())
                .map(|tunnel| tunnel.id)
                .unwrap()
        };

        let edit = HashMap::from([
            ("id".to_string(), tunnel_id.to_string()),
            ("type".to_string(), "tcp".to_string()),
            ("port".to_string(), new_port.to_string()),
            ("target".to_string(), "127.0.0.1:22".to_string()),
            ("remark".to_string(), "switch-test-edited".to_string()),
        ]);
        assert!(mutate_tunnel_edit(&registry, &edit).contains("modified success"));
        assert!(wait_until_port_closed(old_port));
        assert!(wait_until_port_accepting(new_port));

        let delete = HashMap::from([("id".to_string(), tunnel_id.to_string())]);
        assert!(mutate_tunnel_delete(&registry, &delete).contains("delete success"));
        assert!(wait_until_port_closed(new_port));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stopping_tunnel_disconnects_tracked_live_connections() {
        let root = temp_test_dir("rustnps-stop-disconnect-live");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut client_side = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let server_side = listener.accept().unwrap().0;

        let tunnel = Tunnel {
            id: 9090,
            mode: "tcp".to_string(),
            server_ip: "127.0.0.1".to_string(),
            server_port: port,
            ..Tunnel::default()
        };
        let conn_id = register_tunnel_live_conn(&registry, tunnel.id, &server_side);
        assert!(conn_id.is_some());

        stop_tunnel_runtime(&registry, &tunnel);

        client_side
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let _ = client_side.write_all(b"x");
        let mut buf = [0_u8; 8];
        let read = client_side.read(&mut buf);
        assert!(matches!(read, Ok(0) | Err(_)));
        assert!(!registry
            .tunnel_live_conns
            .lock()
            .unwrap()
            .contains_key(&tunnel.id));

        let _ = fs::remove_dir_all(root);
    }

    fn install_test_client(registry: &Arc<Registry>, id: u64, vkey: &str, no_store: bool) {
        let mut client = ClientRuntimeConfig::default();
        client.id = id;
        client.no_store = no_store;
        client.common = CommonConfig::default();
        client.common.vkey = vkey.to_string();
        client.common.client.verify_key = vkey.to_string();
        registry.clients.lock().unwrap().insert(vkey.to_string(), client);
    }

    fn temp_test_dir(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn free_tcp_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn wait_until_port_accepting(port: u16) -> bool {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        for _ in 0..40 {
            if TcpStream::connect_timeout(&addr.into(), Duration::from_millis(50)).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn wait_until_port_closed(port: u16) -> bool {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        for _ in 0..40 {
            if TcpStream::connect_timeout(&addr.into(), Duration::from_millis(50)).is_err() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn caches_static_get_requests_only() {
        let root = temp_test_dir("rustnps-cache");
        let mut server = ServerConfig::default();
        server.http_cache = true;
        server.http_cache_length = 2;
        let registry = Arc::new(Registry::new(server, PersistentStore::new(&root)));
        let mut host = Host::default();
        host.host = "a.test".to_string();

        assert!(cache_key_for_request(&registry, "http", &host, "GET", "/app.js").is_some());
        assert!(cache_key_for_request(&registry, "http", &host, "POST", "/app.js").is_none());
        assert!(cache_key_for_request(&registry, "http", &host, "GET", "/api").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blocks_host_add_when_tunnel_limit_reached() {
        let root = temp_test_dir("rustnps-host-limit");
        let mut server = ServerConfig::default();
        server.allow_tunnel_num_limit = true;
        let registry = Arc::new(Registry::new(server, PersistentStore::new(&root)));

        let mut client = ClientRuntimeConfig::default();
        client.id = 7;
        client.common.vkey = "limit-vkey".to_string();
        client.common.client.max_tunnel_num = 1;
        client.hosts.push(Host {
            id: 8,
            client_vkey: client.common.vkey.clone(),
            host: "a.test".to_string(),
            ..Host::default()
        });
        registry
            .clients
            .lock()
            .unwrap()
            .insert(client.common.vkey.clone(), client);

        let add = HashMap::from([
            ("client_id".to_string(), "7".to_string()),
            ("host".to_string(), "b.test".to_string()),
            ("target".to_string(), "127.0.0.1:8080".to_string()),
        ]);
        assert!(mutate_host_add(&registry, &add).contains("exceeds the limit"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blocks_connection_when_client_max_conn_reached() {
        let root = temp_test_dir("rustnps-conn-limit");
        let mut server = ServerConfig::default();
        server.allow_connection_num_limit = true;
        let registry = Arc::new(Registry::new(server, PersistentStore::new(&root)));

        let mut client = ClientRuntimeConfig::default();
        client.common.vkey = "limit-vkey".to_string();
        client.common.client.max_conn = 1;
        registry
            .clients
            .lock()
            .unwrap()
            .insert(client.common.vkey.clone(), client);

        assert!(acquire_client_connection_slot(&registry, "limit-vkey").unwrap());
        let err = acquire_client_connection_slot(&registry, "limit-vkey").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        release_client_connection_slot(&registry, "limit-vkey");
        assert!(acquire_client_connection_slot(&registry, "limit-vkey").unwrap());
        release_client_connection_slot(&registry, "limit-vkey");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_tunnel_ports_outside_allow_list() {
        let root = temp_test_dir("rustnps-port-allow");
        let mut server = ServerConfig::default();
        server.allow_ports = "9001-9002,10000".to_string();
        let registry = Arc::new(Registry::new(server, PersistentStore::new(&root)));

        let mut client = ClientRuntimeConfig::default();
        client.id = 9;
        client.common.vkey = "port-vkey".to_string();
        registry
            .clients
            .lock()
            .unwrap()
            .insert(client.common.vkey.clone(), client);

        let add = HashMap::from([
            ("client_id".to_string(), "9".to_string()),
            ("type".to_string(), "tcp".to_string()),
            ("port".to_string(), "9003".to_string()),
            ("target".to_string(), "127.0.0.1:8080".to_string()),
        ]);
        assert!(mutate_tunnel_add(&registry, &add).contains("allow_ports"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ip_limit_blocks_unregistered_ip_and_allows_registered_ip() {
        let root = temp_test_dir("rustnps-ip-limit");
        let mut server = ServerConfig::default();
        server.ip_limit = true;
        let registry = Arc::new(Registry::new(server, PersistentStore::new(&root)));

        let mut client = ClientRuntimeConfig::default();
        client.common.vkey = "limit-vkey".to_string();
        registry
            .clients
            .lock()
            .unwrap()
            .insert(client.common.vkey.clone(), client);

        let err = validate_client_access(&registry, "limit-vkey", "203.0.113.9:1234").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        authorize_ip(registry.as_ref(), "203.0.113.9:1234", 2);
        validate_client_access(&registry, "limit-vkey", "203.0.113.9:1234").unwrap();

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn web_login_authorizes_ip_for_two_hours() {
        let root = temp_test_dir("rustnps-web-ip-limit");
        let registry = Arc::new(Registry::new(ServerConfig::default(), PersistentStore::new(&root)));

        authorize_web_login_ip(registry.as_ref(), "198.51.100.8:9000");
        assert!(is_ip_authorized(registry.as_ref(), "198.51.100.8"));

        let _ = fs::remove_dir_all(root);
    }
}

fn default_server_conf() -> String {
    for candidate in ["conf/nps.conf", "../nps/conf/nps.conf", "nps/conf/nps.conf"] {
        if fs::metadata(candidate).is_ok() {
            return candidate.to_string();
        }
    }
    "conf/nps.conf".to_string()
}

fn resolve_web_root_from_conf_path(conf_path: &Path) -> Option<std::path::PathBuf> {
    let absolute_conf = if conf_path.is_absolute() {
        conf_path.to_path_buf()
    } else {
        env::current_dir().ok()?.join(conf_path)
    };

    for ancestor in absolute_conf.ancestors() {
        let candidate = ancestor.join("web");
        if candidate.is_dir() && candidate.join("views").is_dir() {
            return Some(candidate);
        }
    }
    None
}
