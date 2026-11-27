# M5 handoff: installing the voice service on OpenWrt

> **Follow-ups:** the 2026-11-27 review findings and the UCI account/line split are tracked as
> [plan §9 M5.1](../../plans/voip-baresip-plan.md#m51--m5-review-follow-ups-and-uci-schema-split-before-m5-closes)
> (work items A1–A7, B1–B6; tests T5.8–T5.13). M5 closes only after M5.1.

**First deliverable: T5.1.** Build an installable `rbctl-voip` in a clean,
matching SDK and install it on a clean image with the `voip` service enabled.
The existing AArch64 CLI and module builds establish compilation, not package
discovery, dependency resolution, installability or service startup.
[Acceptance criteria: T5.1–T5.7](../../plans/voip-baresip-plan.md#L523).

## Before the first SDK build

- Match the SDK and clean image by revision and target. Let OpenWrt's packager
  choose the native format; use the matching image's installer. The current
  `mediatek/filogic` configuration produces APK. T5.1 does not require IPK
  or a second build in another format.
- Supply the packages and telephony feeds plus the complete local `rbctl-feed`.
  The [package recipe](../../rbctl-feed/rbctl-voip/Makefile) needs `rust/host`
  and the sibling `rbctl-dsl/crates/af_packet`; copying `rbctl-voip` alone
  omits a locked build input. Verify the feed index can discover the packages
  before compiling (Q7: prior local group-ID collision).
- The updated SDK telephony feed provides baresip **4.9.0**, development files,
  `account.so` in the base package, and separate codec/feature modules. Match
  package dependencies to the generated config (e.g. `baresip-mod-g711` for
  PCMA). The base package already ships `/etc/init.d/baresip`: check on the
  clean image that only the `voip` instance opens the board and SIP listener.

## Implemented in the working tree

`rbctl-voip` installs `/etc/config/voip` (private), `/etc/init.d/voip`, the
renderer, the production module, and rpcd ACL group `rbctl-voip`. The ubus
adapter is compiled into the existing module for OpenWrt only: no second board
owner or duplicate FSM. Its socket is watched by libre and reconnected after
a bus loss. The base baresip init service must remain disabled in its own UCI
config. Leave the diagnostic CLI stopped when this service owns the board.

The default starts the board with no enabled SIP accounts. Set `user`, `domain`,
credentials and `enabled=1` on `voip.acc1`; leave both lines mapped for shared
ringing or add another account. Apply with `uci commit voip` then
`/etc/init.d/voip reload` (or OpenWrt `reload_config` to dispatch UCI changes).
Runtime generations are private under `/var/run/voip`; idle account/line changes
are staged before replacing UAs. During a call the old accounts stay active;
`ubus call voip status` reports `reload_pending` until the latest validated generation is applied after both lines go idle. **Board changes are not queued:** applying a new locale, VLAN or playout clock restarts baresip and disconnects active calls. Apply those changes when both lines are idle. The ubus `reload` method reads the already-rendered generation.

`manage_vlan=1` reuses the existing management VLAN or asks netifd to claim the
dotted device with protocol `none`; no address is assigned by the voice service.
`firewall_zone` defaults empty. Setting a zone publishes IPv4 SIP and RTP input
rules through procd's firewall data. Board codec/ptime remain PCMA/20 ms; network
codecs may be PCMA, PCMU or G722. FLASH timing is fixed by the stock DSP.
Each account accepts at most one `alert_info` entry, e.g. `list alert_info 'internal=2'`; a second entry is rejected. A distinct registration route uses `outbound`
(no independent simultaneous registrar and outbound-proxy override).

### Troubleshooting

- **Registered, but incoming calls never ring:** if the provider sees `403 Forbidden`, compare the INVITE source IP with the registrar IP. Providers with separate inbound proxies or DNS SRV pools may legitimately use different addresses. In **Telephony → SIP accounts → Edit**, **Accept incoming calls only from the registrar IP** (`check_origin`) is on by default. If the provider confirms this topology, restrict SIP ingress to its published signaling addresses at the firewall before clearing the checkbox. This setting checks SIP source IPs, not caller ID or RTP.
- **Voice won't start after a manual UCI edit:** an incomplete account is skipped, but a malformed value such as a SIP user containing a space fails the render. A failed reload keeps the running configuration; on the next boot there is no running instance to retain. Correct the field reported by `/etc/init.d/voip reload`, commit the correction, and start the service if it is down. LuCI rejects malformed values before saving.

### Country profile

`voip.board.locale` selects the stock board profile for **both** FXS ports
(default `IT`). Use a country code or `ETSI`, case-insensitively:
`AU BE BR CA CN CO ETSI FR DE IN IT MX NZ US PE PL PT RU ES TW`.
For example, `uci set voip.board.locale='DE'` selects Germany. The renderer
translates the name to the numeric host command index; raw numbers are not
UCI values. If using an earlier draft config, replace `10` with `IT`. Host
index 18 is intentionally unnamed. See the [mapping evidence](peer.md#caller-id-ring-and-tone).
Changing the locale and
reloading `voip` restarts board configuration. It does not regionalize the
module's locally synthesized SIP 180 ringback (425 Hz, 1 s on / 4 s off),
call-waiting beep, howler or SIT fallback. Busy/congestion/disconnect selector
defaults were verified on the Italy bench, not across all locales. No other
country's handset behavior or electrical conformance is implied. See the
[software tone inventory](software-tones.md) for the generators and triggers.

## Acceptance criteria (see [results](test-report.md#m5-review-verification))

| Gate | Exercise | Evidence to retain |
|---|---|---|
| T5.1 | Select `CONFIG_PACKAGE_rbctl-voip=m` in the clean SDK, build `package/feeds/<local-feed>/rbctl-voip/compile V=s`, install the resulting package with matching feeds available. | SDK/image and feed revisions, package file list and dependencies, clean install and service-enabled result. No OEM userspace files. |
| T5.2–T5.3 | Render UCI into private `/var/run/voip` (0700). Distinguish account/line reload from a board-setting change. Preserve the signed per-line gain mapping and zero defaults. | Account change registers anew without a board init; locale change causes restart and board init. Record transitions without credentials. |
| T5.4–T5.5 | Inspect `ubus call voip status`, `lines`, `calls`; listen to `voip.*` during a call. Dial an authorized test URI then hang up with the same `line` argument. | Redacted status/events in order for a call; `dial` and `hangup` operate on the selected line. |
| T5.6–T5.7 | Verify procd startup with the board VLAN available, then exercise the rpcd ACL. | After reboot: one service, registered, both lines ready. Unauthenticated HTTP RPC cannot dial. |

Board pacing is the candidate for the UCI default: [the ten-minute comparison](m4-bench.md#t420-live-comparison)
passed the drift limit on both lines. `timer` remains the implementation default pending
the conversational echo and board-paced DTMF interval checks. T4.10 is accepted as evaluated, T4.14
as partial (Q8 parked). For T5.6, the module now retries first attach even
before the board has ever been ready; the host regression is not a reboot pass.
