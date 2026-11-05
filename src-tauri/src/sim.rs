//! Sensor simulator: answers the LD2402 command set and streams data, no hardware needed.
use crate::protocol::commands::*;
use crate::protocol::convert::{capabilities, db_to_raw, parse_version};
use crate::protocol::frame::*;
use crate::transport::Transport;
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const FRAME_PERIOD: Duration = Duration::from_millis(100);
const IDLE_SLEEP: Duration = Duration::from_millis(20);
const PERTURBED_MASK: u16 = (1 << 1) | (1 << 10) | (1 << 15);
const ENG_HEADER: [u8; 4] = [0xF4, 0xF3, 0xF2, 0xF1];
const ENG_TAIL: [u8; 4] = [0xF8, 0xF7, 0xF6, 0xF5];

pub struct SimOptions {
    pub version: String,
    pub unresponsive: bool,
    pub start_in_engineering: bool,
    pub start_in_config: bool,
    pub fail_after_reads: Option<usize>,
    /// Withholds the 0x00F0 auto-gain completion until `SimHandle::finish_auto_gain`.
    pub hold_auto_gain_done: bool,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            version: "v3.3.5".into(),
            unresponsive: false,
            start_in_engineering: false,
            start_in_config: false,
            fail_after_reads: None,
            hold_auto_gain_done: false,
        }
    }
}

/// Moving-person model from the UI mockup: a target near 3.4 m, absent ~30% of the time.
struct Person {
    t: f32,
    target: f32,
    energy: [f32; 32],
    rng: u64,
}

impl Person {
    fn new() -> Self {
        Self {
            t: 0.0,
            target: 3.4,
            energy: [14.0; 32],
            rng: 0x2545_F491_4F6C_DD1D,
        }
    }

    /// Deterministic xorshift64, uniform in 0..1.
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Advances 100 ms; returns (present, distance in metres).
    fn step(&mut self) -> (bool, f32) {
        self.t += 0.1;
        let present = (self.t / 9.0).sin() > -0.55;
        self.target += (self.t / 3.0).sin() * 0.04;
        let gate = self.target / 0.7;
        for i in 0..32 {
            let base = 14.0 + self.rand() * 8.0;
            let peak = if present && i < 16 {
                (62.0 - (i as f32 - gate).abs() * 14.0).max(0.0) * (0.85 + self.rand() * 0.3)
            } else {
                0.0
            };
            self.energy[i] += (base.max(peak) - self.energy[i]) * 0.35;
        }
        (present, self.target)
    }

    fn engineering_frame(&mut self) -> Vec<u8> {
        let (present, dist) = self.step();
        let mut f = ENG_HEADER.to_vec();
        f.extend(131u16.to_le_bytes());
        f.push(u8::from(present));
        let cm = if present { (dist * 100.0) as u16 } else { 0 };
        f.extend(cm.to_le_bytes());
        for e in self.energy {
            f.extend(db_to_raw(e).to_le_bytes());
        }
        f.extend(ENG_TAIL);
        f
    }

    fn text_line(&mut self) -> Vec<u8> {
        let (present, dist) = self.step();
        if present {
            format!("distance:{}\r\n", (dist * 100.0) as u16).into_bytes()
        } else {
            b"OFF\r\n".to_vec()
        }
    }
}

struct State {
    opts: SimOptions,
    pairs: Vec<(u16, u32)>,
    in_config: bool,
    engineering: bool,
    saved: bool,
    calib_progress: u16,
    reads: usize,
    decoder: FrameDecoder,
    out: VecDeque<u8>,
    person: Person,
    last_emit: Instant,
}

fn ack(word: u16, status: u16, rest: &[u8]) -> Vec<u8> {
    let mut v = status.to_le_bytes().to_vec();
    v.extend(rest);
    encode_command(word | 0x0100, &v)
}

fn len_prefixed(data: &[u8]) -> Vec<u8> {
    let mut v = (data.len() as u16).to_le_bytes().to_vec();
    v.extend(data);
    v
}

impl State {
    fn handle(&mut self, f: CmdFrame) {
        let reply = if f.word == CMD_ENABLE_CONFIG {
            self.in_config = true;
            ack(f.word, 0, &[0x02, 0x00, 0x20, 0x00])
        } else if !self.in_config {
            ack(f.word, 1, &[])
        } else {
            self.handle_config(&f)
        };
        self.out.extend(reply);
        if f.word == CMD_AUTO_GAIN && self.in_config && !self.opts.hold_auto_gain_done {
            self.out
                .extend(encode_command(EVT_AUTO_GAIN_DONE, &[0x01, 0x00]));
        }
    }

    fn serial_is_hex(&self) -> bool {
        !capabilities(parse_version(&self.opts.version)).serial_text
    }

    fn handle_config(&mut self, f: &CmdFrame) -> Vec<u8> {
        let w = f.word;
        match w {
            CMD_END_CONFIG => {
                self.in_config = false;
                self.last_emit = Instant::now();
                ack(w, 0, &[])
            }
            CMD_READ_VERSION => ack(w, 0, &len_prefixed(self.opts.version.as_bytes())),
            CMD_READ_SN_HEX => ack(
                w,
                0,
                &len_prefixed(&[0x4C, 0xD2, 0x40, 0x02, 0xA1, 0x7B, 0x00, 0x19]),
            ),
            // Before v3.3.5 the 0x0011 answer is raw bytes, not ASCII.
            CMD_READ_SN_TEXT if self.serial_is_hex() => {
                ack(w, 0, &len_prefixed(&[0x25, 0x03, 0x18]))
            }
            CMD_READ_SN_TEXT => ack(w, 0, &len_prefixed(b"SIM-LD2402-0001")),
            CMD_READ_PARAMS => {
                let mut vals = vec![];
                for c in f.payload.chunks_exact(2) {
                    let id = u16::from_le_bytes([c[0], c[1]]);
                    match self.pairs.iter().find(|(i, _)| *i == id) {
                        Some((_, v)) => vals.extend(v.to_le_bytes()),
                        None => return ack(w, 1, &[]),
                    }
                }
                ack(w, 0, &vals)
            }
            CMD_WRITE_PARAMS => {
                for c in f.payload.chunks_exact(6) {
                    let id = u16::from_le_bytes([c[0], c[1]]);
                    let v = u32::from_le_bytes([c[2], c[3], c[4], c[5]]);
                    if let Some(p) = self.pairs.iter_mut().find(|(i, _)| *i == id) {
                        p.1 = v;
                    }
                }
                ack(w, 0, &[])
            }
            CMD_OUTPUT_MODE if f.payload.len() >= 6 => {
                let mode =
                    u32::from_le_bytes([f.payload[2], f.payload[3], f.payload[4], f.payload[5]]);
                self.engineering = mode == 4;
                ack(w, 0, &[])
            }
            CMD_CALIB_START => {
                self.calib_progress = 0;
                ack(w, 0, &[])
            }
            CMD_CALIB_PROGRESS => {
                self.calib_progress = (self.calib_progress + 10).min(100);
                ack(w, 0, &self.calib_progress.to_le_bytes())
            }
            CMD_CALIB_INTERFERENCE => ack(w, 0, &PERTURBED_MASK.to_le_bytes()),
            CMD_SAVE => {
                self.saved = true;
                ack(w, 0, &[])
            }
            CMD_AUTO_GAIN => ack(w, 0, &[]),
            _ => ack(w, 1, &[]),
        }
    }

    /// Queues the data frames due since the last emission (capped to avoid bursts).
    fn generate_stream(&mut self) {
        if self.in_config {
            return;
        }
        let due = (self.last_emit.elapsed().as_millis() / FRAME_PERIOD.as_millis()).min(5) as u32;
        if due == 0 {
            return;
        }
        self.last_emit += FRAME_PERIOD * due;
        for _ in 0..due {
            let bytes = if self.engineering {
                self.person.engineering_frame()
            } else {
                self.person.text_line()
            };
            self.out.extend(bytes);
        }
    }
}

pub struct SimSensor {
    shared: Arc<Mutex<State>>,
}

/// Cloneable view of the simulator state, for tests.
#[derive(Clone)]
pub struct SimHandle {
    shared: Arc<Mutex<State>>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl SimSensor {
    pub fn new(opts: SimOptions) -> Self {
        let mut p = SensorParams {
            max_distance_m: 5.0,
            disappear_delay_s: 30,
            trigger_db: [0.0; 16],
            micro_db: [30.0; 16],
        };
        for (i, t) in p.trigger_db.iter_mut().enumerate() {
            *t = 42.0 - 1.2 * i as f32;
        }
        let state = State {
            in_config: opts.start_in_config,
            engineering: opts.start_in_engineering,
            pairs: params_to_pairs(&p),
            saved: false,
            calib_progress: 0,
            reads: 0,
            decoder: FrameDecoder::new(),
            out: VecDeque::new(),
            person: Person::new(),
            last_emit: Instant::now(),
            opts,
        };
        Self {
            shared: Arc::new(Mutex::new(state)),
        }
    }

    pub fn sim_handle(&self) -> SimHandle {
        SimHandle {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl SimHandle {
    pub fn params(&self) -> SensorParams {
        pairs_to_params(&lock(&self.shared).pairs).expect("sim holds all parameter ids")
    }
    pub fn in_config(&self) -> bool {
        lock(&self.shared).in_config
    }
    pub fn engineering(&self) -> bool {
        lock(&self.shared).engineering
    }
    pub fn saved(&self) -> bool {
        lock(&self.shared).saved
    }
    /// Emits the auto-gain completion withheld by `hold_auto_gain_done`.
    pub fn finish_auto_gain(&self) {
        lock(&self.shared)
            .out
            .extend(encode_command(EVT_AUTO_GAIN_DONE, &[0x01, 0x00]));
    }
}

impl Transport for SimSensor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut st = lock(&self.shared);
        if st.opts.fail_after_reads.is_some_and(|n| st.reads >= n) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        st.reads += 1;
        st.generate_stream();
        if st.out.is_empty() {
            drop(st);
            std::thread::sleep(IDLE_SLEEP);
            return Ok(0);
        }
        let n = buf.len().min(st.out.len());
        for b in buf.iter_mut().take(n) {
            *b = st.out.pop_front().unwrap_or_default();
        }
        Ok(n)
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        let mut st = lock(&self.shared);
        if st.opts.unresponsive {
            return Ok(());
        }
        for item in st.decoder.push(data) {
            if let Inbound::Cmd(f) = item {
                st.handle(f);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn send(s: &mut SimSensor, word: u16, value: &[u8]) {
        s.write_all(&encode_command(word, value)).unwrap();
    }

    /// Reads for up to `ms` and returns everything decoded.
    fn pump(s: &mut SimSensor, dec: &mut FrameDecoder, ms: u64) -> Vec<Inbound> {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut out = vec![];
        let mut buf = [0u8; 512];
        while Instant::now() < end {
            let n = s.read(&mut buf).unwrap();
            out.extend(dec.push(&buf[..n]));
        }
        out
    }

    fn cmds(v: Vec<Inbound>) -> Vec<CmdFrame> {
        v.into_iter()
            .filter_map(|i| {
                if let Inbound::Cmd(c) = i {
                    Some(c)
                } else {
                    None
                }
            })
            .collect()
    }

    fn ask(s: &mut SimSensor, d: &mut FrameDecoder, word: u16, value: &[u8]) -> CmdFrame {
        send(s, word, value);
        cmds(pump(s, d, 60)).remove(0)
    }

    fn enable(s: &mut SimSensor, d: &mut FrameDecoder) {
        let ack = ask(s, d, CMD_ENABLE_CONFIG, &[1, 0]);
        assert_eq!(ack.word, 0x01FF);
        assert_eq!(ack.payload, vec![0, 0, 2, 0, 0x20, 0]);
    }

    #[test]
    fn sim_rejects_commands_outside_config() {
        let mut s = SimSensor::new(SimOptions::default());
        let mut d = FrameDecoder::new();
        let ack = ask(&mut s, &mut d, CMD_READ_PARAMS, &read_params_value(&[1]));
        assert_eq!(ack.word, 0x0108);
        assert_eq!(
            parse_status(&ack).unwrap_err().to_string(),
            "command 0x0008 rejected with status 1"
        );
    }

    #[test]
    fn sim_write_then_read_params_roundtrip() {
        let mut s = SimSensor::new(SimOptions::default());
        let h = s.sim_handle();
        let mut d = FrameDecoder::new();
        enable(&mut s, &mut d);
        assert!(h.in_config());
        let mut p = h.params();
        assert_eq!(p.max_distance_m, 5.0);
        assert_eq!(p.disappear_delay_s, 30);
        p.max_distance_m = 7.5;
        p.disappear_delay_s = 12;
        p.trigger_db[3] = 55.0;
        p.micro_db[15] = 21.0;
        for chunk in params_to_pairs(&p).chunks(5) {
            let ack = ask(&mut s, &mut d, CMD_WRITE_PARAMS, &write_params_value(chunk));
            parse_status(&ack).unwrap();
        }
        let ids = all_param_ids();
        let mut pairs = vec![];
        for chunk in ids.chunks(7) {
            let ack = ask(&mut s, &mut d, CMD_READ_PARAMS, &read_params_value(chunk));
            let vals = parse_param_values(parse_status(&ack).unwrap(), chunk.len()).unwrap();
            pairs.extend(chunk.iter().copied().zip(vals));
        }
        assert_eq!(pairs_to_params(&pairs).unwrap(), p);
        assert_eq!(h.params(), p);
    }

    #[test]
    fn sim_streams_engineering_frames() {
        let mut s = SimSensor::new(SimOptions::default());
        let h = s.sim_handle();
        let mut d = FrameDecoder::new();
        enable(&mut s, &mut d);
        let mut v = vec![0, 0];
        v.extend(4u32.to_le_bytes());
        parse_status(&ask(&mut s, &mut d, CMD_OUTPUT_MODE, &v)).unwrap();
        parse_status(&ask(&mut s, &mut d, CMD_END_CONFIG, &[])).unwrap();
        assert!(h.engineering() && !h.in_config());
        let frames = pump(&mut s, &mut d, 320);
        let n = frames
            .iter()
            .filter(|f| matches!(f, Inbound::Engineering(_)))
            .count();
        assert!(n >= 2, "got {n} engineering frames");
    }

    #[test]
    fn sim_streams_text_in_normal_mode() {
        let mut s = SimSensor::new(SimOptions::default());
        let mut d = FrameDecoder::new();
        let n = pump(&mut s, &mut d, 320)
            .iter()
            .filter(|f| matches!(f, Inbound::Text(_)))
            .count();
        assert!(n >= 2, "got {n} text lines");
    }

    #[test]
    fn sim_save_sets_saved_flag() {
        let mut s = SimSensor::new(SimOptions::default());
        let h = s.sim_handle();
        let mut d = FrameDecoder::new();
        enable(&mut s, &mut d);
        assert!(!h.saved());
        parse_status(&ask(&mut s, &mut d, CMD_SAVE, &[])).unwrap();
        assert!(h.saved());
    }

    #[test]
    fn sim_identity_commands() {
        let mut s = SimSensor::new(SimOptions {
            version: "v3.0.3".into(),
            ..Default::default()
        });
        let mut d = FrameDecoder::new();
        enable(&mut s, &mut d);
        let a = ask(&mut s, &mut d, CMD_READ_VERSION, &[]);
        assert_eq!(
            parse_version_ack(parse_status(&a).unwrap()).unwrap(),
            "v3.0.3"
        );
        let a = ask(&mut s, &mut d, CMD_READ_SN_HEX, &[]);
        assert!(!parse_sn_hex(parse_status(&a).unwrap()).is_empty());
        let a = ask(&mut s, &mut d, CMD_READ_SN_TEXT, &[]);
        assert!(!parse_sn_text(parse_status(&a).unwrap()).is_empty());
    }

    #[test]
    fn sim_serial_0x0011_is_hex_before_3_3_5_and_ascii_after() {
        for (version, expect) in [
            ("v3.3.4", "250318"),
            ("garbage", "250318"),
            ("v3.3.5", "SIM-LD2402-0001"),
        ] {
            let mut s = SimSensor::new(SimOptions {
                version: version.into(),
                ..Default::default()
            });
            let mut d = FrameDecoder::new();
            enable(&mut s, &mut d);
            let a = ask(&mut s, &mut d, CMD_READ_SN_TEXT, &[]);
            let rest = parse_status(&a).unwrap();
            let got = if expect == "250318" {
                parse_sn_hex(rest)
            } else {
                parse_sn_text(rest)
            };
            assert_eq!(got, expect, "{version}");
        }
    }

    #[test]
    fn sim_calibration_progress_and_interference() {
        let mut s = SimSensor::new(SimOptions::default());
        let mut d = FrameDecoder::new();
        enable(&mut s, &mut d);
        parse_status(&ask(
            &mut s,
            &mut d,
            CMD_CALIB_START,
            &calib_value(3.0, 2.0, 3.0),
        ))
        .unwrap();
        for want in [10u8, 20, 30] {
            let a = ask(&mut s, &mut d, CMD_CALIB_PROGRESS, &[]);
            assert_eq!(parse_progress(parse_status(&a).unwrap()).unwrap(), want);
        }
        let a = ask(&mut s, &mut d, CMD_CALIB_INTERFERENCE, &[]);
        assert_eq!(
            parse_interference(parse_status(&a).unwrap()).unwrap(),
            vec![1, 10, 15]
        );
    }

    #[test]
    fn sim_auto_gain_emits_completion_event() {
        let mut s = SimSensor::new(SimOptions::default());
        let mut d = FrameDecoder::new();
        enable(&mut s, &mut d);
        send(&mut s, CMD_AUTO_GAIN, &[]);
        let f = cmds(pump(&mut s, &mut d, 60));
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].word, 0x01EE);
        assert_eq!(f[0].payload, vec![0, 0]);
        assert_eq!(
            (f[1].word, f[1].payload.clone()),
            (EVT_AUTO_GAIN_DONE, vec![1, 0])
        );
    }

    #[test]
    fn sim_unresponsive_and_failing_reads() {
        let mut s = SimSensor::new(SimOptions {
            unresponsive: true,
            ..Default::default()
        });
        let mut d = FrameDecoder::new();
        send(&mut s, CMD_ENABLE_CONFIG, &[1, 0]);
        assert!(cmds(pump(&mut s, &mut d, 100)).is_empty());

        let mut s = SimSensor::new(SimOptions {
            fail_after_reads: Some(2),
            ..Default::default()
        });
        let mut b = [0u8; 16];
        assert!(s.read(&mut b).is_ok());
        assert!(s.read(&mut b).is_ok());
        assert_eq!(
            s.read(&mut b).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn sim_start_flags() {
        let s = SimSensor::new(SimOptions {
            start_in_config: true,
            start_in_engineering: true,
            ..Default::default()
        });
        let h = s.sim_handle();
        assert!(h.in_config() && h.engineering());
    }
}
