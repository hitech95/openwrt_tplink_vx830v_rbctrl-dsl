# M5 handoff: installing the voice service on OpenWrt

**First deliverable: T5.1.** Build an installable `rbctl-voip` in a clean,
matching SDK and install it on a clean image with the `voip` service enabled.
The existing AArch64 CLI and module builds establish compilation, not package
discovery, dependency resolution, installability or service startup.
[Acceptance criteria: T5.1–T5.7](../../plans/voip-baresip-plan.md#L523).

## Before the first SDK build

- Match the SDK and clean image by revision, target and package format. The
  local OpenWrt `.config` selects `mediatek/filogic`, `aarch64_cortex-a53`,
  `CONFIG_USE_APK=y` **and** `CONFIG_PACKAGE_opkg=y`. Including the opkg binary
  does not turn an APK build into an IPK build. Use an opkg/IPK SDK and image
  for T5.1 as written, or agree on an APK-based revision of the gate first.
- Supply the packages and telephony feeds plus the complete local `rbctl-feed`.
  The [package recipe](../../rbctl-feed/rbctl-voip/Makefile) needs `rust/host`
  and the sibling `rbctl-dsl/crates/af_packet`; copying `rbctl-voip` alone
  omits a locked build input. Verify the feed index can discover the packages
  before compiling (Q7: prior local group-ID collision).
- The inspected telephony feed provides baresip **3.16.0**, development files,
  `account.so` in the base package, and separate codec/feature modules. Match
  package dependencies to the generated config (e.g. `baresip-mod-g711` for
  PCMA). The base package already ships `/etc/init.d/baresip`: check on the
  clean image that only the `voip` instance opens the board and SIP listener.

## Implementation and evidence queue

| Gate | Work to do | Evidence to retain |
|---|---|---|
| T5.1 | Extend the recipe, which currently emits the diagnostic CLI and optional LED package and stages the Rust FFI. Build the production module against target baresip/libre and that archive, install it in `/usr/lib/baresip/modules`, and add the main `rbctl-voip` package, `/etc/config/voip` and `/etc/init.d/voip`. | SDK/image and feed revisions, package file list and dependencies, clean install and service-enabled result. No OEM userspace files. |
| T5.2–T5.3 | Render UCI into private `/var/run/voip` (0700). Distinguish account/line reload from a board-setting change. Preserve the signed per-line gain mapping and zero defaults. | Account change registers anew without a board init; locale change causes restart and board init. Record transitions without credentials. |
| T5.4–T5.5 | Implement the `rbctl_voip_ubus` module and §8.3 methods/events on the libre loop. | Redacted status/events in order for a call; `dial` and `hangup` operate on the selected line. |
| T5.6–T5.7 | Verify procd startup with the board VLAN available, then exercise the rpcd ACL. | After reboot: one service, registered, both lines ready. Unauthenticated HTTP RPC cannot dial. |

M4 carry-over stays tracked in the [bench queue](m4-bench.md): T4.10 tone
sign-off, T4.14 caller ID (parked), and T4.20 two-line drift/echo. No M5
acceptance result depends on treating any of those as passed.
