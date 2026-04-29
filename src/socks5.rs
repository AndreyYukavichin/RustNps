use std::io::{self, Read, Write};
use std::net::TcpStream;

/// Minimal SOCKS5 CONNECT parser.
/// 最小 SOCKS5 实现：支持 no-auth + CONNECT，足够覆盖 nps 的 socks5 核心代理场景。
pub fn accept_socks5(stream: &mut TcpStream) -> io::Result<String> {
    let mut head = [0_u8; 2];
    stream.read_exact(&mut head)?;
    if head[0] != 0x05 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not socks5"));
    }
    let nmethods = head[1] as usize;
    let mut methods = vec![0_u8; nmethods];
    stream.read_exact(&mut methods)?;
    if !methods.contains(&0x00) {
        stream.write_all(&[0x05, 0xff])?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "no supported auth",
        ));
    }
    stream.write_all(&[0x05, 0x00])?;

    let mut req = [0_u8; 4];
    stream.read_exact(&mut req)?;
    if req[0] != 0x05 || req[1] != 0x01 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "only CONNECT is supported",
        ));
    }

    let host = match req[3] {
        0x01 => {
            let mut ip = [0_u8; 4];
            stream.read_exact(&mut ip)?;
            format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
        }
        0x03 => {
            let mut len = [0_u8; 1];
            stream.read_exact(&mut len)?;
            let mut name = vec![0_u8; len[0] as usize];
            stream.read_exact(&mut name)?;
            String::from_utf8_lossy(&name).to_string()
        }
        0x04 => {
            let mut ip = [0_u8; 16];
            stream.read_exact(&mut ip)?;
            let segments: Vec<String> = ip
                .chunks(2)
                .map(|chunk| format!("{:02x}{:02x}", chunk[0], chunk[1]))
                .collect();
            segments.join(":")
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported address type",
            ))
        }
    };
    let mut port = [0_u8; 2];
    stream.read_exact(&mut port)?;
    let port = u16::from_be_bytes(port);

    // Reply success with 0.0.0.0:0. The real remote connection is established by npc.
    // 回复成功，真实目标连接由 npc 侧建立。
    stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;
    Ok(format!("{host}:{port}"))
}
