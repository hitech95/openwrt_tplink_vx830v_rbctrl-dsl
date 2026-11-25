// SPDX-License-Identifier: GPL-2.0-or-later
//! One CLI session: a [`Link`] over a [`Port`], with the blocking wait loop
//! that pumps DD01 replies, DD02 events and retransmissions together.
#![allow(unsafe_code)] // poll(2) only; see `poll_fds`.

use crate::port::Port;
use rbctl_voip_proto::dsp::{Command, Direction, Record, RECORD_LEN};
use rbctl_voip_proto::link::{Link, Received};
use rbctl_voip_proto::wire::{FRAME_MAX, SUB_ATTACH, SUB_RECORD};
use rbctl_voip_proto::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Set by SIGINT/SIGTERM/SIGHUP in the ring/tone/loop modes.
pub static STOP: AtomicBool = AtomicBool::new(false);

/// CLI failure: a positive errno (printed with `strerror` semantics).
pub type Fail = i32;
/// CLI result.
pub type CliResult<T> = Result<T, Fail>;

/// Map a protocol error to a CLI errno.
pub fn errno(e: Error) -> Fail {
    e.errno()
}

/// Whether a stop signal was received.
pub fn stopped() -> bool {
    STOP.load(Ordering::Relaxed)
}

/// A DD01 transaction and the DD02 event (if any) that completes it.
#[derive(Debug, Clone, Copy)]
pub struct Step {
    /// Label printed in the transcript.
    pub name: &'static str,
    /// DD01 subtype.
    pub sub: u8,
    /// DSP command for `16` requests.
    pub cmd: Command,
    /// DD02 command that must also arrive (async completion), if any.
    pub completion: Option<Command>,
}

/// Keypad verification state (`keypad` subcommand).
#[derive(Debug, Default)]
pub struct Keypad {
    /// Line under test + 1, or 0.
    active: u32,
    /// Matched press/release edges on the active line.
    edges: usize,
    /// Bit mask of passed lines.
    pub passed: u32,
}

/// The fixed key sequence the operator presses on each line.
const KEYS: &[u8] = b"1234567890*#";

impl Keypad {
    /// Compare the complete BE32-decoded word; never log the observed digit.
    fn verify(&mut self, r: &Record) -> CliResult<()> {
        let line = r.word(0);
        if !matches!(
            r.command,
            Command::HookOff | Command::HookOn | Command::DtmfStart | Command::DtmfEnd
        ) {
            return Ok(());
        }
        if line > 1 {
            return Err(libc::EPROTO);
        }
        if r.command == Command::HookOff && self.active == 0 && self.passed & (1 << line) == 0 {
            self.active = line + 1;
            self.edges = 0;
            println!("keypad line={line} off-hook");
            return Ok(());
        }
        if self.active == line + 1 {
            let total = 2 * KEYS.len();
            if r.command == Command::HookOn && self.edges == total {
                println!("keypad line={line} PASS (12 press/release pairs)");
                self.passed |= 1 << line;
                self.active = 0;
                return Ok(());
            }
            let want = if self.edges % 2 == 1 {
                Command::DtmfEnd
            } else {
                Command::DtmfStart
            };
            let key = KEYS.get(self.edges / 2).copied().map(u32::from);
            if self.edges < total && r.command == want && Some(r.word(1)) == key {
                self.edges += 1;
                return Ok(());
            }
        }
        eprintln!("keypad line={line} FAIL after {} matched edges", self.edges);
        Err(libc::EPROTO)
    }
}

/// How far a step got: completion seen, and whether attach reported a
/// running DSP (warm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    No,
    Yes,
    /// Attach reported DSP state 0 (already running).
    Warm,
}

/// A session over one transport.
pub struct Session {
    port: Box<dyn Port>,
    /// The protocol state machine.
    pub link: Link,
    epoch: Instant,
}

impl Session {
    /// New session with a random initial sequence (the peer remembers the
    /// last sequence across master restarts).
    pub fn new(port: Box<dyn Port>) -> CliResult<Self> {
        let mut seed = [0u8; 4];
        // SAFETY: getrandom writes at most seed.len() bytes into seed.
        let n = unsafe { libc::getrandom(seed.as_mut_ptr().cast(), seed.len(), 0) };
        if usize::try_from(n).ok() != Some(seed.len()) {
            return Err(libc::EIO);
        }
        let link = Link::new(port.mac(), u32::from_ne_bytes(seed));
        Ok(Session {
            port,
            link,
            epoch: Instant::now(),
        })
    }

    /// Monotonic milliseconds since the session started.
    pub fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Send a DD01 request for `step` with one argument word (or none for
    /// `1a`/`1b`).
    pub fn submit(&mut self, step: &Step, arg: u32) -> CliResult<()> {
        let now = self.now_ms();
        if step.sub != SUB_RECORD {
            return self
                .link
                .request(&mut *self.port, step.sub, None, now)
                .map_err(errno);
        }
        let rec = Record::words(step.cmd, &[arg])
            .and_then(|r| r.encode(Direction::ToPeer))
            .map_err(errno)?;
        self.link
            .request(&mut *self.port, SUB_RECORD, Some(&rec), now)
            .map_err(errno)
    }

    /// Send one DSP record and wait for its DD01 status (1.5 s budget).
    pub fn record(&mut self, rec: &Record, name: &'static str) -> CliResult<()> {
        let step = Step {
            name,
            sub: SUB_RECORD,
            cmd: rec.command,
            completion: None,
        };
        let bytes = rec.encode(Direction::ToPeer).map_err(errno)?;
        let now = self.now_ms();
        self.link
            .request(&mut *self.port, SUB_RECORD, Some(&bytes), now)
            .map_err(errno)?;
        self.wait(Some(&step), now + 1500, None).map(|_| ())
    }

    /// Drop a request that never got its ACK, so a cleanup command can be
    /// sent instead of retransmitting it.
    pub fn abandon(&mut self) {
        self.link.abandon();
    }

    /// Send one DD03 datagram.
    pub fn send_media(
        &mut self,
        slot: u16,
        sel: rbctl_voip_proto::wire::Selector,
        data: &[u8],
    ) -> CliResult<()> {
        self.link
            .send_media(&mut *self.port, slot, sel, data)
            .map_err(errno)
    }

    /// Drain and interpret queued events.
    fn consume(
        &mut self,
        step: Option<&Step>,
        seen: &mut Seen,
        mut keys: Option<&mut Keypad>,
    ) -> CliResult<()> {
        while let Some(raw) = self.link.pop() {
            let Ok(r) = Record::decode(&raw, Direction::FromPeer) else {
                continue;
            };
            if let Some(k) = keys.as_deref_mut() {
                k.verify(&r)?;
                continue;
            }
            match step {
                Some(s) if Some(r.command) == s.completion => {
                    let result = r.word(0) as i32;
                    println!(
                        "{}: DD02 command={} result={result}",
                        s.name,
                        r.command.id()
                    );
                    // Attach reports existing state, not a DSP_START completion.
                    if s.sub == SUB_RECORD
                        && (result < 0 || (r.command == Command::DspStart && result != 0))
                    {
                        return Err(libc::EIO);
                    }
                    if s.sub == SUB_ATTACH && result != 0 && result != 1 {
                        return Err(libc::EPROTO);
                    }
                    *seen = if s.sub == SUB_ATTACH && result == 0 {
                        Seen::Warm
                    } else {
                        Seen::Yes
                    };
                }
                _ => match r.command {
                    Command::HookOff => println!("HOOK_OFF line={}", r.word(0)),
                    Command::HookOn => println!("HOOK_ON line={}", r.word(0)),
                    Command::HookFlash => println!("HOOK_FLASH line={}", r.word(0)),
                    // Report only the type, not digit or identity data.
                    c => println!("DD02 command={}", c.id()),
                },
            }
        }
        Ok(())
    }

    /// Pump the link until `step` completes, `deadline_ms` passes, or (with
    /// no step) forever / until the keypad test passes.
    ///
    /// Either the DD01 reply or the DD02 completion may arrive first;
    /// neither substitutes for the other. Returns `true` when attach
    /// reported a running DSP (warm attach).
    pub fn wait(
        &mut self,
        step: Option<&Step>,
        deadline_ms: u64,
        mut keys: Option<&mut Keypad>,
    ) -> CliResult<bool> {
        let mut seen = match step {
            Some(s) if s.completion.is_none() => Seen::Yes,
            _ => Seen::No,
        };
        let mut frame = [0u8; FRAME_MAX];
        loop {
            if step.is_none() && keys.is_none() && stopped() {
                return Err(libc::EINTR);
            }
            let now = self.now_ms();
            let mut timeout: i32 = -1;
            if step.is_none() && deadline_ms != 0 {
                if keys.as_ref().is_some_and(|k| k.passed == 3) {
                    return Ok(false);
                }
                if now >= deadline_ms {
                    return Err(libc::ETIMEDOUT);
                }
                timeout = clamp(deadline_ms - now);
            }
            if let Some(s) = step {
                if now >= deadline_ms {
                    let what = if self.link.is_pending() {
                        "DD01 reply"
                    } else {
                        "DD02 completion"
                    };
                    eprintln!("{}: waiting for {what}", s.name);
                    return Err(libc::ETIMEDOUT);
                }
                if !self.link.is_pending() && seen != Seen::No {
                    return Ok(seen == Seen::Warm);
                }
                let next = self
                    .link
                    .deadline()
                    .filter(|&d| d <= deadline_ms)
                    .unwrap_or(deadline_ms);
                timeout = clamp(next.saturating_sub(now));
            }
            if poll_fds(&self.port.fds(), timeout)? {
                while let Some(n) = self
                    .port
                    .recv(&mut frame)
                    .map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))?
                {
                    let rc = self
                        .link
                        .receive(&mut *self.port, frame.get(..n).unwrap_or(&[]));
                    if let (Ok(Received::Control), Some(s)) = (&rc, step) {
                        println!("{}: DD01/{:02x} status=0", s.name, s.sub);
                    }
                    self.consume(step, &mut seen, keys.as_deref_mut())?;
                    match rc {
                        Err(e) if !e.is_ignorable() => return Err(errno(e)),
                        _ => {}
                    }
                }
            }
            let now = self.now_ms();
            self.link.tick(&mut *self.port, now).map_err(errno)?;
        }
    }

    /// Receive one frame for the media bridge: `Ok(None)` on timeout.
    pub fn next_frame<'a>(
        &mut self,
        buf: &'a mut [u8; FRAME_MAX],
        timeout_ms: u64,
    ) -> CliResult<Option<&'a [u8]>> {
        if !poll_fds(&self.port.fds(), clamp(timeout_ms))? {
            return Ok(None);
        }
        match self
            .port
            .recv(buf)
            .map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))?
        {
            Some(n) => Ok(buf.get(..n)),
            None => Ok(Some(&[])),
        }
    }

    /// Feed one frame to the link.
    pub fn receive<'a>(&mut self, frame: &'a [u8]) -> rbctl_voip_proto::Result<Received<'a>> {
        self.link.receive(&mut *self.port, frame)
    }

    /// Take one queued raw event.
    pub fn pop(&mut self) -> Option<[u8; RECORD_LEN]> {
        self.link.pop()
    }
}

fn clamp(ms: u64) -> i32 {
    i32::try_from(ms).unwrap_or(i32::MAX)
}

/// poll(2) for readability on `fds`. `Ok(false)` on timeout; EINTR is a
/// timeout so the caller re-checks [`STOP`].
fn poll_fds(fds: &[std::os::fd::RawFd], timeout: i32) -> CliResult<bool> {
    let mut p: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    // SAFETY: p is a valid array of p.len() pollfd structures.
    let rc = unsafe { libc::poll(p.as_mut_ptr(), p.len() as libc::nfds_t, timeout) };
    if rc < 0 {
        let e = std::io::Error::last_os_error();
        return if e.kind() == std::io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(e.raw_os_error().unwrap_or(libc::EIO))
        };
    }
    if p.iter()
        .any(|x| x.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0)
    {
        return Err(libc::EIO);
    }
    Ok(p.iter().any(|x| x.revents & libc::POLLIN != 0))
}
