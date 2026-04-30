use crate::model::{
    ClientRuntimeConfig, CommonConfig, HealthCheck, Host, LocalServer, ServerConfig, Tunnel,
};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::Path;

type SectionMap = HashMap<String, String>;

pub fn load_server_config(path: impl AsRef<Path>) -> io::Result<ServerConfig> {
    let raw = fs::read_to_string(path)?;
    let kv = parse_plain_kv(&render_env(&raw));
    let mut c = ServerConfig::default();

    c.appname = get_string(&kv, "appname", &c.appname);
    c.runmode = get_string(&kv, "runmode", &c.runmode);
    c.http_proxy_ip = get_string(&kv, "http_proxy_ip", &c.http_proxy_ip);
    c.http_proxy_port = get_u16(&kv, "http_proxy_port", c.http_proxy_port);
    c.https_proxy_port = get_u16(&kv, "https_proxy_port", c.https_proxy_port);
    c.https_just_proxy = get_bool(&kv, "https_just_proxy", c.https_just_proxy);
    c.bridge_type = get_string(&kv, "bridge_type", &c.bridge_type);
    c.bridge_ip = get_string(&kv, "bridge_ip", &c.bridge_ip);
    c.bridge_port = get_u16(&kv, "bridge_port", c.bridge_port);
    c.public_vkey = get_string(&kv, "public_vkey", &c.public_vkey);
    c.web_ip = get_string(&kv, "web_ip", &c.web_ip);
    c.web_port = get_u16(&kv, "web_port", c.web_port);
    c.web_username = get_string(&kv, "web_username", &c.web_username);
    c.web_password = get_string(&kv, "web_password", &c.web_password);
    c.web_base_url = get_string(&kv, "web_base_url", &c.web_base_url);
    c.allow_user_login = get_bool(&kv, "allow_user_login", c.allow_user_login);
    c.allow_user_register = get_bool(&kv, "allow_user_register", c.allow_user_register);
    c.open_captcha = get_bool(&kv, "open_captcha", c.open_captcha);
    c.allow_user_change_username = get_bool(
        &kv,
        "allow_user_change_username",
        c.allow_user_change_username,
    );
    c.allow_flow_limit = get_bool(&kv, "allow_flow_limit", c.allow_flow_limit);
    c.allow_rate_limit = get_bool(&kv, "allow_rate_limit", c.allow_rate_limit);
    c.allow_tunnel_num_limit = get_bool(&kv, "allow_tunnel_num_limit", c.allow_tunnel_num_limit);
    c.allow_connection_num_limit = get_bool(
        &kv,
        "allow_connection_num_limit",
        c.allow_connection_num_limit,
    );
    c.allow_ports = get_string(&kv, "allow_ports", &c.allow_ports);
    c.allow_local_proxy = get_bool(&kv, "allow_local_proxy", c.allow_local_proxy);
    c.allow_multi_ip = get_bool(&kv, "allow_multi_ip", c.allow_multi_ip);
    c.http_cache = get_bool(&kv, "http_cache", c.http_cache);
    c.http_cache_length = get_usize(&kv, "http_cache_length", c.http_cache_length);
    c.http_add_origin_header = get_bool(&kv, "http_add_origin_header", c.http_add_origin_header);
    c.ip_limit = get_bool(&kv, "ip_limit", c.ip_limit);
    c.disconnect_timeout = get_u64(&kv, "disconnect_timeout", c.disconnect_timeout);
    c.tls_enable = get_bool(&kv, "tls_enable", c.tls_enable);
    c.tls_bridge_port = get_u16(&kv, "tls_bridge_port", c.tls_bridge_port);
    c.p2p_ip = get_string(&kv, "p2p_ip", &c.p2p_ip);
    c.p2p_port = get_u16(&kv, "p2p_port", c.p2p_port);
    c.log_level = get_string(&kv, "log_level", &c.log_level);
    c.log_path = get_string(&kv, "log_path", &c.log_path);
    Ok(c)
}

pub fn load_client_config(path: impl AsRef<Path>) -> io::Result<ClientRuntimeConfig> {
    let raw = fs::read_to_string(path)?;
    let sections = parse_ini_sections(&render_env(&raw));
    let mut runtime = ClientRuntimeConfig::default();
    runtime.no_store = true;
    let mut next_id = 1_u64;

    for (title, kv) in sections {
        let title_lc = title.to_ascii_lowercase();
        if title_lc == "common" {
            runtime.common = parse_common(&kv);
            runtime.common.client.verify_key = runtime.common.vkey.clone();
            continue;
        }

        if title_lc.starts_with("health") {
            runtime.healths.push(parse_health(&title, &kv));
            continue;
        }

        let has_mode = kv.contains_key("mode");
        if (title_lc.starts_with("secret") || title_lc.starts_with("p2p")) && !has_mode {
            runtime
                .local_servers
                .push(parse_local_server(&title_lc, &kv));
            continue;
        }

        if kv.contains_key("host") {
            let mut host = parse_host(&title, &kv);
            host.id = next_id;
            host.client_vkey = runtime.common.vkey.clone();
            next_id += 1;
            runtime.hosts.push(host);
        } else {
            let mut tunnel = parse_tunnel(&title, &kv);
            tunnel.id = next_id;
            tunnel.client_vkey = runtime.common.vkey.clone();
            next_id += 1;
            runtime.tunnels.push(tunnel);
        }
    }

    Ok(runtime)
}

fn parse_common(kv: &SectionMap) -> CommonConfig {
    let mut c = CommonConfig::default();
    c.server_addr = get_string(kv, "server_addr", &c.server_addr);
    c.conn_type = get_string(kv, "conn_type", &c.conn_type);
    c.vkey = get_string(kv, "vkey", &c.vkey);
    c.auto_reconnection = get_bool(kv, "auto_reconnection", c.auto_reconnection);
    c.tls_enable = get_bool(kv, "tls_enable", c.tls_enable);
    c.proxy_url = get_string(kv, "proxy_url", &c.proxy_url);
    c.disconnect_timeout = get_u64(kv, "disconnect_timeout", c.disconnect_timeout);
    c.client.basic_username = get_string(kv, "basic_username", "");
    c.client.basic_password = get_string(kv, "basic_password", "");
    c.client.web_username = get_string(kv, "web_username", &c.client.web_username);
    c.client.web_password = get_string(kv, "web_password", &c.vkey);
    c.client.config_conn_allow = get_bool(kv, "config_conn_allow", c.client.config_conn_allow);
    c.client.compress = get_bool(kv, "compress", false);
    c.client.crypt = get_bool(kv, "crypt", false);
    c.client.rate_limit_kb = get_u64(kv, "rate_limit", 0);
    c.client.flow_limit_mb = get_u64(kv, "flow_limit", 0);
    c.client.max_conn = get_usize(kv, "max_conn", 0);
    c.client.remark = get_string(kv, "remark", "");
    c.client.max_tunnel_num = get_usize(kv, "max_tunnel_num", 0);
    c
}

fn parse_host(title: &str, kv: &SectionMap) -> Host {
    let mut h = Host::default();
    h.remark = title.to_string();
    h.host = get_string(kv, "host", "");
    h.target.target_str = get_string(kv, "target_addr", "").replace(',', "\n");
    h.host_change = get_string(kv, "host_change", "");
    h.scheme = get_string(kv, "scheme", &h.scheme);
    h.cert_file_path = get_string(kv, "cert_file_path", "");
    h.key_file_path = get_string(kv, "key_file_path", "");
    h.location = normalize_location(&get_string(kv, "location", &h.location));
    h.auto_https = get_bool(kv, "auto_https", false);

    let mut header_change = String::new();
    for (k, v) in kv {
        if let Some(header_name) = k.strip_prefix("header_") {
            header_change.push_str(header_name);
            header_change.push(':');
            header_change.push_str(v);
            header_change.push('\n');
        }
    }
    h.header_change = header_change;
    h
}

fn parse_tunnel(title: &str, kv: &SectionMap) -> Tunnel {
    let mut t = Tunnel::default();
    t.remark = title.to_string();
    t.mode = get_string(kv, "mode", "tcp");
    t.ports = get_string(kv, "server_port", "");
    t.server_port = first_port(&t.ports).unwrap_or(0);
    t.server_ip = get_string(kv, "server_ip", &t.server_ip);
    t.target.target_str = get_string(kv, "target_addr", "").replace(',', "\n");
    t.target_addr = get_string(kv, "target_ip", "");
    let target_port = get_string(kv, "target_port", "");
    if !target_port.is_empty() && t.target.target_str.is_empty() {
        t.target.target_str = target_port;
    }
    t.password = get_string(kv, "password", "");
    t.local_path = get_string(kv, "local_path", "");
    t.strip_pre = get_string(kv, "strip_pre", "");
    t.proto_version = get_string(kv, "proxy_protocol", "");
    t
}

fn parse_local_server(kind: &str, kv: &SectionMap) -> LocalServer {
    LocalServer {
        kind: if kind.starts_with("p2p") {
            "p2p".to_string()
        } else {
            "secret".to_string()
        },
        ip: get_string(kv, "local_ip", "127.0.0.1"),
        port: get_u16(kv, "local_port", 0),
        password: get_string(kv, "password", ""),
        target: get_string(kv, "target_addr", ""),
    }
}

fn parse_health(title: &str, kv: &SectionMap) -> HealthCheck {
    HealthCheck {
        remark: title.to_string(),
        timeout_secs: get_u64(kv, "health_check_timeout", 1),
        max_failed: get_u64(kv, "health_check_max_failed", 3) as u32,
        interval_secs: get_u64(kv, "health_check_interval", 1),
        http_url: get_string(kv, "health_http_url", "/"),
        kind: get_string(kv, "health_check_type", "tcp"),
        targets: get_string(kv, "health_check_target", "")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    }
}

pub fn expand_ports(s: &str) -> Vec<u16> {
    let mut out = Vec::new();
    for part in s.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) = (a.trim().parse::<u16>(), b.trim().parse::<u16>()) {
                let (lo, hi) = if start <= end {
                    (start, end)
                } else {
                    (end, start)
                };
                out.extend(lo..=hi);
            }
        } else if let Ok(port) = part.parse::<u16>() {
            out.push(port);
        }
    }
    out
}

pub fn expand_targets(base_ip: &str, target: &str) -> Vec<String> {
    target
        .replace(',', "\n")
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            if item.contains(':') {
                item.to_string()
            } else if !base_ip.is_empty() {
                format!("{base_ip}:{item}")
            } else {
                format!("127.0.0.1:{item}")
            }
        })
        .collect()
}

fn first_port(s: &str) -> Option<u16> {
    expand_ports(s).into_iter().next()
}

fn parse_plain_kv(raw: &str) -> SectionMap {
    let mut map = HashMap::new();
    for line in raw.lines() {
        if let Some((k, v)) = parse_kv_line(line) {
            map.insert(k, v);
        }
    }
    map
}

fn parse_ini_sections(raw: &str) -> Vec<(String, SectionMap)> {
    let mut sections = Vec::new();
    let mut current_name = String::new();
    let mut current_map = HashMap::new();

    for line in raw.lines() {
        let trimmed = strip_comment(line).trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if !current_name.is_empty() {
                sections.push((current_name, current_map));
                current_map = HashMap::new();
            }
            current_name = trimmed
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string();
            continue;
        }
        if let Some((k, v)) = parse_kv_line(trimmed) {
            current_map.insert(k, v);
        }
    }

    if !current_name.is_empty() {
        sections.push((current_name, current_map));
    }
    sections
}

fn parse_kv_line(line: &str) -> Option<(String, String)> {
    let clean = strip_comment(line);
    let (k, v) = clean.split_once('=')?;
    let key = k.trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), v.trim().to_string()))
}

fn strip_comment(line: &str) -> &str {
    let hash = line.find('#');
    let semi = line.find(';');
    match (hash, semi) {
        (Some(a), Some(b)) => &line[..a.min(b)],
        (Some(a), None) => &line[..a],
        (None, Some(b)) => &line[..b],
        (None, None) => line,
    }
}

fn render_env(raw: &str) -> String {
    let mut out = raw.to_string();
    let mut start = 0;
    while let Some(open_rel) = out[start..].find("{{.") {
        let open = start + open_rel;
        let Some(close_rel) = out[open..].find("}}") else {
            break;
        };
        let close = open + close_rel + 2;
        let name = out[open + 3..close - 2].trim();
        let value = env::var(name).unwrap_or_default();
        out.replace_range(open..close, &value);
        start = open + value.len();
    }
    out
}

fn normalize_location(v: &str) -> String {
    if v.is_empty() {
        "/".to_string()
    } else if v.starts_with('/') {
        v.to_string()
    } else {
        format!("/{v}")
    }
}

fn get_string(kv: &SectionMap, key: &str, default: &str) -> String {
    kv.get(key).cloned().unwrap_or_else(|| default.to_string())
}

fn get_bool(kv: &SectionMap, key: &str, default: bool) -> bool {
    kv.get(key)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn get_u16(kv: &SectionMap, key: &str, default: u16) -> u16 {
    kv.get(key)
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(default)
}

fn get_u64(kv: &SectionMap, key: &str, default: u64) -> u64 {
    kv.get(key)
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn get_usize(kv: &SectionMap, key: &str, default: usize) -> usize {
    kv.get(key)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}
