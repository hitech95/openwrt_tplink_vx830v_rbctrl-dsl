// Build script — compiles proto/rbctl.proto when the `protobuf` feature is on.
// The OEM protocol core (checksum, frame, pack/unpack) needs no build script.

#[cfg(feature = "protobuf")]
fn main() -> std::io::Result<()> {
    let file_descriptor_set = protox::compile(
        ["proto/rbctl.proto"],
        ["proto"],
    )
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    prost_build::Config::new()
        .compile_fds(file_descriptor_set)?;

    // rbctl.proto imports the section files — watch the whole directory,
    // or edits to common.proto & co. silently reuse stale generated code.
    println!("cargo:rerun-if-changed=proto");
    Ok(())
}

#[cfg(not(feature = "protobuf"))]
fn main() {}
