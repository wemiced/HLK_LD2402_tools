//! Device session tests, all against `SimSensor`.
use super::*;
use crate::sim::{SimHandle, SimOptions, SimSensor};
use std::sync::atomic::AtomicUsize;
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
    assert_eq!(d.info().serial, "250318");
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
            DeviceEvent::CalibDone { gates, strong } => {
                assert!(!strong);
                break gates;
            }
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

/// Counts 0x0008 writes; `on(n, frame)` decides what reaches the sim.
/// Write #1 is open's power-status read, #2 is read_params chunk 1.
fn read_params_filter(
    writes: Arc<AtomicUsize>,
    mut on: impl FnMut(usize, &[u8]) -> Vec<Vec<u8>> + Send + 'static,
) -> Filter {
    Box::new(move |w, f| {
        if w != CMD_READ_PARAMS {
            return vec![f.to_vec()];
        }
        let n = writes.fetch_add(1, Ordering::SeqCst) + 1;
        on(n, f)
    })
}

#[test]
fn late_ack_after_retry_is_not_taken_by_next_chunk() {
    // Chunk 1's first attempt is held and released together with its
    // retry: two ACKs arrive, the second one late (inside the drain window).
    let mut held = None;
    let filter = read_params_filter(Arc::default(), move |n, f| match n {
        2 => {
            held = Some(f.to_vec());
            vec![]
        }
        3 => held.take().into_iter().chain([f.to_vec()]).collect(),
        _ => vec![f.to_vec()],
    });
    let (d, h, _rx) = try_open_tampered(filter);
    let d = d.unwrap_or_else(|e| panic!("open failed: {e}"));
    assert_eq!(d.read_params(), Ok(h.params()));
}

#[test]
fn lost_attempt_does_not_swallow_next_ack() {
    // Chunk 1's first attempt is lost for good; its retry succeeds. The
    // next chunks must succeed on their first attempt (no spurious timeout).
    let writes = Arc::new(AtomicUsize::new(0));
    let filter = read_params_filter(Arc::clone(&writes), |n, f| match n {
        2 => vec![],
        _ => vec![f.to_vec()],
    });
    let (d, h, _rx) = try_open_tampered(filter);
    let d = d.unwrap_or_else(|e| panic!("open failed: {e}"));
    assert_eq!(d.read_params(), Ok(h.params()));
    let chunks = all_param_ids().len().div_ceil(read_chunk_size(0x20));
    // open's read + every chunk once + the single retry of chunk 1.
    assert_eq!(writes.load(Ordering::SeqCst), 1 + chunks + 1);
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
        start.elapsed() < Duration::from_millis(4500),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn open_waits_for_a_sensor_that_is_still_booting() {
    // Hardware validation: right after power-up the LD2402 ignores commands
    // for a moment, so the first Connect failed with "no LD2402 answered".
    let (d, h, _rx) = try_open(SimOptions {
        boot_delay: Some(Duration::from_millis(1500)),
        ..Default::default()
    });
    let d = d.expect("open succeeds once the sensor has booted");
    assert_eq!(d.info().firmware, "v3.3.5");
    assert!(!h.in_config());
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

fn wait_auto_gain_done(rx: &Receiver<DeviceEvent>) -> bool {
    std::iter::from_fn(|| rx.recv_timeout(Duration::from_secs(1)).ok())
        .any(|e| matches!(e, DeviceEvent::AutoGainDone))
}

#[test]
fn busy_during_auto_gain() {
    let (d, h, rx) = open(SimOptions {
        hold_auto_gain_done: true,
        ..Default::default()
    });
    let p = h.params();
    d.auto_gain().unwrap();
    assert!(!h.in_config());
    assert_eq!(d.read_params(), Err(DeviceError::Busy));
    assert_eq!(d.write_params(&p), Err(DeviceError::Busy));
    assert_eq!(d.set_streaming(true), Err(DeviceError::Busy));
    assert_eq!(d.start_calibration(coeffs()), Err(DeviceError::Busy));
    assert_eq!(d.auto_gain(), Err(DeviceError::Busy));
    h.finish_auto_gain();
    assert!(wait_auto_gain_done(&rx), "no AutoGainDone event");
    assert_eq!(d.read_params(), Ok(p));
}

#[test]
fn auto_gain_busy_expires_without_done_event() {
    let (mut d, h, _rx) = open(SimOptions {
        hold_auto_gain_done: true,
        ..Default::default()
    });
    d.auto_gain_limit = Duration::from_millis(300);
    d.auto_gain().unwrap();
    assert_eq!(d.read_params(), Err(DeviceError::Busy));
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(d.read_params(), Ok(h.params()));
}

#[test]
fn failed_auto_gain_is_not_busy() {
    let (d, h, _rx) = try_open_tampered(drop_word(CMD_AUTO_GAIN));
    let d = d.unwrap_or_else(|e| panic!("open failed: {e}"));
    assert_eq!(
        d.auto_gain(),
        Err(DeviceError::Timeout {
            word: CMD_AUTO_GAIN
        })
    );
    assert_eq!(d.read_params(), Ok(h.params()));
}

fn open_recording() -> (Device, super::test_support::Frames) {
    let t = super::test_support::RecordingTransport::new(SimOptions::default());
    let frames = t.frames_handle();
    let d = Device::open(Box::new(t), Box::new(|_| {})).expect("open");
    (d, frames)
}

fn guard_case(word: u16, value: &[u8]) {
    let (d, frames) = open_recording();
    let before = frames.lock().unwrap().len();
    assert_eq!(
        d.raw_cmd_for_tests(word, value),
        Err(DeviceError::Forbidden { word })
    );
    assert_eq!(
        frames.lock().unwrap().len(),
        before,
        "no byte may be written"
    );
}

#[test]
fn guard_blocks_out_of_range_write() {
    guard_case(0x0007, &[0x01, 0, 0x78, 0, 0, 0]);
}

#[test]
fn guard_blocks_unknown_word() {
    guard_case(0x00A0, &[]);
}

#[test]
fn guard_runs_before_the_dead_check() {
    let (d, frames) = open_recording();
    d.link.dead.store(true, Ordering::SeqCst);
    let before = frames.lock().unwrap().len();
    assert_eq!(
        d.raw_cmd_for_tests(0x00A0, &[]),
        Err(DeviceError::Forbidden { word: 0x00A0 })
    );
    assert_eq!(frames.lock().unwrap().len(), before);
    // A permitted word on a dead link is still Disconnected.
    assert_eq!(
        d.raw_cmd_for_tests(0x0000, &[]),
        Err(DeviceError::Disconnected)
    );
}

fn open_recording_with_sim() -> (Device, super::test_support::Frames, SimHandle) {
    let t = super::test_support::RecordingTransport::new(SimOptions::default());
    let frames = t.frames_handle();
    let h = t.sim_handle();
    let d = Device::open(Box::new(t), Box::new(|_| {})).expect("open");
    (d, frames, h)
}

fn sent_word(frames: &super::test_support::Frames, word: u16) -> bool {
    frames
        .lock()
        .unwrap()
        .iter()
        .any(|f| f.len() >= 8 && u16::from_le_bytes([f[6], f[7]]) == word)
}

#[test]
fn guarded_write_backs_up_current_values_first() {
    let (d, frames, h) = open_recording_with_sim();
    let a = h.params();
    let b = modified(&h);
    let seen: Mutex<Vec<SensorParams>> = Mutex::default();
    d.write_params_guarded(&b, &|cur: &SensorParams| {
        seen.lock().unwrap().push(cur.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![a]);
    assert_eq!(h.params(), b);
    assert!(h.saved() && !h.in_config());
    assert!(sent_word(&frames, CMD_WRITE_PARAMS));
}

#[test]
fn backup_failure_blocks_write() {
    let (d, frames, h) = open_recording_with_sim();
    let a = h.params();
    let b = modified(&h);
    let r = d.write_params_guarded(&b, &|_: &SensorParams| Err("disk full".to_string()));
    assert_eq!(
        r,
        Err(DeviceError::BackupFailed {
            message: "disk full".into()
        })
    );
    assert!(!sent_word(&frames, 0x0007), "no write frame may be sent");
    assert!(!sent_word(&frames, CMD_SAVE));
    assert!(!h.in_config());
    assert_eq!(h.params(), a);
    assert_eq!(
        serde_json::to_value(DeviceError::BackupFailed {
            message: "x".into()
        })
        .unwrap(),
        serde_json::json!({"kind": "backupFailed", "message": "x"})
    );
}

#[test]
fn backup_failure_blocks_calibration() {
    let (d, frames, h) = open_recording_with_sim();
    let r = d.start_calibration_guarded(coeffs(), &|_: &SensorParams| Err("disk full".to_string()));
    assert_eq!(
        r,
        Err(DeviceError::BackupFailed {
            message: "disk full".into()
        })
    );
    assert!(
        !sent_word(&frames, 0x0009),
        "no calibration frame may be sent"
    );
    assert!(!h.in_config());
    // Not left busy: a normal read still works.
    assert_eq!(d.read_params(), Ok(h.params()));
}

#[test]
fn guarded_calibration_backs_up_then_runs() {
    let (d, frames, h) = open_recording_with_sim();
    let a = h.params();
    let seen: Mutex<Vec<SensorParams>> = Mutex::default();
    d.start_calibration_guarded(coeffs(), &|cur: &SensorParams| {
        seen.lock().unwrap().push(cur.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![a]);
    assert!(sent_word(&frames, 0x0009));
}

/// A change that fails inside config (here a failed backup) while
/// streaming still puts engineering mode back, so the live view resumes.
#[test]
fn failed_change_while_streaming_resumes_engineering() {
    let (d, frames, h) = open_recording_with_sim();
    d.set_streaming(true).unwrap();
    assert!(h.engineering());
    let n = frames.lock().unwrap().len();
    let r = d.write_params_guarded(&modified(&h), &|_: &SensorParams| Err("disk full".into()));
    assert!(matches!(r, Err(DeviceError::BackupFailed { .. })));
    assert!(h.engineering(), "engineering mode must be restored");
    assert!(!h.in_config());
    let tail: Vec<u16> = frames.lock().unwrap()[n..]
        .iter()
        .map(|f| u16::from_le_bytes([f[6], f[7]]))
        .collect();
    assert_eq!(&tail[tail.len() - 2..], &[CMD_OUTPUT_MODE, CMD_END_CONFIG]);
    let last_mode = frames.lock().unwrap()[n..]
        .iter()
        .rev()
        .find(|f| u16::from_le_bytes([f[6], f[7]]) == CMD_OUTPUT_MODE)
        .cloned()
        .unwrap();
    assert_eq!(
        last_mode,
        encode_command(CMD_OUTPUT_MODE, &mode_value(MODE_ENGINEERING))
    );
    // Not streaming: a failed change sends no mode frame.
    d.set_streaming(false).unwrap();
    let n = frames.lock().unwrap().len();
    let r = d.write_params_guarded(&modified(&h), &|_: &SensorParams| Err("disk full".into()));
    assert!(matches!(r, Err(DeviceError::BackupFailed { .. })));
    assert!(
        frames.lock().unwrap()[n..]
            .iter()
            .all(|f| u16::from_le_bytes([f[6], f[7]]) != CMD_OUTPUT_MODE)
    );
}

#[test]
fn calibration_with_strong_interference_reports_it_instead_of_failing() {
    let (d, h, rx) = try_open(SimOptions {
        interference_status: 1,
        ..Default::default()
    });
    let d = d.unwrap();
    d.start_calibration(coeffs()).unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    let done = loop {
        let left = end.saturating_duration_since(Instant::now());
        match rx
            .recv_timeout(left)
            .expect("a calibration outcome within 10 s")
        {
            DeviceEvent::CalibDone { gates, strong } => break (gates, strong),
            DeviceEvent::CalibFailed { error } => panic!("failed: {error:?}"),
            _ => {}
        }
    };
    assert_eq!(done, (vec![1, 10, 15], true));
    assert!(!h.in_config());
}
