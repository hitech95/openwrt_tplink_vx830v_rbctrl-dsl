// SPDX-License-Identifier: GPL-2.0-or-later
//! Protocol core for the VX830v voice board link.
//!
//! The master (this code) and the stock Econet voice board talk raw Ethernet
//! on the management VLAN (`lan0.500` on the master, `eth0.1.500` on the
//! board) using three EtherTypes:
//!
//! | EtherType | Initiator | Purpose | Module |
//! |---|---|---|---|
//! | `0xDD01` | master | requests (`1a` init, `1b` attach, `16` DSP record) and the board's 4-byte status reply | [`link`] |
//! | `0xDD02` | board  | DSP events (`18`) and the master's 4-byte status reply | [`link`] |
//! | `0xDD03` | either | RTP/RTCP datagrams with a slot/selector prefix | [`wire`], [`link`] |
//!
//! Inside DD01/`16` and DD02/`18` travels a fixed 132-byte *DSP record*
//! (BE32 command + 128-byte body), handled by [`dsp`].
//!
//! The crate is **sans-I/O**: it never opens sockets, reads clocks or
//! allocates. Callers pass frames in, receive frames through a [`link::Transport`],
//! and supply a monotonic time in milliseconds. All failures are returned as
//! [`Error`]; no input can make this crate panic.
//!
//! Wire facts and their evidence live in the repository docs:
//! `docs/voip/board-link.md` (host side), `docs/voip/peer.md` (Econet side).
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::indexing_slicing)]

pub mod dsp;
mod error;
pub mod link;
pub mod wire;

pub use error::{Error, Result};
