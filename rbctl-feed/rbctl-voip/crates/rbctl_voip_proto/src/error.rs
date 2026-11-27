// SPDX-License-Identifier: GPL-2.0-or-later
//! Error type shared by every protocol operation.

use core::fmt;

/// Result alias used throughout the crate.
pub type Result<T> = core::result::Result<T, Error>;

/// Why an operation failed.
///
/// Every variant maps to one Linux `errno` through [`Error::errno`], which is
/// what the C ABI (`rbctl_voip_ffi`) returns as a negative value. The mapping
/// is identical to the C prototype so callers and logs stay comparable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A field or argument is outside its documented range (`EINVAL`).
    InvalidArgument,
    /// A length does not fit: frame too short/long, body larger than the
    /// buffer, or a reply body that is not exactly 4 bytes (`EMSGSIZE`).
    MessageSize,
    /// Unknown EtherType or marker, or an invalid media slot/selector
    /// prefix (`EPROTO`).
    Protocol,
    /// CRC-16/ARC mismatch over the voice header and declared body
    /// (`EBADMSG`).
    BadChecksum,
    /// A DD01 request is still outstanding; only one may be in flight
    /// (`EBUSY`).
    Busy,
    /// Subtype or DSP command this implementation does not send or decode,
    /// e.g. DD01 `1c`, which the stock peer never answers (`EOPNOTSUPP`).
    Unsupported,
    /// The retransmit budget of the pending request ran out (`ETIMEDOUT`).
    TimedOut,
    /// The frame is valid but not for us: wrong peer, wrong sequence or
    /// subtype, own echo, or an event queue that is empty (`EAGAIN`).
    NotMatched,
    /// The board answered the pending request with a nonzero BE32 status
    /// (`EREMOTEIO`). The status value is carried for diagnostics.
    PeerStatus(u32),
    /// The transport failed to write a frame; carries the positive errno.
    Transport(i32),
}

impl Error {
    /// Positive Linux errno for this error (for `-errno` C returns).
    pub fn errno(self) -> i32 {
        match self {
            Error::InvalidArgument => libc::EINVAL,
            Error::MessageSize => libc::EMSGSIZE,
            Error::Protocol => libc::EPROTO,
            Error::BadChecksum => libc::EBADMSG,
            Error::Busy => libc::EBUSY,
            Error::Unsupported => libc::EOPNOTSUPP,
            Error::TimedOut => libc::ETIMEDOUT,
            Error::NotMatched => libc::EAGAIN,
            Error::PeerStatus(_) => libc::EREMOTEIO,
            Error::Transport(errno) => errno,
        }
    }

    /// True for frames that are merely uninteresting (wrong peer, stale
    /// reply, malformed noise). Event loops normally skip these and keep
    /// going; everything else should end the current operation.
    pub fn is_ignorable(self) -> bool {
        matches!(
            self,
            Error::NotMatched | Error::Protocol | Error::MessageSize | Error::BadChecksum
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidArgument => f.write_str("invalid argument"),
            Error::MessageSize => f.write_str("message size out of range"),
            Error::Protocol => f.write_str("protocol error"),
            Error::BadChecksum => f.write_str("bad checksum"),
            Error::Busy => f.write_str("request already pending"),
            Error::Unsupported => f.write_str("operation not supported"),
            Error::TimedOut => f.write_str("request timed out"),
            Error::NotMatched => f.write_str("frame not for this link"),
            Error::PeerStatus(s) => write!(f, "peer status {s}"),
            Error::Transport(e) => write!(f, "transport errno {e}"),
        }
    }
}

impl std::error::Error for Error {}
