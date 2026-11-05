//! JSON configuration export/import for sensor parameters.

use crate::protocol::commands::SensorParams;
use serde::Serialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(thiserror::Error, Debug, Serialize, PartialEq)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum ConfigError {
    #[error("not an LD2402 Studio config file")]
    NotAConfig,
    #[error("unsupported config version {version}")]
    UnsupportedVersion { version: u32 },
    #[error("value out of range: {field}")]
    OutOfRange { field: String },
}

const FORMAT: &str = "ld2402-studio-config";
const VERSION: u32 = 1;

pub fn to_json(p: &SensorParams, firmware: Option<&str>) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let v = serde_json::json!({
        "format": FORMAT,
        "version": VERSION,
        "firmware": firmware,
        "savedAt": rfc3339_utc(secs),
        "params": {
            "maxDistanceM": round_to(p.max_distance_m, 10.0),
            "disappearDelayS": p.disappear_delay_s,
            "triggerDb": p.trigger_db.iter().map(|x| round_to(*x, 100.0)).collect::<Vec<_>>(),
            "microDb": p.micro_db.iter().map(|x| round_to(*x, 100.0)).collect::<Vec<_>>(),
        },
    });
    serde_json::to_string_pretty(&v).expect("config serializes")
}

/// Round an f32 to a decimal f64 (scale 10 -> 0.1, 100 -> 0.01) so files read "0.7", not 0.699999988.
fn round_to(x: f32, scale: f64) -> f64 {
    (f64::from(x) * scale).round() / scale
}

/// Tolerance for boundary comparisons (accepts f32-widened values from older files).
const EPS: f64 = 1e-6;

fn oor(field: &str) -> ConfigError {
    ConfigError::OutOfRange {
        field: field.to_string(),
    }
}

fn number_in(v: Option<&Value>, lo: f64, hi: f64, field: &str) -> Result<f64, ConfigError> {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n >= lo - EPS && n <= hi + EPS => Ok(n.clamp(lo, hi)),
        _ => Err(oor(field)),
    }
}

fn db_array(v: Option<&Value>, name: &str) -> Result<[f32; 16], ConfigError> {
    let arr = v
        .and_then(Value::as_array)
        .filter(|a| a.len() == 16)
        .ok_or_else(|| oor(name))?;
    let mut out = [0.0f32; 16];
    for (i, item) in arr.iter().enumerate() {
        out[i] = number_in(Some(item), 0.0, 95.0, &format!("{name}[{i}]"))? as f32;
    }
    Ok(out)
}

pub fn from_json(s: &str) -> Result<SensorParams, ConfigError> {
    let root: Value = serde_json::from_str(s).map_err(|_| ConfigError::NotAConfig)?;
    let obj = root.as_object().ok_or(ConfigError::NotAConfig)?;
    if obj.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(ConfigError::NotAConfig);
    }
    let version = obj
        .get("version")
        .and_then(Value::as_u64)
        .ok_or(ConfigError::NotAConfig)?;
    if version > u64::from(VERSION) {
        let version = u32::try_from(version).unwrap_or(u32::MAX);
        return Err(ConfigError::UnsupportedVersion { version });
    }
    let params = obj
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| oor("params"))?;
    let max_distance_m = number_in(params.get("maxDistanceM"), 0.7, 10.0, "maxDistanceM")?;
    let delay = number_in(
        params.get("disappearDelayS"),
        0.0,
        65535.0,
        "disappearDelayS",
    )?;
    if delay.fract() != 0.0 {
        return Err(oor("disappearDelayS"));
    }
    Ok(SensorParams {
        max_distance_m: max_distance_m as f32,
        disappear_delay_s: delay as u16,
        trigger_db: db_array(params.get("triggerDb"), "triggerDb")?,
        micro_db: db_array(params.get("microDb"), "microDb")?,
    })
}

/// A value that was outside its range and was clamped on a lenient read.
#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClampedField {
    pub field: String,
    pub from: f64,
    pub to: f64,
}

fn clamped_in(
    v: Option<&Value>,
    lo: f64,
    hi: f64,
    field: &str,
    clamps: &mut Vec<ClampedField>,
) -> Result<f64, ConfigError> {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() => {
            let to = n.clamp(lo, hi);
            if (n - to).abs() > EPS {
                clamps.push(ClampedField {
                    field: field.to_string(),
                    from: n,
                    to,
                });
            }
            Ok(to)
        }
        _ => Err(oor(field)),
    }
}

fn lenient_db_array(
    v: Option<&Value>,
    name: &str,
    clamps: &mut Vec<ClampedField>,
) -> Result<[f32; 16], ConfigError> {
    let arr = v
        .and_then(Value::as_array)
        .filter(|a| a.len() == 16)
        .ok_or_else(|| oor(name))?;
    let mut out = [0.0f32; 16];
    for (i, item) in arr.iter().enumerate() {
        out[i] = clamped_in(Some(item), 0.0, 95.0, &format!("{name}[{i}]"), clamps)? as f32;
    }
    Ok(out)
}

/// Like `from_json`, but out-of-range values are clamped and reported.
/// Structure (format, version, array lengths, numbers) stays strict. Extra
/// top-level fields (snapshot `serial`/`kind`) are ignored.
pub fn from_json_lenient(s: &str) -> Result<(SensorParams, Vec<ClampedField>), ConfigError> {
    let root: Value = serde_json::from_str(s).map_err(|_| ConfigError::NotAConfig)?;
    let obj = root.as_object().ok_or(ConfigError::NotAConfig)?;
    if obj.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(ConfigError::NotAConfig);
    }
    let version = obj
        .get("version")
        .and_then(Value::as_u64)
        .ok_or(ConfigError::NotAConfig)?;
    if version > u64::from(VERSION) {
        let version = u32::try_from(version).unwrap_or(u32::MAX);
        return Err(ConfigError::UnsupportedVersion { version });
    }
    let params = obj
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| oor("params"))?;
    let mut clamps = Vec::new();
    let max_distance_m = clamped_in(
        params.get("maxDistanceM"),
        0.7,
        10.0,
        "maxDistanceM",
        &mut clamps,
    )?;
    let delay = clamped_in(
        params.get("disappearDelayS"),
        0.0,
        65535.0,
        "disappearDelayS",
        &mut clamps,
    )?;
    if delay.fract() != 0.0 {
        return Err(oor("disappearDelayS"));
    }
    let trigger_db = lenient_db_array(params.get("triggerDb"), "triggerDb", &mut clamps)?;
    let micro_db = lenient_db_array(params.get("microDb"), "microDb", &mut clamps)?;
    Ok((
        SensorParams {
            max_distance_m: max_distance_m as f32,
            disappear_delay_s: delay as u16,
            trigger_db,
            micro_db,
        },
        clamps,
    ))
}

/// Snapshot file content: the export format plus `serial` and `kind`.
pub fn snapshot_json(p: &SensorParams, firmware: &str, serial: &str, kind: &str) -> String {
    let mut v: Value =
        serde_json::from_str(&to_json(p, Some(firmware))).expect("config is valid JSON");
    let o = v.as_object_mut().expect("config is an object");
    o.insert("serial".into(), Value::String(serial.to_string()));
    o.insert("kind".into(), Value::String(kind.to_string()));
    serde_json::to_string_pretty(&v).expect("snapshot serializes")
}

/// Format Unix seconds as RFC3339 UTC (civil-from-days algorithm).
pub(crate) fn rfc3339_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn doc(params: Value) -> String {
        json!({"format":"ld2402-studio-config","version":1,"params":params}).to_string()
    }
    fn good() -> Value {
        json!({"maxDistanceM":5.0,"disappearDelayS":30,
               "triggerDb":vec![40.0;16],"microDb":vec![30.0;16]})
    }
    fn oor(field: &str) -> ConfigError {
        ConfigError::OutOfRange {
            field: field.into(),
        }
    }

    #[test]
    fn roundtrip() {
        let p = SensorParams::default_for_tests();
        assert_eq!(from_json(&to_json(&p, Some("v3.3.5"))).unwrap(), p);
        assert_eq!(from_json(&to_json(&p, None)).unwrap(), p);
    }

    #[test]
    fn to_json_has_header() {
        let v: Value =
            serde_json::from_str(&to_json(&SensorParams::default_for_tests(), Some("v3.3.5")))
                .unwrap();
        assert_eq!(v["format"], "ld2402-studio-config");
        assert_eq!(v["version"], 1);
        assert_eq!(v["firmware"], "v3.3.5");
        assert!(v["savedAt"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn rejects_other_json() {
        assert_eq!(from_json(r#"{"a":1}"#), Err(ConfigError::NotAConfig));
        assert_eq!(from_json("[1]"), Err(ConfigError::NotAConfig));
        assert_eq!(from_json("garbage"), Err(ConfigError::NotAConfig));
        let no_version = json!({"format":"ld2402-studio-config","params":good()}).to_string();
        assert_eq!(from_json(&no_version), Err(ConfigError::NotAConfig));
    }

    #[test]
    fn rejects_future_version() {
        let s = json!({"format":"ld2402-studio-config","version":2,"params":good()}).to_string();
        assert_eq!(
            from_json(&s),
            Err(ConfigError::UnsupportedVersion { version: 2 })
        );
    }

    #[test]
    fn rejects_out_of_range() {
        let mut p = good();
        p["maxDistanceM"] = json!(12);
        assert_eq!(from_json(&doc(p)), Err(oor("maxDistanceM")));

        let mut p = good();
        p["maxDistanceM"] = json!(0.5);
        assert_eq!(from_json(&doc(p)), Err(oor("maxDistanceM")));

        let mut p = good();
        p["triggerDb"][3] = json!(96);
        assert_eq!(from_json(&doc(p)), Err(oor("triggerDb[3]")));

        let mut p = good();
        p["microDb"][15] = json!(-1);
        assert_eq!(from_json(&doc(p)), Err(oor("microDb[15]")));

        let mut p = good();
        p["triggerDb"] = json!(vec![40.0; 15]);
        assert_eq!(from_json(&doc(p)), Err(oor("triggerDb")));

        let mut p = good();
        p["disappearDelayS"] = json!(70000);
        assert_eq!(from_json(&doc(p)), Err(oor("disappearDelayS")));

        let mut p = good();
        p["disappearDelayS"] = json!(-1);
        assert_eq!(from_json(&doc(p)), Err(oor("disappearDelayS")));
    }

    #[test]
    fn rejects_bad_types_and_missing() {
        let mut p = good();
        p["triggerDb"][0] = json!("x");
        assert_eq!(from_json(&doc(p)), Err(oor("triggerDb[0]")));

        let mut p = good();
        p.as_object_mut().unwrap().remove("microDb");
        assert_eq!(from_json(&doc(p)), Err(oor("microDb")));

        let s = json!({"format":"ld2402-studio-config","version":1}).to_string();
        assert_eq!(from_json(&s), Err(oor("params")));
    }

    #[test]
    fn accepts_boundaries_and_ignores_extras() {
        let s = json!({"format":"ld2402-studio-config","version":1,"firmware":7,"savedAt":null,
            "params":{"maxDistanceM":0.7,"disappearDelayS":65535,
                      "triggerDb":vec![0.0;16],"microDb":vec![95.0;16]}})
        .to_string();
        let p = from_json(&s).unwrap();
        assert_eq!(p.disappear_delay_s, 65535);
        assert_eq!(p.micro_db[0], 95.0);
    }

    #[test]
    fn error_json_shape() {
        assert_eq!(
            serde_json::to_value(oor("triggerDb[3]")).unwrap(),
            json!({"reason":"outOfRange","field":"triggerDb[3]"})
        );
        assert_eq!(
            serde_json::to_value(ConfigError::UnsupportedVersion { version: 2 }).unwrap(),
            json!({"reason":"unsupportedVersion","version":2})
        );
        assert_eq!(
            serde_json::to_value(ConfigError::NotAConfig).unwrap(),
            json!({"reason":"notAConfig"})
        );
    }

    #[test]
    fn rfc3339_format() {
        assert_eq!(rfc3339_utc(1793886483), "2026-11-05T13:48:03Z");
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(951782400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn roundtrip_boundaries_and_rounded_export() {
        for d in [0.7f32, 10.0] {
            let mut p = SensorParams::default_for_tests();
            p.max_distance_m = d;
            p.trigger_db[0] = 95.0;
            p.trigger_db[1] = 0.0;
            p.micro_db[2] = 41.37;
            let text = to_json(&p, None);
            assert_eq!(from_json(&text).unwrap(), p);
            assert!(text.contains("41.37") && !text.contains("41.369"));
        }
        let mut p = SensorParams::default_for_tests();
        p.max_distance_m = 0.7;
        assert!(to_json(&p, None).contains("\"maxDistanceM\": 0.7,"));
    }

    #[test]
    fn accepts_f32_widened_boundary() {
        let mut p = good();
        p["maxDistanceM"] = json!(0.699999988079071);
        assert!(from_json(&doc(p)).is_ok());
    }

    #[test]
    fn rejects_fractional_delay() {
        let mut p = good();
        p["disappearDelayS"] = json!(30.5);
        assert_eq!(from_json(&doc(p)), Err(oor("disappearDelayS")));
    }

    #[test]
    fn lenient_reports_clamps() {
        let mut g = good();
        g["maxDistanceM"] = json!(12.0);
        g["triggerDb"][3] = json!(98.5);
        let (p, clamps) = from_json_lenient(&doc(g)).unwrap();
        assert_eq!(p.max_distance_m, 10.0);
        assert_eq!(p.trigger_db[3], 95.0);
        assert_eq!(
            clamps,
            vec![
                ClampedField {
                    field: "maxDistanceM".into(),
                    from: 12.0,
                    to: 10.0
                },
                ClampedField {
                    field: "triggerDb[3]".into(),
                    from: 98.5,
                    to: 95.0
                },
            ]
        );
    }

    #[test]
    fn lenient_accepts_snapshot_fields() {
        let p = SensorParams {
            max_distance_m: 5.0,
            disappear_delay_s: 3,
            trigger_db: [40.0; 16],
            micro_db: [30.0; 16],
        };
        let s = snapshot_json(&p, "fw", "SN1", "original");
        let (q, clamps) = from_json_lenient(&s).unwrap();
        assert_eq!(q, p);
        assert!(clamps.is_empty());
        assert!(s.contains("\"serial\"") && s.contains("\"kind\""));
    }

    #[test]
    fn lenient_still_rejects_structure() {
        let bad = json!({"format":"nope","version":1,"params":good()}).to_string();
        assert_eq!(from_json_lenient(&bad), Err(ConfigError::NotAConfig));
        let mut g = good();
        g["triggerDb"] = json!(vec![40.0; 15]);
        assert_eq!(from_json_lenient(&doc(g)), Err(oor("triggerDb")));
        let mut g = good();
        g["maxDistanceM"] = json!("NaN");
        assert_eq!(from_json_lenient(&doc(g)), Err(oor("maxDistanceM")));
        let mut g = good();
        g["disappearDelayS"] = json!(1.5);
        assert_eq!(from_json_lenient(&doc(g)), Err(oor("disappearDelayS")));
    }

    #[test]
    fn lenient_rejects_unsupported_version() {
        let s = json!({"format":"ld2402-studio-config","version":2,"params":good()}).to_string();
        assert_eq!(
            from_json_lenient(&s),
            Err(ConfigError::UnsupportedVersion { version: 2 })
        );
    }
}
