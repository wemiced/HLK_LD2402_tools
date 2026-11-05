//! Tauri command and event layer: thin DTO mapping over the device core.
use crate::config_file::{self, ClampedField, ConfigError};
use crate::device::{CalibCoefficients, Device, DeviceError, DeviceEvent, DeviceInfo};
use crate::ports::{self, PortInfo};
use crate::protocol::commands::{CMD_READ_VERSION, SensorParams};
use crate::protocol::convert::raw_to_db;
use crate::protocol::frame::RawEngineering;
use crate::sim::{SimOptions, SimSensor};
use crate::snapshots::{SnapshotKind, SnapshotMeta, SnapshotStore, serial_key};
use crate::transport::{Transport, open_serial};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, Manager};

/// Serialised frame for the UI.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FrameDto {
    pub presence: &'static str,
    pub distance_m: f32,
    pub motion_db: [f32; 16],
    pub micro_db: [f32; 16],
}

pub fn frame_dto(raw: &RawEngineering) -> FrameDto {
    let mut motion_db = [0.0; 16];
    let mut micro_db = [0.0; 16];
    for i in 0..16 {
        motion_db[i] = raw_to_db(raw.energy[i]);
        micro_db[i] = raw_to_db(raw.energy[16 + i]);
    }
    FrameDto {
        presence: match raw.presence {
            1 => "moving",
            2 => "still",
            _ => "none",
        },
        distance_m: f32::from(raw.distance_cm) / 100.0,
        motion_db,
        micro_db,
    }
}

/// Config error with the `kind: "config"` discriminator the UI switches on.
#[derive(Serialize, Debug, PartialEq)]
pub struct ConfigErrorDto {
    kind: &'static str,
    #[serde(flatten)]
    error: ConfigError,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(untagged)]
pub enum ApiErrorDto {
    Device(DeviceError),
    Config(ConfigErrorDto),
}

impl From<DeviceError> for ApiErrorDto {
    fn from(e: DeviceError) -> Self {
        ApiErrorDto::Device(e)
    }
}

impl From<ConfigError> for ApiErrorDto {
    fn from(error: ConfigError) -> Self {
        ApiErrorDto::Config(ConfigErrorDto {
            kind: "config",
            error,
        })
    }
}

fn io_err(e: std::io::Error) -> ApiErrorDto {
    DeviceError::Io {
        message: e.to_string(),
    }
    .into()
}

type ApiResult<T> = Result<T, ApiErrorDto>;

/// Snapshot state of the connected sensor, for the UI.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotStatus {
    pub original: Option<SnapshotMeta>,
    pub latest_backup: Option<SnapshotMeta>,
    pub unknown_serial: bool,
    pub original_error: Option<String>,
}

/// A snapshot loaded for the restore confirmation (values already clamped).
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RestorePreview {
    pub meta: SnapshotMeta,
    pub params: SensorParams,
    pub clamped: Vec<ClampedField>,
    pub unknown_serial: bool,
}

fn is_unknown_serial(serial: &str) -> bool {
    serial_key(serial) == "unknown"
}

/// For a blank serial, only backups taken during this connection count.
fn backup_not_before(serial: &str, connected_at: SystemTime) -> Option<SystemTime> {
    is_unknown_serial(serial).then_some(connected_at)
}

pub fn snapshot_status_for(
    store: Option<&SnapshotStore>,
    serial: &str,
    connected_at: SystemTime,
    original_error: Option<String>,
) -> SnapshotStatus {
    SnapshotStatus {
        original: store.and_then(|st| st.original(serial)),
        latest_backup: store
            .and_then(|st| st.latest_backup(serial, backup_not_before(serial, connected_at))),
        unknown_serial: is_unknown_serial(serial),
        original_error,
    }
}

pub fn restore_preview(
    store: Option<&SnapshotStore>,
    serial: &str,
    kind: SnapshotKind,
    connected_at: SystemTime,
) -> Result<RestorePreview, DeviceError> {
    let store = store.ok_or_else(|| DeviceError::Io {
        message: NO_STORE.into(),
    })?;
    let meta = match kind {
        SnapshotKind::Original => store.original(serial),
        SnapshotKind::Backup => {
            store.latest_backup(serial, backup_not_before(serial, connected_at))
        }
    }
    .ok_or_else(|| DeviceError::Io {
        message: match kind {
            SnapshotKind::Original => "no first-connection snapshot is saved for this sensor",
            SnapshotKind::Backup => "no backup is available for this sensor",
        }
        .into(),
    })?;
    let (params, clamped) = store
        .load(serial, &meta)
        .map_err(|message| DeviceError::Io { message })?;
    Ok(RestorePreview {
        meta,
        params,
        clamped,
        unknown_serial: is_unknown_serial(serial),
    })
}

const NO_STORE: &str = "snapshots are unavailable: no app data directory";

/// The connected device and what is known about this connection.
struct Conn {
    gen_id: u64,
    device: Device,
    connected_at: SystemTime,
    original_error: Option<String>,
}

/// The connected device, tagged with a generation so a stale disconnect
/// notification can never drop a newer connection.
#[derive(Default)]
pub struct AppState {
    slot: Mutex<Option<Conn>>,
    next_gen: AtomicU64,
    /// Set once at startup; unset when the app data dir is unavailable,
    /// in which case every backup fails and changes are refused.
    store: OnceLock<SnapshotStore>,
}

impl AppState {
    fn lock(&self) -> MutexGuard<'_, Option<Conn>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn take(&self) -> Option<Device> {
        self.lock().take().map(|c| c.device)
    }

    /// Sets the snapshot store root (first call wins).
    pub fn set_store(&self, store: SnapshotStore) {
        let _ = self.store.set(store);
    }

    fn store(&self) -> Option<&SnapshotStore> {
        self.store.get()
    }

    /// Saves a pre-change backup of `current`; Err blocks the change.
    fn backup(&self, serial: &str, fw: &str, current: &SensorParams) -> Result<(), String> {
        self.store()
            .ok_or_else(|| NO_STORE.to_string())?
            .save_backup(serial, fw, current)
            .map(drop)
    }

    /// Runs `f` on the device, or fails with `Disconnected`.
    fn with<T>(&self, f: impl FnOnce(&Device) -> Result<T, DeviceError>) -> ApiResult<T> {
        let g = self.lock();
        let c = g.as_ref().ok_or(DeviceError::Disconnected)?;
        Ok(f(&c.device)?)
    }

    /// Serial and connection time of the current device.
    fn session(&self) -> Result<(String, SystemTime, Option<String>), DeviceError> {
        let g = self.lock();
        let c = g.as_ref().ok_or(DeviceError::Disconnected)?;
        Ok((
            c.device.info().serial,
            c.connected_at,
            c.original_error.clone(),
        ))
    }

    /// Stores a freshly opened device, unless its link already died: its
    /// disconnect notification may have run before the device was stored.
    /// Checked under the lock, so a later death always finds it to drop.
    fn install(
        &self,
        gen_id: u64,
        device: Device,
        connected_at: SystemTime,
    ) -> Result<(), DeviceError> {
        let mut g = self.lock();
        if !device.is_alive() {
            drop(g);
            drop(device);
            return Err(DeviceError::Disconnected);
        }
        *g = Some(Conn {
            gen_id,
            device,
            connected_at,
            original_error: None,
        });
        Ok(())
    }

    /// Reads the sensor and saves its first-connection snapshot if none
    /// exists yet. Failures are recorded for the UI, never returned.
    fn save_original(&self, gen_id: u64) {
        let r = self
            .with(|d| {
                let p = d.read_params()?;
                let info = d.info();
                Ok((p, info))
            })
            .map_err(|e| match e {
                ApiErrorDto::Device(d) => d.to_string(),
                ApiErrorDto::Config(c) => c.error.to_string(),
            })
            .and_then(|(p, info)| {
                self.store()
                    .ok_or_else(|| NO_STORE.to_string())?
                    .save_original_if_absent(&info.serial, &info.firmware, &p)
            });
        if let Err(e) = r {
            let mut g = self.lock();
            if let Some(c) = g.as_mut().filter(|c| c.gen_id == gen_id) {
                c.original_error = Some(e);
            }
        }
    }

    /// Best-effort shutdown, used on app exit.
    pub fn close(&self) {
        if let Some(d) = self.take() {
            d.close();
        }
    }
}

fn emit_event(app: &AppHandle, ev: DeviceEvent) {
    // Emit failures only mean the window is gone; nothing to do.
    let _ = match ev {
        DeviceEvent::Frame(raw) => app.emit("device://frame", frame_dto(&raw)),
        DeviceEvent::Text(_) => Ok(()),
        DeviceEvent::CalibProgress(p) => app.emit("device://calib-progress", p),
        DeviceEvent::CalibDone { gates } => {
            app.emit("device://calib-done", serde_json::json!({ "gates": gates }))
        }
        DeviceEvent::CalibFailed { error } => app.emit(
            "device://calib-failed",
            serde_json::json!({ "error": error }),
        ),
        DeviceEvent::AutoGainDone => app.emit("device://auto-gain-done", ()),
        DeviceEvent::Disconnected { reason } => app.emit(
            "device://disconnected",
            serde_json::json!({ "reason": reason }),
        ),
    };
}

fn sim_options() -> SimOptions {
    let mut o = SimOptions::default();
    if let Ok(v) = std::env::var("LD2402_SIM_VERSION") {
        o.version = v;
    }
    if let Some(n) = std::env::var("LD2402_SIM_FAIL_AFTER")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        o.fail_after_reads = Some(n);
    }
    o
}

fn open_transport(path: &str) -> Result<Box<dyn Transport>, DeviceError> {
    if path == "sim" {
        Ok(Box::new(SimSensor::new(sim_options())))
    } else {
        Ok(Box::new(open_serial(path)?))
    }
}

/// A port that opens but never answers the version read is not an LD2402
/// (wrong device, TX/RX swapped, or no power).
fn open_error(e: DeviceError, path: &str) -> DeviceError {
    match e {
        DeviceError::Timeout {
            word: CMD_READ_VERSION,
        } => DeviceError::NotLd2402 { path: path.into() },
        e => e,
    }
}

#[tauri::command]
pub async fn list_ports(probe: bool) -> ApiResult<Vec<PortInfo>> {
    tauri::async_runtime::spawn_blocking(move || ports::list_ports(probe))
        .await
        .map_err(join_err)
}

fn join_err(e: tauri::Error) -> ApiErrorDto {
    DeviceError::Io {
        message: e.to_string(),
    }
    .into()
}

#[tauri::command]
pub async fn connect(app: AppHandle, path: String) -> ApiResult<DeviceInfo> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app2.state::<AppState>();
        // Close any previous device first (outside the lock while joining).
        state.close();
        let connected_at = SystemTime::now();
        let gen_id = state.next_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let cb_app = app2.clone();
        let transport = open_transport(&path)?;
        let device = Device::open(
            transport,
            Box::new(move |ev| {
                let gone = matches!(ev, DeviceEvent::Disconnected { .. });
                emit_event(&cb_app, ev);
                if gone {
                    // Dropping the device joins the worker that is running this
                    // callback, and a command may hold the state lock, so do it
                    // from a separate thread, and only if still the same device.
                    let app = cb_app.clone();
                    std::thread::spawn(move || {
                        let state = app.state::<AppState>();
                        let dev = {
                            let mut g = state.lock();
                            match g.as_ref() {
                                Some(c) if c.gen_id == gen_id => g.take(),
                                _ => None,
                            }
                        };
                        drop(dev);
                    });
                }
            }),
        )
        .map_err(|e| open_error(e, &path))?;
        let info = device.info();
        state.install(gen_id, device, connected_at)?;
        state.save_original(gen_id);
        Ok::<_, ApiErrorDto>(info)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
pub async fn disconnect(app: AppHandle) -> ApiResult<()> {
    blocking(app, |s| {
        s.close();
        Ok(())
    })
    .await
}

/// Runs a blocking closure against the app state on the blocking pool.
async fn blocking<T: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&AppState) -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tauri::async_runtime::spawn_blocking(move || f(&app.state::<AppState>()))
        .await
        .map_err(join_err)?
}

#[tauri::command]
pub async fn read_params(app: AppHandle) -> ApiResult<SensorParams> {
    blocking(app, |s| s.with(|d| d.read_params())).await
}

#[tauri::command]
pub async fn write_params(app: AppHandle, params: SensorParams) -> ApiResult<()> {
    blocking(app, move |s| {
        s.with(|d| {
            let info = d.info();
            d.write_params_guarded(&params, &|cur| s.backup(&info.serial, &info.firmware, cur))
        })
    })
    .await
}

#[tauri::command]
pub async fn set_streaming(app: AppHandle, on: bool) -> ApiResult<()> {
    blocking(app, move |s| s.with(|d| d.set_streaming(on))).await
}

#[tauri::command]
pub async fn start_calibration(app: AppHandle, coeffs: CalibCoefficients) -> ApiResult<()> {
    blocking(app, move |s| {
        s.with(|d| {
            let info = d.info();
            d.start_calibration_guarded(coeffs, &|cur| s.backup(&info.serial, &info.firmware, cur))
        })
    })
    .await
}

#[tauri::command]
pub async fn snapshot_status(app: AppHandle) -> ApiResult<SnapshotStatus> {
    blocking(app, |s| {
        let (serial, connected_at, original_error) = s.session()?;
        Ok(snapshot_status_for(
            s.store(),
            &serial,
            connected_at,
            original_error,
        ))
    })
    .await
}

#[tauri::command]
pub async fn preview_restore(app: AppHandle, kind: SnapshotKind) -> ApiResult<RestorePreview> {
    blocking(app, move |s| {
        let (serial, connected_at, _) = s.session()?;
        Ok(restore_preview(s.store(), &serial, kind, connected_at)?)
    })
    .await
}

#[tauri::command]
pub async fn auto_gain(app: AppHandle) -> ApiResult<()> {
    blocking(app, |s| s.with(|d| d.auto_gain())).await
}

#[tauri::command]
pub async fn export_config(app: AppHandle, path: String, params: SensorParams) -> ApiResult<()> {
    blocking(app, move |s| {
        let fw = s.lock().as_ref().map(|c| c.device.info().firmware);
        let json = config_file::to_json(&params, fw.as_deref());
        std::fs::write(&path, json).map_err(io_err)
    })
    .await
}

#[tauri::command]
pub async fn import_config(app: AppHandle, path: String) -> ApiResult<SensorParams> {
    blocking(app, move |_| {
        let text = std::fs::read_to_string(&path).map_err(io_err)?;
        Ok(config_file::from_json(&text)?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn sample() -> RawEngineering {
        let mut energy = [0u32; 32];
        energy[0] = 4598;
        energy[16] = 206769;
        RawEngineering {
            presence: 1,
            distance_cm: 102,
            energy,
        }
    }

    #[test]
    fn frame_dto_conversion() {
        let f = frame_dto(&sample());
        assert_eq!(f.presence, "moving");
        assert_eq!(f.distance_m, 1.02);
        assert_eq!(f.motion_db[0], 36.63);
        assert_eq!(f.micro_db[0], 53.15);
        let v = serde_json::to_value(&f).unwrap();
        assert!(v["distanceM"].is_number() && v["motionDb"].is_array());
    }

    #[test]
    fn presence_mapping() {
        let mut r = sample();
        for (p, s) in [(0, "none"), (1, "moving"), (2, "still"), (9, "none")] {
            r.presence = p;
            assert_eq!(frame_dto(&r).presence, s);
        }
    }

    #[test]
    fn error_serialisation() {
        let d: ApiErrorDto = DeviceError::PermissionDenied {
            path: "/dev/ttyUSB0".into(),
        }
        .into();
        assert_eq!(
            serde_json::to_value(d).unwrap(),
            json!({"kind":"permissionDenied","path":"/dev/ttyUSB0"})
        );
        let c: ApiErrorDto = ConfigError::OutOfRange {
            field: "triggerDb[3]".into(),
        }
        .into();
        assert_eq!(
            serde_json::to_value(c).unwrap(),
            json!({"kind":"config","reason":"outOfRange","field":"triggerDb[3]"})
        );
        let n: ApiErrorDto = ConfigError::NotAConfig.into();
        assert_eq!(
            serde_json::to_value(n).unwrap(),
            json!({"kind":"config","reason":"notAConfig"})
        );
    }

    #[test]
    fn no_device_means_disconnected() {
        let s = AppState::default();
        let r = s.with(|d| d.read_params());
        assert_eq!(
            r.unwrap_err(),
            ApiErrorDto::Device(DeviceError::Disconnected)
        );
    }

    fn open_sim(opts: SimOptions) -> Result<Device, DeviceError> {
        Device::open(Box::new(SimSensor::new(opts)), Box::new(|_| {}))
    }

    #[test]
    fn silent_device_is_not_an_ld2402() {
        let e = open_sim(SimOptions {
            unresponsive: true,
            ..Default::default()
        })
        .err()
        .map(|e| open_error(e, "/dev/ttyUSB0"));
        assert_eq!(
            e,
            Some(DeviceError::NotLd2402 {
                path: "/dev/ttyUSB0".into()
            })
        );
        assert_eq!(
            serde_json::to_value(DeviceError::NotLd2402 {
                path: "/dev/x".into()
            })
            .unwrap(),
            json!({"kind": "notLd2402", "path": "/dev/x"})
        );
        // other open failures keep their own meaning
        let t = DeviceError::Timeout { word: 0x0011 };
        assert_eq!(open_error(t.clone(), "/dev/x"), t);
    }

    #[test]
    fn dead_device_is_not_installed() {
        let d = open_sim(SimOptions {
            fail_after_reads: Some(20),
            ..Default::default()
        })
        .unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while d.is_alive() && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let s = AppState::default();
        let now = SystemTime::now();
        assert_eq!(s.install(1, d, now), Err(DeviceError::Disconnected));
        assert!(s.lock().is_none());
        let live = open_sim(SimOptions::default()).unwrap();
        assert_eq!(s.install(2, live, now), Ok(()));
        assert!(s.lock().is_some());
    }

    fn sensor(d: f32) -> SensorParams {
        SensorParams {
            max_distance_m: d,
            disappear_delay_s: 5,
            trigger_db: [40.0; 16],
            micro_db: [30.0; 16],
        }
    }

    #[test]
    fn status_unknown_serial_hides_old_backup() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        st.save_backup("FFFFFFFF", "fw", &sensor(3.0)).unwrap();
        st.save_backup("SN1", "fw", &sensor(3.0)).unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        // Blank serial: a backup from before this connection is hidden.
        let s = snapshot_status_for(Some(&st), "FFFFFFFF", later, None);
        assert!(s.unknown_serial);
        assert_eq!(s.latest_backup, None);
        assert_eq!(s.original, None);
        // Taken during the connection: shown.
        let earlier = SystemTime::now() - Duration::from_secs(5);
        let s = snapshot_status_for(Some(&st), "FFFFFFFF", earlier, None);
        assert!(s.latest_backup.is_some());
        // Known serial: no time filter.
        let s = snapshot_status_for(Some(&st), "SN1", later, Some("boom".into()));
        assert!(!s.unknown_serial);
        assert!(s.latest_backup.is_some());
        assert_eq!(s.original_error.as_deref(), Some("boom"));
        let v = serde_json::to_value(&s).unwrap();
        for k in ["original", "latestBackup", "unknownSerial", "originalError"] {
            assert!(v.get(k).is_some(), "{k} missing in {v}");
        }
        assert_eq!(v["latestBackup"]["kind"], "backup");
        // No store: nothing offered.
        let s = snapshot_status_for(None, "SN1", later, None);
        assert_eq!((s.original, s.latest_backup), (None, None));
    }

    #[test]
    fn preview_reports_clamps() {
        let dir = tempfile::tempdir().unwrap();
        let st = SnapshotStore::new(dir.path().to_path_buf());
        let now = SystemTime::now();
        assert_eq!(
            restore_preview(Some(&st), "SN1", SnapshotKind::Original, now)
                .err()
                .map(|e| matches!(e, DeviceError::Io { .. })),
            Some(true)
        );
        st.save_original_if_absent("SN1", "fw", &sensor(3.0))
            .unwrap();
        let path = dir.path().join("SN1").join("original.json");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["params"]["maxDistanceM"] = json!(12.0);
        std::fs::write(&path, v.to_string()).unwrap();
        let p = restore_preview(Some(&st), "SN1", SnapshotKind::Original, now).unwrap();
        assert_eq!(p.meta.kind, SnapshotKind::Original);
        assert_eq!(p.params.max_distance_m, 10.0);
        assert_eq!(p.clamped.len(), 1);
        assert_eq!(p.clamped[0].field, "maxDistanceM");
        assert!(!p.unknown_serial);
        let j = serde_json::to_value(&p).unwrap();
        assert_eq!(j["meta"]["kind"], "original");
        assert!(j["clamped"].is_array() && j.get("unknownSerial").is_some());
        assert_eq!(
            serde_json::from_value::<SnapshotKind>(json!("backup")).unwrap(),
            SnapshotKind::Backup
        );
        // Corrupt file -> Io with the load error.
        std::fs::write(&path, "not json").unwrap();
        match restore_preview(Some(&st), "SN1", SnapshotKind::Original, now) {
            Err(DeviceError::Io { message }) => {
                assert!(message.contains("invalid snapshot"), "{message}")
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn storeless_backup_is_refused() {
        let s = AppState::default();
        let e = s.backup("SN1", "fw", &sensor(3.0)).unwrap_err();
        assert!(e.contains("no app data directory"), "{e}");
    }
}
