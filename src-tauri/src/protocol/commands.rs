//! Typed LD2402 commands: ids, parameter packing and ACK parsers.
use super::convert::{db_to_raw, distance_to_raw, raw_to_db, raw_to_distance};
use super::frame::CmdFrame;
use serde::{Deserialize, Serialize};

pub const CMD_READ_VERSION: u16 = 0x0000;
pub const CMD_ENABLE_CONFIG: u16 = 0x00FF;
pub const CMD_END_CONFIG: u16 = 0x00FE;
pub const CMD_READ_SN_HEX: u16 = 0x0016;
pub const CMD_READ_SN_TEXT: u16 = 0x0011;
pub const CMD_READ_PARAMS: u16 = 0x0008;
pub const CMD_WRITE_PARAMS: u16 = 0x0007;
pub const CMD_OUTPUT_MODE: u16 = 0x0012;
pub const CMD_CALIB_START: u16 = 0x0009;
pub const CMD_CALIB_PROGRESS: u16 = 0x000A;
pub const CMD_CALIB_INTERFERENCE: u16 = 0x0014;
pub const CMD_SAVE: u16 = 0x00FD;
pub const CMD_AUTO_GAIN: u16 = 0x00EE;
pub const EVT_AUTO_GAIN_DONE: u16 = 0x00F0;

pub const P_MAX_DISTANCE: u16 = 0x0001;
pub const P_DELAY: u16 = 0x0004;
pub const P_POWER_INTERFERENCE: u16 = 0x0005;
pub const P_TRIGGER_BASE: u16 = 0x0010;
pub const P_MICRO_BASE: u16 = 0x0030;
pub const P_SAVE_TRIGGER: u16 = 0x003F;

const ACK_BIT: u16 = 0x0100;

#[derive(thiserror::Error, Debug)]
pub enum ProtocolError {
    #[error("command {word:#06x} rejected with status {status}")]
    Nack { word: u16, status: u16 },
    #[error("malformed frame: {0}")]
    Malformed(&'static str),
    #[error("unexpected command word {0:#06x}")]
    UnexpectedWord(u16),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SensorParams {
    pub max_distance_m: f32,
    pub disappear_delay_s: u16,
    pub trigger_db: [f32; 16],
    pub micro_db: [f32; 16],
}

#[cfg(test)]
impl SensorParams {
    pub fn default_for_tests() -> Self {
        Self {
            max_distance_m: 5.0,
            disappear_delay_s: 30,
            trigger_db: [40.0; 16],
            micro_db: [30.0; 16],
        }
    }
}

/// The 34 parameter ids, in wire order (the last, 0x003F, doubles as the save trigger).
pub fn all_param_ids() -> Vec<u16> {
    let mut ids = vec![P_MAX_DISTANCE, P_DELAY];
    ids.extend((0..16).map(|i| P_TRIGGER_BASE + i));
    ids.extend((0..16).map(|i| P_MICRO_BASE + i));
    ids
}

pub fn params_to_pairs(p: &SensorParams) -> Vec<(u16, u32)> {
    let mut v = vec![
        (P_MAX_DISTANCE, distance_to_raw(p.max_distance_m)),
        (P_DELAY, u32::from(p.disappear_delay_s)),
    ];
    v.extend((0..16).map(|i| (P_TRIGGER_BASE + i as u16, db_to_raw(p.trigger_db[i]))));
    v.extend((0..16).map(|i| (P_MICRO_BASE + i as u16, db_to_raw(p.micro_db[i]))));
    v
}

pub fn pairs_to_params(pairs: &[(u16, u32)]) -> Result<SensorParams, ProtocolError> {
    let get = |id: u16| {
        pairs
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, v)| *v)
            .ok_or(ProtocolError::Malformed("missing parameter id"))
    };
    let mut p = SensorParams {
        max_distance_m: raw_to_distance(get(P_MAX_DISTANCE)?),
        disappear_delay_s: get(P_DELAY)?.min(u32::from(u16::MAX)) as u16,
        trigger_db: [0.0; 16],
        micro_db: [0.0; 16],
    };
    for i in 0..16u16 {
        p.trigger_db[i as usize] = raw_to_db(get(P_TRIGGER_BASE + i)?);
        p.micro_db[i as usize] = raw_to_db(get(P_MICRO_BASE + i)?);
    }
    Ok(p)
}

pub fn read_params_value(ids: &[u16]) -> Vec<u8> {
    ids.iter().flat_map(|id| id.to_le_bytes()).collect()
}

pub fn write_params_value(pairs: &[(u16, u32)]) -> Vec<u8> {
    pairs
        .iter()
        .flat_map(|(id, v)| id.to_le_bytes().into_iter().chain(v.to_le_bytes()))
        .collect()
}

/// Ids per read frame: what the ACK's values fit in `buffer`, capped to the
/// guard's per-frame limit.
pub fn read_chunk_size(buffer: u16) -> usize {
    (usize::from(buffer).saturating_sub(2) / 4).clamp(1, super::safety::MAX_READ_IDS)
}

/// Pairs per write frame, capped to the guard's per-frame limit.
pub fn write_chunk_size(buffer: u16) -> usize {
    (usize::from(buffer).saturating_sub(2) / 6).clamp(1, super::safety::MAX_WRITE_PAIRS)
}

/// Checks the ACK bit and status 0; returns the bytes after the status.
pub fn parse_status(f: &CmdFrame) -> Result<&[u8], ProtocolError> {
    if f.word & ACK_BIT == 0 {
        return Err(ProtocolError::UnexpectedWord(f.word));
    }
    if f.payload.len() < 2 {
        return Err(ProtocolError::Malformed("ack too short"));
    }
    let status = u16::from_le_bytes([f.payload[0], f.payload[1]]);
    if status != 0 {
        return Err(ProtocolError::Nack {
            word: f.word & !ACK_BIT,
            status,
        });
    }
    Ok(&f.payload[2..])
}

/// Rest layout: protocol version (2) + buffer size (2).
pub fn parse_enable_config(rest: &[u8]) -> Result<u16, ProtocolError> {
    match rest {
        [_, _, a, b, ..] => Ok(u16::from_le_bytes([*a, *b])),
        _ => Err(ProtocolError::Malformed("enable-config ack too short")),
    }
}

/// Splits `len(2) + bytes`, returning what is available.
fn len_prefixed(rest: &[u8]) -> &[u8] {
    match rest {
        [a, b, data @ ..] => &data[..data.len().min(usize::from(u16::from_le_bytes([*a, *b])))],
        _ => &[],
    }
}

pub fn parse_version_ack(rest: &[u8]) -> Result<String, ProtocolError> {
    if rest.len() < 2 || rest.len() - 2 < usize::from(u16::from_le_bytes([rest[0], rest[1]])) {
        return Err(ProtocolError::Malformed("version ack too short"));
    }
    Ok(String::from_utf8_lossy(len_prefixed(rest)).into_owned())
}

pub fn parse_sn_hex(rest: &[u8]) -> String {
    len_prefixed(rest)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

pub fn parse_sn_text(rest: &[u8]) -> String {
    String::from_utf8_lossy(len_prefixed(rest)).into_owned()
}

pub fn parse_param_values(rest: &[u8], n: usize) -> Result<Vec<u32>, ProtocolError> {
    if rest.len() < n * 4 {
        return Err(ProtocolError::Malformed("param values too short"));
    }
    Ok(rest
        .chunks_exact(4)
        .take(n)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

pub fn parse_progress(rest: &[u8]) -> Result<u8, ProtocolError> {
    match rest {
        [a, b, ..] => Ok(u16::from_le_bytes([*a, *b]).min(100) as u8),
        _ => Err(ProtocolError::Malformed("progress ack too short")),
    }
}

/// Bit i of the LE u16 mask marks gate i as disturbed.
pub fn parse_interference(rest: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let [a, b, ..] = rest else {
        return Err(ProtocolError::Malformed("interference ack too short"));
    };
    let mask = u16::from_le_bytes([*a, *b]);
    Ok((0..16u8).filter(|i| mask & (1 << i) != 0).collect())
}

/// Calibration coefficients, x10 each as u16 LE, clamped to 10..=200.
pub fn calib_value(trigger: f32, hold: f32, micro: f32) -> Vec<u8> {
    [trigger, hold, micro]
        .into_iter()
        .flat_map(|c| ((c * 10.0).round().clamp(10.0, 200.0) as u16).to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::frame::CmdFrame;
    use crate::protocol::test_util::hex;

    #[test]
    fn write_value_matches_manual() {
        assert_eq!(
            write_params_value(&[(0x0001, 0x78)]),
            hex("01 00 78 00 00 00")
        );
    }
    #[test]
    fn pairs_always_include_save_trigger_last() {
        let pairs = params_to_pairs(&SensorParams::default_for_tests());
        assert_eq!(pairs.len(), 34);
        assert_eq!(pairs.last().unwrap().0, 0x003F);
    }
    #[test]
    fn pairs_roundtrip() {
        let p = SensorParams::default_for_tests();
        assert_eq!(pairs_to_params(&params_to_pairs(&p)).unwrap(), p);
        assert!(matches!(
            pairs_to_params(&[(1, 50)]),
            Err(ProtocolError::Malformed(_))
        ));
    }
    #[test]
    fn read_value_ids() {
        assert_eq!(read_params_value(&[1, 0x3F]), hex("01 00 3F 00"));
        assert_eq!(all_param_ids().len(), 34);
    }
    #[test]
    fn calib_value_matches_manual() {
        assert_eq!(calib_value(3.0, 2.0, 3.0), hex("1E 00 14 00 1E 00"));
    }
    #[test]
    fn progress_ack() {
        let f = CmdFrame {
            word: 0x010A,
            payload: hex("00 00 3C 00"),
        };
        assert_eq!(parse_progress(parse_status(&f).unwrap()).unwrap(), 60);
    }
    #[test]
    fn version_ack() {
        let f = CmdFrame {
            word: 0x0100,
            payload: hex("00 00 06 00 76 33 2E 30 2E 33"),
        };
        assert_eq!(
            parse_version_ack(parse_status(&f).unwrap()).unwrap(),
            "v3.0.3"
        );
    }
    #[test]
    fn enable_config_ack() {
        let f = CmdFrame {
            word: 0x01FF,
            payload: hex("00 00 02 00 20 00"),
        };
        assert_eq!(
            parse_enable_config(parse_status(&f).unwrap()).unwrap(),
            0x20
        );
    }
    #[test]
    fn param_values() {
        assert_eq!(
            parse_param_values(&hex("78 00 00 00 01 00 00 00"), 2).unwrap(),
            vec![0x78, 1]
        );
        assert!(parse_param_values(&hex("78 00"), 1).is_err());
    }
    #[test]
    fn sn_hex_ack() {
        assert_eq!(parse_sn_hex(&hex("03 00 25 03 18")), "250318");
        assert_eq!(parse_sn_hex(&hex("04 00 FF FF FF FF")), "FFFFFFFF");
        assert_eq!(parse_sn_hex(&hex("04 00 AB")), "AB");
        assert_eq!(parse_sn_hex(&[]), "");
    }
    #[test]
    fn sn_text_ack() {
        assert_eq!(parse_sn_text(&hex("04 00 31 41 32 42")), "1A2B");
        assert_eq!(parse_sn_text(&[]), "");
    }
    #[test]
    fn interference_mask() {
        assert_eq!(parse_interference(&hex("02 84")).unwrap(), vec![1, 10, 15]);
    }
    #[test]
    fn nack_is_error() {
        let f = CmdFrame {
            word: 0x0107,
            payload: hex("01 00"),
        };
        assert!(matches!(
            parse_status(&f),
            Err(ProtocolError::Nack {
                word: 0x0007,
                status: 1
            })
        ));
        let g = CmdFrame {
            word: 0x0007,
            payload: hex("00 00"),
        };
        assert!(matches!(
            parse_status(&g),
            Err(ProtocolError::UnexpectedWord(0x0007))
        ));
    }
    #[test]
    fn chunk_sizes() {
        assert_eq!((read_chunk_size(0x20), write_chunk_size(0x20)), (7, 5));
    }
}
