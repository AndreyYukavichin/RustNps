use crate::relay::{IntoIoHalves, RelayRead, RelayWrite};
use kcp_io::tokio_rt::{KcpListener as TokioKcpListener, KcpSessionConfig, KcpStream as TokioKcpStream, OwnedReadHalf, OwnedWriteHalf};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::{Builder, Runtime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeMode {
    Tcp,
    Kcp,
}

impl BridgeMode {
    pub fn from_text(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "kcp" => Self::Kcp,
            _ => Self::Tcp,
        }
    }
}

pub fn connect_bridge_stream(addr: &str, mode: BridgeMode) -> io::Result<BridgeConn> {
    match mode {
        BridgeMode::Tcp => TcpStream::connect(addr).map(BridgeConn::Tcp),
        BridgeMode::Kcp => {
            let runtime = new_runtime()?;
            let resolved = resolve_socket_addr(addr)?;
            let stream = runtime
                .block_on(async { TokioKcpStream::connect(resolved, default_kcp_config()).await })
                .map_err(kcp_error)?;
            Ok(BridgeConn::Kcp { runtime, stream })
        }
    }
}

pub struct BridgeListener {
    inner: BridgeListenerKind,
}

enum BridgeListenerKind {
    Tcp(TcpListener),
    Kcp {
        runtime: Runtime,
        listener: TokioKcpListener,
    },
}

impl BridgeListener {
    pub fn bind(addr: &str, mode: BridgeMode) -> io::Result<Self> {
        let inner = match mode {
            BridgeMode::Tcp => BridgeListenerKind::Tcp(TcpListener::bind(addr)?),
            BridgeMode::Kcp => {
                let runtime = new_runtime()?;
                let resolved = resolve_socket_addr(addr)?;
                let listener = runtime
                    .block_on(async { TokioKcpListener::bind(resolved, default_kcp_config()).await })
                    .map_err(kcp_error)?;
                BridgeListenerKind::Kcp { runtime, listener }
            }
        };
        Ok(Self { inner })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match &self.inner {
            BridgeListenerKind::Tcp(listener) => listener.local_addr(),
            BridgeListenerKind::Kcp { listener, .. } => Ok(listener.local_addr()),
        }
    }

    pub fn accept(&mut self) -> io::Result<(BridgeConn, SocketAddr)> {
        match &mut self.inner {
            BridgeListenerKind::Tcp(listener) => {
                let (stream, peer_addr) = listener.accept()?;
                Ok((BridgeConn::Tcp(stream), peer_addr))
            }
            BridgeListenerKind::Kcp { runtime, listener } => {
                let (stream, peer_addr) = runtime
                    .block_on(async { listener.accept().await })
                    .map_err(kcp_error)?;
                Ok((BridgeConn::kcp(stream)?, peer_addr))
            }
        }
    }
}

pub enum BridgeConn {
    Tcp(TcpStream),
    Kcp {
        runtime: Runtime,
        stream: TokioKcpStream,
    },
}

impl BridgeConn {
    fn kcp(stream: TokioKcpStream) -> io::Result<Self> {
        Ok(Self::Kcp {
            runtime: new_runtime()?,
            stream,
        })
    }
}

impl Read for BridgeConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            BridgeConn::Tcp(stream) => stream.read(buf),
            BridgeConn::Kcp { runtime, stream } => runtime.block_on(async move {
                stream.read(buf).await.map_err(kcp_error)
            }),
        }
    }
}

impl Write for BridgeConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            BridgeConn::Tcp(stream) => stream.write(buf),
            BridgeConn::Kcp { runtime, stream } => runtime.block_on(async move {
                stream.write(buf).await.map_err(kcp_error)
            }),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            BridgeConn::Tcp(stream) => stream.flush(),
            BridgeConn::Kcp { stream, .. } => {
                stream.flush();
                Ok(())
            }
        }
    }
}

impl IntoIoHalves for BridgeConn {
    fn into_io_halves(self) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        match self {
            BridgeConn::Tcp(stream) => {
                let reader = stream.try_clone()?;
                Ok((
                    Box::new(BridgeTcpReadHalf { inner: reader }),
                    Box::new(BridgeTcpWriteHalf { inner: stream }),
                ))
            }
            BridgeConn::Kcp { stream, .. } => {
                let (reader, writer) = stream.into_split();
                Ok((
                    Box::new(BridgeKcpReadHalf {
                        runtime: new_runtime()?,
                        inner: Mutex::new(reader),
                    }),
                    Box::new(BridgeKcpWriteHalf {
                        runtime: new_runtime()?,
                        inner: Mutex::new(writer),
                    }),
                ))
            }
        }
    }
}

impl crate::relay::RelayStream for BridgeConn {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        IntoIoHalves::into_io_halves(*self)
    }
}

struct BridgeTcpReadHalf {
    inner: TcpStream,
}

impl Read for BridgeTcpReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

struct BridgeTcpWriteHalf {
    inner: TcpStream,
}

impl Write for BridgeTcpWriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl RelayWrite for BridgeTcpWriteHalf {
    fn shutdown_write(&mut self) -> io::Result<()> {
        self.inner.shutdown(Shutdown::Write)
    }
}

struct BridgeKcpReadHalf {
    runtime: Runtime,
    inner: Mutex<OwnedReadHalf>,
}

impl Read for BridgeKcpReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let inner = &mut *self.inner.lock().unwrap();
        self.runtime.block_on(async move { inner.read(buf).await.map_err(kcp_error) })
    }
}

struct BridgeKcpWriteHalf {
    runtime: Runtime,
    inner: Mutex<OwnedWriteHalf>,
}

impl Write for BridgeKcpWriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let inner = &mut *self.inner.lock().unwrap();
        self.runtime.block_on(async move { inner.write(buf).await.map_err(kcp_error) })
    }

    fn flush(&mut self) -> io::Result<()> {
        let inner = &mut *self.inner.lock().unwrap();
        self.runtime.block_on(async move {
            inner.flush().await;
            Ok(())
        })
    }
}

impl RelayWrite for BridgeKcpWriteHalf {}

fn default_kcp_config() -> KcpSessionConfig {
    KcpSessionConfig::fast()
}

fn new_runtime() -> io::Result<Runtime> {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))
}

fn resolve_socket_addr(addr: &str) -> io::Result<SocketAddr> {
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid socket address: {addr}")))
}

fn kcp_error<E: std::fmt::Display>(err: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_message, write_message, BridgeHello, ServerMessage};
    use std::thread;

    #[test]
    fn bridge_mode_keeps_kcp() {
        assert_eq!(BridgeMode::from_text("kcp"), BridgeMode::Kcp);
        assert_eq!(BridgeMode::from_text("TCP"), BridgeMode::Tcp);
    }

    #[test]
    fn kcp_bridge_round_trip_works() {
        let mut listener = BridgeListener::bind("127.0.0.1:0", BridgeMode::Kcp).unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut conn, peer_addr) = listener.accept().unwrap();
            assert_ne!(peer_addr.port(), 0);

            let hello: BridgeHello = read_message(&mut conn).unwrap();
            match hello {
                BridgeHello::Control {
                    vkey,
                    version,
                    core_version,
                } => {
                    assert_eq!(vkey, "demo-key");
                    assert_eq!(version, "1.0");
                    assert_eq!(core_version, "1.0");
                }
                other => panic!("unexpected hello: {other:?}"),
            }

            write_message(
                &mut conn,
                &ServerMessage::Ok {
                    message: "ready".to_string(),
                },
            )
            .unwrap();
        });

        let mut client = connect_bridge_stream(&addr.to_string(), BridgeMode::Kcp).unwrap();
        write_message(
            &mut client,
            &BridgeHello::Control {
                vkey: "demo-key".to_string(),
                version: "1.0".to_string(),
                core_version: "1.0".to_string(),
            },
        )
        .unwrap();

        let response: ServerMessage = read_message(&mut client).unwrap();
        match response {
            ServerMessage::Ok { message } => assert_eq!(message, "ready"),
            other => panic!("unexpected response: {other:?}"),
        }

        server.join().unwrap();
    }
}