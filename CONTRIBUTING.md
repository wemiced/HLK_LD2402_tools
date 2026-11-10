# Contributing

Thanks for helping! Issues and pull requests are welcome.

## Running the tests

```sh
npm ci
npm test                                   # front-end (vitest)
cd src-tauri
cargo test                                 # Rust core
cargo clippy --all-targets -- -D warnings  # lints, must be clean
```

CI runs the same checks. `npm run build` type-checks the front-end.

## Adding a language

1. Copy `src/locales/en.json` to `src/locales/<code>.json` (for example `de.json`).
2. Translate every value. Keep the keys and the `{placeholders}` unchanged.
3. Set `_meta.name` to the language's own name (for example `"Deutsch"`); it is shown in the language picker.
4. Run `npm test`. The tests require every locale to have exactly the same keys as the others.

## Protocol notes

The sensor speaks a binary protocol over serial, 115200 8N1 (HLK-LD2402 manual). The code lives in `src-tauri/src/protocol/`.

- Command and ACK frames: header `FD FC FB FA`, 2-byte little-endian length, 2-byte command word, payload, tail `04 03 02 01`.
- Engineering-mode reports: header `F4 F3 F2 F1`, fixed 131-byte frame (presence, distance, 32 gate energies), tail `F8 F7 F6 F5`. Normal mode sends text lines.
- Configuration commands run between "enable configuration" (`0x00FF`) and "end configuration" (`0x00FE`). The sensor is always put back in normal mode on disconnect and exit.
- Parameters are addressed by id: `0x0001` max distance, `0x0004` disappear delay, `0x0010`+ trigger thresholds, `0x0030`+ micro-motion thresholds. Permanent save (`0x00FD`) needs firmware 3.3.2 or later; features are gated by firmware version.
- Device logic is in `src-tauri/src/device/`; a full simulated sensor (`src-tauri/src/sim.rs`) reproduces the protocol, so most behaviour can be tested without hardware.

Firmware update is intentionally not implemented: the vendor protocol is not public.
