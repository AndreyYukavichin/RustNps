use crate::model::ClientRuntimeConfig;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::TcpStream;

const MAGIC: &[u8; 4] = b"RNP1";
const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum BridgeHello {
    Control {
        vkey: String,
        version: String,
        core_version: String,
    },
    Mux {
        vkey: String,
        version: String,
        core_version: String,
    },
    Config {
        vkey: String,
        version: String,
        core_version: String,
        config: ClientRuntimeConfig,
    },
    Health {
        vkey: String,
        target: String,
        status: bool,
    },
    Data {
        vkey: String,
        link_id: u64,
    },
    SecretVisitor {
        password: String,
        target: String,
    },
    P2pVisitor {
        password: String,
        target: String,
    },
    RegisterIp {
        vkey: String,
        hours: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ok { message: String },
    Error { message: String },
    Open { link_id: u64, link: Link },
    Ping,
    Stop { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub kind: LinkKind,
    pub target: String,
    pub remote_addr: String,
    pub crypt: bool,
    pub compress: bool,
    pub local_proxy: bool,
    pub proto_version: String,
    pub file: Option<FileServe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Tcp,
    Http,
    Udp,
    File,
    Secret,
    P2p,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileServe {
    pub local_path: String,
    pub strip_pre: String,
}

impl Link {
    pub fn tcp(target: String, remote_addr: String, crypt: bool, compress: bool) -> Self {
        Self {
            kind: LinkKind::Tcp,
            target,
            remote_addr,
            crypt,
            compress,
            local_proxy: false,
            proto_version: String::new(),
            file: None,
        }
    }
}

pub fn write_message<T: Serialize>(stream: &mut TcpStream, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(invalid_data)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    stream.write_all(MAGIC)?;
    stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}

pub fn read_message<T: DeserializeOwned>(stream: &mut TcpStream) -> io::Result<T> {
    let mut magic = [0_u8; 4];
    stream.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad protocol magic",
        ));
    }
    let mut len_buf = [0_u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(invalid_data)
}

pub fn write_blob<W: Write + ?Sized>(stream: &mut W, data: &[u8]) -> io::Result<()> {
    if data.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "blob too large"));
    }
    stream.write_all(&(data.len() as u32).to_le_bytes())?;
    stream.write_all(data)?;
    stream.flush()
}

pub fn read_blob<R: Read + ?Sized>(stream: &mut R) -> io::Result<Vec<u8>> {
    let mut len_buf = [0_u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "blob too large"));
    }
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body)?;
    Ok(body)
}

pub fn ok(message: impl Into<String>) -> ServerMessage {
    ServerMessage::Ok {
        message: message.into(),
    }
}

pub fn error(message: impl Into<String>) -> ServerMessage {
    ServerMessage::Error {
        message: message.into(),
    }
}

fn invalid_data(err: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}
