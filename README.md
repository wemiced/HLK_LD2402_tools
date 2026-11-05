# LD2402 Studio

[Français](README.fr.md)

A desktop app to configure, calibrate and monitor the **HLK-LD2402** 24 GHz mmWave radar sensor over a USB-serial adapter. Built with Tauri 2 (Rust core, TypeScript front-end). Works on Windows, macOS and Linux.

![LD2402 Studio dashboard](docs/images/dashboard-en.png)

## Features

- Live dashboard: presence, distance, and the energy of the 16 range gates against their thresholds.
- Drag the trigger and micro-motion thresholds directly on the chart.
- One-click calibration (with adjustable coefficients) and automatic gain adjustment.
- Read and write all sensor parameters (max distance, disappear delay, per-gate thresholds).
- Export and import the configuration as a JSON file.
- English and French interface; more languages are easy to add (see [CONTRIBUTING.md](CONTRIBUTING.md)).
- Built-in sensor simulator, so you can try everything without hardware.

Firmware update is not available: the update protocol is not public (see [docs/firmware-update-research.md](docs/firmware-update-research.md)).

## Download and install

Get the installer for your system from the [Releases](../../releases) page: `.msi` / `.exe` (Windows), `.dmg` (macOS), `.deb` / `.AppImage` (Linux).

The builds are **not code-signed**, so your OS will warn you the first time:

- **macOS**: right-click the app, choose **Open**, then confirm. If macOS still refuses, run `xattr -dr com.apple.quarantine "/Applications/LD2402 Studio.app"`.
- **Windows**: in the SmartScreen window click **More info**, then **Run anyway**.
- **Linux**: `sudo apt install ./*.deb`, or `chmod +x *.AppImage` and run it.

### Linux serial permissions

To open the serial port you must be in the `dialout` group:

```sh
sudo usermod -aG dialout $USER
```

Then log out and back in.

## Wiring

Serial settings: 115200 baud, 8N1. Connect the sensor to a 3.3 V USB-serial adapter (manual, Table 4-1):

| Sensor | Adapter |
|--------|---------|
| RX     | TXD     |
| TX     | RXD     |
| GND    | GND     |
| 3V3    | VCCIO (3.3 V) |

Do not use 5 V logic levels.

## Usage tips

- Pick the port, click **Connect**. The sensor is put in engineering mode for the live view, and put back in normal mode when you disconnect or quit (best effort: not possible if the link is lost or the app crashes).
- Edits are **drafts**: nothing is sent to the sensor until you press **Write**. On firmware 3.3.2 and later, Write also saves the values permanently; on older firmware, whether they survive a power cycle is still to be verified on hardware.
- Drag a threshold handle on the chart to change it; the value is still a draft until written.
- Calibration coefficients: a higher coefficient makes the sensor more sensitive (1.0 to 20.0).
- Import requires a connected sensor.

## Sensor safety

The app is built so that it cannot put the sensor in a state the manual does not describe.

- **Closed list of commands.** Only the 13 command words documented in the manual can ever be sent (version, enable/end configuration, serial number, read/write parameters, output mode, calibration start/progress/interference, save, auto-gain). The check lives in the Rust core (`src-tauri/src/protocol/safety.rs`) and runs on every request and during port detection. A forbidden frame is never written: the request fails with the error kind `forbidden`.
- **No firmware or serial-number writes.** No command that could write firmware or the serial number is in the list. The serial number is only read (command `0x0011`, which answers in text on firmware 3.3.5 and later and in hex on older firmware).
- **Bounded values.** Only the documented parameters can be read or written, and every written value is checked against the manual's range (for example max distance 0.7 to 10 m, thresholds up to 95 dB, calibration coefficients 1.0 to 20.0). These limits are enforced in the Rust core, not only in the interface.
- **Conformity to the manual.** Automated tests (`src-tauri/src/device/conformity_tests.rs`) check that every frame type the app emits matches the manual's byte examples, and that the read, write, calibration and streaming sequences (including the backup taken before each change) follow sections 5.2, 5.3 and 5.5.
- **Back to normal mode.** The sensor is put back in normal mode when you disconnect or quit. This is best effort: if the USB link is lost or the app crashes, the sensor stays in its current mode until the next connection, which puts it back in normal mode first.

### Snapshots and Restore

The app keeps a copy of your sensor's settings, per serial number, in the app data folder, in `snapshots/<serial>/`:

- Linux: `~/.local/share/io.github.ld2402studio.app/snapshots`
- macOS: `~/Library/Application Support/io.github.ld2402studio.app/snapshots`
- Windows: `%APPDATA%\io.github.ld2402studio.app\snapshots`

Contents: `original.json` (the settings at the first connection, never overwritten) and up to 10 `backup-*.json` files. A backup of the current settings is taken inside every write and every calibration; if the backup fails, nothing is written. If `original.json` could not be saved at connection, it is saved from those same pre-change values at the first write or calibration.

In the parameters card, **Restore…** offers "Settings at first connection" and "Before the last change". You review the values before anything is written. Values outside the manual's ranges are listed and clamped back into range. If the sensor has no serial number, a warning tells you the snapshot may come from another sensor.

These are **not factory settings**: the protocol has no factory-reset command, so the first-connection snapshot is the closest the app can offer, and only if the sensor was not already modified before you first used this app.

### Auto-gain

Auto-gain asks for confirmation and **cannot be undone from the app**. The manual does not say whether the new gain survives a power cycle.

## Configuration file

Exported files look like this (distance rounded to 0.1 m, dB to 0.01):

```json
{
  "format": "ld2402-studio-config",
  "version": 1,
  "firmware": "v3.3.1",
  "savedAt": "2026-11-05T10:00:00Z",
  "params": {
    "maxDistanceM": 6.0,
    "disappearDelayS": 5,
    "triggerDb": [40.0, 40.0, 38.5, 36.0, 34.0, 32.0, 30.0, 30.0, 28.0, 28.0, 26.0, 26.0, 24.0, 24.0, 22.0, 22.0],
    "microDb": [32.0, 32.0, 30.5, 28.0, 26.0, 24.0, 22.0, 22.0, 20.0, 20.0, 18.0, 18.0, 16.0, 16.0, 14.0, 14.0]
  }
}
```

Each array holds 16 numbers, one per gate. Import checks the ranges: distance 0.7 to 10 m, delay 0 to 65535 s, thresholds 0 to 95 dB.

## Simulator

In debug builds, or when `LD2402_SIM=1` is set, a **Simulator** port appears in the port list.

```sh
LD2402_SIM=1 ./ld2402-studio
LD2402_SIM_VERSION=v3.3.1 LD2402_SIM=1 ...   # pretend to be another firmware
LD2402_SIM_FAIL_AFTER=50 LD2402_SIM=1 ...    # drop the link after 50 reads
```

## Build from source

Requirements: Node 22, Rust (stable), and on Linux `libwebkit2gtk-4.1-dev libudev-dev librsvg2-dev libayatana-appindicator3-dev build-essential`.

```sh
npm ci
npm run tauri dev     # development
npm run tauri build   # installers in src-tauri/target/release/bundle/
```

## License

MIT, see [LICENSE](LICENSE).
