//! Unit conversions (dB, distance) and firmware capability detection.
use serde::Serialize;

/// Raw energy value to dB, rounded to 0.01. Raw 0 maps to 0.0.
pub fn raw_to_db(raw: u32) -> f32 {
    if raw == 0 {
        return 0.0;
    }
    let db = 10.0 * f64::from(raw).log10();
    ((db * 100.0).round() / 100.0) as f32
}

/// dB to raw energy value; dB is clamped to 0..=95.
pub fn db_to_raw(db: f32) -> u32 {
    let db = f64::from(db.clamp(0.0, 95.0));
    10f64.powf(db / 10.0).round() as u32
}

/// Metres to raw units (0.1 m), clamped to 0.7..=10.0 m.
pub fn distance_to_raw(m: f32) -> u32 {
    (m.clamp(0.7, 10.0) * 10.0).round() as u32
}

pub fn raw_to_distance(raw: u32) -> f32 {
    raw as f32 / 10.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FwVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
}

/// Parses `v3.3.5` / `V3.3.5`.
pub fn parse_version(s: &str) -> Option<FwVersion> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V'])?;
    let mut it = s.split('.');
    let v = FwVersion {
        major: it.next()?.parse().ok()?,
        minor: it.next()?.parse().ok()?,
        patch: it.next()?.parse().ok()?,
    };
    it.next().is_none().then_some(v)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub permanent_save: bool,
    pub auto_gain: bool,
    pub serial_text: bool,
}

/// Feature support by firmware version; unknown version supports nothing.
pub fn capabilities(v: Option<FwVersion>) -> Capabilities {
    let at_least = |maj, min, pat| {
        v.is_some_and(|v| {
            v >= FwVersion {
                major: maj,
                minor: min,
                patch: pat,
            }
        })
    };
    Capabilities {
        permanent_save: at_least(3, 3, 2),
        auto_gain: at_least(3, 3, 5),
        serial_text: at_least(3, 3, 5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_examples_from_manual() {
        assert_eq!(raw_to_db(65536), 48.16);
        assert_eq!(db_to_raw(70.0), 10_000_000);
        assert_eq!(raw_to_db(0), 0.0);
        assert_eq!(db_to_raw(120.0), db_to_raw(95.0));
    }
    #[test]
    fn distance_examples() {
        assert_eq!(distance_to_raw(12.0), 100);
        assert_eq!(raw_to_distance(0x64), 10.0);
        assert_eq!(distance_to_raw(0.1), 7);
    }
    #[test]
    fn version_and_caps() {
        let v = parse_version("v3.3.5");
        assert!(capabilities(v).auto_gain);
        let c = capabilities(parse_version("v3.3.2"));
        assert!(c.permanent_save && !c.auto_gain && !c.serial_text);
        assert!(!capabilities(parse_version("garbage")).permanent_save);
        assert_eq!(
            parse_version("V3.0.3"),
            Some(FwVersion {
                major: 3,
                minor: 0,
                patch: 3
            })
        );
    }
}
