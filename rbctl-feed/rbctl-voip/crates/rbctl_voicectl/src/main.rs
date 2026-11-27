// SPDX-License-Identifier: GPL-2.0-or-later
//! `rbctl-voicectl` — bring-up and diagnostic CLI for the VX830v voice
//! board link (M2 tests T2.1–T2.8). Port of the C `vx830v-voicectl`.
//!
//! Every subcommand first runs the boot sequence (`1a` init, `1b` attach,
//! SLIC locale, DSP defaults, DSP start, channel defaults). If attach reports
//! a running DSP (warm attach, e.g. after a master restart), DSP_START is
//! skipped — the running peer does not answer a second start — and all four
//! slots are reclaimed first. Subcommands that only observe (`keypad`,
//! `monitor`, `ring`, `tone`, `loop`) stop after attach and require an
//! already initialized DSP.
//!
//! Stdout carries a protocol transcript (no digits, names or audio are
//! printed). Set `RUST_LOG=debug` for frame-level logs on stderr.
#![allow(unsafe_code)] // signal handler installation only

mod port;
mod session;

use clap::{Parser, Subcommand};
use rbctl_voip_proto::dsp::{Body, Cid, Command, Direction, MediaConfig, Record, Tone, AF_INET};
use rbctl_voip_proto::link::Received;
use rbctl_voip_proto::wire::{Selector, FRAME_MAX, SUB_ATTACH, SUB_INIT, SUB_RECORD};
use session::{errno, stopped, CliResult, Keypad, Session, Step, STOP};
use std::process::ExitCode;
use std::sync::atomic::Ordering;

#[derive(Parser, Debug)]
#[command(
    name = "rbctl-voicectl",
    version,
    about = "VX830v voice board bring-up tool"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// VLAN device toward the voice board.
    #[arg(long, global = true, default_value = "lan0.500")]
    device: String,
    /// SLIC/DSP locale index 0..=20 (e.g. 10 = Italy profile).
    #[arg(long, global = true, value_parser = clap::value_parser!(u32).range(0..=20))]
    locale: Option<u32>,
    /// Overall boot budget in milliseconds.
    #[arg(long = "timeout-ms", global = true, default_value_t = 5000,
          value_parser = clap::value_parser!(u32).range(1..=120_000))]
    timeout_ms: u32,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Initialize the board and exit when ready (T2.1).
    Init,
    /// Initialize, then print hook/digit events until interrupted (T2.2).
    Events,
    /// Attach to a running DSP and verify the 12-key sequence on both lines (T2.3).
    Keypad,
    /// Attach to a running DSP and print events for 300 s.
    Monitor,
    /// Evaluate SLIC_CONTROL port LED sub-operations (T4.23).
    Led {
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1))]
        line: u32,
        #[arg(long, value_parser = clap::value_parser!(u32).range(2..=7))]
        op: u32,
    },
    /// Ring a line, optionally with caller ID (T2.4).
    Ring {
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1))]
        line: u32,
        #[arg(long = "duration-ms", default_value_t = 6000,
              value_parser = clap::value_parser!(u32).range(1..=20_000))]
        duration_ms: u32,
        /// Caller ID as NAME,DIGITS (each 1..=15 characters).
        #[arg(long, value_parser = parse_cid)]
        cid: Option<(String, String)>,
    },
    /// Play a peer tone selector for a bounded time, then stop and close.
    Tone {
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1))]
        line: u32,
        /// Peer selector (see docs/voip/peer.md); 4 = dial, 0 = stop only.
        #[arg(long, default_value_t = 4, value_parser = parse_sel)]
        sel: u32,
        #[arg(long = "duration-ms", default_value_t = 6000,
              value_parser = clap::value_parser!(u32).range(1..=20_000))]
        duration_ms: u32,
    },
    /// T4.18: signed raw CHANNEL_OPEN gain words; both default to zero.
    #[command(about = "Measure one port's gain with a quiet fixed PCMA marker (no echo)")]
    Gain {
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=1))]
        line: u32,
        #[arg(long, default_value_t=0, allow_hyphen_values=true, value_parser=clap::value_parser!(i32).range(-6..=6))]
        tx: i32,
        #[arg(long, default_value_t=0, allow_hyphen_values=true, value_parser=clap::value_parser!(i32).range(-6..=6))]
        rx: i32,
    },
    /// Bridge PHONE1 and PHONE2 audio through the master over DD03.
    Loop {
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=0))]
        line: u32,
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=1))]
        to: u32,
        #[arg(long = "duration-ms", default_value_t = 20_000,
              value_parser = clap::value_parser!(u32).range(1..=60_000))]
        duration_ms: u32,
        /// Test-only activation profile A..E (T2.6 direction matrix):
        /// A = direction 0; B..E = direction 3; C.. = DTMF mode 1;
        /// D.. = TX event PT 101; E = RX event PT 101.
        #[arg(long, hide = true, value_parser = ["A", "B", "C", "D", "E"])]
        profile: Option<String>,
        /// Evaluate G.722 on the local board leg (T4.8); default is PCMA.
        #[arg(long)]
        g722: bool,
    },
}

fn parse_cid(s: &str) -> Result<(String, String), String> {
    let (name, digits) = s.split_once(',').ok_or("expected NAME,DIGITS")?;
    if name.is_empty() || name.len() > 15 {
        return Err("name must be 1..=15 bytes".into());
    }
    if digits.is_empty() || digits.len() > 15 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("number must be 1..=15 digits".into());
    }
    Ok((name.to_owned(), digits.to_owned()))
}

fn parse_sel(s: &str) -> Result<u32, String> {
    match s.parse::<u32>() {
        Ok(n @ (0..=7 | 9..=10 | 15..=17 | 19 | 31 | 60..=71)) => Ok(n),
        _ => Err("peer selectors: 0–7, 9–10, 15–17, 19, 31, 60–71".into()),
    }
}

const BOOT: [Step; 6] = [
    Step {
        name: "link",
        sub: SUB_INIT,
        cmd: Command::DspDefault,
        completion: None,
    },
    Step {
        name: "attach",
        sub: SUB_ATTACH,
        cmd: Command::DspDefault,
        completion: Some(Command::DspStart),
    },
    Step {
        name: "slic-locale",
        sub: SUB_RECORD,
        cmd: Command::SlicLocale,
        completion: Some(Command::SlicLocale),
    },
    Step {
        name: "dsp-default",
        sub: SUB_RECORD,
        cmd: Command::DspDefault,
        completion: None,
    },
    Step {
        name: "dsp-start",
        sub: SUB_RECORD,
        cmd: Command::DspStart,
        completion: Some(Command::DspStart),
    },
    Step {
        name: "channel-default",
        sub: SUB_RECORD,
        cmd: Command::ChannelDefault,
        completion: None,
    },
];

/// What to do once attach completed.
enum After {
    Gain {
        line: u32,
        tx: i32,
        rx: i32,
    },
    Led {
        line: u32,
        op: u32,
    },
    Ready {
        watch: bool,
    },
    Keypad,
    Monitor,
    Ring {
        line: u32,
        duration: u32,
        cid: Option<(String, String)>,
    },
    Tone {
        line: u32,
        sel: u32,
        duration: u32,
    },
    Loop {
        duration: u32,
        profile: Option<u8>,
        g722: bool,
    },
}

fn run(s: &mut Session, locale: Option<u32>, timeout: u32, after: After) -> CliResult<()> {
    let start = s.now_ms();
    let deadline = start + u64::from(timeout);
    let observe = !matches!(after, After::Ready { .. });
    let mut warm = false;
    for step in &BOOT {
        if warm && step.cmd == Command::DspStart && step.sub == SUB_RECORD {
            println!("dsp-start: skipped (warm attach; no new START sent)");
            continue;
        }
        let slots = if step.cmd == Command::ChannelDefault {
            4
        } else {
            1
        };
        for slot in 0..slots {
            let arg = match step.cmd {
                Command::ChannelDefault => slot,
                Command::DspStart => 1,
                _ => locale.unwrap_or(0),
            };
            if step.sub == SUB_RECORD {
                println!(
                    "{}: sending DD01/{:02x} command={} arg={arg}",
                    step.name,
                    step.sub,
                    step.cmd.id()
                );
            } else {
                println!("{}: sending DD01/{:02x}", step.name, step.sub);
            }
            let rc = s
                .submit(step, arg)
                .and_then(|()| s.wait(Some(step), deadline, None));
            match rc {
                Ok(w) if step.sub == SUB_ATTACH => {
                    // Observing subcommands need a previously initialized DSP.
                    if observe && !w {
                        return Err(libc::EHOSTDOWN);
                    }
                    warm = w;
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!(
                        "{} failed (peer status={})",
                        step.name,
                        s.link.peer_status()
                    );
                    return Err(e);
                }
            }
        }
        if step.sub != SUB_ATTACH {
            continue;
        }
        if warm && !observe {
            recover_slots(s, deadline)?;
        }
        match &after {
            After::Ready { .. } => {}
            After::Monitor => {
                println!("event monitor armed (300 seconds)");
                let end = s.now_ms() + 300_000;
                return s.wait(None, end, None).map(|_| ());
            }
            After::Keypad => {
                println!("keypad armed (300 seconds)");
                let mut keys = Keypad::default();
                let end = s.now_ms() + 300_000;
                return s.wait(None, end, Some(&mut keys)).map(|_| ());
            }
            After::Ring {
                line,
                duration,
                cid,
            } => {
                prepare_ring(s, *line, locale, cid.as_ref())?;
                return ring_test(s, *line, *duration);
            }
            After::Tone {
                line,
                sel,
                duration,
            } => return tone_test(s, *line, *sel, *duration),
            After::Gain { line, tx, rx } => return gain_test(s, *line, *tx, *rx),
            After::Led { line, op } => {
                return s.record(
                    &Record::words(Command::SlicControl, &[*line, *op]).map_err(errno)?,
                    "port-led",
                )
            }
            After::Loop {
                duration,
                profile,
                g722,
            } => return loop_test(s, *duration, *profile, *g722),
        }
    }
    println!("ready: elapsed_ms={}", s.now_ms() - start);
    match after {
        After::Ready { watch: true } => s.wait(None, 0, None).map(|_| ()),
        _ => Ok(()),
    }
}

/// A restarted master no longer knows which slots its predecessor opened.
/// Reclaim all four while we hold exclusive ownership of the board link.
fn recover_slots(s: &mut Session, deadline: u64) -> CliResult<()> {
    let reset = [
        Step {
            name: "recover-rtp-off",
            sub: SUB_RECORD,
            cmd: Command::RtpDeactive,
            completion: None,
        },
        Step {
            name: "recover-close",
            sub: SUB_RECORD,
            cmd: Command::ChannelClose,
            completion: None,
        },
    ];
    for slot in 0..4 {
        for step in &reset {
            if s.now_ms() >= deadline {
                return Err(libc::ETIMEDOUT);
            }
            if let Err(e) = s
                .submit(step, slot)
                .and_then(|()| s.wait(Some(step), deadline, None))
            {
                eprintln!(
                    "{} slot={slot} failed (peer status={})",
                    step.name,
                    s.link.peer_status()
                );
                return Err(e);
            }
        }
    }
    Ok(())
}

fn text<const N: usize>(src: &str) -> [u8; N] {
    let mut a = [0u8; N];
    for (d, b) in a.iter_mut().zip(src.bytes()) {
        *d = b;
    }
    a
}

/// Local time as `MMDDhhmm` (the OEM CID date format).
fn cid_date() -> CliResult<[u8; 9]> {
    // SAFETY: time(NULL) and localtime_r with valid out-pointers.
    let tm = unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return Err(libc::EINVAL);
        }
        tm
    };
    Ok(text(&format!(
        "{:02}{:02}{:02}{:02}",
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )))
}

fn prepare_ring(
    s: &mut Session,
    line: u32,
    locale: Option<u32>,
    cid: Option<&(String, String)>,
) -> CliResult<()> {
    if let Some(l) = locale {
        println!("requesting DSP_DEFAULT locale={l}");
        s.record(
            &Record::words(Command::DspDefault, &[l]).map_err(errno)?,
            "hardware-profile request",
        )?;
    }
    if stopped() {
        return Err(libc::EINTR);
    }
    let Some((name, number)) = cid else {
        return Ok(());
    };
    let c = Cid {
        line,
        select: 1 << line,
        date: cid_date()?,
        name: text(name),
        number: text(number),
        ..Default::default()
    };
    s.record(
        &Record {
            command: Command::CidPlay,
            body: Body::Cid(c),
        },
        "caller-id",
    )
}

fn ring_test(s: &mut Session, line: u32, duration: u32) -> CliResult<()> {
    let ring =
        Record::words(Command::RingStart, &[line, 0, 0, u32::MAX, 1 << line, 0]).map_err(errno)?;
    if stopped() {
        return Err(libc::EINTR);
    }
    println!("ring-start line={line} duration_ms={duration}");
    let mut rc = s.record(&ring, "ring-start");
    if rc.is_ok() {
        let end = s.now_ms() + u64::from(duration);
        rc = match s.wait(None, end, None) {
            Err(libc::ETIMEDOUT) => Ok(()),
            r => r.map(|_| ()),
        };
    }
    // Even without a start ACK the record may have reached the SLIC. Never
    // retransmit that start in place of the cleanup command.
    s.abandon();
    let cleanup = s.record(
        &Record::words(Command::RingStop, &[line]).map_err(errno)?,
        "ring-stop",
    );
    if let Err(e) = cleanup {
        eprintln!(
            "line={line} ring-stop unconfirmed: {}",
            std::io::Error::from_raw_os_error(e)
        );
        return Err(e);
    }
    rc
}

fn tone_test(s: &mut Session, line: u32, sel: u32, duration: u32) -> CliResult<()> {
    // The bring-up board is idle: use the slot with this port's affinity and
    // the OEM fixture's CHANNEL_OPEN settings.
    let open = Record::words(Command::ChannelOpen, &[line, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0])
        .map_err(errno)?;
    let tone = |selector| Record {
        command: Command::TonePlay,
        body: Body::Tone(Tone {
            args: [line, selector, 0, 0],
            ..Default::default()
        }),
    };
    let mut rc = Ok(());
    if sel != 0 {
        if stopped() {
            return Err(libc::EINTR);
        }
        rc = s.record(&open, "channel-open");
        if rc.is_ok() && !stopped() {
            println!("tone probe line={line} selector={sel} duration_ms={duration}");
            rc = s.record(&tone(sel), "tone-play");
        }
        if rc.is_ok() {
            let end = s.now_ms() + u64::from(duration);
            rc = match s.wait(None, end, None) {
                Err(libc::ETIMEDOUT) => Ok(()),
                r => r.map(|_| ()),
            };
        }
    }
    // Stop even after a lost ACK.
    s.abandon();
    let stop_rc = s.record(&tone(0), "tone selector=0");
    if sel == 0 {
        return stop_rc;
    }
    if rc.is_ok() && stop_rc.is_ok() && !stopped() {
        let end = s.now_ms() + 3000;
        rc = match s.wait(None, end, None) {
            Err(libc::ETIMEDOUT) => Ok(()),
            r => r.map(|_| ()),
        };
    }
    s.abandon();
    let close = s.record(
        &Record::words(Command::ChannelClose, &[line]).map_err(errno)?,
        "channel-close",
    );
    close.and(stop_rc).and(rc)
}

/// Activation profile for slot `slot`. The default (no profile) is the
/// hardware-verified working profile: direction 3 (sendrecv), PCMA/PT 8,
/// 20 ms, telephone-event mode 1 with PT 101 both ways.
fn media(slot: u32, profile: Option<u8>) -> MediaConfig {
    let port = u16::try_from(slot).unwrap_or(0) * 2;
    let mut m = MediaConfig {
        activation: 0xfffe,
        slot,
        direction: 3,
        codec: 2,
        tx_pt: 8,
        rx_pt: 8,
        tx_ms: 20,
        rx_ms: 20,
        dtmf: 1,
        tx_event_pt: 101,
        rx_event_pt: 101,
        local4: [127, 0, 0, 1],
        remote4: [127, 0, 0, 1],
        local_port: 4000 + port,
        remote_port: 5000 + port,
        control_port: 5001 + port,
        family: AF_INET,
        ..Default::default()
    };
    if let Some(row) = profile {
        m.direction = if row == b'A' { 0 } else { 3 };
        m.dtmf = u32::from(row >= b'C');
        m.tx_event_pt = if row >= b'D' { 101 } else { 0 };
        m.rx_event_pt = if row == b'E' { 101 } else { 0 };
    }
    m
}

fn bridge(s: &mut Session, duration: u32, payload_type: u8) -> CliResult<()> {
    let (mut rtp, mut rtcp) = ([0u32; 2], [0u32; 2]);
    let (mut dd03, mut accepted) = (0u32, 0u32);
    let end = s.now_ms() + u64::from(duration);
    let mut buf = [0u8; FRAME_MAX];
    let mut rc = Ok(());
    println!("loop active: PHONE1 <-> PHONE2");
    'outer: while rc.is_ok() && !stopped() {
        let now = s.now_ms();
        if now >= end {
            break;
        }
        let Some(frame) = s.next_frame(&mut buf, end - now)? else {
            break;
        };
        if frame.len() >= 14 && frame.get(12..14) == Some(&[0xdd, 0x03]) {
            dd03 += 1;
        }
        let frame = frame.to_vec();
        match s.receive(&frame) {
            Ok(Received::Media(m)) => {
                accepted += 1;
                if m.slot > 1 {
                    continue;
                }
                let d = m.datagram;
                // Forward only the requested RTP PT and any RTCP,
                // unchanged except for the destination slot.
                if m.selector == Selector::Rtp
                    && (d.len() < 12
                        || d.first().map(|b| b >> 6) != Some(2)
                        || d.get(1).map(|b| b & 127) != Some(payload_type))
                {
                    continue;
                }
                let data = d.to_vec();
                rc = s.send_media(m.slot ^ 1, m.selector, &data);
                if rc.is_ok() {
                    let c = if m.selector == Selector::Rtp {
                        &mut rtp
                    } else {
                        &mut rtcp
                    };
                    if let Some(x) = c.get_mut(usize::from(m.slot)) {
                        *x += 1;
                    }
                }
            }
            Ok(Received::Event) => {
                while let Some(raw) = s.pop() {
                    if let Ok(ev) = Record::decode(&raw, Direction::FromPeer) {
                        if ev.command == Command::HookOn && ev.word(0) < 2 {
                            break 'outer;
                        }
                    }
                }
            }
            Ok(Received::Control) => {}
            Err(e) if e.is_ignorable() => {}
            Err(e) => rc = Err(errno(e)),
        }
    }
    println!("bridge-window DD03 received={dd03} accepted={accepted}");
    println!(
        "forwarded RTP 0->1={} 1->0={}; RTCP 0->1={} 1->0={}",
        rtp[0], rtp[1], rtcp[0], rtcp[1]
    );
    rc?;
    if rtp[0] > 0 && rtp[1] > 0 {
        Ok(())
    } else {
        Err(libc::ENODATA)
    }
}

fn gain_test(s: &mut Session, slot: u32, tx: i32, rx: i32) -> CliResult<()> {
    // Sample one port only. Never return microphone audio to the earpiece.
    let rc = (|| {
        s.record(
            &Record::words(
                Command::ChannelOpen,
                &[slot, rx as u32, tx as u32, 1, 0, 0, 0, 0, 0, 1, 0],
            )
            .map_err(errno)?,
            "gain-open",
        )?;
        s.record(
            &Record {
                command: Command::RtpActive,
                body: Body::Media(media(slot, None)),
            },
            "gain-active",
        )?;
        // 1 kHz, 8 samples per cycle, peak ~500 counts (-36 dBFS), A-law.
        let cycle = [0xd5, 0xc3, 0xca, 0xc3, 0xd5, 0x43, 0x4a, 0x43];
        let mut pkt = [0u8; 172];
        pkt[0] = 128;
        pkt[1] = 8;
        pkt[8..12].copy_from_slice(&0x18421842u32.to_be_bytes());
        for (i, b) in pkt[12..].iter_mut().enumerate() {
            *b = cycle[i % 8];
        }
        let mut buf = [0u8; FRAME_MAX];
        let end = s.now_ms() + 3500;
        let mut n = 0u16;
        let mut powers = Vec::new();
        let mut peak = 0u32;
        while !stopped() {
            let now = s.now_ms();
            if now >= end {
                break;
            }
            let Some(frame) = s.next_frame(&mut buf, end - now)? else {
                break;
            };
            match s.receive(frame) {
                Ok(Received::Media(m))
                    if u32::from(m.slot) == slot
                        && m.selector == Selector::Rtp
                        && m.datagram.len() == 172
                        && m.datagram[0] == 128
                        && m.datagram[1] & 127 == 8 =>
                {
                    // Coherent 1500 Hz bin, 30 cycles per 160-sample block.
                    let (mut re, mut im) = (0., 0.);
                    for (i, b) in m.datagram[12..].iter().enumerate() {
                        let a = b ^ 85;
                        let seg = (a >> 4) & 7;
                        let l = i32::from(a & 15) * 16;
                        let v = if seg == 0 {
                            l + 8
                        } else {
                            (l + 264) << (seg - 1)
                        };
                        peak = peak.max(v as u32);
                        let x = f64::from(if a & 128 == 0 { -v } else { v });
                        let angle = std::f64::consts::TAU * 0.1875 * i as f64;
                        re += x * angle.cos();
                        im += x * angle.sin();
                    }
                    if n >= 25 {
                        powers.push(re.hypot(im) / 80.);
                    }
                    pkt[2..4].copy_from_slice(&n.to_be_bytes());
                    pkt[4..8].copy_from_slice(&(u32::from(n) * 160).to_be_bytes());
                    s.send_media(slot as u16, Selector::Rtp, &pkt)?;
                    n += 1;
                }
                Ok(Received::Event) => {
                    while let Some(raw) = s.pop() {
                        if let Ok(e) = Record::decode(&raw, Direction::FromPeer) {
                            if e.command == Command::HookOn && e.word(0) == slot {
                                return Err(libc::EINTR);
                            }
                        }
                    }
                }
                Err(e) if !e.is_ignorable() => return Err(errno(e)),
                _ => {}
            }
        }
        if stopped() || powers.len() < 80 {
            return Err(libc::ENODATA);
        }
        powers.sort_by(f64::total_cmp);
        println!(
            "gain line={slot} tx={tx} rx={rx} frames={n} tx1500_dbfs={:.2} input_peak={peak}",
            20. * (powers[powers.len() / 2].max(0.01) / 32768.).log10()
        );
        Ok(())
    })();
    s.abandon();
    let off = s.record(
        &Record::words(Command::RtpDeactive, &[slot]).map_err(errno)?,
        "gain-off",
    );
    s.abandon();
    let close = s.record(
        &Record::words(Command::ChannelClose, &[slot]).map_err(errno)?,
        "gain-close",
    );
    close.and(off).and(rc)
}

fn loop_test(s: &mut Session, duration: u32, profile: Option<u8>, g722: bool) -> CliResult<()> {
    let (mut opened, mut active) = (0u32, 0u32);
    let mut rc = Ok(());
    for slot in 0..2u32 {
        if rc.is_err() || stopped() {
            break;
        }
        // An unacknowledged request may already have taken effect.
        opened |= 1 << slot;
        let open = Record::words(Command::ChannelOpen, &[slot, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0])
            .map_err(errno)?;
        rc = s.record(&open, "channel-open");
        if rc.is_ok() && !stopped() {
            active |= 1 << slot;
            let mut config = media(slot, profile);
            if g722 {
                config.codec = 3;
                config.tx_pt = 9;
                config.rx_pt = 9;
            }
            let r = Record {
                command: Command::RtpActive,
                body: Body::Media(config),
            };
            rc = s.record(&r, "rtp-active");
        }
    }
    if rc.is_ok() {
        rc = if stopped() {
            Err(libc::EINTR)
        } else {
            bridge(s, duration, if g722 { 9 } else { 8 })
        };
    }
    for slot in (0..2u32).rev() {
        for (mask, cmd, name) in [
            (active, Command::RtpDeactive, "rtp-off"),
            (opened, Command::ChannelClose, "channel-close"),
        ] {
            if mask & (1 << slot) == 0 {
                continue;
            }
            s.abandon();
            if let Err(e) = s.record(&Record::words(cmd, &[slot]).map_err(errno)?, name) {
                eprintln!(
                    "slot {slot} cleanup cmd={}: {}",
                    cmd.id(),
                    std::io::Error::from_raw_os_error(e)
                );
                if rc.is_ok() {
                    rc = Err(e);
                }
            }
        }
    }
    rc
}

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn install_stop_handlers() -> bool {
    [libc::SIGINT, libc::SIGTERM, libc::SIGHUP]
        .iter()
        .all(|&sig| {
            // SAFETY: the handler only stores to an atomic (async-signal-safe).
            let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            unsafe { libc::signal(sig, handler) != libc::SIG_ERR }
        })
}

fn init_logging() {
    let level = match std::env::var("RUST_LOG")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "error" => nanologger::LogLevel::Error,
        "debug" | "trace" => nanologger::LogLevel::Debug,
        "info" => nanologger::LogLevel::Info,
        _ => nanologger::LogLevel::Warn,
    };
    nanologger::LoggerBuilder::new().level(level).init().ok();
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    let interruptible = matches!(
        cli.cmd,
        Cmd::Ring { .. } | Cmd::Tone { .. } | Cmd::Loop { .. } | Cmd::Gain { .. }
    );
    if matches!(cli.cmd, Cmd::Loop { .. }) && cli.locale.is_some() {
        eprintln!("loop does not accept --locale");
        return ExitCode::from(2);
    }
    if interruptible && !install_stop_handlers() {
        return ExitCode::FAILURE;
    }
    let after = match cli.cmd {
        Cmd::Init => After::Ready { watch: false },
        Cmd::Events => After::Ready { watch: true },
        Cmd::Keypad => After::Keypad,
        Cmd::Monitor => After::Monitor,
        Cmd::Gain { line, tx, rx } => After::Gain { line, tx, rx },
        Cmd::Led { line, op } => After::Led { line, op },
        Cmd::Ring {
            line,
            duration_ms,
            cid,
        } => After::Ring {
            line,
            duration: duration_ms,
            cid,
        },
        Cmd::Tone {
            line,
            sel,
            duration_ms,
        } => After::Tone {
            line,
            sel,
            duration: duration_ms,
        },
        Cmd::Loop {
            duration_ms,
            profile,
            g722,
            ..
        } => After::Loop {
            duration: duration_ms,
            profile: profile.and_then(|p| p.bytes().next()),
            g722,
        },
    };
    // Ring only reprograms the profile when --locale is given explicitly.
    let locale = cli.locale;
    let rc = port::open(&cli.device)
        .map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))
        .and_then(Session::new)
        .and_then(|mut s| run(&mut s, locale, cli.timeout_ms, after));
    match rc {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}: {}", cli.device, std::io::Error::from_raw_os_error(e));
            ExitCode::FAILURE
        }
    }
}
