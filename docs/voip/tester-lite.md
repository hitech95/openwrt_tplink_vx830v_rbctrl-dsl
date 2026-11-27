# VX830v voice lite tester build

Lite r2 has the same features as r1. The AArch64 module now exports only
baresip's `exports` descriptor; the linker discards unused sections of the
embedded archive. Its stripped file size fell from 1,256,152 to 399,368 bytes.

Use this version for a private LAN PBX that accepts SIP/UDP or SIP/TCP and
plain RTP using PCMA or PCMU. It includes both FXS lines and MWI. The full
tester bundle remains the choice for SIP/TLS, SRTP or G.722. There is no
automatic downgrade: encrypted transports, encrypted media and G.722 account
settings are rejected by the lite renderer. Do not install both editions.

Lite uses the router's existing `libmbedcrypto.so.16` (OpenWrt `libmbedtls21`)
for the libre digest routines. This does **not** make the SIP or RTP traffic
encrypted. Upstream libre 3.16 only enables SIP/TLS with OpenSSL; it also
selects an AES stub when built without OpenSSL on this platform. The lite
build uses Linux `getrandom` rather than libre's non-OpenSSL libc PRNG fallback.

The single APK contains private baresip/libre libraries and just `g711.so`,
`mwi.so` and `rbctl_voip.so`. No extra standalone baresip service, WAV/PNG
assets, video plugins, traversal plugins, SpanDSP, TIFF, JPEG or OpenSSL.
The full build didn't have video plugins either; its transitive dependencies
were the main size cost.

Verify the distributor's archive checksum on the workstation, extract the
runtime tarball in `/tmp` on a matching VX830v, then `sh tester-lite.sh check`.
This simulates the offline transaction. `sh tester-lite.sh install` requires
4 MiB free on the overlay. The package key is used for this transaction only.
Configure the account in `/etc/config/voip`, then `/etc/init.d/voip reload`.
Back up that config before switching editions. Uninstall with
`/etc/init.d/voip stop` then `apk del rbctl-voip-lite`.

Scope: AArch64 Cortex-A53, musl, libubox ABI 20260721, libubus ABI 20260628,
mbedTLS 3.6.7 on the bench. The physical router passed the single-package
offline install in tmpfs and loopback startup with G.711 and the voice module;
no permanent install, encrypted-call claim, or live handset call in this test.
