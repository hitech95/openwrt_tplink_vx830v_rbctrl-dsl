// SPDX-License-Identifier: GPL-2.0-or-later
//! T2.13 vector emitter for `tests/qemu/diff.py`: writes, in order, the
//! records and frames the OEM AArch64 routines produce for the same inputs
//! (CID+ring per line, stop-ring ×2, channel default ×4, open/activate/
//! deactivate per line, DD01 init and record frames, one DD03 frame).
//!
//! stdin: the two OEM final ring words (BE32 each). That word is stack
//! residue in the OEM client; it is replayed, not generated.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rbctl_voip_proto::dsp::{Body, Cid, Command, Direction, MediaConfig, Record, AF_INET};
use rbctl_voip_proto::link::{Link, Transport};
use rbctl_voip_proto::wire::{
    self, Header, Selector, BROADCAST, ETH_CONTROL, MARKER_HOST, SUB_INIT, SUB_RECORD,
};
use std::io::{Read, Write};

struct Out(std::io::Stdout);
impl Transport for Out {
    fn send(&mut self, frame: &[u8]) -> rbctl_voip_proto::Result<()> {
        self.0.write_all(frame).unwrap();
        Ok(())
    }
}

fn text<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut a = [0u8; N];
    a[..s.len()].copy_from_slice(s);
    a
}

fn main() {
    let mut out = Out(std::io::stdout());
    let mut rec = |r: Record| {
        out.0
            .write_all(&r.encode(Direction::ToPeer).unwrap())
            .unwrap()
    };
    let mut opaque = [0u8; 8];
    std::io::stdin().read_exact(&mut opaque).unwrap();
    for line in 0..2u32 {
        let cid = Cid {
            line,
            select: 1 << line,
            date: text(b"01020304"),
            name: text(b"Fixture"),
            number: text(b"5550100"),
            ..Default::default()
        };
        rec(Record {
            command: Command::CidPlay,
            body: Body::Cid(cid),
        });
        let w = u32::from_be_bytes(opaque[line as usize * 4..][..4].try_into().unwrap());
        rec(Record::words(Command::RingStart, &[line, 0, 0, u32::MAX, 1 << line, w]).unwrap());
    }
    for i in 0..2 {
        rec(Record::words(Command::RingStop, &[i]).unwrap());
    }
    for i in 0..4 {
        rec(Record::words(Command::ChannelDefault, &[i]).unwrap());
    }
    for line in 0..2u32 {
        rec(Record::words(Command::ChannelOpen, &[line, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0]).unwrap());
        let p = line as u16 * 2;
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
            local_port: 4000 + p,
            remote_port: 5000 + p,
            control_port: 5001 + p,
            family: AF_INET,
            ..Default::default()
        };
        rec(Record {
            command: Command::RtpActive,
            body: Body::Media(m),
        });
        rec(Record::words(Command::RtpDeactive, &[line]).unwrap());
    }
    let mac = [0, 17, 34, 51, 68, 85];
    let mut link = Link::new(mac, 0);
    link.request(&mut out, SUB_INIT, None, 0).unwrap();
    // The isolated OEM builder sends sequence 2 without an init reply.
    let stop = Record::words(Command::RingStop, &[1])
        .unwrap()
        .encode(Direction::ToPeer)
        .unwrap();
    let h = Header {
        dst: BROADCAST,
        src: mac,
        ethertype: ETH_CONTROL,
        marker: MARKER_HOST,
        subtype: SUB_RECORD,
        seq: 2,
    };
    let mut f = [0u8; 156];
    assert_eq!(wire::encode_frame(&mut f, &h, &stop).unwrap(), 156);
    out.0.write_all(&f).unwrap();
    let mut rtp = [0xd5u8; 172];
    rtp[..12].copy_from_slice(&[128, 8, 0, 1, 0, 0, 0, 160, 1, 2, 3, 4]);
    link.send_media(&mut out, 1, Selector::Rtp, &rtp).unwrap();
    out.0.flush().unwrap();
}
