//! Snapshot store: the sensor settings at first connection (never overwritten)
//! plus the last 10 pre-change backups, per serial number.

use crate::config_file::{ClampedField, from_json_lenient, rfc3339_utc, snapshot_json};
use crate::protocol::commands::SensorParams;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BACKUPS: usize = 10;
const ORIGINAL: &str = "original.json";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
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

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `content` to a unique tmp file (synced), then publish it at `path`
/// with a hard link, which fails instead of replacing an existing file.
/// Returns Ok(false) when `path` already exists (left untouched).
fn publish_new(path: &Path, content: &str) -> Result<bool, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = format!(
        "{}.{}.{nanos}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("snap"),
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let tmp = path.with_file_name(unique);
    let werr = |p: &Path, e: std::io::Error| format!("cannot write {}: {e}", p.display());
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(werr(&tmp, e));
    }
    let res = std::fs::hard_link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    match res {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(werr(path, e)),
    }
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
        publish_new(&path, &snapshot_json(p, fw, serial, "original"))
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
        let mut n = existing
            .iter()
            .filter(|(s, _, _)| *s == now)
            .map(|(_, n, _)| n + 1)
            .max()
            .unwrap_or(0);
        let content = snapshot_json(p, fw, serial, "backup");
        let file = loop {
            let file = format!("backup-{}-{n}.json", compact_ts(now));
            if publish_new(&dir.join(&file), &content)? {
                break file;
            }
            n += 1;
            if n > 10_000 {
                return Err(format!(
                    "cannot find a free backup name in {}",
                    dir.display()
                ));
            }
        };
        // Prune the oldest others; never the file just written.
        let others: Vec<_> = self
            .backups(serial)
            .into_iter()
            .filter(|(_, _, name)| *name != file)
            .collect();
        for (_, _, name) in others
            .iter()
            .take(others.len().saturating_sub(MAX_BACKUPS - 1))
        {
            let _ = std::fs::remove_file(dir.join(name));
        }
        if !dir.join(&file).is_file() {
            return Err(format!(
                "backup {} disappeared after saving",
                dir.join(&file).display()
            ));
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
        if meta.file != ORIGINAL && parse_backup_name(&meta.file).is_none() {
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

    #[test]
    fn existing_original_survives_publish() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        st.save_original_if_absent("SN1", "fw", &params(3.0))
            .unwrap();
        let path = dir.path().join("SN1").join("original.json");
        let before = std::fs::read_to_string(&path).unwrap();
        // Simulates a concurrent writer that got there first.
        assert_eq!(publish_new(&path, "other"), Ok(false));
        assert_eq!(
            st.save_original_if_absent("SN1", "fw", &params(9.0)),
            Ok(false)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let names: Vec<_> = std::fs::read_dir(dir.path().join("SN1"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["original.json".to_string()]);
    }

    #[test]
    fn future_named_backups_do_not_prune_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        let sd = dir.path().join("SN1");
        std::fs::create_dir_all(&sd).unwrap();
        for n in 0..10 {
            std::fs::write(sd.join(format!("backup-29990101T000000Z-{n}.json")), "{}").unwrap();
        }
        let meta = st.save_backup("SN1", "fw", &params(3.0)).unwrap();
        assert!(sd.join(&meta.file).is_file());
        assert_eq!(st.load("SN1", &meta).unwrap().0, params(3.0));
        let n = std::fs::read_dir(&sd).unwrap().count();
        assert_eq!(n, 10);
    }

    #[test]
    fn foreign_files_ignored_and_original_kept() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        st.save_original_if_absent("SN1", "fw", &params(2.0))
            .unwrap();
        let sd = dir.path().join("SN1");
        for f in [
            "notes.txt",
            "backup-garbage.json",
            "backup-x-1.json",
            "backup-20200101T000000Z-q.json",
        ] {
            std::fs::write(sd.join(f), "junk").unwrap();
        }
        for _ in 0..12 {
            st.save_backup("SN1", "fw", &params(3.0)).unwrap();
        }
        for f in [
            "notes.txt",
            "backup-garbage.json",
            "backup-x-1.json",
            "backup-20200101T000000Z-q.json",
            "original.json",
        ] {
            assert!(sd.join(f).is_file(), "{f} missing");
        }
    }

    #[test]
    fn load_rejects_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        for f in ["../x", "other.json", "notes.txt"] {
            let meta = SnapshotMeta {
                kind: SnapshotKind::Backup,
                saved_at: String::new(),
                file: f.into(),
            };
            assert!(st.load("SN1", &meta).is_err(), "{f}");
        }
    }

    #[test]
    fn not_before_boundary_is_inclusive() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        let sd = dir.path().join("SN1");
        std::fs::create_dir_all(&sd).unwrap();
        std::fs::write(sd.join("backup-20240101T000010Z-0.json"), "{}").unwrap();
        let at = |s: u64| UNIX_EPOCH + std::time::Duration::from_secs(s);
        let t = parse_backup_name("backup-20240101T000010Z-0.json")
            .unwrap()
            .0;
        assert!(st.latest_backup("SN1", Some(at(t))).is_some());
        assert!(st.latest_backup("SN1", Some(at(t + 1))).is_none());
    }
}
