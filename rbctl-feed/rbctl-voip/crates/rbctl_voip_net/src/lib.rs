// SPDX-License-Identifier: GPL-2.0-or-later
//! Linux transport for the voice link: three non-blocking `AF_PACKET`
//! sockets on the VLAN device (`lan0.500`), one per EtherType
//! (`0xDD01`, `0xDD02`, `0xDD03`), built on the shared `af_packet` crate.
//!
//! Each socket is bound to its EtherType with a kernel BPF filter, so the
//! kernel never echoes our own transmissions back (only `ETH_P_ALL`
//! sockets see outgoing copies). The VLAN device adds/strips the 802.1Q tag.
//!
//! [`BoardSocket`] implements [`Transport`] for
//! [`rbctl_voip_proto::link::Link`] and exposes its descriptors so an
//! external loop (`poll`, baresip/libre `fd_listen`) can wait on them.
#![deny(missing_docs)]

use af_packet::RawSocket;
use rbctl_voip_proto::link::Transport;
use rbctl_voip_proto::wire::{
    self, Mac, ETH_CONTROL, ETH_EVENT, ETH_MEDIA, FRAME_MAX, MARKER_HOST,
};
use rbctl_voip_proto::{Error, Result};
use std::io;
use std::os::fd::{AsRawFd, RawFd};

/// Linux `PACKET_OUTGOING` (`<linux/if_packet.h>`).
const PACKET_OUTGOING: u8 = 4;

/// The three voice sockets on one VLAN device.
pub struct BoardSocket {
    /// Indexed by EtherType: DD01, DD02, DD03.
    sockets: [RawSocket; 3],
    mac: Mac,
    /// Next socket to read first, so one busy EtherType (media) cannot
    /// starve the others.
    next: usize,
}

impl BoardSocket {
    /// Open and bind all three sockets on `device` (e.g. `"lan0.500"`),
    /// non-blocking. Requires `CAP_NET_RAW`. Does not create the VLAN.
    pub fn open(device: &str) -> io::Result<Self> {
        let open = |et| -> io::Result<RawSocket> {
            let s = RawSocket::open(device, et)?;
            s.set_nonblocking(true)?;
            Ok(s)
        };
        let sockets = [open(ETH_CONTROL)?, open(ETH_EVENT)?, open(ETH_MEDIA)?];
        let mac = sockets[0].local_mac();
        Ok(BoardSocket {
            sockets,
            mac,
            next: 0,
        })
    }

    /// Interface MAC used as the source of every frame.
    pub fn mac(&self) -> Mac {
        self.mac
    }

    /// Descriptors to wait on for readability (DD01, DD02, DD03).
    pub fn fds(&self) -> [RawFd; 3] {
        [
            self.sockets[0].as_raw_fd(),
            self.sockets[1].as_raw_fd(),
            self.sockets[2].as_raw_fd(),
        ]
    }

    /// Read one queued frame into `buf` (size it [`FRAME_MAX`]).
    ///
    /// Returns `Ok(None)` when no socket has data. Frames are untagged and
    /// exclude the FCS. Our own outgoing copies are skipped defensively.
    pub fn recv(&mut self, buf: &mut [u8; FRAME_MAX]) -> io::Result<Option<usize>> {
        for i in 0..3 {
            let idx = (self.next + i) % 3;
            let Some(sock) = self.sockets.get(idx) else {
                continue;
            };
            match sock.recv(buf) {
                Ok(p) if p.pkt_type == PACKET_OUTGOING => continue,
                Ok(p) => {
                    self.next = (idx + 1) % 3;
                    return Ok(Some(p.data.len()));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

impl std::fmt::Debug for BoardSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardSocket")
            .field("fds", &self.fds())
            .field("mac", &self.mac)
            .finish()
    }
}

impl Transport for BoardSocket {
    /// Send a complete host frame on the socket of its EtherType.
    ///
    /// Rejects frames that do not decode, are not host-marked, or carry a
    /// source other than this interface ([`Error::InvalidArgument`]).
    fn send(&mut self, frame: &[u8]) -> Result<()> {
        let f = wire::decode_frame(frame)?;
        if f.header.marker != MARKER_HOST || f.header.src != self.mac {
            return Err(Error::InvalidArgument);
        }
        let idx = usize::from(f.header.ethertype - ETH_CONTROL);
        let sock = self.sockets.get(idx).ok_or(Error::InvalidArgument)?;
        match sock.send(frame) {
            Ok(n) if n == frame.len() => Ok(()),
            Ok(_) => Err(Error::Transport(libc::EIO)),
            Err(e) => Err(Error::Transport(e.raw_os_error().unwrap_or(libc::EIO))),
        }
    }
}
