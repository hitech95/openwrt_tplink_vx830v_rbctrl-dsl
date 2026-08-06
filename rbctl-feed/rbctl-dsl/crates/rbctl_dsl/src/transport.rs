//! Unix-socket `Transport` impl for the ubus-zero protocol.
//!
//! This is the one piece of integration code rbctl-dsl owns on the ubus side:
//! ubus-zero is runtime-neutral (no_std) and ships no socket adapter; we provide
//! a non-blocking `AF_UNIX` transport over `/var/run/ubus.sock`. Frame
//! (de)serialisation is delegated to `ubus::wire::Codec`; this module only owns
//! the byte-pipe and the partial-frame buffer.
//!
//! Modelled on pawelchcki/ubus-zero `crates/testkit/src/unix_transport.rs`,
//! trimmed for production (no test hooks, fixed send deadline).

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ubus::error::UbusError;
use ubus::transport::Transport;
use ubus::wire::{Codec, Frame, MAX_MSG_LEN};

/// Non-blocking `AF_UNIX` transport for ubus-zero.
pub struct UnixUbusTransport {
    stream: UnixStream,
    rx_buf: Vec<u8>,
}

impl UnixUbusTransport {
    /// Connect to a ubusd listening Unix socket (normally `/var/run/ubus.sock`).
    pub fn connect(path: impl AsRef<Path>) -> Result<Self, UbusError> {
        let stream = UnixStream::connect(path).map_err(io_err)?;
        // Non-blocking: ubus-zero's recv() contract returns Ok(None) when no
        // full frame is buffered, and its handshake loops call wait_recv()
        // between polls.
        stream.set_nonblocking(true).map_err(io_err)?;
        Ok(Self { stream, rx_buf: Vec::with_capacity(1024) })
    }

    /// Drain the kernel socket buffer into `rx_buf`. Returns `true` if the peer
    /// closed the connection (EOF), `false` if the read just WouldBlocked.
    fn fill_rx(&mut self) -> Result<bool, UbusError> {
        const RX_BUF_CAP: usize = MAX_MSG_LEN * 2;
        let mut chunk = [0u8; 2048];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(true), // EOF
                Ok(n) => {
                    if self.rx_buf.len().saturating_add(n) > RX_BUF_CAP {
                        self.rx_buf.clear();
                        return Err(UbusError::Malformed("rx buffer exceeded cap"));
                    }
                    self.rx_buf.extend_from_slice(&chunk[..n]);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_err(e)),
            }
        }
    }
}

impl Transport for UnixUbusTransport {
    fn send(&mut self, frame: &Frame) -> Result<(), UbusError> {
        let bytes = Codec::write_frame(frame);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut sent = 0;
        while sent < bytes.len() {
            match self.stream.write(&bytes[sent..]) {
                Ok(0) => return Err(UbusError::Closed),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(UbusError::Io("send timed out".into()));
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(io_err(e)),
            }
        }
        Ok(())
    }

    fn recv(&mut self) -> Result<Option<Frame>, UbusError> {
        let closed = match self.fill_rx() {
            Ok(c) => c,
            Err(e) => {
                self.rx_buf.clear();
                return Err(e);
            }
        };
        match Codec::read_frame(&self.rx_buf) {
            Ok((frame, consumed)) => {
                self.rx_buf.drain(..consumed);
                Ok(Some(frame))
            }
            Err(UbusError::Short { .. }) if closed => Err(UbusError::Closed),
            Err(UbusError::Short { .. }) => Ok(None), // no full frame yet
            Err(e) => {
                self.rx_buf.clear();
                Err(e)
            }
        }
    }

    fn wait_recv(&mut self) {
        // Yield the CPU between non-blocking recv() polls. The real event loop
        // (Phase 3) drives this from uloop instead of sleeping.
        thread::sleep(Duration::from_millis(1));
    }
}

fn io_err(e: io::Error) -> UbusError {
    UbusError::Io(e.to_string())
}
