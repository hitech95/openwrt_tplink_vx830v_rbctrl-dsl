// SPDX-License-Identifier: GPL-2.0-or-later
//! Link state machine on a stream of arbitrary frames (length-prefixed
//! chunks), interleaved with requests, ticks and pops.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rbctl_voip_proto::link::{Link, Transport};
use rbctl_voip_proto::wire::{SUB_ATTACH, SUB_RECORD};
use rbctl_voip_proto::Result;

struct Sink;
impl Transport for Sink {
    fn send(&mut self, frame: &[u8]) -> Result<()> {
        assert!(frame.len() >= 24 && frame.len() <= 1514);
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    let mut l = Link::new([2, 0, 0, 0, 0, 1], 0);
    let mut now = 0u64;
    let mut rest = data;
    while let Some((&op, tail)) = rest.split_first() {
        let n = usize::from(op & 0x7f).min(tail.len());
        let (chunk, next) = tail.split_at(n);
        rest = next;
        match op >> 7 {
            0 => {
                let _ = l.receive(&mut Sink, chunk);
            }
            _ => {
                now += u64::from(op) * 10;
                let _ = l.tick(&mut Sink, now);
                let _ = l.request(
                    &mut Sink,
                    if op & 1 == 1 { SUB_ATTACH } else { SUB_RECORD },
                    None,
                    now,
                );
                while l.pop().is_some() {}
            }
        }
    }
});
