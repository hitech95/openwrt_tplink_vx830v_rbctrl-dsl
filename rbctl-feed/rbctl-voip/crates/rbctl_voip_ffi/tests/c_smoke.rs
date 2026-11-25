// SPDX-License-Identifier: GPL-2.0-or-later
//! T2.15: build `tests/ffi/smoke.c` against the generated header and the
//! staticlib with ASan/UBSan and run it; and check the committed header is
//! what `cbindgen` generates today (skipped when cbindgen is not installed).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `target/<profile>` containing the staticlib built for this test run.
fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().and_then(Path::parent).unwrap().to_path_buf()
}

/// The staticlib from this build: `target/<profile>/librbctl_voip_ffi.a`
/// after `cargo build`, or the newest hashed copy in `deps/` under
/// `cargo test`.
fn staticlib() -> PathBuf {
    let dir = profile_dir();
    let plain = dir.join("librbctl_voip_ffi.a");
    let hashed = std::fs::read_dir(dir.join("deps"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("librbctl_voip_ffi-") && n.ends_with(".a")
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path());
    hashed.filter(|_| !plain.is_file()).unwrap_or(plain)
}

#[test]
fn c_smoke_against_staticlib() {
    let lib = staticlib();
    assert!(lib.is_file(), "staticlib missing: {}", lib.display());
    let out = std::env::temp_dir().join(format!("rbctl-voip-smoke-{}", std::process::id()));
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let status = Command::new(&cc)
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ])
        .arg("-I")
        .arg(manifest().join("include"))
        .arg(manifest().join("../../tests/ffi/smoke.c"))
        .arg(&lib)
        .args(["-lpthread", "-ldl", "-lm", "-o"])
        .arg(&out)
        .status()
        .expect("C compiler");
    assert!(status.success(), "compiling smoke.c failed");
    let run = Command::new(&out).output().expect("run smoke");
    let _ = std::fs::remove_file(&out);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(stdout.contains("T2.15: C smoke test passed"));
}

#[test]
fn header_is_current() {
    let Ok(out) = Command::new("cbindgen")
        .current_dir(manifest())
        .args(["--config", "cbindgen.toml", "--quiet"])
        .output()
    else {
        eprintln!("SKIP header_is_current: cbindgen not installed");
        return;
    };
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let committed = std::fs::read(manifest().join("include/rbctl_voip.h")).unwrap();
    assert!(
        out.stdout == committed,
        "include/rbctl_voip.h is stale: run `cbindgen --config cbindgen.toml --output include/rbctl_voip.h` in crates/rbctl_voip_ffi"
    );
}
