//! Device session: a worker thread owns the transport and matches requests to ACKs.
//! Also holds the device-level errors shared with the transport layer.
use crate::protocol::commands::*;
use crate::protocol::convert::{Capabilities, capabilities, parse_version};
use crate::protocol::frame::{
    CmdFrame, FrameDecoder, Inbound, RawEngineering, TextReport, encode_command,
};
use crate::transport::Transport;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const ACK_BIT: u16 = 0x0100;
const ACK_TIMEOUT: Duration = Duration::from_millis(500);
const ACK_ATTEMPTS: u32 = 2;
const CLEANUP_TIMEOUT: Duration = Duration::from_millis(300);
const CALIB_POLL: Duration = Duration::from_millis(500);
/// How long an unanswered attempt's ACK is still expected (and discarded).
const STALE_WINDOW: Duration = Duration::from_millis(1000);
/// Extra caller-side margin over the worker's own ACK deadline.
const REPLY_MARGIN: Duration = Duration::from_millis(500);
const DEFAULT_BUFFER: u16 = 0x20;
const MODE_ENGINEERING: u32 = 0x04;
const MODE_NORMAL: u32 = 0x64;
const ENABLE_VALUE: [u8; 2] = [0x01, 0x00];

#[derive(thiserror::Error, Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DeviceError {
    #[error("permission denied opening {path}")]
    PermissionDenied { path: String },
    #[error("serial port {path} not found")]
    NotFound { path: String },
    #[error("no answer to command {word:#06x}")]
    Timeout { word: u16 },
    #[error("command {word:#06x} rejected with status {status}")]
    Nack { word: u16, status: u16 },
    #[error("{feature} needs firmware {required} or newer")]
    Unsupported { feature: String, required: String },
    #[error("device disconnected")]
    Disconnected,
    #[error("device is busy")]
    Busy,
    #[error("I/O error: {message}")]
    Io { message: String },
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub firmware: String,
    pub serial: String,
    pub capabilities: Capabilities,
    pub power: PowerStatus,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum PowerStatus {
    NotTested,
    Ok,
    Interference,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibCoefficients {
    pub trigger: f32,
    pub hold: f32,
    pub micro: f32,
}

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    Frame(RawEngineering),
    Text(TextReport),
    CalibProgress(u8),
    CalibDone { gates: Vec<u8> },
    CalibFailed { error: DeviceError },
    AutoGainDone,
    Disconnected { reason: String },
}

type Reply = Result<CmdFrame, DeviceError>;

struct Request {
    word: u16,
    frame: Vec<u8>,
    timeout: Duration,
    attempts: u32,
    reply: Sender<Reply>,
}

enum Msg {
    Req(Request),
    Emit(DeviceEvent),
    Shutdown,
}

fn proto_err(e: ProtocolError) -> DeviceError {
    match e {
        ProtocolError::Nack { word, status } => DeviceError::Nack { word, status },
        other => DeviceError::Io {
            message: other.to_string(),
        },
    }
}

fn mode_value(mode: u32) -> Vec<u8> {
    let mut v = vec![0, 0];
    v.extend(mode.to_le_bytes());
    v
}

fn power_status(value: Option<u32>) -> PowerStatus {
    match value {
        Some(1) => PowerStatus::Ok,
        Some(2) => PowerStatus::Interference,
        _ => PowerStatus::NotTested,
    }
}

/// Handle to the worker thread; cheap to clone for helper threads.
#[derive(Clone)]
struct Link {
    tx: Sender<Msg>,
    dead: Arc<AtomicBool>,
}

impl Link {
    fn request(&self, word: u16, value: &[u8], timeout: Duration, attempts: u32) -> Reply {
        if self.dead.load(Ordering::SeqCst) {
            return Err(DeviceError::Disconnected);
        }
        let (reply, rx) = mpsc::channel();
        let req = Request {
            word,
            frame: encode_command(word, value),
            timeout,
            attempts,
            reply,
        };
        self.tx
            .send(Msg::Req(req))
            .map_err(|_| DeviceError::Disconnected)?;
        // The worker answers or drops the sender; the bound guards against a
        // worker stuck in a callback, so `close` can never hang.
        match rx.recv_timeout(timeout * attempts + REPLY_MARGIN) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => Err(DeviceError::Timeout { word }),
            Err(RecvTimeoutError::Disconnected) => Err(DeviceError::Disconnected),
        }
    }

    fn send(
        &self,
        word: u16,
        value: &[u8],
        timeout: Duration,
        attempts: u32,
    ) -> Result<Vec<u8>, DeviceError> {
        let f = self.request(word, value, timeout, attempts)?;
        parse_status(&f).map(<[u8]>::to_vec).map_err(proto_err)
    }

    /// Normal request: 500 ms ACK timeout, one retry, status checked.
    fn cmd(&self, word: u16, value: &[u8]) -> Result<Vec<u8>, DeviceError> {
        self.send(word, value, ACK_TIMEOUT, ACK_ATTEMPTS)
    }

    /// Non-idempotent command: a single 500 ms attempt, never resent.
    fn once(&self, word: u16, value: &[u8]) -> Result<Vec<u8>, DeviceError> {
        self.send(word, value, ACK_TIMEOUT, 1)
    }

    /// Recovery/cleanup request: a single 300 ms attempt.
    fn quick(&self, word: u16, value: &[u8]) -> Result<Vec<u8>, DeviceError> {
        self.send(word, value, CLEANUP_TIMEOUT, 1)
    }

    fn emit(&self, e: DeviceEvent) {
        let _ = self.tx.send(Msg::Emit(e));
    }

    /// Best effort: enter config, normal output, end config.
    fn restore_normal(&self) {
        let _ = self.quick(CMD_ENABLE_CONFIG, &ENABLE_VALUE);
        let _ = self.quick(CMD_OUTPUT_MODE, &mode_value(MODE_NORMAL));
        let _ = self.quick(CMD_END_CONFIG, &[]);
    }
}

struct Pending {
    req: Request,
    deadline: Instant,
    left: u32,
    sent: u32,
}

/// ACKs still owed for attempts that timed out; the next matching one is
/// discarded so it cannot satisfy a later request with the same word.
#[derive(Default)]
struct Stale(Vec<(u16, Instant)>);

impl Stale {
    fn add(&mut self, word: u16, n: u32) {
        let until = Instant::now() + STALE_WINDOW;
        self.0.extend((0..n).map(|_| (word, until)));
    }
    /// True (and forgets one entry) when an ACK of `word` is still owed.
    fn take(&mut self, word: u16) -> bool {
        let now = Instant::now();
        self.0.retain(|(_, until)| *until > now);
        match self.0.iter().position(|(w, _)| *w == word) {
            Some(i) => {
                self.0.remove(i);
                true
            }
            None => false,
        }
    }
}

/// Owns the transport: sends requests one at a time, routes inbound frames.
fn worker_loop(
    mut t: Box<dyn Transport>,
    rx: Receiver<Msg>,
    on_event: Box<dyn Fn(DeviceEvent) + Send>,
    dead: Arc<AtomicBool>,
) {
    let mut dec = FrameDecoder::new();
    let mut queue: VecDeque<Request> = VecDeque::new();
    let mut pending: Option<Pending> = None;
    let mut stale = Stale::default();
    let mut buf = [0u8; 512];
    let reason = loop {
        loop {
            match rx.try_recv() {
                Ok(Msg::Req(r)) => queue.push_back(r),
                Ok(Msg::Emit(e)) => on_event(e),
                Ok(Msg::Shutdown) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }
        if pending.is_none()
            && let Some(req) = queue.pop_front()
        {
            if let Err(e) = t.write_all(&req.frame) {
                break e.to_string();
            }
            pending = Some(Pending {
                deadline: Instant::now() + req.timeout,
                left: req.attempts.saturating_sub(1),
                sent: 1,
                req,
            });
        }
        // Blocks for at most the transport read timeout (~50 ms): no busy loop.
        let n = match t.read(&mut buf) {
            Ok(n) => n,
            Err(e) => break e.to_string(),
        };
        for item in dec.push(&buf[..n]) {
            match item {
                Inbound::Cmd(f) if f.word == EVT_AUTO_GAIN_DONE => {
                    on_event(DeviceEvent::AutoGainDone)
                }
                Inbound::Cmd(f) => {
                    let word = f.word & !ACK_BIT;
                    if f.word & ACK_BIT == 0 || stale.take(word) {
                        continue;
                    }
                    if let Some(p) = pending.take_if(|p| word == p.req.word) {
                        // Either attempt's ACK answers this request; the others are owed.
                        stale.add(word, p.sent - 1);
                        let _ = p.req.reply.send(Ok(f));
                    }
                }
                Inbound::Engineering(e) => on_event(DeviceEvent::Frame(e)),
                Inbound::Text(r) => on_event(DeviceEvent::Text(r)),
            }
        }
        if let Some(p) = pending.as_mut()
            && Instant::now() >= p.deadline
        {
            if p.left > 0 {
                p.left -= 1;
                p.sent += 1;
                p.deadline = Instant::now() + p.req.timeout;
                if let Err(e) = t.write_all(&p.req.frame) {
                    break e.to_string();
                }
            } else if let Some(p) = pending.take() {
                stale.add(p.req.word, p.sent);
                let _ = p
                    .req
                    .reply
                    .send(Err(DeviceError::Timeout { word: p.req.word }));
            }
        }
    };
    // Dropping `pending`, `queue` and `rx` makes every waiting caller see Disconnected.
    dead.store(true, Ordering::SeqCst);
    on_event(DeviceEvent::Disconnected { reason });
}

/// Polls progress until 100 % and returns the disturbed gates (config still
/// open), or `None` when `stop` fires (cleanup is then left to `close`).
fn poll_calibration(link: &Link, stop: &Receiver<()>) -> Result<Option<Vec<u8>>, DeviceError> {
    loop {
        if !matches!(
            stop.recv_timeout(CALIB_POLL),
            Err(RecvTimeoutError::Timeout)
        ) {
            return Ok(None);
        }
        let p = parse_progress(&link.cmd(CMD_CALIB_PROGRESS, &[])?).map_err(proto_err)?;
        link.emit(DeviceEvent::CalibProgress(p));
        if p >= 100 {
            let rest = link.cmd(CMD_CALIB_INTERFERENCE, &[])?;
            return parse_interference(&rest).map(Some).map_err(proto_err);
        }
    }
}

/// Calibration helper body. Config is ended and `busy` cleared before the
/// final event, so a call made on `CalibDone`/`CalibFailed` is not `Busy`.
fn run_calibration(link: &Link, stop: &Receiver<()>, busy: &AtomicBool) {
    let outcome = match poll_calibration(link, stop) {
        Ok(None) => None,
        Ok(Some(gates)) => Some(link.cmd(CMD_END_CONFIG, &[]).map(|_| gates)),
        Err(e) => {
            let _ = link.quick(CMD_END_CONFIG, &[]);
            Some(Err(e))
        }
    };
    busy.store(false, Ordering::SeqCst);
    match outcome {
        Some(Ok(gates)) => link.emit(DeviceEvent::CalibDone { gates }),
        // Transport death already emitted `Disconnected`.
        Some(Err(DeviceError::Disconnected)) | None => {}
        Some(Err(error)) => link.emit(DeviceEvent::CalibFailed { error }),
    }
}

/// Reads identity while recovering the sensor into a known state.
/// If the recovery enable gets no ACK, one more quick enable is tried (still
/// ignored), so an unresponsive sensor fails on the version read in ~1.9 s.
/// `entered` is set once an enable was ACKed, so a failed open can clean up.
fn identify(link: &Link, entered: &mut bool) -> Result<DeviceInfo, DeviceError> {
    *entered = link.quick(CMD_ENABLE_CONFIG, &ENABLE_VALUE).is_ok()
        || link.quick(CMD_ENABLE_CONFIG, &ENABLE_VALUE).is_ok();
    let _ = link.quick(CMD_OUTPUT_MODE, &mode_value(MODE_NORMAL));
    let firmware = parse_version_ack(&link.cmd(CMD_READ_VERSION, &[])?).map_err(proto_err)?;
    let caps = capabilities(parse_version(&firmware));
    let serial = if caps.serial_text {
        parse_sn_text(&link.cmd(CMD_READ_SN_TEXT, &[])?)
    } else {
        parse_sn_hex(&link.cmd(CMD_READ_SN_HEX, &[])?)
    };
    let power = match link.cmd(CMD_READ_PARAMS, &read_params_value(&[P_POWER_INTERFERENCE])) {
        Ok(rest) => power_status(parse_param_values(&rest, 1).ok().map(|v| v[0])),
        // Firmware without the power-interference parameter rejects the read.
        Err(DeviceError::Nack { .. }) => PowerStatus::NotTested,
        Err(e) => return Err(e),
    };
    link.cmd(CMD_END_CONFIG, &[])?;
    Ok(DeviceInfo {
        firmware,
        serial,
        capabilities: caps,
        power,
    })
}

type CalibHelper = (Sender<()>, JoinHandle<()>);

pub struct Device {
    link: Link,
    info: DeviceInfo,
    worker: Option<JoinHandle<()>>,
    /// Serialises multi-step sequences. The worker never takes it, so waiting
    /// on a reply while holding it cannot deadlock.
    op: Mutex<()>,
    calibrating: Arc<AtomicBool>,
    calib: Mutex<Option<CalibHelper>>,
    streaming: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Device {
    pub fn open(
        t: Box<dyn Transport>,
        on_event: Box<dyn Fn(DeviceEvent) + Send>,
    ) -> Result<Device, DeviceError> {
        let (tx, rx) = mpsc::channel();
        let dead = Arc::new(AtomicBool::new(false));
        let worker_dead = Arc::clone(&dead);
        let worker = thread::Builder::new()
            .name("ld2402-worker".into())
            .spawn(move || worker_loop(t, rx, on_event, worker_dead))
            .map_err(|e| DeviceError::Io {
                message: e.to_string(),
            })?;
        let link = Link { tx, dead };
        let mut entered = false;
        let info = match identify(&link, &mut entered) {
            Ok(v) => v,
            Err(e) => {
                if entered {
                    link.restore_normal(); // never leave the sensor in config mode
                }
                let _ = link.tx.send(Msg::Shutdown);
                let _ = worker.join();
                return Err(e);
            }
        };
        Ok(Device {
            link,
            info,
            worker: Some(worker),
            op: Mutex::new(()),
            calibrating: Arc::new(AtomicBool::new(false)),
            calib: Mutex::new(None),
            streaming: AtomicBool::new(false),
        })
    }

    pub fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn begin(&self) -> Result<MutexGuard<'_, ()>, DeviceError> {
        if self.calibrating.load(Ordering::SeqCst) {
            return Err(DeviceError::Busy);
        }
        let g = lock(&self.op);
        if self.calibrating.load(Ordering::SeqCst) {
            return Err(DeviceError::Busy);
        }
        Ok(g)
    }

    /// Enters config; returns the buffer size from the ACK (chunking basis).
    fn enable_config(&self) -> Result<u16, DeviceError> {
        let rest = self.link.cmd(CMD_ENABLE_CONFIG, &ENABLE_VALUE)?;
        Ok(parse_enable_config(&rest).unwrap_or(DEFAULT_BUFFER))
    }

    /// enable → body → (engineering again if streaming and `restore`) → end.
    /// On failure, config is ended best effort.
    fn in_config<T>(
        &self,
        restore: bool,
        body: impl FnOnce(u16) -> Result<T, DeviceError>,
    ) -> Result<T, DeviceError> {
        let r = self.enable_config().and_then(body).and_then(|v| {
            if restore && self.streaming.load(Ordering::SeqCst) {
                self.link
                    .cmd(CMD_OUTPUT_MODE, &mode_value(MODE_ENGINEERING))?;
            }
            self.link.cmd(CMD_END_CONFIG, &[])?;
            Ok(v)
        });
        if r.is_err() {
            let _ = self.link.quick(CMD_END_CONFIG, &[]);
        }
        r
    }

    pub fn read_params(&self) -> Result<SensorParams, DeviceError> {
        let _g = self.begin()?;
        self.in_config(true, |buffer| {
            let mut pairs = vec![];
            for chunk in all_param_ids().chunks(read_chunk_size(buffer)) {
                let rest = self.link.cmd(CMD_READ_PARAMS, &read_params_value(chunk))?;
                let vals = parse_param_values(&rest, chunk.len()).map_err(proto_err)?;
                pairs.extend(chunk.iter().copied().zip(vals));
            }
            pairs_to_params(&pairs).map_err(proto_err)
        })
    }

    pub fn write_params(&self, p: &SensorParams) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        self.in_config(true, |buffer| {
            for chunk in params_to_pairs(p).chunks(write_chunk_size(buffer)) {
                self.link
                    .cmd(CMD_WRITE_PARAMS, &write_params_value(chunk))?;
            }
            if self.info.capabilities.permanent_save {
                self.link.cmd(CMD_SAVE, &[])?;
            }
            Ok(())
        })
    }

    pub fn set_streaming(&self, on: bool) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        let mode = if on { MODE_ENGINEERING } else { MODE_NORMAL };
        self.in_config(false, |_| self.link.cmd(CMD_OUTPUT_MODE, &mode_value(mode)))?;
        self.streaming.store(on, Ordering::SeqCst);
        Ok(())
    }

    pub fn start_calibration(&self, c: CalibCoefficients) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        self.enable_config()?;
        let value = calib_value(c.trigger, c.hold, c.micro);
        if let Err(e) = self.link.once(CMD_CALIB_START, &value) {
            let _ = self.link.quick(CMD_END_CONFIG, &[]);
            return Err(e);
        }
        let mut slot = lock(&self.calib);
        if let Some((_, old)) = slot.take() {
            let _ = old.join(); // already finished: `calibrating` was false
        }
        self.calibrating.store(true, Ordering::SeqCst);
        let (stop_tx, stop_rx) = mpsc::channel();
        let link = self.link.clone();
        let flag = Arc::clone(&self.calibrating);
        let helper = thread::spawn(move || run_calibration(&link, &stop_rx, &flag));
        *slot = Some((stop_tx, helper));
        Ok(())
    }

    pub fn auto_gain(&self) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        if !self.info.capabilities.auto_gain {
            return Err(DeviceError::Unsupported {
                feature: "autoGain".into(),
                required: "3.3.5".into(),
            });
        }
        self.in_config(false, |_| self.link.once(CMD_AUTO_GAIN, &[]).map(drop))
    }

    pub fn close(self) {
        drop(self);
    }

    fn shutdown(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        // Take the helper out first so no lock is held while joining.
        let helper = lock(&self.calib).take();
        if let Some((stop, h)) = helper {
            drop(stop); // wakes the helper's poll wait
            let _ = h.join();
        }
        self.link.restore_normal();
        let _ = self.link.tx.send(Msg::Shutdown);
        let _ = worker.join();
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{SimHandle, SimOptions, SimSensor};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    fn try_open(
        opts: SimOptions,
    ) -> (
        Result<Device, DeviceError>,
        SimHandle,
        Receiver<DeviceEvent>,
    ) {
        let sim = SimSensor::new(opts);
        let h = sim.sim_handle();
        let (tx, rx) = mpsc::channel();
        let d = Device::open(
            Box::new(sim),
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        (d, h, rx)
    }

    /// Sim wrapper whose filter decides which frames (by command word) reach the sim.
    type Filter = Box<dyn FnMut(u16, &[u8]) -> Vec<Vec<u8>> + Send>;

    struct Tamper {
        inner: SimSensor,
        filter: Filter,
    }

    impl Transport for Tamper {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }
        fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
            let word = u16::from_le_bytes([data[6], data[7]]);
            for f in (self.filter)(word, data) {
                self.inner.write_all(&f)?;
            }
            Ok(())
        }
    }

    fn try_open_tampered(
        filter: Filter,
    ) -> (
        Result<Device, DeviceError>,
        SimHandle,
        Receiver<DeviceEvent>,
    ) {
        let inner = SimSensor::new(SimOptions::default());
        let h = inner.sim_handle();
        let (tx, rx) = mpsc::channel();
        let d = Device::open(
            Box::new(Tamper { inner, filter }),
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        (d, h, rx)
    }

    /// Drops every frame carrying `word`.
    fn drop_word(word: u16) -> Filter {
        Box::new(move |w, f| if w == word { vec![] } else { vec![f.to_vec()] })
    }

    fn open(opts: SimOptions) -> (Device, SimHandle, Receiver<DeviceEvent>) {
        let (d, h, rx) = try_open(opts);
        match d {
            Ok(d) => (d, h, rx),
            Err(e) => panic!("open failed: {e}"),
        }
    }

    fn version(v: &str) -> SimOptions {
        SimOptions {
            version: v.into(),
            ..Default::default()
        }
    }

    fn coeffs() -> CalibCoefficients {
        CalibCoefficients {
            trigger: 3.0,
            hold: 2.0,
            micro: 3.0,
        }
    }

    fn modified(h: &SimHandle) -> SensorParams {
        let mut p = h.params();
        p.max_distance_m = 7.5;
        p.disappear_delay_s = 12;
        p.trigger_db[3] = 55.0;
        p.micro_db[15] = 21.0;
        p
    }

    #[test]
    fn device_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Device>();
    }

    #[test]
    fn open_reads_info() {
        let (d, h, _rx) = open(SimOptions::default());
        let info = d.info();
        assert_eq!(info.firmware, "v3.3.5");
        assert!(info.capabilities.auto_gain);
        assert_eq!(info.serial, "SIM-LD2402-0001");
        assert_eq!(info.power, PowerStatus::NotTested);
        assert!(!h.in_config());
    }

    #[test]
    fn open_reads_hex_serial_on_old_firmware() {
        let (d, _h, _rx) = open(version("v3.3.1"));
        assert_eq!(d.info().serial, "4CD24002A17B0019");
        assert!(!d.info().capabilities.serial_text);
    }

    #[test]
    fn open_recovers_from_engineering_mode() {
        let (d, h, _rx) = open(SimOptions {
            start_in_engineering: true,
            start_in_config: true,
            ..Default::default()
        });
        assert_eq!(d.info().firmware, "v3.3.5");
        assert!(!h.engineering() && !h.in_config());
    }

    #[test]
    fn read_params_matches_sim_and_keeps_streaming() {
        let (d, h, _rx) = open(SimOptions::default());
        d.set_streaming(true).unwrap();
        assert_eq!(d.read_params().unwrap(), h.params());
        assert!(h.engineering() && !h.in_config());
    }

    #[test]
    fn write_params_persists_and_saves() {
        let (d, h, _rx) = open(SimOptions::default());
        let p = modified(&h);
        d.write_params(&p).unwrap();
        assert_eq!(h.params(), p);
        assert!(h.saved());
        assert!(!h.in_config());
    }

    #[test]
    fn write_params_without_save_capability() {
        let (d, h, _rx) = open(version("v3.3.1"));
        let p = modified(&h);
        d.write_params(&p).unwrap();
        assert_eq!(h.params(), p);
        assert!(!h.saved());
    }

    #[test]
    fn streaming_emits_frames_and_close_restores_normal() {
        let (d, h, rx) = open(SimOptions::default());
        d.set_streaming(true).unwrap();
        assert!(h.engineering() && !h.in_config());
        std::thread::sleep(Duration::from_millis(350));
        let frames = rx
            .try_iter()
            .filter(|e| matches!(e, DeviceEvent::Frame(_)))
            .count();
        assert!(frames >= 2, "got {frames} frames");
        d.close();
        assert!(!h.engineering() && !h.in_config());
    }

    #[test]
    fn drop_restores_normal_mode() {
        let (d, h, _rx) = open(SimOptions::default());
        d.set_streaming(true).unwrap();
        assert!(h.engineering());
        drop(d);
        assert!(!h.engineering() && !h.in_config());
    }

    #[test]
    fn calibration_reports_progress_and_gates() {
        let (d, h, rx) = open(SimOptions::default());
        d.start_calibration(coeffs()).unwrap();
        let mut progress = vec![];
        let end = Instant::now() + Duration::from_secs(10);
        let gates = loop {
            let left = end.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left).expect("CalibDone within 10 s") {
                DeviceEvent::CalibProgress(p) => progress.push(p),
                DeviceEvent::CalibDone { gates } => break gates,
                _ => {}
            }
        };
        assert!(progress.windows(2).all(|w| w[0] < w[1]), "{progress:?}");
        assert_eq!(progress.last(), Some(&100));
        assert_eq!(gates, vec![1, 10, 15]);
        assert!(!h.in_config());
        assert_eq!(d.read_params(), Ok(h.params()));
    }

    #[test]
    fn calibration_failure_emits_event_and_clears_busy() {
        let (d, h, rx) = try_open_tampered(drop_word(CMD_CALIB_PROGRESS));
        let d = d.unwrap_or_else(|e| panic!("open failed: {e}"));
        d.start_calibration(coeffs()).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        let error = loop {
            let left = end.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left).expect("CalibFailed within 5 s") {
                DeviceEvent::CalibFailed { error } => break error,
                DeviceEvent::CalibDone { .. } => panic!("unexpected CalibDone"),
                _ => {}
            }
        };
        assert_eq!(
            error,
            DeviceError::Timeout {
                word: CMD_CALIB_PROGRESS
            }
        );
        assert!(!h.in_config());
        assert_eq!(d.read_params(), Ok(h.params()));
    }

    #[test]
    fn failed_open_leaves_config_mode() {
        let (d, h, _rx) = try_open_tampered(drop_word(CMD_READ_SN_TEXT));
        assert_eq!(
            d.err(),
            Some(DeviceError::Timeout {
                word: CMD_READ_SN_TEXT
            })
        );
        assert!(!h.in_config() && !h.engineering());
    }

    #[test]
    fn late_ack_after_retry_is_not_taken_by_next_chunk() {
        // 0x0008 writes: #1 is open's power read; #2 (chunk 1) is held until
        // #4 (chunk 2), so chunk 1's late ACK lands while chunk 2 is pending.
        let mut n = 0;
        let mut held = None;
        let filter: Filter = Box::new(move |w, f| {
            if w != CMD_READ_PARAMS {
                return vec![f.to_vec()];
            }
            n += 1;
            match n {
                2 => {
                    held = Some(f.to_vec());
                    vec![]
                }
                4 => held.take().into_iter().chain([f.to_vec()]).collect(),
                _ => vec![f.to_vec()],
            }
        });
        let (d, h, _rx) = try_open_tampered(filter);
        let d = d.unwrap_or_else(|e| panic!("open failed: {e}"));
        assert_eq!(d.read_params(), Ok(h.params()));
    }

    #[test]
    fn busy_during_calibration() {
        let (d, h, _rx) = open(SimOptions::default());
        let p = h.params();
        d.start_calibration(coeffs()).unwrap();
        assert_eq!(d.read_params(), Err(DeviceError::Busy));
        assert_eq!(d.write_params(&p), Err(DeviceError::Busy));
        assert_eq!(d.set_streaming(true), Err(DeviceError::Busy));
        assert_eq!(d.auto_gain(), Err(DeviceError::Busy));
        assert_eq!(d.start_calibration(coeffs()), Err(DeviceError::Busy));
        assert_eq!(d.info().firmware, "v3.3.5");
        d.close();
        assert!(!h.in_config() && !h.engineering());
    }

    #[test]
    fn auto_gain_unsupported_on_old_firmware() {
        let (d, _h, _rx) = open(version("v3.3.1"));
        assert_eq!(
            d.auto_gain(),
            Err(DeviceError::Unsupported {
                feature: "autoGain".into(),
                required: "3.3.5".into()
            })
        );
    }

    #[test]
    fn auto_gain_emits_done_event() {
        let (d, h, rx) = open(SimOptions::default());
        d.auto_gain().unwrap();
        let done = std::iter::from_fn(|| rx.recv_timeout(Duration::from_secs(1)).ok())
            .any(|e| matches!(e, DeviceEvent::AutoGainDone));
        assert!(done, "no AutoGainDone event");
        assert!(!h.in_config());
    }

    #[test]
    fn unresponsive_open_times_out() {
        let start = Instant::now();
        let (d, _h, _rx) = try_open(SimOptions {
            unresponsive: true,
            ..Default::default()
        });
        assert_eq!(d.err(), Some(DeviceError::Timeout { word: 0x0000 }));
        assert!(
            start.elapsed() < Duration::from_millis(2500),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn transport_error_emits_disconnected() {
        let (d, _h, rx) = open(SimOptions {
            fail_after_reads: Some(20),
            ..Default::default()
        });
        let end = Instant::now() + Duration::from_secs(1);
        let got = loop {
            let left = end.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(DeviceEvent::Disconnected { .. }) => break true,
                Ok(_) => {}
                Err(_) => break false,
            }
        };
        assert!(got, "no Disconnected event within 1 s");
        assert_eq!(d.read_params(), Err(DeviceError::Disconnected));
    }
}
