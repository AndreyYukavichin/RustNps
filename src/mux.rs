use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::model::FlowCounter;
use crate::relay::{IntoIoHalves, RelayRead, RelayStream, RelayWrite};

const FRAME_OPEN: u8 = 1;
const FRAME_DATA: u8 = 2;
const FRAME_CLOSE: u8 = 3;
const HEADER_LEN: usize = 13;
const MAX_FRAME: usize = 16 * 1024 * 1024;

pub struct MuxSession {
    inner: Arc<MuxSessionInner>,
    accept_rx: Mutex<Receiver<MuxStream>>,
}

#[derive(Clone, Default)]
pub struct MuxTrafficPolicy {
    pub rate_limit_kb: u64,
    pub flow_limit_mb: u64,
    pub flow_counter: Option<Arc<Mutex<FlowCounter>>>,
}

struct MuxSessionInner {
    writer: Mutex<Box<dyn RelayWrite>>,
    streams: Mutex<HashMap<u64, Sender<Option<Vec<u8>>>>>,
    accept_tx: Sender<MuxStream>,
    closed: AtomicBool,
    last_activity: AtomicU64,
    traffic: Option<MuxTrafficState>,
}

struct MuxTrafficState {
    limiter: Option<Mutex<MuxRateGovernor>>,
    flow_counter: Arc<Mutex<FlowCounter>>,
    flow_limit_bytes: u64,
}

pub struct MuxStream {
    id: u64,
    session: Weak<MuxSessionInner>,
    rx: Mutex<Receiver<Option<Vec<u8>>>>,
    read_buf: Mutex<VecDeque<u8>>,
    read_closed: AtomicBool,
    write_closed: AtomicBool,
}

impl MuxSession {
    pub fn new<S>(stream: S) -> io::Result<Arc<Self>>
    where
        S: IntoIoHalves + Send + 'static,
    {
        Self::new_with_policy(stream, MuxTrafficPolicy::default())
    }

    pub fn new_with_policy<S>(stream: S, policy: MuxTrafficPolicy) -> io::Result<Arc<Self>>
    where
        S: IntoIoHalves + Send + 'static,
    {
        let (reader, writer) = stream.into_io_halves()?;
        let (accept_tx, accept_rx) = mpsc::channel();
        let traffic = policy.flow_counter.map(|counter| MuxTrafficState {
            limiter: (policy.rate_limit_kb > 0)
                .then(|| Mutex::new(MuxRateGovernor::new(policy.rate_limit_kb * 1024))),
            flow_counter: counter,
            flow_limit_bytes: policy.flow_limit_mb.saturating_mul(1024 * 1024),
        });
        let session = Arc::new(Self {
            inner: Arc::new(MuxSessionInner {
                writer: Mutex::new(writer),
                streams: Mutex::new(HashMap::new()),
                accept_tx,
                closed: AtomicBool::new(false),
                last_activity: AtomicU64::new(
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
                ),
                traffic,
            }),
            accept_rx: Mutex::new(accept_rx),
        });
        let weak = Arc::downgrade(&session.inner);
        let weak_ping = Arc::downgrade(&session.inner);
        thread::spawn(move || read_loop(reader, weak));
        thread::spawn(move || ping_loop(weak_ping));
        Ok(session)
    }

    pub fn open_stream(&self, id: u64) -> io::Result<MuxStream> {
        let stream = self.inner.create_stream(id)?;
        self.inner.send_frame(FRAME_OPEN, id, &[])?;
        Ok(stream)
    }

    pub fn accept(&self) -> io::Result<MuxStream> {
        self.accept_rx
            .lock()
            .unwrap()
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "mux session closed"))
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }
}

impl MuxSessionInner {
    fn create_stream(self: &Arc<Self>, id: u64) -> io::Result<MuxStream> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "mux session closed",
            ));
        }
        let (tx, rx) = mpsc::channel();
        self.streams.lock().unwrap().insert(id, tx);
        Ok(MuxStream {
            id,
            session: Arc::downgrade(self),
            rx: Mutex::new(rx),
            read_buf: Mutex::new(VecDeque::new()),
            read_closed: AtomicBool::new(false),
            write_closed: AtomicBool::new(false),
        })
    }

    fn send_frame(&self, kind: u8, id: u64, payload: &[u8]) -> io::Result<()> {
        if payload.len() > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "mux frame too large"));
        }
        if kind == FRAME_DATA {
            self.record_inlet(payload.len() as u64)?;
            self.limit_rate(payload.len() as u64);
        }
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(&[kind])?;
        writer.write_all(&id.to_le_bytes())?;
        writer.write_all(&(payload.len() as u32).to_le_bytes())?;
        if !payload.is_empty() {
            writer.write_all(payload)?;
        }
        writer.flush()
    }

    fn drop_stream(&self, id: u64) {
        self.streams.lock().unwrap().remove(&id);
    }

    fn close_session(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let streams = std::mem::take(&mut *self.streams.lock().unwrap());
        for (_, tx) in streams {
            let _ = tx.send(None);
        }
    }

    fn record_inlet(&self, bytes: u64) -> io::Result<()> {
        self.record_flow(bytes, 0)
    }

    fn record_export(&self, bytes: u64) -> io::Result<()> {
        self.record_flow(0, bytes)
    }

    fn record_flow(&self, inlet: u64, export: u64) -> io::Result<()> {
        let Some(traffic) = self.traffic.as_ref() else {
            return Ok(());
        };
        let mut counter = traffic.flow_counter.lock().unwrap();
        let next_inlet = counter.inlet_flow.saturating_add(inlet);
        let next_export = counter.export_flow.saturating_add(export);
        if traffic.flow_limit_bytes > 0
            && next_inlet.saturating_add(next_export) > traffic.flow_limit_bytes
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

    fn limit_rate(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        if let Some(traffic) = self.traffic.as_ref() {
            if let Some(limiter) = traffic.limiter.as_ref() {
                limiter.lock().unwrap().wait(bytes);
            }
        }
    }
}

impl MuxStream {
    pub fn id(&self) -> u64 {
        self.id
    }

    fn close_write_once(&self) {
        if self.write_closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(session) = self.session.upgrade() {
            let _ = session.send_frame(FRAME_CLOSE, self.id, &[]);
            session.drop_stream(self.id);
        }
    }
}

impl Read for MuxStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut read_buf = self.read_buf.lock().unwrap();
                if !read_buf.is_empty() {
                    let n = buf.len().min(read_buf.len());
                    for (idx, slot) in buf[..n].iter_mut().enumerate() {
                        *slot = read_buf.pop_front().unwrap_or_else(|| {
                            let _ = idx;
                            0
                        });
                    }
                    return Ok(n);
                }
            }
            if self.read_closed.load(Ordering::SeqCst) {
                return Ok(0);
            }
            match self.rx.lock().unwrap().recv() {
                Ok(Some(bytes)) => {
                    let mut read_buf = self.read_buf.lock().unwrap();
                    read_buf.extend(bytes);
                }
                Ok(None) | Err(_) => {
                    self.read_closed.store(true, Ordering::SeqCst);
                    return Ok(0);
                }
            }
        }
    }
}

impl Write for MuxStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.write_closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "mux stream closed",
            ));
        }
        let Some(session) = self.session.upgrade() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "mux session closed",
            ));
        };
        session.send_frame(FRAME_DATA, self.id, buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RelayStream for MuxStream {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn into_io_halves(self: Box<Self>) -> io::Result<(Box<dyn RelayRead>, Box<dyn RelayWrite>)> {
        let stream = std::mem::ManuallyDrop::new(*self);
        let id = stream.id;
        let session = unsafe { std::ptr::read(&stream.session) };
        let rx = unsafe { std::ptr::read(&stream.rx) };
        let read_buf = unsafe { std::ptr::read(&stream.read_buf) };
        let read_closed = AtomicBool::new(stream.read_closed.load(Ordering::SeqCst));
        let write_closed = AtomicBool::new(stream.write_closed.load(Ordering::SeqCst));
        Ok((
            Box::new(MuxReadHalf {
                rx,
                read_buf,
                read_closed,
            }),
            Box::new(MuxWriteHalf {
                id,
                session,
                write_closed,
            }),
        ))
    }
}

struct MuxReadHalf {
    rx: Mutex<Receiver<Option<Vec<u8>>>>,
    read_buf: Mutex<VecDeque<u8>>,
    read_closed: AtomicBool,
}

impl Read for MuxReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut read_buf = self.read_buf.lock().unwrap();
                if !read_buf.is_empty() {
                    let n = buf.len().min(read_buf.len());
                    for (idx, slot) in buf[..n].iter_mut().enumerate() {
                        *slot = read_buf.pop_front().unwrap_or_else(|| {
                            let _ = idx;
                            0
                        });
                    }
                    return Ok(n);
                }
            }
            if self.read_closed.load(Ordering::SeqCst) {
                return Ok(0);
            }
            match self.rx.lock().unwrap().recv() {
                Ok(Some(bytes)) => {
                    let mut read_buf = self.read_buf.lock().unwrap();
                    read_buf.extend(bytes);
                }
                Ok(None) | Err(_) => {
                    self.read_closed.store(true, Ordering::SeqCst);
                    return Ok(0);
                }
            }
        }
    }
}

struct MuxWriteHalf {
    id: u64,
    session: Weak<MuxSessionInner>,
    write_closed: AtomicBool,
}

impl MuxWriteHalf {
    fn close_write_once(&self) {
        if self.write_closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(session) = self.session.upgrade() {
            let _ = session.send_frame(FRAME_CLOSE, self.id, &[]);
            session.drop_stream(self.id);
        }
    }
}

impl Write for MuxWriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.write_closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "mux stream closed",
            ));
        }
        let Some(session) = self.session.upgrade() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "mux session closed",
            ));
        };
        session.send_frame(FRAME_DATA, self.id, buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RelayWrite for MuxWriteHalf {
    fn shutdown_write(&mut self) -> io::Result<()> {
        self.close_write_once();
        Ok(())
    }
}

impl Drop for MuxWriteHalf {
    fn drop(&mut self) {
        self.close_write_once();
    }
}

impl Drop for MuxStream {
    fn drop(&mut self) {
        self.close_write_once();
    }
}

fn read_loop(mut reader: Box<dyn RelayRead>, session: Weak<MuxSessionInner>) {
    loop {
        let mut header = [0_u8; HEADER_LEN];
        if let Err(err) = reader.read_exact(&mut header) {
            if err.kind() == io::ErrorKind::UnexpectedEof {
                crate::log_debug!("npc", "mux: read session unpack from connection err EOF");
            } else if err.kind() == io::ErrorKind::TimedOut || err.kind() == io::ErrorKind::WouldBlock {
                crate::log_debug!("npc", "mux: read session unpack from connection err timeout");
            } else {
                crate::log_debug!("npc", "mux: read session unpack from connection err {}", err);
            }
            break;
        }
        let kind = header[0];
        let mut id_buf = [0_u8; 8];
        id_buf.copy_from_slice(&header[1..9]);
        let id = u64::from_le_bytes(id_buf);
        let mut len_buf = [0_u8; 4];
        len_buf.copy_from_slice(&header[9..13]);
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > MAX_FRAME {
            crate::log_error!("npc", "mux: frame too large");
            break;
        }
        let mut payload = vec![0_u8; len];
        if len > 0 {
            if let Err(err) = reader.read_exact(&mut payload) {
                if err.kind() == io::ErrorKind::UnexpectedEof {
                    crate::log_debug!("npc", "mux: read session unpack from connection err EOF");
                } else if err.kind() == io::ErrorKind::TimedOut || err.kind() == io::ErrorKind::WouldBlock {
                    crate::log_debug!("npc", "mux: read session unpack from connection err timeout");
                } else {
                    crate::log_debug!("npc", "mux: read session unpack from connection err {}", err);
                }
                break;
            }
        }
        let Some(session) = session.upgrade() else {
            break;
        };
        session.last_activity.store(
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            Ordering::Relaxed,
        );
        if kind == FRAME_DATA {
            if session.record_export(payload.len() as u64).is_err() {
                session.close_session();
                break;
            }
            session.limit_rate(payload.len() as u64);
        }
        match kind {
            FRAME_OPEN => {
                if let Ok(stream) = session.create_stream(id) {
                    let _ = session.accept_tx.send(stream);
                }
            }
            FRAME_DATA => {
                let tx = session.streams.lock().unwrap().get(&id).cloned();
                if let Some(tx) = tx {
                    let _ = tx.send(Some(payload));
                }
            }
            FRAME_CLOSE => {
                let tx = session.streams.lock().unwrap().remove(&id);
                if let Some(tx) = tx {
                    let _ = tx.send(None);
                }
            }
            _ => break,
        }
    }
    crate::log_debug!("npc", "close mux");
    if let Some(session) = session.upgrade() {
        session.close_session();
    }
}

fn ping_loop(session_weak: Weak<MuxSessionInner>) {
    let mut checktime = 0;
    let threshold = 60;
    loop {
        thread::sleep(Duration::from_secs(5));
        let Some(session) = session_weak.upgrade() else {
            break;
        };
        if session.closed.load(Ordering::SeqCst) {
            break;
        }
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let last = session.last_activity.load(Ordering::Relaxed);
        
        if now.saturating_sub(last) < 5 {
            checktime = 0;
        } else {
            checktime += 1;
        }
        
        if checktime > threshold {
            crate::log_debug!(
                "npc",
                "mux: ping time out, checktime {} threshold {}",
                checktime,
                threshold
            );
            session.close_session();
            break;
        }
    }
}

struct MuxRateGovernor {
    bytes_per_sec: u64,
    allowance: f64,
    last: Instant,
}

impl MuxRateGovernor {
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