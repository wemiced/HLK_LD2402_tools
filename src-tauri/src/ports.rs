//! Serial port listing and LD2402 probing.
use crate::protocol::commands::{
    CMD_ENABLE_CONFIG, CMD_END_CONFIG, CMD_READ_VERSION, parse_status, parse_version_ack,
};
use crate::protocol::frame::{CmdFrame, FrameDecoder, Inbound, encode_command};
use crate::transport::{Transport, open_serial};
use serde::Serialize;
use serialport::{SerialPortInfo, SerialPortType};
use std::time::{Duration, Instant};

const PROBE_BUDGET_MS: u64 = 400;
const ACK_BIT: u16 = 0x0100;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub path: String,
    pub label: String,
    pub usb_serial: bool,
    pub detected: Option<bool>,
}

fn wait_ack(
    t: &mut dyn Transport,
    dec: &mut FrameDecoder,
    word: u16,
    deadline: Instant,
) -> Option<CmdFrame> {
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        let n = t.read(&mut buf).ok()?;
        for msg in dec.push(&buf[..n]) {
            if let Inbound::Cmd(f) = msg
                && f.word == word | ACK_BIT
            {
                return Some(f);
            }
        }
    }
    None
}

fn guarded_write(t: &mut dyn Transport, word: u16, value: &[u8]) -> bool {
    crate::protocol::safety::check(word, value).is_ok()
        && t.write_all(&encode_command(word, value)).is_ok()
}

/// Checks whether the transport talks like an LD2402. Always sends end-config.
pub fn probe(t: &mut dyn Transport, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut dec = FrameDecoder::new();
    let mut ok = false;
    if guarded_write(t, CMD_ENABLE_CONFIG, &[0x01, 0x00])
        && wait_ack(t, &mut dec, CMD_ENABLE_CONFIG, deadline)
            .is_some_and(|f| parse_status(&f).is_ok())
        && guarded_write(t, CMD_READ_VERSION, &[])
    {
        ok = wait_ack(t, &mut dec, CMD_READ_VERSION, deadline)
            .is_some_and(|f| parse_status(&f).and_then(parse_version_ack).is_ok());
    }
    if guarded_write(t, CMD_END_CONFIG, &[]) {
        wait_ack(t, &mut dec, CMD_END_CONFIG, deadline);
    }
    ok
}

fn label_for(info: &SerialPortInfo) -> String {
    match &info.port_type {
        SerialPortType::UsbPort(u) => {
            let name = u
                .product
                .clone()
                .or_else(|| u.manufacturer.clone())
                .unwrap_or_else(|| "USB serial".into());
            format!("{name} ({:04x}:{:04x})", u.vid, u.pid)
        }
        _ => info.port_name.clone(),
    }
}

/// Pure part of listing: `probe_port` runs only for USB ports when `probe` is set.
fn build_ports(
    infos: &[SerialPortInfo],
    probe: bool,
    mut probe_port: impl FnMut(&str) -> bool,
    sim: bool,
) -> Vec<PortInfo> {
    let mut out: Vec<PortInfo> = infos
        .iter()
        .map(|i| {
            let usb_serial = matches!(i.port_type, SerialPortType::UsbPort(_));
            PortInfo {
                path: i.port_name.clone(),
                label: label_for(i),
                usb_serial,
                detected: (usb_serial && probe).then(|| probe_port(&i.port_name)),
            }
        })
        .collect();
    if sim {
        out.push(PortInfo {
            path: "sim".into(),
            label: "Simulator".into(),
            usb_serial: false,
            detected: Some(true),
        });
    }
    out
}

pub fn list_ports(probe: bool) -> Vec<PortInfo> {
    let infos = serialport::available_ports().unwrap_or_default();
    let sim = std::env::var("LD2402_SIM").is_ok_and(|v| v == "1") || cfg!(debug_assertions);
    build_ports(
        &infos,
        probe,
        |path| match open_serial(path) {
            Ok(mut t) => self::probe(&mut t, PROBE_BUDGET_MS),
            Err(_) => false,
        },
        sim,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{SimOptions, SimSensor};
    use serialport::UsbPortInfo;

    fn usb(name: &str, product: Option<&str>, manu: Option<&str>) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.into(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: 0x0403,
                pid: 0x6001,
                serial_number: None,
                manufacturer: manu.map(Into::into),
                product: product.map(Into::into),
            }),
        }
    }

    fn other(name: &str) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.into(),
            port_type: SerialPortType::Unknown,
        }
    }

    #[test]
    fn probe_detects_sim() {
        let mut sim = SimSensor::new(SimOptions::default());
        let h = sim.sim_handle();
        assert!(probe(&mut sim, 400));
        assert!(!h.in_config());
    }

    #[test]
    fn probe_unresponsive_times_out() {
        let mut sim = SimSensor::new(SimOptions {
            unresponsive: true,
            ..Default::default()
        });
        let start = Instant::now();
        assert!(!probe(&mut sim, 400));
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn labels_are_formatted() {
        let infos = [
            usb("/dev/ttyUSB0", Some("FT232R USB UART"), Some("FTDI")),
            usb("/dev/ttyUSB1", None, Some("FTDI")),
            usb("/dev/ttyUSB2", None, None),
            other("/dev/ttyS0"),
        ];
        let p = build_ports(&infos, false, |_| true, false);
        assert_eq!(p[0].label, "FT232R USB UART (0403:6001)");
        assert_eq!(p[1].label, "FTDI (0403:6001)");
        assert_eq!(p[2].label, "USB serial (0403:6001)");
        assert_eq!(p[3].label, "/dev/ttyS0");
        assert!(p[0].usb_serial && !p[3].usb_serial);
        assert!(p.iter().all(|x| x.detected.is_none()));
    }

    #[test]
    fn non_usb_not_probed_and_failure_is_false() {
        let infos = [usb("/dev/ttyUSB0", None, None), other("/dev/ttyS0")];
        let mut probed = Vec::new();
        let p = build_ports(
            &infos,
            true,
            |path| {
                probed.push(path.to_string());
                false
            },
            false,
        );
        assert_eq!(probed, vec!["/dev/ttyUSB0"]);
        assert_eq!(p[0].detected, Some(false));
        assert_eq!(p[1].detected, None);
    }

    #[test]
    fn sim_entry_appended() {
        let p = build_ports(&[], false, |_| false, true);
        assert_eq!(
            p,
            vec![PortInfo {
                path: "sim".into(),
                label: "Simulator".into(),
                usb_serial: false,
                detected: Some(true)
            }]
        );
        assert!(build_ports(&[], false, |_| false, false).is_empty());
    }
}
