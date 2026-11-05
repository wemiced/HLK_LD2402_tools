//! Snapshot store: the sensor settings at first connection (never overwritten)
//! plus the last 10 pre-change backups, per serial number.

use crate::config_file::{ClampedField, from_json_lenient, rfc3339_utc, snapshot_json};
use crate::protocol::commands::SensorParams;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BACKUPS: usize = 10;
const ORIGINAL: &str = "original.json";

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotKind {
    Original,
    Backup,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    pub kind: SnapshotKind,
    pub saved_at: String,
    /// File name only (not a full path).
    pub file: String,
}

pub struct SnapshotStore {
    root: PathBuf,
}

/// Folder name for a serial number: blank/all-F -> "unknown", unsafe chars -> `_`.
pub fn serial_key(serial: &str) -> String {
    let t = serial.trim();
    if t.is_empty() || t.chars().all(|c| c == 'F' || c == 'f') {
        return "unknown".into();
    }
    t.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn secs_of(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse `backup-<YYYYMMDDTHHMMSSZ>-<n>.json` into (unix seconds, n).
fn parse_backup_name(name: &str) -> Option<(u64, u32)> {
    let rest = name.strip_prefix("backup-")?.strip_suffix(".json")?;
    let (ts, n) = rest.split_once('-')?;
    let n: u32 = n.parse().ok()?;
    let b = ts.strip_suffix('Z')?;
    if b.len() != 15
        || b.as_bytes()[8] != b'T'
        || !b.bytes().all(|c| c.is_ascii_digit() || c == b'T')
    {
        return None;
    }
    let f = |a: usize, z: usize| b[a..z].parse::<i64>().ok();
    let days = days_from_civil(f(0, 4)?, f(4, 6)?, f(6, 8)?);
    let secs = days * 86_400 + f(9, 11)? * 3600 + f(11, 13)? * 60 + f(13, 15)?;
    Some((u64::try_from(secs).ok()?, n))
}

fn compact_ts(secs: u64) -> String {
    rfc3339_utc(secs).replace(['-', ':'], "")
}

fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, content).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

fn saved_at_of(path: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    v.get("savedAt")?.as_str().map(str::to_string)
}

impl SnapshotStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn dir(&self, serial: &str) -> PathBuf {
        self.root.join(serial_key(serial))
    }

    fn ensure_dir(&self, serial: &str) -> Result<PathBuf, String> {
        let dir = self.dir(serial);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        Ok(dir)
    }

    /// Backups in the serial's folder, oldest first: (secs, n, file name).
    fn backups(&self, serial: &str) -> Vec<(u64, u32, String)> {
        let Ok(rd) = std::fs::read_dir(self.dir(serial)) else {
            return Vec::new();
        };
        let mut v: Vec<_> = rd
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                let (s, n) = parse_backup_name(&name)?;
                Some((s, n, name))
            })
            .collect();
        v.sort();
        v
    }

    fn meta_of(
        &self,
        serial: &str,
        kind: SnapshotKind,
        file: String,
        fallback: u64,
    ) -> SnapshotMeta {
        let saved_at =
            saved_at_of(&self.dir(serial).join(&file)).unwrap_or_else(|| rfc3339_utc(fallback));
        SnapshotMeta {
            kind,
            saved_at,
            file,
        }
    }

    pub fn save_original_if_absent(
        &self,
        serial: &str,
        fw: &str,
        p: &SensorParams,
    ) -> Result<bool, String> {
        let dir = self.ensure_dir(serial)?;
        let path = dir.join(ORIGINAL);
        if path.exists() {
            return Ok(false);
        }
        write_atomic(&path, &snapshot_json(p, fw, serial, "original"))?;
        Ok(true)
    }

    pub fn save_backup(
        &self,
        serial: &str,
        fw: &str,
        p: &SensorParams,
    ) -> Result<SnapshotMeta, String> {
        let dir = self.ensure_dir(serial)?;
        let now = secs_of(SystemTime::now());
        let existing = self.backups(serial);
        let n = existing
            .iter()
            .filter(|(s, _, _)| *s == now)
            .map(|(_, n, _)| n + 1)
            .max()
            .unwrap_or(0);
        let file = format!("backup-{}-{n}.json", compact_ts(now));
        write_atomic(&dir.join(&file), &snapshot_json(p, fw, serial, "backup"))?;
        // Prune: keep the newest MAX_BACKUPS (including the one just written).
        let all = self.backups(serial);
        for (_, _, name) in all.iter().take(all.len().saturating_sub(MAX_BACKUPS)) {
            let _ = std::fs::remove_file(dir.join(name));
        }
        Ok(self.meta_of(serial, SnapshotKind::Backup, file, now))
    }

    pub fn original(&self, serial: &str) -> Option<SnapshotMeta> {
        let path = self.dir(serial).join(ORIGINAL);
        path.is_file()
            .then(|| self.meta_of(serial, SnapshotKind::Original, ORIGINAL.to_string(), 0))
    }

    pub fn latest_backup(
        &self,
        serial: &str,
        not_before: Option<SystemTime>,
    ) -> Option<SnapshotMeta> {
        let min = not_before.map(secs_of);
        let (s, _, name) = self
            .backups(serial)
            .into_iter()
            .rev()
            .find(|(s, _, _)| min.is_none_or(|m| *s >= m))?;
        Some(self.meta_of(serial, SnapshotKind::Backup, name, s))
    }

    pub fn load(
        &self,
        serial: &str,
        meta: &SnapshotMeta,
    ) -> Result<(SensorParams, Vec<ClampedField>), String> {
        if meta.file.contains(['/', '\\']) || meta.file.contains("..") {
            return Err(format!("invalid snapshot file name: {}", meta.file));
        }
        let path = self.dir(serial).join(&meta.file);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        from_json_lenient(&text).map_err(|e| format!("invalid snapshot {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(d: f32) -> SensorParams {
        SensorParams {
            max_distance_m: d,
            disappear_delay_s: 5,
            trigger_db: [40.0; 16],
            micro_db: [30.0; 16],
        }
    }

    #[test]
    fn original_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        assert!(st.original("SN1").is_none());
        assert_eq!(
            st.save_original_if_absent("SN1", "fw", &params(3.0)),
            Ok(true)
        );
        assert_eq!(
            st.save_original_if_absent("SN1", "fw", &params(4.0)),
            Ok(false)
        );
        let meta = st.original("SN1").unwrap();
        assert_eq!(meta.kind, SnapshotKind::Original);
        let (p, clamps) = st.load("SN1", &meta).unwrap();
        assert_eq!(p, params(3.0));
        assert!(clamps.is_empty());
    }

    #[test]
    fn backups_pruned_to_10() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        let mut last = None;
        for i in 0..12 {
            last = Some(
                st.save_backup("SN1", "fw", &params(1.0 + i as f32 * 0.5))
                    .unwrap(),
            );
        }
        let n = std::fs::read_dir(dir.path().join("SN1"))
            .unwrap()
            .filter(|e| {
                let name = e.as_ref().unwrap().file_name().into_string().unwrap();
                name.starts_with("backup-") && name.ends_with(".json")
            })
            .count();
        assert_eq!(n, 10);
        let latest = st.latest_backup("SN1", None).unwrap();
        assert_eq!(Some(latest.clone()), last);
        let (p, _) = st.load("SN1", &latest).unwrap();
        assert_eq!(p, params(6.5));
    }

    #[test]
    fn serial_key_rules() {
        assert_eq!(serial_key("FFFFFFFF"), "unknown");
        assert_eq!(serial_key(""), "unknown");
        assert_eq!(serial_key("  "), "unknown");
        assert_eq!(serial_key("1A2B/3C"), "1A2B_3C");
    }

    #[test]
    fn unknown_serial_latest_only_this_session() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        st.save_backup("FFFFFFFF", "fw", &params(3.0)).unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        assert!(st.latest_backup("FFFFFFFF", Some(later)).is_none());
        assert!(st.latest_backup("FFFFFFFF", None).is_some());
        let before = SystemTime::now() - std::time::Duration::from_secs(5);
        assert!(st.latest_backup("FFFFFFFF", Some(before)).is_some());
    }

    #[test]
    fn corrupt_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        st.save_original_if_absent("SN1", "fw", &params(3.0))
            .unwrap();
        std::fs::write(dir.path().join("SN1").join("original.json"), "{").unwrap();
        let meta = st.original("SN1").unwrap();
        assert!(st.load("SN1", &meta).is_err());
    }

    #[test]
    fn unwritable_dir_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("afile");
        std::fs::write(&file, "x").unwrap();
        let st = SnapshotStore::new(file);
        assert!(st.save_backup("SN1", "fw", &params(3.0)).is_err());
        assert!(
            st.save_original_if_absent("SN1", "fw", &params(3.0))
                .is_err()
        );
    }
}
