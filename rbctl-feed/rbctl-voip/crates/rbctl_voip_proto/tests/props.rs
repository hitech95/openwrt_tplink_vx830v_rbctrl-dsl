// SPDX-License-Identifier: GPL-2.0-or-later
//! T2.14 property tests: decoders never panic on arbitrary input, and
//! encode/decode round-trips are exact.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use proptest::prelude::*;
use rbctl_voip_proto::dsp::{Command, Direction, Record, RECORD_LEN};
use rbctl_voip_proto::link::{Link, Transport};
use rbctl_voip_proto::wire::{self, Header, Selector, FRAME_MAX};
use rbctl_voip_proto::Result;

struct Sink;
impl Transport for Sink {
    fn send(&mut self, _: &[u8]) -> Result<()> {
        Ok(())
    }
}

fn header() -> impl Strategy<Value = Header> {
    (
        any::<[u8; 6]>(),
        any::<[u8; 6]>(),
        0xdd01u16..=0xdd03,
        prop_oneof![Just(0x10u8), Just(0x11u8)],
        any::<u8>(),
        any::<u32>(),
    )
        .prop_map(|(dst, src, ethertype, marker, subtype, seq)| Header {
            dst,
            src,
            ethertype,
            marker,
            subtype,
            seq,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn frame_round_trip(h in header(), body in proptest::collection::vec(any::<u8>(), 0..1490)) {
        let mut f = [0u8; FRAME_MAX];
        let n = wire::encode_frame(&mut f, &h, &body).unwrap();
        let d = wire::decode_frame(&f[..n]).unwrap();
        prop_assert_eq!(d.header, h);
        prop_assert_eq!(d.body, &body[..]);
    }

    #[test]
    fn decoders_total(bytes in proptest::collection::vec(any::<u8>(), 0..1600)) {
        let _ = wire::decode_frame(&bytes);
        let _ = wire::decode_media(&bytes);
        for dir in [Direction::ToPeer, Direction::FromPeer] {
            let _ = Record::decode(&bytes, dir);
        }
        let mut l = Link::new([2, 0, 0, 0, 0, 1], 0);
        let _ = l.receive(&mut Sink, &bytes);
    }

    #[test]
    fn record_normalization_is_stable(idx in 0usize..34, body in any::<[u8; 128]>(), rx in any::<bool>()) {
        let cmd = Command::ALL[idx];
        let dir = if rx { Direction::FromPeer } else { Direction::ToPeer };
        let mut raw = [0u8; RECORD_LEN];
        raw[..4].copy_from_slice(&cmd.id().to_be_bytes());
        raw[4..].copy_from_slice(&body);
        if let Ok(r) = Record::decode(&raw, dir) {
            // Invalid media slot/family is rejected on encode, not decode.
            if let Ok(once) = r.encode(dir) {
                let twice = Record::decode(&once, dir).unwrap().encode(dir).unwrap();
                prop_assert_eq!(once, twice);
                prop_assert!(once[4 + cmd.body_len(dir)..].iter().all(|&b| b == 0));
            }
        }
    }

    #[test]
    fn media_round_trip(slot in 0u16..4, rtcp in any::<bool>(), data in proptest::collection::vec(any::<u8>(), 1..1486)) {
        let sel = if rtcp { Selector::Rtcp } else { Selector::Rtp };
        let mut b = [0u8; FRAME_MAX];
        let n = wire::encode_media(&mut b, slot, sel, &data).unwrap();
        let m = wire::decode_media(&b[..n]).unwrap();
        prop_assert_eq!((m.slot, m.selector, m.datagram), (slot, sel, &data[..]));
    }
}
