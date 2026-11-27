# rbctl_voip — baresip module for analog phone ports

`module.c` owns the baresip 4.9 module lifecycle. `board.c`/`board.h` own the
Rust protocol link; the transport supplies its MAC and synchronous send callback.
`attach.c` registers nonblocking board fds with libre and starts DD01 init and
attach. It waits for both attach status and the DD02 DSP state (in either order).
The Rust link supplies retry deadlines; a five-second limit also bounds waiting
for a missing event across the whole startup. On a stopped DSP the module applies
SLIC/DSP settings and waits for a fresh start completion; on a running DSP it
reclaims the four slots before applying settings and skips DSP start. Both paths
wait for the SLIC result and all four channel defaults before `board ready`. Module
unload or attach failure cancels the timer and unregisters fds before closing.
The default transport uses `lan0.500`; override with `rbctl_voip_device`.
`rbctl_voip_locale` selects the SLIC/DSP locale (0–20; default 10, Italy).

## Build on Linux

From the `rbctl-voip` workspace, with baresip and libre 4.9.0 headers installed:

```sh
cargo build --locked -p rbctl_voip_ffi
cmake -S modules/rbctl_voip -B target/m3-module \
  -DVOICE_ARCHIVE="$PWD/target/debug/librbctl_voip_ffi.a"
cmake --build target/m3-module
```

With upstream source checkouts, pass `-DBARESIP_INCLUDE_DIR=/path/to/baresip/include`
and `-DRE_INCLUDE_DIR=/path/to/re/include` at configure time. For a target build,
use the target toolchain, target headers and staged Rust archive together.

### Autonomous host checks

Run `ctest --test-dir target/m3-module --output-on-failure` after building.
`board_lifecycle` checks duplicate binding, propagation of transport errors,
and release/rebind against the real Rust archive. For `baresip_registration_smoke`,
configure with `-DRBCTL_HARNESS=ON -DBUILD_TESTING=ON` and
`-DCMAKE_PREFIX_PATH=/path/to/host-install` or set `BARESIP_EXE`
and `BARESIP_MODULE_DIR` explicitly. For the full suite, unpack the SDK's re and baresip **4.9.0** source tarballs, install the host OpenSSL and SpanDSP development packages, then build both into one prefix:

```sh
cmake -S re-4.9.0 -B build-re -DCMAKE_INSTALL_PREFIX="$prefix" -DHAVE_THREADS=ON
cmake --build build-re --parallel && cmake --install build-re
cmake -S baresip-4.9.0 -B build-bs -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_PREFIX_PATH="$prefix" \
  -DMODULES='account;menu;g711;g722;auresamp;mwi;srtp' -DHAVE_THREADS=ON
cmake --build build-bs --parallel && cmake --install build-bs
BARESIP_PREFIX="$prefix" scripts/check.sh  # from rbctl-voip/
```

The LuCI account editor exposes `interdigit_timeout`, `interdigit_long_timeout`, `require_registration` and one `alert_info` mapping. Line `cw_cid`, `first_digit_timeout`, `offhook_warning_timeout` and `howler_duration` are configuration-only. Hunt uses physical port order. CLI `dial` is a test hook; it does not ring an on-hook handset first.

The smoke test uses a temporary config, a synthetic account, and a UDP registrar
on 127.0.0.1. Its board peer uses an inherited Unix datagram socket pair
(`rbctl_voip_device fixture`, `RBCTL_VOIP_TEST_FD`) instead of a VLAN. The
suite checks REGISTER/200 OK and the module's DD02 ACK for stopped and running
DSP states in opposite reply orders. It also withholds the state event to verify
the attach timeout independently of SIP registration. Startup checks cover warm
slot recovery, cold start, missing completions and a rejected final slot default.
Fixture support is disabled
in the default module build.

## Live call checks (T3.2–T3.4)

**Two-line configuration (T4.2):** use one module/board owner and two distinct
accounts. Set `rbctl_voip_account0` and `rbctl_voip_account1` in `config` to
their exact SIP AORs, e.g. `sip:701@pbx.example` and `sip:702@pbx.example`.
In each matching `accounts` entry append the corresponding device pair:

```text
<sip:701@pbx.example>;audio_codecs=PCMA/8000/1;audio_source=rbctl_voip,phone0;audio_player=rbctl_voip,phone0
<sip:702@pbx.example>;audio_codecs=PCMA/8000/1;audio_source=rbctl_voip,phone1;audio_player=rbctl_voip,phone1
```

Add the PBX authentication parameters locally. Each line owns its digits, audio
device and RTP slot (0 or 1). To share an account, put the same AOR in both
mapping keys and define only one account. Both idle ports ring; the first answer
selects its audio device and stops the other ringer. Set `rbctl_voip_hunt0 1`
and `rbctl_voip_hunt1 1` to ring just the first free port. On a connected line, a second caller
produces a short waiting beep. R holds the active caller and answers or resumes
the other; on-hook with a held caller rings the handset back. With no AOR mappings the original
`rbctl_voip_line` and `rbctl_voip,phone` single-line setup still works.

Use the non-harness AArch64 module, one registered test account and a second
reachable extension/softphone on the LAN PBX. The earlier registration-only
PBX had no dialplan; configure internal routes in **both** directions. Use a
numeric synthetic caller ID. Enable PCMA (`alaw` on Asterisk) and 20 ms frames.
In the test account's `accounts` entry set `audio_codecs=PCMA/8000/1`.

```text
rbctl_voip_device lan0.500
rbctl_voip_line 1
rbctl_voip_locale 10
audio_source rbctl_voip,phone
audio_player rbctl_voip,phone
audio_txmode poll
# Tested on the wired LAN (RTP packets, 20 ms each); retune for other networks.
audio_jitter_buffer_delay 2-8
module g711.so
module_app rbctl_voip.so
module_app account.so
```

Line 1 selects PHONE2 (caller-ID display); use 0 for PHONE1. The module owns
incoming INVITE handling and handset tones: omit `menu.so` and disable automatic
answer. Run one board owner only. Wait for board ready **and** registration.

1. Lift the handset, listen for dial tone, dial the other extension and press
   `#`. Verify ringback, answer there, talk both ways, then hang up the handset.
2. Call back from the other extension. Check the synthetic number on the display,
   answer by lifting, talk both ways, then hang up the remote endpoint. Replace
   the handset and dial again to check slot reuse.
3. For T3.5 separately check Asterisk's digit log in RFC 4733 and SIP INFO modes.

`call.c` maps hooks and digits to the SIP dialog; `audio.c` bridges PCMA/PT 8
on a single slot. For the T4.20 clock comparison set `rbctl_voip_playout_clock board`:
one valid, new board PCMA packet triggers one downlink packet, including
ringback before a SIP source exists. No board PCMA means no playout ticks in
this experimental mode. The default `timer` retains the measured M3 cadence.
The old `vx830v,phone` test configs must use `rbctl_voip,phone` as above.

## M4 bench controls

### Front-panel indicator

The optional OpenWrt `rbctl-voip-led` package installs a worker and a `voip`
hotplug handler. The module samples both ports together and sends only state
transitions. Board unavailable → red; otherwise ringing/waiting on either port
→ fast green blink, off-hook/call → slow green blink, any mapped UA registered
→ solid green, otherwise red. Worker EOF turns both off. The handler uses
`/lib/functions/leds.sh`, defaults to `green:fxs`/`red:fxs`, and honours
`system.led_fxs.sysfs`/`system.led_fxs_fault.sysfs` overrides. It does not
modify UCI or the GPIO mapping. No helper installed means no panel writes.

### Per-line options

These are **baresip config** keys; append `0` or `1` for the physical port
(e.g. `rbctl_voip_digitmap1 20x|31[0-5]T`). UCI rendering is an M5 task.

| `rbctl_voip_` key stem | Values and defaults |
|---|---|
| `digitmap` | Up to 16 `|`-separated alternatives of up to 32 digits. Literals, `x`, sets/ranges (`[2-7*]`); final `.` repeats the preceding atom zero or more times, `+` one or more. Final `T` requires a timeout. Default `[0-9*]+`; `#` explicitly sends the collected digits. Invalid maps reject module startup. |
| `first_digit`, `short_digit`, `long_digit` | Seconds (1–600); defaults 15, 4, 10. Exact unambiguous match dials now; complete but extendable match dials after short timeout; incomplete prefix becomes unobtainable on long timeout. |
| `offhook_warning`, `howler_duration` | Seconds (1–600), defaults 60 and 15. Progress/disconnect → bounded local warning → closed slot/lockout until hook-on. No first digit goes straight to warning. |
| `busy_sel`, `congestion_sel`, `disconnect_sel` | Defaults 1, 2, 1. Change for locale after T4.10; selector 2 is a fast cadence, not yet confirmed as congestion by ear. Unobtainable uses a generated SIT. |
| `inband` | 0 (default) delegates digits to the account RFC 4733/INFO mode. 1 selects board relay-off and emits touch-tone pairs into outgoing PCM during DD02 press/release. For the PBX test set the Asterisk endpoint to `dtmf_mode=inband`. |
| `require_registration` | 0 default. 1 provides congestion when unregistered and restores dial tone on REGISTER OK. |
| `tx_gain`, `rx_gain` | Per-port raw steps −6…+6, default 0. TX = mouthpiece → network (`+08`), RX = network → earpiece (`+04`). E.g. `rbctl_voip_tx_gain1 -2` attenuates PHONE2 uplink. Applied at the next CHANNEL_OPEN after loading config; no universal dB or acoustic clipping guarantee. |
| `cid`, `cw_cid`, `call_waiting`, `mwi`, `hunt` | Boolean; defaults 1, 0, 1, 1, 0. Enable `cw_cid` only for a type-II test. For MWI load `module_app mwi.so`, enable account `mwi=yes`, and provision a PBX mailbox. |
| `alert_info`, `alert_entry` | Match one whole Alert-Info value or its `info=` parameter, case insensitive. Entry 0–9, default 0. Example `rbctl_voip_alert_info1 internal` and `rbctl_voip_alert_entry1 2`. |
| `fax_profile` | 0 default. For T4.19 use 1, a G.711-only account, `audio_jitter_buffer_type fixed`, `audio_buffer_mode fixed`, and omit PLC, VAD and speech-processing modules. The module reacts to DSP 21/22 and sustained 2100 Hz in board PCM; it bypasses digit relay via RTP_UPDATE. This is a preconfigured passthrough profile, not a T.38 gateway or automatic renegotiation from arbitrary codecs. |

For T4.6 load `module srtp.so` and set `mediaenc=srtp-mand` on the test
account; require SDES on the PBX and verify SAVP/crypto without retaining keys.
For T4.7 load `module g722.so` and `module auresamp.so`, set
`ausrc_srate 8000`, `auplay_srate 8000`, and pin the test account to
`audio_codecs=G722/16000/1`. Router staging also needs compatible SpanDSP and
libtiff for this G.722 build. Neither option changes native board PT 8.

See the [gate-by-gate bench list](../../../../docs/voip/m4-bench.md) for what is still unproven.

The audio device bridges PCMA/PT 8
on the board to 8 kHz mono signed PCM in baresip. On-hook/release queues RTP off
and channel close. `tests/calls.py` runs two actual baresip processes and checks
CID fields, ringback packets, distinct synthetic audio in each direction and
release from either endpoint. Physical acoustics and PBX interop are live tests.
