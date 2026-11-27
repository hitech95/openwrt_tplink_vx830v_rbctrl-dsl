# VX830v full voice tester bundle (0.1.0-r4)

Install target: **`rbctl-voip-full`** (dependency-only metapackage). It selects
`rbctl-voip` (managed service), which always pulls in `rbctl-voip-led` (FXS hotplug helper),
even when installed directly. `rbctl-voicectl` is available in the offline repository
for explicit installation when debugging; neither install target requires it.
The service depends on **`baresip-mod-rbctl-voip`**,
the package owning `/usr/lib/baresip/modules/rbctl_voip.so`. The baresip config
still loads `rbctl_voip.so`. The full service and private lite service are alternatives.

For the **TP-Link VX830v**, stock Econet voice board, `mediatek/filogic`,
`aarch64_cortex-a53`, APK-based OpenWrt. Bench firmware:
`r35484+502-11a859cca7` (`libubus20260628`, `libubox20260721`). Other
firmware ABIs need their own build. This is a hardware-targeted preview,
not an official OpenWrt release.

1. Check the **archive SHA-256 supplied separately by the distributor** on
   your workstation. Copy the runtime tarball to `/tmp` on the router and
   extract it there (not into flash). The companion source archive stays on
   the workstation. Do not copy either archive into `/root` on a small overlay.
2. Inside the extracted directory run `sh tester-apk.sh check`. This performs
   target/ABI checks, verifies the manifest and simulates the whole install.
   It uses the bundled repositories offline: four unchanged OpenWrt APKs via
   their official signed indexes, and twelve locally signed APKs. The test key
   is used only for this transaction, not installed as a system trust anchor.
3. If compatible, with **at least 12 MiB free on `/overlay`**, run
   `sh tester-apk.sh install`. Default accounts are disabled; no credentials
   are supplied. Do not run another board owner at the same time. The bench
   with 7.7 MiB free was tested in an isolated `/tmp` install root instead;
   for small-overlay testers use extroot or an image with the packages built in.
4. Set `/etc/config/voip` locally, including account/domain and mnemonic
   country (default `IT`), then `/etc/init.d/voip reload`. Check
   `ubus call voip status` and `ubus call voip lines`. Report firmware revision,
   install output and anonymized line states; omit accounts, passwords and numbers.
5. To remove the bundle: `/etc/init.d/voip stop`, then `apk del rbctl-voip-full`.
   Packages independently installed as explicit world entries are retained.
   Keep your configuration backup. No repository URL is added permanently.

**Coverage:** for the original r2 bundle, physical VX830v: offline 12-package solver, isolated APK install
with scripts disabled, startup of the packaged baresip and loading of voice,
G.711/G.722, resampler and SRTP modules. This run used loopback and no real
board or account. Permanent install hooks, reboot and clean-image M5 acceptance
are still pending. Conversational echo is unverified; don't use a dual echo
loop or enable call-waiting CID for first-call testing.
