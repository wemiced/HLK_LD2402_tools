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
/// Longest an auto-gain run keeps the device Busy without its 0x00F0 report.
const AUTO_GAIN_LIMIT: Duration = Duration::from_secs(30);
const CALIB_POLL: Duration = Duration::from_millis(500);
/// Pause after a retried or timed-out request: late ACKs of its word are
/// discarded and no new request is started, so none can be mistaken.
const DRAIN_WINDOW: Duration = Duration::from_millis(150);
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
    #[error("serial port {path} is in use by another program")]
    InUse { path: String },
    #[error("no LD2402 answered on {path}")]
    NotLd2402 { path: String },
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
    #[error("command {word:#06x} is outside the LD2402 protocol and was not sent")]
    Forbidden { word: u16 },
    #[error("I/O error: {message}")]
    Io { message: String },
    #[error("could not back up the current settings: {message}")]
    BackupFailed { message: String },
}

/// Saves the sensor's current values before a change; `Err` blocks the change.
pub type BackupFn<'a> = dyn Fn(&SensorParams) -> Result<(), String> + 'a;

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
        if let Err(f) = crate::protocol::safety::check(word, value) {
            eprintln!("blocked command {:#06x}: {}", f.word, f.reason);
            return Err(DeviceError::Forbidden { word: f.word });
        }
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

/// Starts a drain after a request that was retried or timed out. Single-
/// attempt requests (recovery/cleanup) skip it to keep their timing bounded.
fn drain_after(p: &Pending) -> Option<(u16, Instant)> {
    (p.req.attempts > 1 && p.sent > 1).then(|| (p.req.word, Instant::now() + DRAIN_WINDOW))
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
    let mut drain: Option<(u16, Instant)> = None;
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
        drain = drain.filter(|(_, until)| Instant::now() < *until);
        if pending.is_none()
            && drain.is_none()
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
                    if f.word & ACK_BIT == 0 || drain.is_some_and(|(w, _)| w == word) {
                        continue;
                    }
                    // Any attempt's ACK answers the request; later ones drain.
                    if let Some(p) = pending.take_if(|p| word == p.req.word) {
                        drain = drain_after(&p);
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
                drain = drain_after(&p);
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
    // Manual 5.2.4: the only serial-number command is 0x0011; before v3.3.5
    // it answers with raw bytes instead of ASCII.
    let sn = link.cmd(CMD_READ_SN_TEXT, &[])?;
    let serial = if caps.serial_text {
        parse_sn_text(&sn)
    } else {
        parse_sn_hex(&sn)
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
    /// Deadline of the auto-gain run in progress; cleared by the 0x00F0 report.
    auto_gain_until: Arc<Mutex<Option<Instant>>>,
    auto_gain_limit: Duration,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Device {
    #[cfg(test)]
    pub(crate) fn raw_cmd_for_tests(
        &self,
        word: u16,
        value: &[u8],
    ) -> Result<Vec<u8>, DeviceError> {
        self.link.cmd(word, value)
    }

    pub fn open(
        t: Box<dyn Transport>,
        on_event: Box<dyn Fn(DeviceEvent) + Send>,
    ) -> Result<Device, DeviceError> {
        let (tx, rx) = mpsc::channel();
        let dead = Arc::new(AtomicBool::new(false));
        let worker_dead = Arc::clone(&dead);
        let auto_gain_until: Arc<Mutex<Option<Instant>>> = Arc::default();
        let ag = Arc::clone(&auto_gain_until);
        // Cleared before forwarding, so a call made on AutoGainDone is not Busy.
        let on_event: Box<dyn Fn(DeviceEvent) + Send> = Box::new(move |ev| {
            if matches!(ev, DeviceEvent::AutoGainDone) {
                *lock(&ag) = None;
            }
            on_event(ev)
        });
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
            auto_gain_until,
            auto_gain_limit: AUTO_GAIN_LIMIT,
        })
    }

    /// False once the worker has lost the transport.
    pub fn is_alive(&self) -> bool {
        !self.link.dead.load(Ordering::SeqCst)
    }

    pub fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    /// True while an auto-gain run is in progress (until 0x00F0 or the limit).
    fn auto_gaining(&self) -> bool {
        let mut until = lock(&self.auto_gain_until);
        match *until {
            Some(t) if Instant::now() < t => true,
            Some(_) => {
                *until = None;
                false
            }
            None => false,
        }
    }

    fn busy(&self) -> bool {
        self.calibrating.load(Ordering::SeqCst) || self.auto_gaining()
    }

    fn begin(&self) -> Result<MutexGuard<'_, ()>, DeviceError> {
        if self.busy() {
            return Err(DeviceError::Busy);
        }
        let g = lock(&self.op);
        if self.busy() {
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

    /// Chunked read of every parameter; config mode must already be on.
    fn read_all(&self, buffer: u16) -> Result<SensorParams, DeviceError> {
        let mut pairs = vec![];
        for chunk in all_param_ids().chunks(read_chunk_size(buffer)) {
            let rest = self.link.cmd(CMD_READ_PARAMS, &read_params_value(chunk))?;
            let vals = parse_param_values(&rest, chunk.len()).map_err(proto_err)?;
            pairs.extend(chunk.iter().copied().zip(vals));
        }
        pairs_to_params(&pairs).map_err(proto_err)
    }

    /// Writes (and saves when capable); config mode must already be on.
    fn write_all(&self, buffer: u16, p: &SensorParams) -> Result<(), DeviceError> {
        for chunk in params_to_pairs(p).chunks(write_chunk_size(buffer)) {
            self.link
                .cmd(CMD_WRITE_PARAMS, &write_params_value(chunk))?;
        }
        if self.info.capabilities.permanent_save {
            self.link.cmd(CMD_SAVE, &[])?;
        }
        Ok(())
    }

    /// Reads the current values and hands them to `backup`, in config mode.
    fn back_up(&self, buffer: u16, backup: &BackupFn<'_>) -> Result<(), DeviceError> {
        let current = self.read_all(buffer)?;
        backup(&current).map_err(|message| DeviceError::BackupFailed { message })
    }

    pub fn read_params(&self) -> Result<SensorParams, DeviceError> {
        let _g = self.begin()?;
        self.in_config(true, |buffer| self.read_all(buffer))
    }

    /// Unguarded write, test-only: outside tests every change must go
    /// through `write_params_guarded`, so the compiler enforces backups.
    #[cfg(test)]
    pub(crate) fn write_params(&self, p: &SensorParams) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        self.in_config(true, |buffer| self.write_all(buffer, p))
    }

    /// In one config session: read the current values, back them up, then
    /// write. A failed backup ends config and nothing is written.
    pub fn write_params_guarded(
        &self,
        p: &SensorParams,
        backup: &BackupFn<'_>,
    ) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        self.in_config(true, |buffer| {
            self.back_up(buffer, backup)?;
            self.write_all(buffer, p)
        })
    }

    pub fn set_streaming(&self, on: bool) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        let mode = if on { MODE_ENGINEERING } else { MODE_NORMAL };
        self.in_config(false, |_| self.link.cmd(CMD_OUTPUT_MODE, &mode_value(mode)))?;
        self.streaming.store(on, Ordering::SeqCst);
        Ok(())
    }

    /// Unguarded calibration, test-only (see `write_params`); the app uses
    /// `start_calibration_guarded`.
    #[cfg(test)]
    pub(crate) fn start_calibration(&self, c: CalibCoefficients) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        self.enable_config()?;
        self.calibrate_in_config(c)
    }

    /// Backs up the current values in the same config session before
    /// CALIB_START. A failed backup ends config and calibration is not sent.
    pub fn start_calibration_guarded(
        &self,
        c: CalibCoefficients,
        backup: &BackupFn<'_>,
    ) -> Result<(), DeviceError> {
        let _g = self.begin()?;
        let buffer = self.enable_config()?;
        if let Err(e) = self.back_up(buffer, backup) {
            let _ = self.link.quick(CMD_END_CONFIG, &[]);
            return Err(e);
        }
        self.calibrate_in_config(c)
    }

    /// Sends CALIB_START (config already on) and starts the progress helper.
    fn calibrate_in_config(&self, c: CalibCoefficients) -> Result<(), DeviceError> {
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
        // Armed before sending: the 0x00F0 report may follow the ACK at once.
        *lock(&self.auto_gain_until) = Some(Instant::now() + self.auto_gain_limit);
        let mut acked = false;
        let r = self.in_config(false, |_| {
            let r = self.link.once(CMD_AUTO_GAIN, &[]);
            acked = r.is_ok();
            r.map(drop)
        });
        if !acked {
            *lock(&self.auto_gain_until) = None;
        }
        r
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
mod conformity_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
