// SPDX-License-Identifier: GPL-2.0-or-later
//! Frame and media decoders on arbitrary bytes; re-encode must round-trip.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rbctl_voip_proto::wire;

fuzz_target!(|data: &[u8]| {
    if let Ok(f) = wire::decode_frame(data) {
        let mut out = [0u8; wire::FRAME_MAX];
        let n = wire::encode_frame(&mut out, &f.header, f.body).expect("re-encode");
        let again = wire::decode_frame(&out[..n]).expect("decode re-encoded");
        assert_eq!(again.header, f.header);
        assert_eq!(again.body, f.body);
    }
    let _ = wire::decode_media(data);
});
