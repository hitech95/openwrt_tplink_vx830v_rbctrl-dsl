// SPDX-License-Identifier: GPL-2.0-or-later
//! T2.12: byte-for-byte parity with the C prototype.
//!
//! `tests/vectors/c_prototype.txt` was produced by `tests/vectors/gen_c.c`
//! linked against the C library (M1/M1.1). Every case is rebuilt here with
//! the Rust API from the same inputs and must match exactly. The file stays
//! committed after the C sources are removed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use rbctl_voip_proto::dsp::{
    Body, Cid, Command, Direction, MediaConfig, Record, Tone, AF_INET, AF_INET6,
};
use rbctl_voip_proto::link::{Link, Received, Transport};
use rbctl_voip_proto::wire::{
    self, Header, Selector, ETH_CONTROL, ETH_EVENT, ETH_MEDIA, MARKER_PEER,
};
use rbctl_voip_proto::{Error, Result};
use std::collections::BTreeMap;

const HOST: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn vectors() -> BTreeMap<String, Vec<u8>> {
    let text = include_str!("../../../tests/vectors/c_prototype.txt");
    text.lines()
        .map(|l| {
            let (name, hex) = l.split_once(' ').expect("name hex");
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            (name.to_owned(), bytes)
        })
        .collect()
}

struct Checker {
    want: BTreeMap<String, Vec<u8>>,
    seen: usize,
}

impl Checker {
    fn check(&mut self, name: &str, got: &[u8]) {
        let want = self
            .want
            .get(name)
            .unwrap_or_else(|| panic!("missing vector {name}"));
        if let Some(i) = want.iter().zip(got).position(|(a, b)| a != b) {
            panic!("{name}: first mismatch at byte {i}");
        }
        assert_eq!(want.len(), got.len(), "{name}: length");
        self.seen += 1;
    }
}

fn text<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut a = [0u8; N];
    a[..s.len()].copy_from_slice(s);
    a
}

#[derive(Default)]
struct Capture(Vec<u8>);
impl Transport for Capture {
    fn send(&mut self, frame: &[u8]) -> Result<()> {
        self.0 = frame.to_vec();
        Ok(())
    }
}

fn inject(
    l: &mut Link,
    tx: &mut Capture,
    ethertype: u16,
    sub: u8,
    seq: u32,
    body: &[u8],
) -> Result<()> {
    let h = Header {
        dst: HOST,
        src: PEER,
        ethertype,
        marker: MARKER_PEER,
        subtype: sub,
        seq,
    };
    let mut f = [0u8; wire::FRAME_MAX];
    let n = wire::encode_frame(&mut f, &h, body)?;
    l.receive(tx, &f[..n]).map(|_| ())
}

#[test]
fn golden_parity_with_c_prototype() {
    let mut c = Checker {
        want: vectors(),
        seen: 0,
    };
    let tx = Direction::ToPeer;

    // 1. Semantic records.
    for line in 0..2u32 {
        let cid = Cid {
            line,
            select: 1 << line,
            date: text(b"01020304"),
            name: text(b"Fixture"),
            number: text(b"5550100"),
            ..Default::default()
        };
        let r = Record {
            command: Command::CidPlay,
            body: Body::Cid(cid),
        };
        c.check(&format!("cid_line{line}"), &r.encode(tx).unwrap());
        let ring =
            Record::words(Command::RingStart, &[line, 0, 0, u32::MAX, 1 << line, 0]).unwrap();
        c.check(&format!("ring_line{line}"), &ring.encode(tx).unwrap());
        let open =
            Record::words(Command::ChannelOpen, &[line, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0]).unwrap();
        c.check(&format!("open_slot{line}"), &open.encode(tx).unwrap());
        let port = u16::try_from(line).unwrap() * 2;
        let m = MediaConfig {
            activation: 0xfffe,
            slot: line,
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
        let r = Record {
            command: Command::RtpActive,
            body: Body::Media(m),
        };
        c.check(&format!("rtp_active_slot{line}"), &r.encode(tx).unwrap());
    }
    let mut local6 = [0u8; 16];
    local6[..4].copy_from_slice(&[0x20, 0x01, 0x0d, 0xb8]);
    local6[15] = 1;
    let mut remote6 = [0u8; 16];
    remote6[..2].copy_from_slice(&[0xfe, 0x80]);
    remote6[15] = 2;
    let v6 = MediaConfig {
        slot: 3,
        direction: 2,
        codec: 3,
        tos: 0xb8,
        control_tos: 0x88,
        local6,
        remote6,
        family: AF_INET6,
        ..Default::default()
    };
    c.check(
        "rtp_update_v6",
        &Record {
            command: Command::RtpUpdate,
            body: Body::Media(v6),
        }
        .encode(tx)
        .unwrap(),
    );
    let tone = |args: [u32; 4]| Record {
        command: Command::TonePlay,
        body: Body::Tone(Tone {
            args,
            ..Default::default()
        }),
    };
    c.check("tone_dial_line0", &tone([0, 4, 0, 0]).encode(tx).unwrap());
    c.check("tone_stop_line1", &tone([1, 0, 0, 0]).encode(tx).unwrap());
    let t = Tone {
        args: [1, 31, u32::MAX, 1],
        text: [text(b"Name"), text(b"123")],
        last: 9,
    };
    c.check(
        "tone_text",
        &Record {
            command: Command::TonePlay,
            body: Body::Tone(t),
        }
        .encode(tx)
        .unwrap(),
    );
    c.check(
        "slic_locale_10",
        &Record::words(Command::SlicLocale, &[10])
            .unwrap()
            .encode(tx)
            .unwrap(),
    );
    c.check(
        "dsp_start",
        &Record::words(Command::DspStart, &[1])
            .unwrap()
            .encode(tx)
            .unwrap(),
    );
    c.check(
        "mgmt70",
        &Record::words(Command::Mgmt70, &[1, 2, 3, 4])
            .unwrap()
            .encode(tx)
            .unwrap(),
    );
    let dtmf = Record::words(Command::DtmfEnd, &[1, u32::from(b'#'), 1]).unwrap();
    c.check("dtmf_end_tx", &dtmf.encode(tx).unwrap());
    c.check("dtmf_end_rx", &dtmf.encode(Direction::FromPeer).unwrap());

    // 2. Pseudo-random bodies for every command: decode -> encode normalizes
    //    reserved bytes exactly like the C codec.
    for (i, cmd) in Command::ALL.iter().enumerate() {
        for (ev, dir) in [(0u32, Direction::ToPeer), (1, Direction::FromPeer)] {
            let tag = if ev == 1 { "rx" } else { "tx" };
            let mut state = cmd.id() * 2 + ev + 1;
            let mut input = [0u8; 132];
            input[3] = u8::try_from(cmd.id()).unwrap();
            for b in input.iter_mut().skip(4) {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                *b = (state >> 16) as u8;
            }
            if matches!(cmd, Command::RtpActive | Command::RtpUpdate) {
                input[20..24].fill(0);
                input[23] = (i & 3) as u8;
                input[120..124].fill(0);
                input[123] = if ev == 1 { 2 } else { 10 };
            }
            c.check(&format!("in_{}_{tag}", cmd.id()), &input);
            let r = Record::decode(&input, dir).unwrap();
            c.check(&format!("norm_{}_{tag}", cmd.id()), &r.encode(dir).unwrap());
        }
    }

    // 3. Link transactions.
    let mut cap = Capture::default();
    let mut l = Link::new(HOST, 0x7fff_fffe);
    l.request(&mut cap, wire::SUB_INIT, None, 0).unwrap();
    c.check("req_init", &cap.0);
    l.tick(&mut cap, 1000).unwrap();
    c.check("req_init_retry", &cap.0);
    let seq = l.seq();
    inject(&mut l, &mut cap, ETH_CONTROL, wire::SUB_INIT, seq, &[0; 4]).unwrap();
    l.request(&mut cap, wire::SUB_ATTACH, None, 0).unwrap();
    c.check("req_attach_unicast", &cap.0);
    let seq = l.seq();
    inject(
        &mut l,
        &mut cap,
        ETH_CONTROL,
        wire::SUB_ATTACH,
        seq,
        &[0; 4],
    )
    .unwrap();
    let stop = Record::words(Command::RingStop, &[1])
        .unwrap()
        .encode(tx)
        .unwrap();
    l.request(&mut cap, wire::SUB_RECORD, Some(&stop), 0)
        .unwrap();
    c.check("req_record", &cap.0);
    let seq = l.seq();
    assert_eq!(
        inject(
            &mut l,
            &mut cap,
            ETH_CONTROL,
            wire::SUB_RECORD,
            seq,
            &[0, 0, 0, 7]
        ),
        Err(Error::PeerStatus(7))
    );
    let mut ev = [0u8; 132];
    ev[3] = 18;
    ev[7] = 1;
    inject(&mut l, &mut cap, ETH_EVENT, wire::SUB_EVENT, 9, &ev).unwrap();
    c.check("ack_ok_seq9", &cap.0);
    inject(&mut l, &mut cap, ETH_EVENT, wire::SUB_EVENT, 9, &ev).unwrap();
    c.check("ack_dup_seq9", &cap.0);
    inject(&mut l, &mut cap, ETH_EVENT, wire::SUB_EVENT, 10, &ev[..131]).unwrap();
    c.check("ack_short_seq10", &cap.0);
    for s in 11..25 {
        inject(&mut l, &mut cap, ETH_EVENT, wire::SUB_EVENT, s, &ev).unwrap();
    }
    inject(&mut l, &mut cap, ETH_EVENT, wire::SUB_EVENT, 25, &ev).unwrap();
    c.check("ack_full_seq25", &cap.0);
    let mut rtp = [0xd5u8; 172];
    rtp[..12].copy_from_slice(&[128, 8, 0, 1, 0, 0, 0, 160, 1, 2, 3, 4]);
    l.send_media(&mut cap, 1, Selector::Rtp, &rtp).unwrap();
    c.check("media_tx_broadcast", &cap.0);
    let mbody = [0, 2, 0, 4, 0x80, 0xc9, 0, 1];
    let h = Header {
        dst: HOST,
        src: PEER,
        ethertype: ETH_MEDIA,
        marker: MARKER_PEER,
        subtype: wire::SUB_MEDIA,
        seq: 0,
    };
    let mut f = [0u8; 64];
    let n = wire::encode_frame(&mut f, &h, &mbody).unwrap();
    assert!(matches!(
        l.receive(&mut cap, &f[..n]),
        Ok(Received::Media(_))
    ));
    l.send_media(&mut cap, 2, Selector::Rtcp, &mbody[4..])
        .unwrap();
    c.check("media_tx_learned", &cap.0);

    assert_eq!(c.seen, c.want.len(), "every golden vector must be checked");
    println!("T2.12: {} golden vectors matched", c.seen);
}
