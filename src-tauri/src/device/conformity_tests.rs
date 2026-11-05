//! Golden-frame tests: every frame the app writes is pinned to the manual
//! (chapter 5). Each test drives the path the app ships (guarded writes and
//! calibration included) over a recording transport.
use super::test_support::{Frames, RecordingTransport};
use super::*;
use crate::protocol::convert::{raw_to_db, raw_to_distance};
use crate::protocol::safety;
use crate::protocol::test_util::hex;
use crate::sim::SimOptions;

const M: &[(&str, &str)] = &[
    ("read_version", "FD FC FB FA 02 00 00 00 04 03 02 01"),
    ("enable_config", "FD FC FB FA 04 00 FF 00 01 00 04 03 02 01"),
    ("end_config", "FD FC FB FA 02 00 FE 00 04 03 02 01"),
    ("read_sn_text", "FD FC FB FA 02 00 11 00 04 03 02 01"),
    ("read_0x3f", "FD FC FB FA 04 00 08 00 3F 00 04 03 02 01"),
    (
        "mode_eng",
        "FD FC FB FA 08 00 12 00 00 00 04 00 00 00 04 03 02 01",
    ),
    (
        "mode_normal",
        "FD FC FB FA 08 00 12 00 00 00 64 00 00 00 04 03 02 01",
    ),
    (
        "calib_3_2_3",
        "FD FC FB FA 08 00 09 00 1E 00 14 00 1E 00 04 03 02 01",
    ),
    ("calib_progress", "FD FC FB FA 02 00 0A 00 04 03 02 01"),
    ("calib_interf", "FD FC FB FA 02 00 14 00 04 03 02 01"),
    ("save", "FD FC FB FA 02 00 FD 00 04 03 02 01"),
    ("auto_gain", "FD FC FB FA 02 00 EE 00 04 03 02 01"),
    (
        "write_0x3f_50db",
        "FD FC FB FA 08 00 07 00 3F 00 A0 86 01 00 04 03 02 01",
    ),
];

fn m(name: &str) -> Vec<u8> {
    let (_, h) = M.iter().find(|(n, _)| *n == name).expect("known vector");
    hex(h)
}

fn word(f: &[u8]) -> u16 {
    u16::from_le_bytes([f[6], f[7]])
}

fn value(f: &[u8]) -> &[u8] {
    &f[8..f.len() - 4]
}

fn open_with(
    opts: SimOptions,
) -> (
    Device,
    Frames,
    crate::sim::SimHandle,
    mpsc::Receiver<DeviceEvent>,
) {
    let rt = RecordingTransport::new(opts);
    let frames = rt.frames_handle();
    let sim = rt.sim_handle();
    let (tx, rx) = mpsc::channel();
    let d = Device::open(
        Box::new(rt),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .expect("open");
    (d, frames, sim, rx)
}

fn snapshot(f: &Frames) -> Vec<Vec<u8>> {
    f.lock().unwrap().clone()
}

/// Frames recorded since `from`.
fn since(f: &Frames, from: usize) -> Vec<Vec<u8>> {
    snapshot(f)[from..].to_vec()
}

fn contains(frames: &[Vec<u8>], name: &str) -> bool {
    frames.contains(&m(name))
}

/// Asserts the named frames appear in this order (other frames may interleave).
fn assert_subsequence(frames: &[Vec<u8>], names: &[&str]) {
    let mut it = frames.iter();
    for n in names {
        let want = m(n);
        assert!(it.any(|f| *f == want), "frame {n} missing or out of order");
    }
}

/// Backup hook that accepts every pre-change snapshot (the app's store
/// is exercised in `api`); the device path is the one the app ships.
fn ok_backup(_: &SensorParams) -> Result<(), String> {
    Ok(())
}

fn calib_3_2_3() -> CalibCoefficients {
    CalibCoefficients {
        trigger: 3.0,
        hold: 2.0,
        micro: 3.0,
    }
}

fn wait_calib_done(rx: &mpsc::Receiver<DeviceEvent>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("calibration ends")
        {
            DeviceEvent::CalibDone { .. } => break,
            DeviceEvent::CalibFailed { error } => panic!("calibration failed: {error}"),
            _ => {}
        }
    }
}

/// Manual 5.2.6: the read-all chunks of a guarded change, in order. The
/// first carries ids 0x0001 and 0x0004 first; together they read all 34 ids.
fn assert_backup_reads(reads: &[Vec<u8>]) {
    assert_eq!(reads.len(), 34usize.div_ceil(read_chunk_size(0x20)));
    assert!(value(&reads[0]).starts_with(&[0x01, 0x00, 0x04, 0x00]));
    let ids: Vec<u8> = reads.iter().flat_map(|r| value(r).to_vec()).collect();
    assert_eq!(ids, read_params_value(&all_param_ids()));
    for r in reads {
        assert_eq!(word(r), 0x0008);
        assert!(value(r).len() / 2 <= read_chunk_size(0x20));
    }
}

/// Settings whose last pair (micro gate 15 = 50 dB) is manual 5.5.2's.
fn params_5_5_2() -> SensorParams {
    let mut p = SensorParams::default_for_tests();
    p.micro_db[15] = 50.0;
    p
}

/// Manual 5.5: the write chunks; 0x003F is the last pair of the last one,
/// byte for byte the 5.5.2 example value.
fn assert_writes(writes: &[Vec<u8>], p: &SensorParams) {
    assert_eq!(writes.len(), 34usize.div_ceil(5));
    for w in writes {
        assert_eq!(word(w), 0x0007);
        assert!(value(w).len().is_multiple_of(6) && value(w).len() / 6 <= 5);
    }
    let pairs: Vec<u8> = writes.iter().flat_map(|w| value(w).to_vec()).collect();
    assert_eq!(pairs, write_params_value(&params_to_pairs(p)));
    let last = value(writes.last().unwrap());
    assert_eq!(&last[last.len() - 6..last.len() - 4], &[0x3F, 0x00]);
    if p.micro_db[15] == 50.0 {
        assert_eq!(last[last.len() - 6..], *value(&m("write_0x3f_50db")));
    }
}

/// The app's write path: enable, read x5 (backup), write x7, save, end.
#[test]
fn guarded_write_sequence_matches_manual() {
    let (d, frames, _s, _rx) = open_with(SimOptions::default());
    let n = snapshot(&frames).len();
    d.write_params_guarded(&params_5_5_2(), &ok_backup).unwrap();
    let f = since(&frames, n);
    let r = 34usize.div_ceil(read_chunk_size(0x20));
    assert_eq!(f.len(), 1 + r + 7 + 2, "{f:02X?}");
    assert_eq!(f[0], m("enable_config"));
    assert_backup_reads(&f[1..1 + r]);
    assert_writes(&f[1 + r..1 + r + 7], &params_5_5_2());
    assert_eq!(f[f.len() - 2], m("save"));
    assert_eq!(f[f.len() - 1], m("end_config"));
}

/// While streaming, engineering mode is re-sent before end (manual 5.3).
#[test]
fn guarded_write_while_streaming_restores_engineering() {
    let (d, frames, _s, _rx) = open_with(SimOptions::default());
    d.set_streaming(true).unwrap();
    let n = snapshot(&frames).len();
    d.write_params_guarded(&params_5_5_2(), &ok_backup).unwrap();
    let f = since(&frames, n);
    let r = 34usize.div_ceil(read_chunk_size(0x20));
    assert_eq!(f.len(), 1 + r + 7 + 3, "{f:02X?}");
    assert_eq!(f[0], m("enable_config"));
    assert_backup_reads(&f[1..1 + r]);
    assert_writes(&f[1 + r..1 + r + 7], &params_5_5_2());
    assert_eq!(
        f[f.len() - 3..],
        [m("save"), m("mode_eng"), m("end_config")]
    );
}

/// Before v3.3.2 there is no permanent save: no 0x00FD frame at all.
#[test]
fn guarded_write_old_firmware_sends_no_save() {
    let (d, frames, _s, _rx) = open_with(SimOptions {
        version: "v3.3.1".into(),
        ..Default::default()
    });
    let n = snapshot(&frames).len();
    d.write_params_guarded(&params_5_5_2(), &ok_backup).unwrap();
    let f = since(&frames, n);
    let r = 34usize.div_ceil(read_chunk_size(0x20));
    assert_eq!(f.len(), 1 + r + 7 + 1, "{f:02X?}");
    assert!(f.iter().all(|x| word(x) != 0x00FD), "no save on v3.3.1");
    assert_eq!(f[0], m("enable_config"));
    assert_backup_reads(&f[1..1 + r]);
    assert_writes(&f[1 + r..1 + r + 7], &params_5_5_2());
    assert_eq!(f[f.len() - 1], m("end_config"));
}

/// The app's calibration path: enable, read x5 (backup), start,
/// progress..., interference, end (manual 5.2.9-5.2.11).
#[test]
fn guarded_calibration_sequence_matches_manual() {
    let (d, frames, sim, rx) = open_with(SimOptions::default());
    let n = snapshot(&frames).len();
    d.start_calibration_guarded(calib_3_2_3(), &ok_backup)
        .unwrap();
    wait_calib_done(&rx);
    let f = since(&frames, n);
    let r = 34usize.div_ceil(read_chunk_size(0x20));
    assert_eq!(f[0], m("enable_config"));
    assert_backup_reads(&f[1..1 + r]);
    assert_eq!(f[1 + r], m("calib_3_2_3"));
    let mid = &f[2 + r..f.len() - 2];
    assert!(!mid.is_empty());
    assert!(mid.iter().all(|x| *x == m("calib_progress")));
    assert_eq!(f[f.len() - 2], m("calib_interf"));
    assert_eq!(f[f.len() - 1], m("end_config"));
    assert!(!sim.in_config());
}

#[test]
fn open_emits_manual_frames() {
    let (_d, frames, _s, _rx) = open_with(SimOptions::default());
    let f = snapshot(&frames);
    assert_subsequence(
        &f,
        &[
            "enable_config",
            "mode_normal",
            "read_version",
            "read_sn_text",
            "end_config",
        ],
    );
    assert!(
        f.iter().all(|x| word(x) != 0x0016),
        "0x0016 must not be sent"
    );
}

#[test]
fn open_old_firmware_uses_0x0011_hex() {
    let (d, frames, _s, _rx) = open_with(SimOptions {
        version: "v3.3.1".into(),
        ..Default::default()
    });
    assert_eq!(d.info().serial, "250318");
    let f = snapshot(&frames);
    assert!(contains(&f, "read_sn_text"));
    assert!(f.iter().all(|x| word(x) != 0x0016));
}

#[test]
fn streaming_emits_mode_frames() {
    let (d, frames, _s, _rx) = open_with(SimOptions::default());
    let n = snapshot(&frames).len();
    d.set_streaming(true).unwrap();
    assert_eq!(
        since(&frames, n),
        vec![m("enable_config"), m("mode_eng"), m("end_config")]
    );
    let n = snapshot(&frames).len();
    d.set_streaming(false).unwrap();
    assert_eq!(
        since(&frames, n),
        vec![m("enable_config"), m("mode_normal"), m("end_config")]
    );
}

#[test]
fn save_and_auto_gain_frames() {
    let (d, frames, _s, _rx) = open_with(SimOptions::default());
    let n = snapshot(&frames).len();
    d.write_params_guarded(&SensorParams::default_for_tests(), &ok_backup)
        .unwrap();
    let f = since(&frames, n);
    assert_eq!(f[f.len() - 2], m("save"));
    assert_eq!(f[f.len() - 1], m("end_config"));

    let n = snapshot(&frames).len();
    d.auto_gain().unwrap();
    let f = since(&frames, n);
    assert_eq!(f, vec![m("enable_config"), m("auto_gain"), m("end_config")]);
}

#[test]
fn read_param_frame_matches_5_2_6() {
    assert_eq!(
        encode_command(0x0008, &[0x01, 0x00]),
        hex("FD FC FB FA 04 00 08 00 01 00 04 03 02 01")
    );
    assert_eq!(encode_command(0x0008, &[0x3F, 0x00]), m("read_0x3f"));

    let (d, frames, _s, _rx) = open_with(SimOptions::default());
    let n = snapshot(&frames).len();
    d.read_params().unwrap();
    let reads: Vec<Vec<u8>> = since(&frames, n)
        .into_iter()
        .filter(|x| word(x) == 0x0008)
        .collect();
    assert!(value(&reads[0]).starts_with(&[0x01, 0x00]));
    assert!(
        value(reads.last().unwrap())
            .chunks_exact(2)
            .any(|c| c == [0x3F, 0x00])
    );
}

#[test]
fn write_param_encoder_matches_5_5_2() {
    assert_eq!(
        encode_command(0x0007, &write_params_value(&[(0x003F, 0x0001_86A0)])),
        m("write_0x3f_50db")
    );
    assert_eq!(crate::protocol::convert::db_to_raw(50.0), 0x0001_86A0);
}

#[test]
fn write_param_encoder_matches_5_2_7() {
    assert_eq!(
        encode_command(0x0007, &write_params_value(&[(0x0001, 0x78)])),
        hex("FD FC FB FA 08 00 07 00 01 00 78 00 00 00 04 03 02 01")
    );
    assert!(safety::check(0x0007, &write_params_value(&[(0x0001, 0x78)])).is_err()); // 12 m refused
}

#[test]
fn manual_ack_examples_decode() {
    let mut dec = FrameDecoder::new();
    let mut one = |h: &str| -> CmdFrame {
        match dec.push(&hex(h)).pop() {
            Some(Inbound::Cmd(f)) => f,
            _ => panic!("not a command frame"),
        }
    };
    let f = one("FD FC FB FA 08 00 FF 01 00 00 02 00 20 00 04 03 02 01");
    assert_eq!(
        parse_enable_config(parse_status(&f).unwrap()).unwrap(),
        0x20
    );

    assert_eq!(
        parse_param_values(&hex("64 00 00 00"), 1).unwrap(),
        vec![100]
    );
    assert_eq!(raw_to_distance(100), 10.0);
    assert_eq!(
        raw_to_db(parse_param_values(&hex("A0 86 01 00"), 1).unwrap()[0]),
        50.0
    );

    let f = one("FD FC FB FA 06 00 0A 01 00 00 3C 00 04 03 02 01");
    assert_eq!(parse_progress(parse_status(&f).unwrap()).unwrap(), 60);

    let f = one("FD FC FB FA 04 00 F0 00 01 00 04 03 02 01");
    assert_eq!(
        f,
        CmdFrame {
            word: 0x00F0,
            payload: vec![1, 0]
        }
    );
}

#[test]
fn every_recorded_frame_passes_the_guard() {
    let (d, frames, _s, rx) = open_with(SimOptions::default());
    d.read_params().unwrap();
    d.write_params_guarded(&SensorParams::default_for_tests(), &ok_backup)
        .unwrap();
    d.set_streaming(true).unwrap();
    d.write_params_guarded(&SensorParams::default_for_tests(), &ok_backup)
        .unwrap();
    d.set_streaming(false).unwrap();
    d.start_calibration_guarded(calib_3_2_3(), &ok_backup)
        .unwrap();
    wait_calib_done(&rx);
    d.auto_gain().unwrap();
    drop(d);
    let f = snapshot(&frames);
    assert!(f.len() > 20);
    for x in &f {
        assert_eq!(safety::check(word(x), value(x)), Ok(()), "{x:02X?}");
        assert_eq!(
            *x,
            encode_command(word(x), value(x)),
            "re-encodes identically"
        );
    }
}
