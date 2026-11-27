// SPDX-License-Identifier: GPL-2.0-or-later
//! DSP record decoder; normalized encodings must be stable.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rbctl_voip_proto::dsp::{Direction, Record};

fuzz_target!(|data: &[u8]| {
    for dir in [Direction::ToPeer, Direction::FromPeer] {
        if let Ok(r) = Record::decode(data, dir) {
            if let Ok(once) = r.encode(dir) {
                let twice = Record::decode(&once, dir)
                    .expect("decode")
                    .encode(dir)
                    .expect("encode");
                assert_eq!(once, twice);
            }
        }
    }
});
