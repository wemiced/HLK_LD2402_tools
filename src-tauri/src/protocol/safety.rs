//! Closed-list guard: only the manual's documented commands and value ranges
//! may ever be encoded and written to the serial port.
use super::commands::*;
use super::convert::db_to_raw;
use std::ops::RangeInclusive;

#[derive(Debug, PartialEq)]
pub struct Forbidden {
    pub word: u16,
    pub reason: &'static str,
}

/// The sensor's command buffer is 0x20 bytes (manual 5.2.2 ACK): a frame
/// value holds at most 5 (id u16, value u32) write pairs or 15 read ids.
pub const MAX_WRITE_PAIRS: usize = 5;
pub const MAX_READ_IDS: usize = 15;

pub const ALLOWED_WORDS: [u16; 13] = [
    CMD_READ_VERSION,
    CMD_ENABLE_CONFIG,
    CMD_END_CONFIG,
    CMD_READ_SN_HEX,
    CMD_READ_SN_TEXT,
    CMD_READ_PARAMS,
    CMD_WRITE_PARAMS,
    CMD_OUTPUT_MODE,
    CMD_CALIB_START,
    CMD_CALIB_PROGRESS,
    CMD_CALIB_INTERFERENCE,
    CMD_SAVE,
    CMD_AUTO_GAIN,
];

fn is_gate_param(id: u16) -> bool {
    (P_TRIGGER_BASE..P_TRIGGER_BASE + 16).contains(&id)
        || (P_MICRO_BASE..P_MICRO_BASE + 16).contains(&id)
}

pub fn readable_param(id: u16) -> bool {
    matches!(id, P_MAX_DISTANCE | P_DELAY | P_POWER_INTERFERENCE) || is_gate_param(id)
}

pub fn writable_param(id: u16) -> bool {
    matches!(id, P_MAX_DISTANCE | P_DELAY) || is_gate_param(id)
}

/// Accepted raw values for a writable parameter; `None` if it is not writable.
pub fn write_range(id: u16) -> Option<RangeInclusive<u32>> {
    match id {
        P_MAX_DISTANCE => Some(7..=100),
        P_DELAY => Some(0..=u32::from(u16::MAX)),
        _ if is_gate_param(id) => Some(0..=db_to_raw(95.0)),
        _ => None,
    }
}

fn forbid<T>(word: u16, reason: &'static str) -> Result<T, Forbidden> {
    Err(Forbidden { word, reason })
}

pub fn check(word: u16, value: &[u8]) -> Result<(), Forbidden> {
    if !ALLOWED_WORDS.contains(&word) {
        return forbid(word, "command word not in the manual");
    }
    match word {
        CMD_ENABLE_CONFIG => {
            if value != [0x01, 0x00] {
                return forbid(word, "enable-config value must be 01 00");
            }
        }
        CMD_OUTPUT_MODE => match value {
            [0, 0, m, 0, 0, 0] if *m == 0x04 || *m == 0x64 => {}
            _ => return forbid(word, "output mode must be 0x04 or 0x64"),
        },
        CMD_READ_PARAMS => {
            if value.is_empty() || !value.len().is_multiple_of(2) {
                return forbid(word, "read-params needs a list of u16 ids");
            }
            if value.len() / 2 > MAX_READ_IDS {
                return forbid(word, "too many parameter ids in one read");
            }
            if !value
                .as_chunks::<2>()
                .0
                .iter()
                .all(|c| readable_param(u16::from_le_bytes(*c)))
            {
                return forbid(word, "unknown parameter id");
            }
        }
        CMD_WRITE_PARAMS => {
            if value.is_empty() || !value.len().is_multiple_of(6) {
                return forbid(word, "write-params needs (id u16, value u32) pairs");
            }
            if value.len() / 6 > MAX_WRITE_PAIRS {
                return forbid(word, "too many parameters in one write");
            }
            for c in value.as_chunks::<6>().0 {
                let id = u16::from_le_bytes([c[0], c[1]]);
                let v = u32::from_le_bytes([c[2], c[3], c[4], c[5]]);
                match write_range(id) {
                    None => return forbid(word, "parameter is not writable"),
                    Some(r) if !r.contains(&v) => {
                        return forbid(word, "parameter value out of range");
                    }
                    Some(_) => {}
                }
            }
        }
        CMD_CALIB_START => {
            if value.len() != 6
                || !value
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|c| (10..=200).contains(&u16::from_le_bytes(*c)))
            {
                return forbid(word, "calibration coefficients must be 10..=200");
            }
        }
        _ => {
            if !value.is_empty() {
                return forbid(word, "this command takes no value");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::commands::{
        SensorParams, all_param_ids, params_to_pairs, read_chunk_size, read_params_value,
        write_chunk_size, write_params_value,
    };

    #[test]
    fn allows_every_frame_the_app_emits() {
        for (w, v) in [
            (0x0000, &[][..]),
            (0x00FF, &[1, 0]),
            (0x00FE, &[]),
            (0x0011, &[]),
            (0x0016, &[]),
            (0x0008, &[0x01, 0x00]),
            (0x0007, &[0x01, 0, 100, 0, 0, 0]),
            (0x0012, &[0, 0, 4, 0, 0, 0]),
            (0x0012, &[0, 0, 0x64, 0, 0, 0]),
            (0x0009, &[0x1E, 0, 0x14, 0, 0x1E, 0]),
            (0x000A, &[]),
            (0x0014, &[]),
            (0x00FD, &[]),
            (0x00EE, &[]),
        ] {
            assert_eq!(check(w, v), Ok(()), "{w:#06x}");
        }
    }
    #[test]
    fn rejects_unknown_words() {
        for w in [0x0001, 0x0010, 0x00A0, 0x00F0, 0x0100, 0xFFFF] {
            assert_eq!(check(w, &[]).unwrap_err().word, w);
        }
    }
    #[test]
    fn exact_boundaries() {
        use crate::protocol::convert::db_to_raw;
        let w1 = |id: u16, v: u32| {
            let mut b = id.to_le_bytes().to_vec();
            b.extend(v.to_le_bytes());
            b
        };
        // max distance: 7..=100
        assert!(check(0x0007, &w1(0x0001, 101)).is_err());
        assert!(check(0x0007, &w1(0x0001, 100)).is_ok());
        assert!(check(0x0007, &w1(0x0001, 7)).is_ok());
        assert!(check(0x0007, &w1(0x0001, 6)).is_err());
        // gate threshold: 0..=db_to_raw(95.0)
        assert!(check(0x0007, &w1(0x0010, db_to_raw(95.0))).is_ok());
        assert!(check(0x0007, &w1(0x0010, db_to_raw(95.0) + 1)).is_err());
        assert!(check(0x0007, &w1(0x0030, db_to_raw(95.0) + 1)).is_err());
        // read-params: odd length
        assert!(check(0x0008, &[0x01, 0x00, 0x04]).is_err());
        // calibration: exactly 6 bytes, each 10..=200
        for bad in [5usize, 7] {
            assert!(check(0x0009, &vec![10; bad]).is_err(), "{bad} bytes");
        }
        assert!(check(0x0009, &[10, 0, 10, 0, 10, 0]).is_ok());
        assert!(check(0x0009, &[200, 0, 200, 0, 200, 0]).is_ok());
        assert!(check(0x0009, &[9, 0, 10, 0, 10, 0]).is_err());
        assert!(check(0x0009, &[201, 0, 200, 0, 200, 0]).is_err());
    }
    #[test]
    fn rejects_bad_values() {
        assert!(check(0x00FF, &[0, 0]).is_err());
        assert!(check(0x0012, &[0, 0, 5, 0, 0, 0]).is_err());
        assert!(check(0x0000, &[1]).is_err());
        assert!(check(0x0009, &[9, 0, 20, 0, 30, 0]).is_err());
        assert!(check(0x0009, &[201, 0, 20, 0, 30, 0]).is_err());
        assert!(check(0x0008, &[]).is_err());
        assert!(check(0x0008, &[0x02, 0x00]).is_err());
        assert!(check(0x0007, &[0x05, 0, 1, 0, 0, 0]).is_err());
        assert!(check(0x0007, &[0x01, 0, 0x78, 0, 0, 0]).is_err());
        assert!(check(0x0007, &[0x01, 0, 6, 0, 0, 0]).is_err());
        assert!(check(0x0007, &[0x10, 0, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
        assert!(check(0x0007, &[0x01, 0, 7, 0, 0]).is_err());
    }
    /// Payload caps for the sensor's 0x20 buffer: at most 5 write pairs
    /// and 15 read ids per frame.
    #[test]
    fn payload_length_caps() {
        let pairs =
            |n: u16| write_params_value(&(0..n).map(|i| (0x0010 + i, 0)).collect::<Vec<_>>());
        let ids = |n: u16| read_params_value(&(0..n).map(|i| 0x0010 + i).collect::<Vec<_>>());
        assert_eq!(check(0x0007, &pairs(5)), Ok(()));
        assert!(check(0x0007, &pairs(6)).is_err());
        assert_eq!(check(0x0008, &ids(15)), Ok(()));
        assert!(check(0x0008, &ids(16)).is_err());
        // The app's chunking stays within the caps whatever buffer the
        // enable-config ACK announces.
        for buffer in [0u16, 0x08, 0x20, 0x40, 0x100, u16::MAX] {
            assert!((1..=5).contains(&write_chunk_size(buffer)), "{buffer:#x}");
            assert!((1..=15).contains(&read_chunk_size(buffer)), "{buffer:#x}");
            for c in all_param_ids().chunks(read_chunk_size(buffer)) {
                assert_eq!(check(0x0008, &read_params_value(c)), Ok(()));
            }
            let p = SensorParams::default_for_tests();
            for c in params_to_pairs(&p).chunks(write_chunk_size(buffer)) {
                assert_eq!(check(0x0007, &write_params_value(c)), Ok(()));
            }
        }
    }

    #[test]
    fn every_clamped_params_set_passes() {
        let mut s: u64 = 0x2402;
        let mut next = |lo: f32, hi: f32| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            lo + ((s >> 33) as f32 / (1u64 << 31) as f32) * (hi - lo)
        };
        for _ in 0..5_000 {
            let mut p = SensorParams::default_for_tests();
            p.max_distance_m = next(-5.0, 20.0);
            p.disappear_delay_s = next(0.0, 65535.0) as u16;
            for i in 0..16 {
                p.trigger_db[i] = next(-50.0, 150.0);
                p.micro_db[i] = next(-50.0, 150.0);
            }
            for chunk in params_to_pairs(&p).chunks(write_chunk_size(0x20)) {
                assert_eq!(check(0x0007, &write_params_value(chunk)), Ok(()));
            }
        }
    }
}
