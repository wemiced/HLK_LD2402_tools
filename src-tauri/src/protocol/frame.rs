//! LD2402 serial frame codec: command encoder and streaming decoder.

const CMD_HEADER: [u8; 4] = [0xFD, 0xFC, 0xFB, 0xFA];
const CMD_TAIL: [u8; 4] = [0x04, 0x03, 0x02, 0x01];
const ENG_HEADER: [u8; 4] = [0xF4, 0xF3, 0xF2, 0xF1];
const ENG_TAIL: [u8; 4] = [0xF8, 0xF7, 0xF6, 0xF5];
const MAX_CMD_LEN: usize = 256;
const ENG_LEN: usize = 131;
const MAX_BUFFER: usize = 4096;

/// Encode a command frame: header, length, word (LE), value, tail.
pub fn encode_command(word: u16, value: &[u8]) -> Vec<u8> {
    let len = (2 + value.len()) as u16;
    let mut out = Vec::with_capacity(12 + value.len());
    out.extend_from_slice(&CMD_HEADER);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&word.to_le_bytes());
    out.extend_from_slice(value);
    out.extend_from_slice(&CMD_TAIL);
    out
}

/// A command/ACK frame; `payload` is the bytes after the command word.
#[derive(Debug, Clone, PartialEq)]
pub struct CmdFrame {
    pub word: u16,
    pub payload: Vec<u8>,
}

/// Engineering-mode report: presence flag, distance and 32 gate energies.
#[derive(Debug, Clone, PartialEq)]
pub struct RawEngineering {
    pub presence: u8,
    pub distance_cm: u16,
    pub energy: [u32; 32],
}

/// Text-mode report line.
#[derive(Debug, Clone, PartialEq)]
pub enum TextReport {
    Off,
    Distance(u16),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    Cmd(CmdFrame),
    Engineering(RawEngineering),
    Text(TextReport),
}

/// Streaming decoder that resynchronises on garbage and split frames.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

fn find(hay: &[u8], needle: &[u8; 4]) -> Option<usize> {
    hay.windows(4).position(|w| w == needle)
}

fn parse_text(line: &[u8]) -> Option<TextReport> {
    let s = std::str::from_utf8(line).ok()?.trim();
    if s == "OFF" {
        return Some(TextReport::Off);
    }
    s.strip_prefix("distance:")?
        .trim()
        .parse()
        .ok()
        .map(TextReport::Distance)
}

enum Step {
    Frame(usize, Option<Inbound>),
    NeedMore,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<Inbound> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            let header = [
                (find(&self.buf, &CMD_HEADER), true),
                (find(&self.buf, &ENG_HEADER), false),
            ]
            .into_iter()
            .filter_map(|(p, is_cmd)| p.map(|p| (p, is_cmd)))
            .min_by_key(|(p, _)| *p);
            let newline = self.buf.iter().position(|&b| b == b'\n');

            match (header, newline) {
                (Some((h, _)), Some(n)) if n < h => self.take_text(n, &mut out),
                (None, Some(n)) => self.take_text(n, &mut out),
                (Some((h, is_cmd)), _) => {
                    self.buf.drain(..h);
                    let step = if is_cmd {
                        self.try_cmd()
                    } else {
                        self.try_eng()
                    };
                    match step {
                        Step::NeedMore => break,
                        Step::Frame(consumed, item) => {
                            self.buf.drain(..consumed);
                            out.extend(item);
                        }
                    }
                }
                (None, None) => break,
            }
        }
        if self.buf.len() > MAX_BUFFER {
            let excess = self.buf.len() - MAX_BUFFER;
            self.buf.drain(..excess);
        }
        out
    }

    fn take_text(&mut self, newline: usize, out: &mut Vec<Inbound>) {
        if let Some(t) = parse_text(&self.buf[..newline]) {
            out.push(Inbound::Text(t));
        }
        self.buf.drain(..=newline);
    }

    /// Buffer starts with a command header. A bad frame consumes one byte.
    fn try_cmd(&self) -> Step {
        let b = &self.buf;
        if b.len() < 6 {
            return Step::NeedMore;
        }
        let len = u16::from_le_bytes([b[4], b[5]]) as usize;
        if !(2..=MAX_CMD_LEN).contains(&len) {
            return Step::Frame(1, None);
        }
        let total = 6 + len + 4;
        if b.len() < total {
            return Step::NeedMore;
        }
        if b[6 + len..total] != CMD_TAIL {
            return Step::Frame(1, None);
        }
        let frame = CmdFrame {
            word: u16::from_le_bytes([b[6], b[7]]),
            payload: b[8..6 + len].to_vec(),
        };
        Step::Frame(total, Some(Inbound::Cmd(frame)))
    }

    /// Buffer starts with an engineering header. A bad frame consumes one byte.
    fn try_eng(&self) -> Step {
        let b = &self.buf;
        if b.len() < 6 {
            return Step::NeedMore;
        }
        if u16::from_le_bytes([b[4], b[5]]) as usize != ENG_LEN {
            return Step::Frame(1, None);
        }
        let total = 6 + ENG_LEN + 4;
        if b.len() < total {
            return Step::NeedMore;
        }
        if b[6 + ENG_LEN..total] != ENG_TAIL {
            return Step::Frame(1, None);
        }
        let mut energy = [0u32; 32];
        for (i, e) in energy.iter_mut().enumerate() {
            let o = 9 + i * 4;
            *e = u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        }
        let frame = RawEngineering {
            presence: b[6],
            distance_cm: u16::from_le_bytes([b[7], b[8]]),
            energy,
        };
        Step::Frame(total, Some(Inbound::Engineering(frame)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::test_util::hex;

    const MANUAL_ENG_SAMPLE: &str = "F4 F3 F2 F1 83 00 01 66 00 F6 11 00 00 6C 0A 00 00 3D 02 00 00 A3 02 00 00 20 03 00 00 50 06 00 00 57 03 00 00 48 01 00 00 F3 01 00 00 3B 01 00 00 07 01 00 00 00 01 00 00 D2 00 00 00 23 01 00 00 F3 00 00 00 F4 00 00 00 B1 27 03 00 F3 0B 01 00 70 3E 00 00 8E 12 00 00 C5 08 00 00 3F 10 00 00 25 03 00 00 7A 06 00 00 7F 08 00 00 7E 07 00 00 FB 05 00 00 64 04 00 00 F3 04 00 00 2D 04 00 00 F9 03 00 00 43 04 00 00 F8 F7 F6 F5";

    #[test]
    fn encode_read_version() {
        assert_eq!(
            encode_command(0x0000, &[]),
            hex("FD FC FB FA 02 00 00 00 04 03 02 01")
        );
    }
    #[test]
    fn encode_enable_config() {
        assert_eq!(
            encode_command(0x00FF, &[0x01, 0x00]),
            hex("FD FC FB FA 04 00 FF 00 01 00 04 03 02 01")
        );
    }
    #[test]
    fn decode_enable_config_ack() {
        let out = FrameDecoder::new().push(&hex(
            "FD FC FB FA 08 00 FF 01 00 00 02 00 20 00 04 03 02 01",
        ));
        assert_eq!(
            out,
            vec![Inbound::Cmd(CmdFrame {
                word: 0x01FF,
                payload: hex("00 00 02 00 20 00")
            })]
        );
    }
    #[test]
    fn decode_manual_engineering_sample() {
        assert_eq!(hex(MANUAL_ENG_SAMPLE).len(), 141);
        let out = FrameDecoder::new().push(&hex(MANUAL_ENG_SAMPLE));
        assert_eq!(out.len(), 1);
        let Inbound::Engineering(f) = &out[0] else {
            panic!()
        };
        assert_eq!(
            (
                f.presence,
                f.distance_cm,
                f.energy[0],
                f.energy[16],
                f.energy[31]
            ),
            (1, 102, 4598, 206769, 1091)
        );
    }
    #[test]
    fn decode_text_lines() {
        let out = FrameDecoder::new().push(b"OFF\r\ndistance:102\r\n");
        assert_eq!(
            out,
            vec![
                Inbound::Text(TextReport::Off),
                Inbound::Text(TextReport::Distance(102))
            ]
        );
    }
    #[test]
    fn unknown_text_line_dropped() {
        assert!(FrameDecoder::new().push(b"hello\r\n").is_empty());
    }
    #[test]
    fn resync_after_garbage_and_split() {
        let mut d = FrameDecoder::new();
        let ack = hex("FD FC FB FA 04 00 FE 01 00 00 04 03 02 01");
        assert!(d.push(&[0x00, 0x13, 0xFD, 0x37]).is_empty());
        assert!(d.push(&ack[..5]).is_empty());
        assert_eq!(d.push(&ack[5..]).len(), 1);
    }
    #[test]
    fn resync_bad_length_skips_header() {
        let mut bytes = hex("F4 F3 F2 F1 05 00");
        let ack = hex("FD FC FB FA 04 00 FE 01 00 00 04 03 02 01");
        bytes.extend_from_slice(&ack);
        let out = FrameDecoder::new().push(&bytes);
        assert_eq!(
            out,
            vec![Inbound::Cmd(CmdFrame {
                word: 0x01FE,
                payload: hex("00 00")
            })]
        );
    }
    #[test]
    fn bad_tail_drops_one_byte_and_rescans() {
        let mut bytes = hex("FD FC FB FA 02 00 00 00 04 03 02 99");
        bytes.extend_from_slice(&hex("FD FC FB FA 02 00 00 01 04 03 02 01"));
        let out = FrameDecoder::new().push(&bytes);
        assert_eq!(
            out,
            vec![Inbound::Cmd(CmdFrame {
                word: 0x0100,
                payload: vec![]
            })]
        );
    }
    const ACK: &str = "FD FC FB FA 04 00 FE 01 00 00 04 03 02 01";
    fn ack_item() -> Inbound {
        Inbound::Cmd(CmdFrame {
            word: 0x01FE,
            payload: hex("00 00"),
        })
    }

    #[test]
    fn text_between_frames() {
        let mut bytes = hex(ACK);
        bytes.extend_from_slice(b"OFF\r\n");
        bytes.extend_from_slice(&hex(ACK));
        let out = FrameDecoder::new().push(&bytes);
        assert_eq!(
            out,
            vec![ack_item(), Inbound::Text(TextReport::Off), ack_item()]
        );
    }
    #[test]
    fn text_before_and_after_frame_in_one_push() {
        let mut bytes = b"distance:5\r\n".to_vec();
        bytes.extend_from_slice(&hex(ACK));
        bytes.extend_from_slice(b"OFF\r\n");
        let out = FrameDecoder::new().push(&bytes);
        assert_eq!(
            out,
            vec![
                Inbound::Text(TextReport::Distance(5)),
                ack_item(),
                Inbound::Text(TextReport::Off)
            ]
        );
    }
    #[test]
    fn garbage_with_newline_before_frame() {
        let mut bytes = vec![0x41, 0x0A, 0x00, 0x0A, 0x13];
        bytes.extend_from_slice(&hex(ACK));
        assert_eq!(FrameDecoder::new().push(&bytes), vec![ack_item()]);
    }
    #[test]
    fn header_split_mid_header() {
        let ack = hex(ACK);
        let mut d = FrameDecoder::new();
        assert!(d.push(&ack[..2]).is_empty());
        assert!(d.push(&ack[2..3]).is_empty());
        assert_eq!(d.push(&ack[3..]), vec![ack_item()]);
    }
    #[test]
    fn command_length_over_256_rejected_and_resyncs() {
        let mut bytes = hex("FD FC FB FA 01 01");
        bytes.extend_from_slice(&hex(ACK));
        assert_eq!(FrameDecoder::new().push(&bytes), vec![ack_item()]);
    }
    #[test]
    fn long_unterminated_stream_is_capped_and_recovers() {
        let mut d = FrameDecoder::new();
        for _ in 0..10 {
            assert!(d.push(&[b'A'; 1000]).is_empty());
            assert!(d.buf.len() <= 4096);
        }
        assert_eq!(d.buf.len(), 4096);
        assert_eq!(d.push(&hex(ACK)), vec![ack_item()]);
    }

    #[test]
    fn fuzz_never_panics() {
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        // Pure random chunks: must never panic.
        let mut d = FrameDecoder::new();
        for _ in 0..10_000 {
            let len = (next() % 65) as usize;
            let chunk: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let _ = d.push(&chunk);
            assert!(d.buf.len() <= 4096);
        }

        // Structured stream: noise (no header-start bytes, newline-terminated), invalid
        // frames and valid frames, randomly re-chunked. Valid items must be recovered.
        let eng = hex(MANUAL_ENG_SAMPLE);
        let eng_item = FrameDecoder::new().push(&eng).remove(0);
        let mut stream = Vec::new();
        let mut expected = Vec::new();
        for _ in 0..2_000 {
            let n = next() % 12;
            for _ in 0..n {
                let b = next() as u8;
                stream.push(if b == 0xFD || b == 0xF4 { 0x41 } else { b });
            }
            stream.push(0x0A);
            match next() % 6 {
                0 => {
                    stream.extend_from_slice(&hex(ACK));
                    expected.push(ack_item());
                }
                1 => {
                    stream.extend_from_slice(&eng);
                    expected.push(eng_item.clone());
                }
                2 => {
                    stream.extend_from_slice(b"OFF\r\n");
                    expected.push(Inbound::Text(TextReport::Off));
                }
                3 => stream.extend_from_slice(&hex("F4 F3 F2 F1 05 00")),
                4 => stream.extend_from_slice(&hex("FD FC FB FA 01 01")),
                _ => stream.extend_from_slice(&hex("FD FC FB FA 02 00 00 00 04 03 02 99")),
            }
        }
        let mut d = FrameDecoder::new();
        let mut got = Vec::new();
        let mut i = 0;
        while i < stream.len() {
            let end = (i + 1 + (next() % 64) as usize).min(stream.len());
            got.extend(d.push(&stream[i..end]));
            assert!(d.buf.len() <= 4096);
            i = end;
        }
        let first_diff = got.iter().zip(&expected).position(|(g, e)| g != e);
        assert_eq!((got.len(), first_diff), (expected.len(), None));
        assert_eq!(got, expected);
    }
}
