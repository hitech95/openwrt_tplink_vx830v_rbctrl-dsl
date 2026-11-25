// SPDX-License-Identifier: GPL-2.0-or-later
//! Drive the real binary through every M2 scenario against the Python peer
//! emulator (`tests/peer-emu/cli.py`) over a Unix datagram pair.
//! Requires the `fixture` feature: `cargo test -p rbctl_voicectl --features fixture`.
#![cfg(feature = "fixture")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

#[test]
fn cli_scenarios_against_peer_emulator() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/peer-emu");
    let out = Command::new("python3")
        .args(["-B", "cli.py", env!("CARGO_BIN_EXE_rbctl-voicectl")])
        .current_dir(dir)
        .output()
        .expect("python3");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout.matches(" passed").count(), 17, "{stdout}");
}
