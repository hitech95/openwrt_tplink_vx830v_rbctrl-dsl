# rbctl_voip — single-line baresip module

`module.c` owns the baresip 3.16 module lifecycle. `board.c`/`board.h` own the
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

From the `rbctl-voip` workspace, with baresip and libre 3.16.0 headers installed:

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
and `BARESIP_MODULE_DIR` explicitly. Use baresip/libre 3.16.0 and build the
`account`, `menu`, and `g711` modules. On hosts using C11 threads, libre and
baresip can be configured with `-DHAVE_THREADS:BOOL=ON`.

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

The audio device bridges PCMA/PT 8
on the board to 8 kHz mono signed PCM in baresip. On-hook/release queues RTP off
and channel close. `tests/calls.py` runs two actual baresip processes and checks
CID fields, ringback packets, distinct synthetic audio in each direction and
release from either endpoint. Physical acoustics and PBX interop are live tests.
