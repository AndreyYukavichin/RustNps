use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Server side configuration parsed from `conf/nps.conf`.
/// 服务端配置：尽量保留 Go nps 的字段名，方便从原配置迁移。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub appname: String,
    pub runmode: String,
    pub http_proxy_ip: String,
    pub http_proxy_port: u16,
    pub https_proxy_port: u16,
    pub https_just_proxy: bool,
    pub bridge_type: String,
    pub bridge_ip: String,
    pub bridge_port: u16,
    pub public_vkey: String,
    pub web_ip: String,
    pub web_port: u16,
    pub web_username: String,
    pub web_password: String,
    pub web_base_url: String,
    pub allow_user_login: bool,
    pub allow_user_register: bool,
    pub open_captcha: bool,
    pub allow_user_change_username: bool,
    pub allow_flow_limit: bool,
    pub allow_rate_limit: bool,
    pub allow_tunnel_num_limit: bool,
    pub allow_connection_num_limit: bool,
    pub allow_ports: String,
    pub allow_local_proxy: bool,
    pub allow_multi_ip: bool,
    pub http_cache: bool,
    pub http_cache_length: usize,
    pub http_add_origin_header: bool,
    pub ip_limit: bool,
    pub disconnect_timeout: u64,
    pub tls_enable: bool,
    pub tls_bridge_port: u16,
    pub p2p_ip: String,
    pub p2p_port: u16,
    pub log_level: String,
    pub log_path: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            appname: "nps".to_string(),
            runmode: "dev".to_string(),
            http_proxy_ip: "0.0.0.0".to_string(),
            http_proxy_port: 80,
            https_proxy_port: 443,
            https_just_proxy: true,
            bridge_type: "tcp".to_string(),
            bridge_ip: "0.0.0.0".to_string(),
            bridge_port: 8024,
            public_vkey: "123".to_string(),
            web_ip: "0.0.0.0".to_string(),
            web_port: 8081,
            web_username: "admin".to_string(),
            web_password: "123".to_string(),
            web_base_url: String::new(),
            allow_user_login: true,
            allow_user_register: false,
            open_captcha: false,
            allow_user_change_username: true,
            allow_flow_limit: false,
            allow_rate_limit: false,
            allow_tunnel_num_limit: false,
            allow_connection_num_limit: false,
            allow_ports: String::new(),
            allow_local_proxy: false,
            allow_multi_ip: true,
            http_cache: false,
            http_cache_length: 100,
            http_add_origin_header: true,
            ip_limit: false,
            disconnect_timeout: 60,
            tls_enable: false,
            tls_bridge_port: 8025,
            p2p_ip: "127.0.0.1".to_string(),
            p2p_port: 6000,
            log_level: "7".to_string(),
            log_path: String::new(),
        }
    }
}

/// Common section in `npc.conf`.
/// 客户端公共配置，兼容 Go npc 的 common 段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommonConfig {
    pub server_addr: String,
    pub conn_type: String,
    pub vkey: String,
    pub auto_reconnection: bool,
    pub tls_enable: bool,
    pub proxy_url: String,
    pub disconnect_timeout: u64,
    pub client: ClientInfo,
}

impl Default for CommonConfig {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:8024".to_string(),
            conn_type: "tcp".to_string(),
            vkey: "123".to_string(),
            auto_reconnection: true,
            tls_enable: false,
            proxy_url: String::new(),
            disconnect_timeout: 60,
            client: ClientInfo::default(),
        }
    }
}

/// Runtime client metadata.
/// 客户端运行态元信息。字段命名贴近 Go 版 file.Client。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub verify_key: String,
    pub remark: String,
    pub status: bool,
    pub compress: bool,
    pub crypt: bool,
    pub basic_username: String,
    pub basic_password: String,
    pub web_username: String,
    pub web_password: String,
    #[serde(default = "default_config_conn_allow")]
    pub config_conn_allow: bool,
    pub rate_limit_kb: u64,
    pub flow_limit_mb: u64,
    pub max_conn: usize,
    pub max_tunnel_num: usize,
    pub black_ip_list: Vec<String>,
    pub ip_white: bool,
    pub ip_white_pass: String,
    pub ip_white_list: Vec<String>,
}

fn default_config_conn_allow() -> bool {
    true
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            verify_key: String::new(),
            remark: String::new(),
            status: true,
            compress: false,
            crypt: false,
            basic_username: String::new(),
            basic_password: String::new(),
            web_username: "user".to_string(),
            web_password: String::new(),
            config_conn_allow: true,
            rate_limit_kb: 0,
            flow_limit_mb: 0,
            max_conn: 0,
            max_tunnel_num: 0,
            black_ip_list: Vec::new(),
            ip_white: false,
            ip_white_pass: String::new(),
            ip_white_list: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRuntimeConfig {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub no_store: bool,
    #[serde(default)]
    pub no_display: bool,
    #[serde(default)]
    pub create_time: String,
    #[serde(default)]
    pub last_online_time: String,
    #[serde(default)]
    pub last_online_addr: String,
    #[serde(default)]
    pub common: CommonConfig,
    #[serde(default)]
    pub hosts: Vec<Host>,
    #[serde(default)]
    pub tunnels: Vec<Tunnel>,
    #[serde(default)]
    pub healths: Vec<HealthCheck>,
    #[serde(default)]
    pub local_servers: Vec<LocalServer>,
}

impl Default for ClientRuntimeConfig {
    fn default() -> Self {
        Self {
            id: 0,
            no_store: false,
            no_display: false,
            create_time: String::new(),
            last_online_time: String::new(),
            last_online_addr: String::new(),
            common: CommonConfig::default(),
            hosts: Vec::new(),
            tunnels: Vec::new(),
            healths: Vec::new(),
            local_servers: Vec::new(),
        }
    }
}

/// TCP/UDP/SOCKS/HTTP/file/secret/p2p task.
/// 隧道任务：mode 保持字符串是为了兼容旧配置中的扩展值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub server_ip: String,
    #[serde(default)]
    pub server_port: u16,
    #[serde(default)]
    pub status: bool,
    #[serde(default)]
    pub run_status: bool,
    #[serde(default)]
    pub ports: String,
    #[serde(default)]
    pub target: Target,
    #[serde(default)]
    pub target_addr: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub local_path: String,
    #[serde(default)]
    pub strip_pre: String,
    #[serde(default)]
    pub multi_account: HashMap<String, String>,
    #[serde(default)]
    pub proto_version: String,
    #[serde(default)]
    pub client_vkey: String,
    #[serde(default)]
    pub no_store: bool,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self {
            id: 0,
            mode: "tcp".to_string(),
            remark: String::new(),
            server_ip: "0.0.0.0".to_string(),
            server_port: 0,
            status: true,
            run_status: true,
            ports: String::new(),
            target: Target::default(),
            target_addr: String::new(),
            password: String::new(),
            local_path: String::new(),
            strip_pre: String::new(),
            multi_account: HashMap::new(),
            proto_version: String::new(),
            client_vkey: String::new(),
            no_store: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub header_change: String,
    #[serde(default)]
    pub host_change: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub scheme: String,
    #[serde(default)]
    pub cert_file_path: String,
    #[serde(default)]
    pub key_file_path: String,
    #[serde(default)]
    pub target: Target,
    #[serde(default)]
    pub client_vkey: String,
    #[serde(default)]
    pub no_store: bool,
    #[serde(default)]
    pub is_close: bool,
    #[serde(default)]
    pub auto_https: bool,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            id: 0,
            host: String::new(),
            header_change: String::new(),
            host_change: String::new(),
            location: "/".to_string(),
            remark: String::new(),
            scheme: "all".to_string(),
            cert_file_path: String::new(),
            key_file_path: String::new(),
            target: Target::default(),
            client_vkey: String::new(),
            no_store: true,
            is_close: false,
            auto_https: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Target {
    #[serde(default)]
    pub target_str: String,
    #[serde(default)]
    pub local_proxy: bool,
}

impl Target {
    /// Pick one target with deterministic round-robin compatible behavior.
    /// 这里保持简单：调用方传入 cursor，函数返回下一项和新的 cursor。
    pub fn pick(&self, cursor: usize) -> Option<(String, usize)> {
        let items: Vec<String> = self
            .target_str
            .split('\n')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if items.is_empty() {
            return None;
        }
        let index = cursor % items.len();
        Some((items[index].clone(), index + 1))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthCheck {
    pub remark: String,
    pub timeout_secs: u64,
    pub max_failed: u32,
    pub interval_secs: u64,
    pub http_url: String,
    pub kind: String,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalServer {
    pub kind: String,
    pub ip: String,
    pub port: u16,
    pub password: String,
    pub target: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowCounter {
    pub inlet_flow: u64,
    pub export_flow: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default, rename = "BlackIpList", alias = "black_ip_list")]
    pub black_ip_list: Vec<String>,
    #[serde(default, rename = "ServerUrl", alias = "server_url")]
    pub server_url: String,
}
