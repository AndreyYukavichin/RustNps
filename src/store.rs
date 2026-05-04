use crate::model::{ClientRuntimeConfig, GlobalConfig, Host, Tunnel};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const RECORD_SEPARATOR: &str = "*#*";

#[derive(Debug, Clone)]
pub struct PersistentStore {
    conf_dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Default)]
pub struct PersistentState {
    pub clients: Vec<ClientRuntimeConfig>,
    pub tunnels: Vec<Tunnel>,
    pub hosts: Vec<Host>,
    pub global: GlobalConfig,
}

impl PersistentStore {
    pub fn from_conf_path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let conf_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("conf"));
        Self::new(conf_dir)
    }

    pub fn new(conf_dir: impl Into<PathBuf>) -> Self {
        Self {
            conf_dir: conf_dir.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn load(&self) -> io::Result<PersistentState> {
        let clients = read_values(&self.clients_path())?
            .into_iter()
            .map(decode_client)
            .collect::<io::Result<Vec<_>>>()?;
        let tunnels = read_values(&self.tasks_path())?
            .into_iter()
            .map(decode_tunnel)
            .collect::<io::Result<Vec<_>>>()?;
        let hosts = read_values(&self.hosts_path())?
            .into_iter()
            .map(decode_host)
            .collect::<io::Result<Vec<_>>>()?;
        let global = read_single::<GlobalConfig>(&self.global_path())?.unwrap_or_default();
        Ok(PersistentState {
            clients,
            tunnels,
            hosts,
            global,
        })
    }

    pub fn save_all(
        &self,
        clients: &HashMap<String, ClientRuntimeConfig>,
        global: &GlobalConfig,
    ) -> io::Result<()> {
        let _guard = self.lock.lock().unwrap();
        fs::create_dir_all(&self.conf_dir)?;

        let mut client_records = Vec::new();
        let mut tunnel_records = Vec::new();
        let mut host_records = Vec::new();

        let mut keys: Vec<_> = clients.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let Some(client) = clients.get(&key) else {
                continue;
            };
            let stored_tunnels: Vec<Tunnel> = client
                .tunnels
                .iter()
                .filter(|tunnel| !tunnel.no_store)
                .cloned()
                .collect();
            let stored_hosts: Vec<Host> = client
                .hosts
                .iter()
                .filter(|host| !host.no_store)
                .cloned()
                .collect();
            if client.no_store && stored_tunnels.is_empty() && stored_hosts.is_empty() {
                continue;
            }

            let mut client_record = client.clone();
            client_record.hosts.clear();
            client_record.tunnels.clear();
            client_record.healths.clear();
            client_record.local_servers.clear();
            client_record.no_store = false;
            client_records.push(client_record);

            tunnel_records.extend(stored_tunnels);
            host_records.extend(stored_hosts);
        }

        write_records(&self.clients_path(), &client_records)?;
        write_records(&self.tasks_path(), &tunnel_records)?;
        write_records(&self.hosts_path(), &host_records)?;
        write_single(&self.global_path(), global)
    }

    pub fn conf_dir(&self) -> &Path {
        &self.conf_dir
    }

    fn clients_path(&self) -> PathBuf {
        self.conf_dir.join("clients.json")
    }

    fn tasks_path(&self) -> PathBuf {
        self.conf_dir.join("tasks.json")
    }

    fn hosts_path(&self) -> PathBuf {
        self.conf_dir.join("hosts.json")
    }

    fn global_path(&self) -> PathBuf {
        self.conf_dir.join("global.json")
    }
}

fn read_values(path: &Path) -> io::Result<Vec<Value>> {
    read_records::<Value>(path)
}

fn read_records<T: DeserializeOwned>(path: &Path) -> io::Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<T>>(trimmed).map_err(invalid_data);
    }

    let mut out = Vec::new();
    for part in raw.split(&format!("\n{RECORD_SEPARATOR}")) {
        let part = part.trim();
        if part.is_empty() || part == RECORD_SEPARATOR {
            continue;
        }
        out.push(serde_json::from_str(part).map_err(invalid_data)?);
    }
    Ok(out)
}

fn decode_client(value: Value) -> io::Result<ClientRuntimeConfig> {
    if value.get("common").is_some() || value.get("id").is_some() {
        if let Ok(client) = serde_json::from_value::<ClientRuntimeConfig>(value.clone()) {
            return Ok(client);
        }
    }

    let mut client = ClientRuntimeConfig::default();
    client.id = value_u64(&value, "Id");
    client.common.vkey = value_string(&value, "VerifyKey");
    client.common.client.verify_key = client.common.vkey.clone();
    client.common.client.remark = value_string(&value, "Remark");
    client.common.client.status = value_bool(&value, "Status", true);
    client.common.client.rate_limit_kb = value_u64(&value, "RateLimit");
    client.common.client.max_conn = value_u64(&value, "MaxConn") as usize;
    client.common.client.max_tunnel_num = value_u64(&value, "MaxTunnelNum") as usize;
    client.common.client.web_username = value_string(&value, "WebUserName");
    client.common.client.web_password = value_string(&value, "WebPassword");
    client.common.client.config_conn_allow = value_bool(&value, "ConfigConnAllow", true);
    client.common.client.black_ip_list = value_string_vec(&value, "BlackIpList");
    client.common.client.ip_white = value_bool(&value, "IpWhite", false);
    client.common.client.ip_white_pass = value_string(&value, "IpWhitePass");
    client.common.client.ip_white_list = value_string_vec(&value, "IpWhiteList");
    client.no_store = value_bool(&value, "NoStore", false);
    client.no_display = value_bool(&value, "NoDisplay", false);
    client.create_time = value_string(&value, "CreateTime");
    client.last_online_time = value_string(&value, "LastOnlineTime");
    client.last_online_addr = value_string(&value, "LastOnlineAddr");

    if let Some(cnf) = value.get("Cnf") {
        client.common.client.basic_username = value_string(cnf, "U");
        client.common.client.basic_password = value_string(cnf, "P");
        client.common.client.compress = value_bool(cnf, "Compress", false);
        client.common.client.crypt = value_bool(cnf, "Crypt", false);
    }
    if let Some(flow) = value.get("Flow") {
        client.common.client.flow_limit_mb = value_u64(flow, "FlowLimit");
    }
    Ok(client)
}

fn decode_tunnel(value: Value) -> io::Result<Tunnel> {
    if value.get("mode").is_some() || value.get("target_addr").is_some() {
        if let Ok(tunnel) = serde_json::from_value::<Tunnel>(value.clone()) {
            return Ok(tunnel);
        }
    }

    let mut tunnel = Tunnel::default();
    tunnel.id = value_u64(&value, "Id");
    tunnel.server_port = value_u64(&value, "Port") as u16;
    tunnel.server_ip = value_string(&value, "ServerIp");
    tunnel.mode = value_string(&value, "Mode");
    tunnel.status = value_bool(&value, "Status", true);
    tunnel.run_status = value_bool(&value, "RunStatus", tunnel.status);
    tunnel.ports = value_string(&value, "Ports");
    if tunnel.ports.is_empty() && tunnel.server_port > 0 {
        tunnel.ports = tunnel.server_port.to_string();
    }
    tunnel.password = value_string(&value, "Password");
    tunnel.remark = value_string(&value, "Remark");
    tunnel.target_addr = value_string(&value, "TargetAddr");
    tunnel.no_store = value_bool(&value, "NoStore", false);
    tunnel.local_path = value_string(&value, "LocalPath");
    tunnel.strip_pre = value_string(&value, "StripPre");
    tunnel.proto_version = value_string(&value, "ProtoVersion");
    if let Some(target) = value.get("Target") {
        tunnel.target.target_str = value_string(target, "TargetStr");
        tunnel.target.local_proxy = value_bool(target, "LocalProxy", false);
    }
    if let Some(client) = value.get("Client") {
        tunnel.client_vkey = value_string(client, "VerifyKey");
    }
    Ok(tunnel)
}

fn decode_host(value: Value) -> io::Result<Host> {
    if value.get("host").is_some() || value.get("cert_file_path").is_some() {
        if let Ok(host) = serde_json::from_value::<Host>(value.clone()) {
            return Ok(host);
        }
    }

    let mut host = Host::default();
    host.id = value_u64(&value, "Id");
    host.host = value_string(&value, "Host");
    host.header_change = value_string(&value, "HeaderChange");
    host.host_change = value_string(&value, "HostChange");
    host.location = value_string(&value, "Location");
    host.remark = value_string(&value, "Remark");
    host.scheme = value_string(&value, "Scheme");
    host.cert_file_path = value_string(&value, "CertFilePath");
    host.key_file_path = value_string(&value, "KeyFilePath");
    host.proto_version = value_string(&value, "ProtoVersion");
    host.no_store = value_bool(&value, "NoStore", false);
    host.is_close = value_bool(&value, "IsClose", false);
    host.auto_https = value_bool(&value, "AutoHttps", false);
    if let Some(target) = value.get("Target") {
        host.target.target_str = value_string(target, "TargetStr");
        host.target.local_proxy = value_bool(target, "LocalProxy", false);
    }
    if let Some(client) = value.get("Client") {
        host.client_vkey = value_string(client, "VerifyKey");
    }
    Ok(host)
}

fn value_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn value_u64(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn value_bool(value: &Value, key: &str, default: bool) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn value_string_vec(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn read_single<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(invalid_data)
}

fn write_records<T: Serialize>(path: &Path, records: &[T]) -> io::Result<()> {
    write_atomic(path, |file| {
        for record in records {
            let bytes = serde_json::to_vec(record).map_err(invalid_data)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.write_all(RECORD_SEPARATOR.as_bytes())?;
        }
        Ok(())
    })
}

fn write_single<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomic(path, |file| {
        let bytes = serde_json::to_vec_pretty(value).map_err(invalid_data)?;
        file.write_all(&bytes)
    })
}

fn write_atomic(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut file = File::create(&tmp)?;
    write(&mut file)?;
    file.sync_all()?;
    drop(file);

    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(tmp, path)
}

fn invalid_data(err: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CommonConfig, Target};

    #[test]
    fn round_trips_line_delimited_state() {
        let root = std::env::temp_dir().join(format!(
            "rustnps-store-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = PersistentStore::new(&root);
        let mut clients = HashMap::new();
        let mut client = ClientRuntimeConfig::default();
        client.id = 7;
        client.common = CommonConfig::default();
        client.common.vkey = "abc".to_string();
        client.common.client.verify_key = "abc".to_string();

        let mut tunnel = Tunnel::default();
        tunnel.id = 9;
        tunnel.no_store = false;
        tunnel.client_vkey = "abc".to_string();
        tunnel.target = Target {
            target_str: "127.0.0.1:80".to_string(),
            local_proxy: false,
        };
        client.tunnels.push(tunnel);
        clients.insert("abc".to_string(), client);

        let global = GlobalConfig {
            black_ip_list: vec!["10.0.0.1".to_string()],
            server_url: "https://nps.example.com".to_string(),
        };

        store.save_all(&clients, &global).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.clients.len(), 1);
        assert_eq!(loaded.tunnels.len(), 1);
        assert_eq!(loaded.global.server_url, "https://nps.example.com");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loads_go_style_records() {
        let root = std::env::temp_dir().join(format!(
            "rustnps-go-store-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("clients.json"),
            r#"{"Id":3,"VerifyKey":"go-vkey","Remark":"go client","Status":true,"Cnf":{"U":"u","P":"p","Compress":true,"Crypt":false},"NoStore":false}
*#*"#,
        )
        .unwrap();
        fs::write(
            root.join("tasks.json"),
            r#"{"Id":8,"Port":9000,"Mode":"tcp","Status":true,"RunStatus":true,"Client":{"VerifyKey":"go-vkey"},"Target":{"TargetStr":"127.0.0.1:80","LocalProxy":false},"NoStore":false}
*#*"#,
        )
        .unwrap();
        fs::write(
            root.join("hosts.json"),
            r#"{"Id":9,"Host":"example.com","Location":"/","Scheme":"all","Client":{"VerifyKey":"go-vkey"},"Target":{"TargetStr":"127.0.0.1:8080"},"NoStore":false}
*#*"#,
        )
        .unwrap();

        let loaded = PersistentStore::new(&root).load().unwrap();
        assert_eq!(loaded.clients[0].common.vkey, "go-vkey");
        assert_eq!(loaded.clients[0].common.client.basic_username, "u");
        assert_eq!(loaded.tunnels[0].client_vkey, "go-vkey");
        assert_eq!(loaded.hosts[0].host, "example.com");

        let _ = fs::remove_dir_all(root);
    }
}
