//! Test transport that records every frame written to the "serial port".
use crate::sim::{SimHandle, SimOptions, SimSensor};
use crate::transport::Transport;
use std::sync::{Arc, Mutex};

pub(crate) type Frames = Arc<Mutex<Vec<Vec<u8>>>>;

pub(crate) struct RecordingTransport {
    inner: SimSensor,
    frames: Frames,
}

impl RecordingTransport {
    pub(crate) fn new(opts: SimOptions) -> Self {
        Self {
            inner: SimSensor::new(opts),
            frames: Frames::default(),
        }
    }
    pub(crate) fn frames_handle(&self) -> Frames {
        Arc::clone(&self.frames)
    }
    #[allow(dead_code)] // used by Task 2
    pub(crate) fn sim_handle(&self) -> SimHandle {
        self.inner.sim_handle()
    }
}

impl Transport for RecordingTransport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.frames
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(data.to_vec());
        self.inner.write_all(data)
    }
}
