use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use crate::model::FlowCounter;
use crate::tls::{transport_client_config, transport_server_config, transport_server_name};
use rustls::{ClientConnection, ServerConnection};
use snap::read::FrameDecoder;
use snap::write::FrameEncoder;
use std::any::Any;
use std::io::{self, Cursor, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub trait RelayStream: Read + Write + Send + Any {
    fn as_any(&self) -> &dyn Any;
    fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)>;
}

pub trait RelayRead: Read + Send {}

impl<T> RelayRead for T where T: Read + Send {}

pub trait RelayWrite: Write + Send {}

impl<T> RelayWrite for T where T: Write + Send {}

pub trait IntoIoHalves {
    fn into_io_halves(self) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)>;
}

impl IntoIoHalves for TcpStream {
    fn into_io_halves(self) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        let reader = self.try_clone()?;
        Ok((Box::new(reader), Box::new(self)))
    }
}

impl IntoIoHalves for Box<dyn RelayStream> {
    fn into_io_halves(self) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        RelayStream::into_io_halves(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TransportSide {
    Client,
    Server,
}

#[derive(Clone, Default)]
pub struct RelayPolicy {
    pub rate_limit_kb: u64,
    pub flow_limit_mb: u64,
    pub flow_counter: Option<Arc<Mutex<FlowCounter>>>,
}

pub fn wrap_client_transport(
    stream: Box<dyn RelayStream>,
    crypt: bool,
    compress: bool,
) -> io::Result<Box<dyn RelayStream>> {
    wrap_transport_stream(stream, crypt, compress, TransportSide::Client)
}

pub fn wrap_server_transport(
    stream: Box<dyn RelayStream>,
    crypt: bool,
    compress: bool,
    policy: RelayPolicy,
) -> io::Result<Box<dyn RelayStream>> {
    let stream = wrap_transport_stream(stream, crypt, compress, TransportSide::Server)?;
    if policy.rate_limit_kb == 0 && policy.flow_limit_mb == 0 && policy.flow_counter.is_none() {
        return Ok(stream);
    }
    Ok(Box::new(MeteredRelayStream::new(stream, policy)))
}

fn wrap_transport_stream(
    stream: Box<dyn RelayStream>,
    crypt: bool,
    compress: bool,
    side: TransportSide,
) -> io::Result<Box<dyn RelayStream>> {
    if crypt {
        return Ok(Box::new(TlsRelayStream::new(stream, side)?));
    }
    if compress {
        return Ok(Box::new(SnappyRelayStream::new(stream)?));
    }
    Ok(stream)
}

/// Copy two TCP streams until either side closes.
/// 双向复制：这是所有 TCP/HTTP/SOCKS/secret relay 的数据面核心。
pub fn copy_bidirectional<A, B>(left: A, right: B) -> io::Result<()>
where
    A: IntoIoHalves,
    B: IntoIoHalves,
{
    let (left_reader, left_writer) = left.into_io_halves()?;
    let (right_reader, right_writer) = right.into_io_halves()?;

    let a_to_b = thread::spawn(move || copy_one_way(left_reader, right_writer));
    let b_to_a = thread::spawn(move || copy_one_way(right_reader, left_writer));

    let _ = a_to_b.join().ok();
    let _ = b_to_a.join().ok();
    Ok(())
}

pub fn copy_bidirectional_legacy<A, B>(left: A, right: B) -> io::Result<()>
where
    A: Read + Write + Send + 'static,
    B: Read + Write + Send + 'static,
{
    let left = Arc::new(Mutex::new(left));
    let right = Arc::new(Mutex::new(right));

    let left_reader = Arc::clone(&left);
    let right_writer = Arc::clone(&right);
    let a_to_b = thread::spawn(move || copy_one_way_legacy(left_reader, right_writer));

    let right_reader = Arc::clone(&right);
    let left_writer = Arc::clone(&left);
    let b_to_a = thread::spawn(move || copy_one_way_legacy(right_reader, left_writer));

    let _ = a_to_b.join().ok();
    let _ = b_to_a.join().ok();
    Ok(())
}

fn copy_one_way(mut reader: Box<dyn RelayRead>, mut writer: Box<dyn RelayWrite>) -> io::Result<()> {
    let mut buf = vec![0_u8; 16 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        writer.write_all(&buf[..n])?;
        writer.flush()?;
    }
}

fn copy_one_way_legacy<R, W>(reader: Arc<Mutex<R>>, writer: Arc<Mutex<W>>) -> io::Result<()>
where
    R: Read + Write + Send + 'static,
    W: Read + Write + Send + 'static,
{
    let mut buf = vec![0_u8; 16 * 1024];
    loop {
        let n = {
            let mut reader = reader.lock().unwrap();
            reader.read(&mut buf)?
        };
        if n == 0 {
            return Ok(());
        }
        let mut writer = writer.lock().unwrap();
        writer.write_all(&buf[..n])?;
        writer.flush()?;
    }
}

impl RelayStream for TcpStream {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        IntoIoHalves::into_io_halves(*self)
    }
}

struct MeteredRelayStream {
    inner: Box<dyn RelayStream>,
    limiter: Option<Arc<Mutex<RateGovernor>>>,
    flow_counter: Option<Arc<Mutex<FlowCounter>>>,
    flow_limit_bytes: u64,
}

impl MeteredRelayStream {
    fn new(inner: Box<dyn RelayStream>, policy: RelayPolicy) -> Self {
        Self {
            inner,
            limiter: (policy.rate_limit_kb > 0)
                .then(|| Arc::new(Mutex::new(RateGovernor::new(policy.rate_limit_kb * 1024)))),
            flow_counter: policy.flow_counter,
            flow_limit_bytes: policy.flow_limit_mb.saturating_mul(1024 * 1024),
        }
    }

    fn record_flow(&self, inlet: u64, export: u64) -> io::Result<()> {
        let Some(counter) = self.flow_counter.as_ref() else {
            return Ok(());
        };
        let mut counter = counter.lock().unwrap();
        let next_inlet = counter.inlet_flow.saturating_add(inlet);
        let next_export = counter.export_flow.saturating_add(export);
        if self.flow_limit_bytes > 0
            && next_inlet.saturating_add(next_export) > self.flow_limit_bytes
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "traffic exceeded",
            ));
        }
        counter.inlet_flow = next_inlet;
        counter.export_flow = next_export;
        Ok(())
    }

    fn limit_rate(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        if let Some(limiter) = self.limiter.as_ref() {
            limiter.lock().unwrap().wait(bytes as u64);
        }
    }
}

impl Read for MeteredRelayStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.record_flow(0, n as u64)?;
            self.limit_rate(n);
        }
        Ok(n)
    }
}

impl Write for MeteredRelayStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        if n > 0 {
            self.record_flow(n as u64, 0)?;
            self.limit_rate(n);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl RelayStream for MeteredRelayStream {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        let Self {
            inner,
            limiter,
            flow_counter,
            flow_limit_bytes,
        } = *self;
        let (reader, writer) = inner.into_io_halves()?;
        let reader_limiter = limiter.clone();
        Ok((
            Box::new(MeteredReaderHalf {
                inner: reader,
                limiter: reader_limiter,
                flow_counter: flow_counter.clone(),
                flow_limit_bytes,
            }),
            Box::new(MeteredWriterHalf {
                inner: writer,
                limiter,
                flow_counter,
                flow_limit_bytes,
            }),
        ))
    }
}

struct MeteredReaderHalf {
    inner: Box<dyn RelayRead>,
    limiter: Option<Arc<Mutex<RateGovernor>>>,
    flow_counter: Option<Arc<Mutex<FlowCounter>>>,
    flow_limit_bytes: u64,
}

impl Read for MeteredReaderHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            record_flow_counter(&self.flow_counter, self.flow_limit_bytes, 0, n as u64)?;
            limit_rate(&self.limiter, n);
        }
        Ok(n)
    }
}

struct MeteredWriterHalf {
    inner: Box<dyn RelayWrite>,
    limiter: Option<Arc<Mutex<RateGovernor>>>,
    flow_counter: Option<Arc<Mutex<FlowCounter>>>,
    flow_limit_bytes: u64,
}

impl Write for MeteredWriterHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        if n > 0 {
            record_flow_counter(&self.flow_counter, self.flow_limit_bytes, n as u64, 0)?;
            limit_rate(&self.limiter, n);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct RateGovernor {
    bytes_per_sec: u64,
    allowance: f64,
    last: Instant,
}

impl RateGovernor {
    fn new(bytes_per_sec: u64) -> Self {
        Self {
            bytes_per_sec,
            allowance: bytes_per_sec as f64,
            last: Instant::now(),
        }
    }

    fn plan_delay(&mut self, bytes: u64, now: Instant) -> Duration {
        if self.bytes_per_sec == 0 || bytes == 0 {
            self.last = now;
            return Duration::ZERO;
        }
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.allowance = (self.allowance + elapsed * self.bytes_per_sec as f64)
            .min(self.bytes_per_sec as f64);
        self.last = now;
        if self.allowance >= bytes as f64 {
            self.allowance -= bytes as f64;
            Duration::ZERO
        } else {
            let missing = bytes as f64 - self.allowance;
            self.allowance = 0.0;
            let delay = Duration::from_secs_f64(missing / self.bytes_per_sec as f64);
            self.last += delay;
            delay
        }
    }

    fn wait(&mut self, bytes: u64) {
        let delay = self.plan_delay(bytes, Instant::now());
        if !delay.is_zero() {
            thread::sleep(delay);
        }
    }
}

struct TlsRelayStream {
    reader: Option<TlsReadHalf>,
    writer: Option<TlsWriteHalf>,
}

enum TlsConnection {
    Client(ClientConnection),
    Server(ServerConnection),
}

struct TlsSharedState {
    conn: Mutex<TlsConnection>,
    reader: Mutex<Box<dyn RelayRead>>,
    writer: Mutex<Box<dyn RelayWrite>>,
}

impl TlsRelayStream {
    fn new(stream: Box<dyn RelayStream>, side: TransportSide) -> io::Result<Self> {
        let conn = match side {
            TransportSide::Client => TlsConnection::Client(
                ClientConnection::new(transport_client_config(), transport_server_name())
                    .map_err(invalid_data)?,
            ),
            TransportSide::Server => {
                TlsConnection::Server(ServerConnection::new(transport_server_config()?).map_err(invalid_data)?)
            }
        };
        let (reader, writer) = stream.into_io_halves()?;
        let shared = Arc::new(TlsSharedState {
            conn: Mutex::new(conn),
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
        });
        complete_tls_handshake(&shared)?;
        Ok(Self {
            reader: Some(TlsReadHalf {
                shared: Arc::clone(&shared),
            }),
            writer: Some(TlsWriteHalf { shared }),
        })
    }
}

impl TlsConnection {
    fn is_handshaking(&self) -> bool {
        match self {
            Self::Client(conn) => conn.is_handshaking(),
            Self::Server(conn) => conn.is_handshaking(),
        }
    }

    fn wants_write(&self) -> bool {
        match self {
            Self::Client(conn) => conn.wants_write(),
            Self::Server(conn) => conn.wants_write(),
        }
    }

    fn write_tls(&mut self, writer: &mut dyn Write) -> io::Result<usize> {
        match self {
            Self::Client(conn) => conn.write_tls(writer),
            Self::Server(conn) => conn.write_tls(writer),
        }
    }

    fn read_tls(&mut self, reader: &mut dyn Read) -> io::Result<usize> {
        match self {
            Self::Client(conn) => conn.read_tls(reader),
            Self::Server(conn) => conn.read_tls(reader),
        }
    }

    fn process_new_packets(&mut self) -> io::Result<()> {
        match self {
            Self::Client(conn) => conn.process_new_packets().map(|_| ()).map_err(invalid_data),
            Self::Server(conn) => conn.process_new_packets().map(|_| ()).map_err(invalid_data),
        }
    }

    fn read_plaintext(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Client(conn) => conn.reader().read(buf),
            Self::Server(conn) => conn.reader().read(buf),
        }
    }

    fn write_plaintext(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Client(conn) => conn.writer().write(buf),
            Self::Server(conn) => conn.writer().write(buf),
        }
    }
}

#[derive(Clone)]
struct TlsReadHalf {
    shared: Arc<TlsSharedState>,
}

struct TlsWriteHalf {
    shared: Arc<TlsSharedState>,
}

fn flush_pending_tls(shared: &Arc<TlsSharedState>) -> io::Result<()> {
    loop {
        let pending = {
            let conn = shared.conn.lock().unwrap();
            conn.wants_write()
        };
        if !pending {
            return Ok(());
        }
        let mut out = Vec::new();
        {
            let mut conn = shared.conn.lock().unwrap();
            while conn.wants_write() {
                let wrote = conn.write_tls(&mut out)?;
                if wrote == 0 {
                    break;
                }
            }
        }
        if out.is_empty() {
            return Ok(());
        }
        let mut writer = shared.writer.lock().unwrap();
        writer.write_all(&out)?;
        writer.flush()?;
    }
}

fn feed_tls_from_wire(shared: &Arc<TlsSharedState>) -> io::Result<usize> {
    let mut buf = vec![0_u8; 16 * 1024];
    let n = {
        let mut reader = shared.reader.lock().unwrap();
        reader.read(&mut buf)?
    };
    if n == 0 {
        return Ok(0);
    }
    let mut cursor = Cursor::new(&buf[..n]);
    {
        let mut conn = shared.conn.lock().unwrap();
        let _ = conn.read_tls(&mut cursor)?;
        conn.process_new_packets()?;
    }
    flush_pending_tls(shared)?;
    Ok(n)
}

fn complete_tls_handshake(shared: &Arc<TlsSharedState>) -> io::Result<()> {
    loop {
        flush_pending_tls(shared)?;
        let done = {
            let conn = shared.conn.lock().unwrap();
            !conn.is_handshaking()
        };
        if done {
            return Ok(());
        }
        if feed_tls_from_wire(shared)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "tls handshake closed",
            ));
        }
    }
}

impl Read for TlsRelayStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.as_mut().unwrap().read(buf)
    }
}

impl Write for TlsRelayStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.as_mut().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.as_mut().unwrap().flush()
    }
}

impl Read for TlsReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let result = {
                let mut conn = self.shared.conn.lock().unwrap();
                conn.read_plaintext(buf)
            };
            match result {
                Ok(n) if n > 0 => return Ok(n),
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err),
            }
            if feed_tls_from_wire(&self.shared)? == 0 {
                return Ok(0);
            }
        }
    }
}

impl Write for TlsWriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = {
            let mut conn = self.shared.conn.lock().unwrap();
            conn.write_plaintext(buf)?
        };
        flush_pending_tls(&self.shared)?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        flush_pending_tls(&self.shared)
    }
}

impl RelayStream for TlsRelayStream {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        let Self { reader, writer } = *self;
        Ok((
            Box::new(reader.ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "tls reader unavailable"))?),
            Box::new(writer.ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "tls writer unavailable"))?),
        ))
    }
}

struct SnappyRelayStream {
    reader: Option<FrameDecoder<Box<dyn RelayRead>>>,
    writer: Option<FrameEncoder<Box<dyn RelayWrite>>>,
}

impl SnappyRelayStream {
    fn new(stream: Box<dyn RelayStream>) -> io::Result<Self> {
        let (reader, writer) = stream.into_io_halves()?;
        Ok(Self {
            reader: Some(FrameDecoder::new(reader)),
            writer: Some(FrameEncoder::new(writer)),
        })
    }
}

impl Read for SnappyRelayStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.as_mut().unwrap().read(buf)
    }
}

impl Write for SnappyRelayStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.as_mut().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.as_mut().unwrap().flush()
    }
}

impl RelayStream for SnappyRelayStream {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        let Self { reader, writer } = *self;
        Ok((
            Box::new(SnappyReadHalf {
                inner: reader.ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "snappy reader unavailable"))?,
            }),
            Box::new(SnappyWriteHalf {
                inner: writer.ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "snappy writer unavailable"))?,
            }),
        ))
    }
}

struct SnappyReadHalf {
    inner: FrameDecoder<Box<dyn RelayRead>>,
}

impl Read for SnappyReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

struct SnappyWriteHalf {
    inner: FrameEncoder<Box<dyn RelayWrite>>,
}

impl Write for SnappyWriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
struct LegacyReadHalf {
    inner: Arc<Mutex<Box<dyn RelayStream>>>,
}

#[cfg(test)]
impl Read for LegacyReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.lock().unwrap().read(buf)
    }
}

#[cfg(test)]
struct LegacyWriteHalf {
    inner: Arc<Mutex<Box<dyn RelayStream>>>,
}

#[cfg(test)]
impl Write for LegacyWriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.lock().unwrap().flush()
    }
}

#[cfg(test)]
fn legacy_split_stream(stream: Box<dyn RelayStream>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
    let inner = Arc::new(Mutex::new(stream));
    Ok((
        Box::new(LegacyReadHalf {
            inner: Arc::clone(&inner),
        }),
        Box::new(LegacyWriteHalf { inner }),
    ))
}

fn record_flow_counter(
    flow_counter: &Option<Arc<Mutex<FlowCounter>>>,
    flow_limit_bytes: u64,
    inlet: u64,
    export: u64,
) -> io::Result<()> {
    let Some(counter) = flow_counter.as_ref() else {
        return Ok(());
    };
    let mut counter = counter.lock().unwrap();
    let next_inlet = counter.inlet_flow.saturating_add(inlet);
    let next_export = counter.export_flow.saturating_add(export);
    if flow_limit_bytes > 0 && next_inlet.saturating_add(next_export) > flow_limit_bytes {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "traffic exceeded",
        ));
    }
    counter.inlet_flow = next_inlet;
    counter.export_flow = next_export;
    Ok(())
}

fn limit_rate(limiter: &Option<Arc<Mutex<RateGovernor>>>, bytes: usize) {
    if bytes == 0 {
        return;
    }
    if let Some(limiter) = limiter.as_ref() {
        limiter.lock().unwrap().wait(bytes as u64);
    }
}

fn invalid_data(err: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

/// Read an HTTP request head without consuming the body.
/// 读取 HTTP 头部，供 HTTP 正向代理和域名代理判断目标。
pub fn read_http_head<R: Read + ?Sized>(stream: &mut R, max: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut one = [0_u8; 1];
    while buf.len() < max {
        let n = stream.read(&mut one)?;
        if n == 0 {
            break;
        }
        buf.push(one[0]);
        if buf.ends_with(b"\r\n\r\n") {
            return Ok(buf);
        }
    }
    if buf.ends_with(b"\r\n\r\n") {
        Ok(buf)
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete http header",
        ))
    }
}

pub fn parse_http_target(head: &[u8]) -> Option<(String, bool)> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.lines();
    let request = lines.next()?.trim();
    let mut parts = request.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let uri = parts.next().unwrap_or_default();
    if method == "CONNECT" {
        return Some((ensure_port(uri, 443), true));
    }
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("host") {
                return Some((ensure_port(v.trim(), 80), false));
            }
        }
    }
    None
}

pub fn parse_http_host_path(head: &[u8]) -> Option<(String, String)> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.lines();
    let request = lines.next()?.trim();
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("host") {
                return Some((v.trim().to_ascii_lowercase(), path));
            }
        }
    }
    None
}

pub fn parse_http_request_line(head: &[u8]) -> Option<(String, String, String)> {
    let text = String::from_utf8_lossy(head);
    let first = text.lines().next()?.trim();
    let mut parts = first.split_whitespace();
    Some((
        parts.next()?.to_string(),
        parts.next()?.to_string(),
        parts.next().unwrap_or("HTTP/1.1").to_string(),
    ))
}

pub fn parse_http_status_code(head: &[u8]) -> Option<u16> {
    let text = String::from_utf8_lossy(head);
    let first = text.lines().next()?.trim();
    first.split_whitespace().nth(1)?.parse().ok()
}

pub fn check_http_basic_auth(head: &[u8], user: &str, passwd: &str) -> bool {
    let expected = format!("{user}:{passwd}");
    for name in ["Authorization", "Proxy-Authorization"] {
        let Some(value) = http_header_value(head, name) else {
            continue;
        };
        let mut parts = value.splitn(2, ' ');
        let scheme = parts.next().unwrap_or_default();
        let token = parts.next().unwrap_or_default();
        if !scheme.eq_ignore_ascii_case("Basic") {
            continue;
        }
        if let Ok(decoded) = BASE64_STANDARD.decode(token.trim()) {
            if decoded == expected.as_bytes() {
                return true;
            }
        }
    }
    false
}

pub fn http_header_value(head: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    for line in text.lines().skip(1) {
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case(name) {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

pub fn rewrite_http_request_head(
    head: &[u8],
    host_override: Option<&str>,
    header_change: &str,
    remote_addr: &str,
    add_origin_header: bool,
    force_close: bool,
) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    if let Some(host) = host_override.filter(|host| !host.trim().is_empty()) {
        set_header(&mut headers, "Host", host.trim());
    }

    for line in header_change.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some((name, value)) = line.split_once(':') {
            set_header(&mut headers, name.trim(), value.trim());
        }
    }

    if add_origin_header {
        let remote_ip = remote_addr
            .parse::<std::net::SocketAddr>()
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|_| remote_addr.split(':').next().unwrap_or(remote_addr).to_string());
        let forwarded = match get_header(&headers, "X-Forwarded-For") {
            Some(existing) if !existing.is_empty() => format!("{existing}, {remote_ip}"),
            _ => remote_ip.clone(),
        };
        set_header(&mut headers, "X-Forwarded-For", &forwarded);
        set_header(&mut headers, "X-Real-IP", &remote_ip);
    }

    if force_close {
        remove_header(&mut headers, "Proxy-Connection");
        set_header(&mut headers, "Connection", "close");
    }

    let mut out = String::new();
    out.push_str(&request_line);
    out.push_str("\r\n");
    for (name, value) in headers {
        out.push_str(&name);
        out.push_str(": ");
        out.push_str(&value);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.into_bytes()
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some((_, existing)) = headers
        .iter_mut()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
    {
        *existing = value.to_string();
    } else {
        headers.push((name.to_string(), value.to_string()));
    }
}

fn get_header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
}

pub fn ensure_port(addr: &str, default_port: u16) -> String {
    let clean = addr.trim();
    if clean.starts_with('[') {
        return clean.to_string();
    }
    if clean
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .is_some()
    {
        clean.to_string()
    } else {
        format!("{clean}:{default_port}")
    }
}

pub fn write_http_response<W: Write + ?Sized>(
    stream: &mut W,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

pub fn md5_hex(s: &str) -> String {
    format!("{:x}", md5::compute(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    #[test]
    fn validates_basic_auth_from_authorization_header() {
        let token = BASE64_STANDARD.encode("demo:secret");
        let head = format!(
            "GET / HTTP/1.1\r\nHost: example.com\r\nAuthorization: Basic {token}\r\n\r\n"
        );
        assert!(check_http_basic_auth(head.as_bytes(), "demo", "secret"));
        assert!(!check_http_basic_auth(head.as_bytes(), "demo", "wrong"));
    }

    #[test]
    fn rewrites_host_and_headers() {
        let head = b"GET /a HTTP/1.1\r\nHost: old.test\r\nConnection: keep-alive\r\n\r\n";
        let rewritten = rewrite_http_request_head(
            head,
            Some("new.test"),
            "X-Test: 1\nX-Flag: yes",
            "127.0.0.1:9000",
            true,
            true,
        );
        let text = String::from_utf8(rewritten).unwrap();
        assert!(text.contains("Host: new.test\r\n"));
        assert!(text.contains("X-Test: 1\r\n"));
        assert!(text.contains("X-Flag: yes\r\n"));
        assert!(text.contains("X-Real-IP: 127.0.0.1\r\n"));
        assert!(text.contains("Connection: close\r\n"));
    }

    #[test]
    fn rate_governor_requires_delay_when_bucket_is_empty() {
        let now = Instant::now();
        let mut governor = RateGovernor {
            bytes_per_sec: 1024,
            allowance: 1024.0,
            last: now,
        };
        assert_eq!(governor.plan_delay(1024, now), Duration::ZERO);
        assert!(governor.plan_delay(1024, now) >= Duration::from_millis(900));
    }

    #[test]
    fn metered_stream_blocks_when_flow_limit_is_exceeded() {
        let counter = Arc::new(Mutex::new(FlowCounter::default()));
        let inner = Box::new(MockStream::default()) as Box<dyn RelayStream>;
        let mut stream = MeteredRelayStream::new(
            inner,
            RelayPolicy {
                flow_limit_mb: 1,
                flow_counter: Some(Arc::clone(&counter)),
                ..RelayPolicy::default()
            },
        );
        stream.write_all(&vec![1_u8; 512 * 1024]).unwrap();
        stream.write_all(&vec![1_u8; 512 * 1024]).unwrap();
        let err = stream.write_all(&[1]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(counter.lock().unwrap().inlet_flow, 1024 * 1024);
    }

    #[test]
    fn wraps_snappy_transport_round_trip() {
        round_trip_transport(false, true, b"snappy-payload");
    }

    #[test]
    fn wraps_tls_transport_round_trip() {
        round_trip_transport(true, false, b"tls-payload");
    }

    fn round_trip_transport(crypt: bool, compress: bool, payload: &[u8]) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = payload.to_vec();
        let server_payload = payload.clone();
        let server = thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let mut wrapped = wrap_server_transport(
                Box::new(socket),
                crypt,
                compress,
                RelayPolicy::default(),
            )
            .unwrap();
            let mut buf = vec![0_u8; server_payload.len()];
            wrapped.read_exact(&mut buf).unwrap();
            assert_eq!(buf, server_payload);
            wrapped.write_all(b"ok").unwrap();
            wrapped.flush().unwrap();
        });

        let socket = TcpStream::connect(addr).unwrap();
        let mut wrapped = wrap_client_transport(Box::new(socket), crypt, compress).unwrap();
        wrapped.write_all(payload.as_slice()).unwrap();
        wrapped.flush().unwrap();
        let mut ack = [0_u8; 2];
        wrapped.read_exact(&mut ack).unwrap();
        assert_eq!(&ack, b"ok");
        server.join().unwrap();
    }

    #[derive(Default)]
    struct MockStream {
        written: Vec<u8>,
    }

    impl Read for MockStream {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for MockStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl RelayStream for MockStream {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
            legacy_split_stream(self)
        }
    }
}
