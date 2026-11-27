// SPDX-License-Identifier: GPL-2.0-or-later
//! Like `frame`, but repairs the declared length and CRC first so mutations
//! reach the post-checksum paths (header fields, padding, media prefix,
//! link dispatch) instead of stopping at the CRC check.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rbctl_voip_proto::link::{Link, Transport};
use rbctl_voip_proto::{wire, Result};

struct Sink;
impl Transport for Sink {
    fn send(&mut self, _: &[u8]) -> Result<()> {
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < wire::HEADER_LEN || data.len() > wire::FRAME_MAX {
        return;
    }
    let mut f = data.to_vec();
    let body = f.len() - wire::HEADER_LEN;
    let declared = usize::from(u16::from_be_bytes([f[20], f[21]])).min(body);
    f[20..22].copy_from_slice(&(declared as u16).to_be_bytes());
    f[22..24].fill(0);
    let mut covered = f[14..24].to_vec();
    covered.extend_from_slice(&f[24..24 + declared]);
    let crc = wire::crc16(&covered);
    f[22..24].copy_from_slice(&crc.to_be_bytes());
    if let Ok(d) = wire::decode_frame(&f) {
        let _ = wire::decode_media(d.body);
    }
    let mut l = Link::new([2, 0, 0, 0, 0, 1], 0);
    let _ = l.receive(&mut Sink, &f);
});
