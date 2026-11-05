//! Byte transport abstraction over a serial port or the simulator.
use crate::device::DeviceError;
use serialport::SerialPort;
use std::io::{self, Read, Write};
use std::time::Duration;

pub trait Transport: Send {
    /// Reads available bytes; returns `Ok(0)` when the 50 ms timeout expires.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
}

pub struct SerialTransport {
    port: Box<dyn SerialPort>,
}

impl Transport for SerialTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.port.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            other => other,
        }
    }
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.port.write_all(data)?;
        self.port.flush()
    }
}

pub(crate) fn map_open_error(e: serialport::Error, path: &str) -> DeviceError {
    use serialport::ErrorKind;
    match e.kind {
        ErrorKind::Io(io::ErrorKind::PermissionDenied) => {
            DeviceError::PermissionDenied { path: path.into() }
        }
        // serialport maps EBUSY (port held by another program) to NoDevice:
        // if the device node exists, it is in use rather than missing.
        ErrorKind::NoDevice if std::path::Path::new(path).exists() => {
            DeviceError::InUse { path: path.into() }
        }
        ErrorKind::NoDevice | ErrorKind::Io(io::ErrorKind::NotFound) => {
            DeviceError::NotFound { path: path.into() }
        }
        _ => DeviceError::Io {
            message: e.description,
        },
    }
}

/// Opens a port at 115200 8N1 with a 50 ms read timeout.
pub fn open_serial(path: &str) -> Result<SerialTransport, DeviceError> {
    let port = serialport::new(path, 115_200)
        .timeout(Duration::from_millis(50))
        .open()
        .map_err(|e| map_open_error(e, path))?;
    Ok(SerialTransport { port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_denied_maps_to_error() {
        let e = serialport::Error::new(
            serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied),
            "x",
        );
        assert_eq!(
            map_open_error(e, "/dev/ttyUSB0"),
            DeviceError::PermissionDenied {
                path: "/dev/ttyUSB0".into()
            }
        );
    }

    #[test]
    fn no_device_on_existing_path_means_in_use() {
        // serialport reports EBUSY (port held by another program) as NoDevice.
        let path = std::env::temp_dir();
        let path = path.to_str().unwrap();
        let e = serialport::Error::new(serialport::ErrorKind::NoDevice, "busy");
        assert_eq!(
            map_open_error(e, path),
            DeviceError::InUse { path: path.into() }
        );
        let e = serialport::Error::new(serialport::ErrorKind::NoDevice, "gone");
        assert_eq!(
            map_open_error(e, "/nonexistent/ttyUSB9"),
            DeviceError::NotFound {
                path: "/nonexistent/ttyUSB9".into()
            }
        );
        let v = serde_json::to_value(DeviceError::InUse {
            path: "/dev/x".into(),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({"kind": "inUse", "path": "/dev/x"}));
    }
}
