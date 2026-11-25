// SPDX-License-Identifier: GPL-2.0-or-later
//! Frame I/O for the CLI: the real AF_PACKET sockets, or (test builds with
//! the `fixture` feature) a datagram socket handed over by the test harness.

use rbctl_voip_net::BoardSocket;
use rbctl_voip_proto::link::Transport;
use rbctl_voip_proto::wire::{Mac, FRAME_MAX};
use std::io;
use std::os::fd::RawFd;

/// What the CLI needs from a frame transport.
pub trait Port: Transport {
    /// Interface MAC used as frame source.
    fn mac(&self) -> Mac;
    /// Descriptors to poll for readability.
    fn fds(&self) -> Vec<RawFd>;
    /// Read one frame, `Ok(None)` if nothing is queued.
    fn recv(&mut self, buf: &mut [u8; FRAME_MAX]) -> io::Result<Option<usize>>;
}

impl Port for BoardSocket {
    fn mac(&self) -> Mac {
        BoardSocket::mac(self)
    }
    fn fds(&self) -> Vec<RawFd> {
        BoardSocket::fds(self).to_vec()
    }
    fn recv(&mut self, buf: &mut [u8; FRAME_MAX]) -> io::Result<Option<usize>> {
        BoardSocket::recv(self, buf)
    }
}

/// Open the transport for `device`.
pub fn open(device: &str) -> io::Result<Box<dyn Port>> {
    #[cfg(feature = "fixture")]
    if device == "fixture" {
        return fixture::Fixture::from_env().map(|f| Box::new(f) as Box<dyn Port>);
    }
    Ok(Box::new(BoardSocket::open(device)?))
}

#[cfg(feature = "fixture")]
mod fixture {
    //! Unix datagram socket from `RBCTL_VOIP_TEST_FD`; MAC 02:00:00:00:00:01.
    #![allow(unsafe_code)]
    use super::*;
    use rbctl_voip_proto::{Error, Result};

    pub struct Fixture {
        fd: RawFd,
    }

    impl Fixture {
        pub fn from_env() -> io::Result<Self> {
            let fd = std::env::var("RBCTL_VOIP_TEST_FD")
                .ok()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
            Ok(Fixture { fd })
        }
    }

    impl Transport for Fixture {
        fn send(&mut self, frame: &[u8]) -> Result<()> {
            // SAFETY: fd is a valid descriptor owned by the harness; the
            // buffer is valid for frame.len() bytes.
            let n = unsafe { libc::send(self.fd, frame.as_ptr().cast(), frame.len(), 0) };
            if n < 0 {
                return Err(Error::Transport(
                    io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(libc::EIO),
                ));
            }
            if usize::try_from(n).ok() == Some(frame.len()) {
                Ok(())
            } else {
                Err(Error::Transport(libc::EIO))
            }
        }
    }

    impl Port for Fixture {
        fn mac(&self) -> Mac {
            [2, 0, 0, 0, 0, 1]
        }
        fn fds(&self) -> Vec<RawFd> {
            vec![self.fd]
        }
        fn recv(&mut self, buf: &mut [u8; FRAME_MAX]) -> io::Result<Option<usize>> {
            // SAFETY: buf is valid for FRAME_MAX bytes.
            let n = unsafe {
                libc::recv(
                    self.fd,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n < 0 {
                let e = io::Error::last_os_error();
                return if e.kind() == io::ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(e)
                };
            }
            Ok(usize::try_from(n).ok())
        }
    }
}
