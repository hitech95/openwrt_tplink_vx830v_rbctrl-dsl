# Data-link configuration & provisioning plan — post session-4

Status: **IMPLEMENTED (2026-08-27, released as 0.5.0)** — every F-step
landed and is E2E-verified (E9b validation matrix, ED multi-link, EX
materialization semantics, EA auto swap; see docs/e2e.md). The
tester-verified 0.4.2-r2 hardcodes were superseded by F1/F2. Target
release: **0.5.0**.

Context: session 4 (debug_sessions/fourth) proved the data-plane root cause
and the working flow. Tester confirmed the dirty build (0.4.2-r2): **op15
queued while the line is down + bearer tag `vid=100`** produces a working
link. This plan makes that flow config-driven and cleans up around it.

Verified facts this plan builds on (all RE-confirmed, see
debug_sessions/fourth/FINDINGS.md):
- The board materializes a PTM link in two places: the **up-transition**
  (walks the interface list built by op15s sent while the line was DOWN —
  the OEM flow, tester-verified) and the **direct path** in the op15
  handler (requires `apiGetXdslStatus` to parse `"<status>:<type>"` from
  `/proc/tc3162/xdsl_stats`; the real `tccicmd.ko` driver writes `"1:2"`,
  so it works on hardware — our E2E fake wrote `"1 2"` and hit the hollow
  ACK).
- One op15 = one link = one service VLAN action: `tag_enable=0` → smux
  `vlan_mode 1` (strip), `tag_enable=1 vid=N` → `vlan_mode 3` (insert /
  rewrite TCI to N). Mode 2 (transparent Q-in-Q) is unreachable via op15.
- Board limits: transport VLANs 2000–2007 (max 8 links); the vid-dedup in
  `vdsl_linkAdd` keys on the **service vid** → service vids must be unique,
  and at most ONE untagged link can ever exist.

---

## Fix list

### F1 — UCI data-link configuration (multi-link, named sections)

Mirror the ATM style — one named section per data link, so links are
addressable like interfaces (`network.internet`, `network.voice`):

```
# PTM (VDSL) links — type 'ptm' (NOT 'ptp': that is IEEE-1588; reject
# with a clear error naming the correct type)
uci set network.internet=ptm
uci set network.internet.index='0'    # data-link index -> transport 2000+i
uci set network.internet.vid='100'    # service VLAN; absent = untagged
uci set network.internet.pbit='0'     # 802.1p, default 0

# ATM (ADSL) links — modern named-section schema (replaces the legacy
# anonymous @atm-bridge[0]; keep reading the old form for compat)
uci set network.atm=atm
uci set network.atm.index='1'         # same index space as ptm sections
uci set network.atm.encaps='llc'
uci set network.atm.payload='bridged'
uci set network.atm.vpi='8'
uci set network.atm.vci='32'
```

- **`index` (user requirement)**: the transport VLAN is derived from the
  EXPLICIT per-link index (`2000 + index`), NOT from section order —
  UCI list position can change across reloads/edits, and netifd
  (pppoe/ipoe interfaces) hook the created `lan0.<transport>` by name:
  a reorder would silently re-point interfaces to the wrong link.
  Rules: optional per section; **default 0 when missing**; **error if
  more than one link section exists and any lacks an explicit index**
  (implicit-0 would collide or lie); duplicates are an error.
- Validation at load: indices 0–7 unique; ≤ 8 links; vids unique (board
  vid-dedup); at most one untagged link; pbit 0–7; vpi 0–255,
  vci 1–65535. Errors abort startup naming the section (e.g.
  `network.voice: duplicate index 1`).

### F2 — Queue-at-open provisioning (the tester-verified flow)

Startup order: `CleanupLink(2000..2000+N-1)` → `OpenLine` →
`ProvisionLink × N` (all links, line still down — board materializes each
at the up-transition). No more provisioning-at-showtime as the primary
path. `link_provisioned` becomes per-link tracking; `LinkProvisioned`
events confirm each vlan individually.

- Link type: PTM for `ptm` sections, ATM for `atm` sections — both
  queued at open (the up-transition handler dispatches on the loaded
  driver, so both materialize correctly). Confirmed op5 shares the same
  handler structure as op15 (apiGetXdslStatus ':' parse + hollow-ACK on
  mismatch), so queue-at-down is the right flow for both.
- Shutdown: op16 each provisioned link (works for queued-not-yet-
  materialized nodes too — `delNodeFromInterfaceList`), then op3.
- Reload with changed links: op16 removed links, op15 added links
  (**post-up path** → F3 warning), unchanged links untouched.

### F3 — Post-up provisioning warning

Whenever a ProvisionLink executes while the line is up (reload-added
link, auto-mode type correction below, unreachable→reachable re-provision):

```
WARN link add while line is up: the econet firmware's direct path
     (apiGetXdslStatus ':' parse) may hollow-ACK this request — if the
     link does not pass data, restart the line (op16 + restart-line).
```

Rationale: post-up works on real hardware (tccicmd writes `"1:2"`), but
it is the path our E2E fake broke and the only path we could NOT verify
on-target — the warning directs the operator to the known-good remedy.

### F4 — Auto-detect bring-up (open question, proposal)

Problem: queue-at-open needs the link type before training; `auto`
(multimode) resolves the type only at showtime.

Proposal — **optimistic queue + corrective swap**:

1. No link sections + `xfer auto` (default config): queue **PTM
   untagged** at open (covers the common VDSL case; zero-config — this is
   user suggestion "auto + no transport specified → untagged").
2. At line-up, if the trained modulation is ADSL (auto resolved to ATM):
   `op16` the queued PTM node, provision the ATM link post-up (with F3
   warning). The board handles the mismatch gracefully meanwhile
   (`vdsl_linkAdd` on an ATM connection logs "It is not PTM mode" and
   materializes nothing).
3. `xfer atm`/`ptm` explicit: type known at config load; queue the
   matching op5/op15 at open. Explicit-vs-trained contradiction check
   (current `compatible_with` rules) stays.

Rejected alternatives: queue both types (board state pollution, dedup
interactions), provision only post-up (abandons the verified flow),
config-time modulation inference for multimode (unknowable).

### F5 — E2E truth-up + coverage

- **Fake format fix**: `set_line_state` writes `"1:2\n"` / `"0:3\n"`
  (real `tccicmd.ko` format). This un-breaks the board's direct path in
  E2E and lets us test BOTH materialization paths.
- **EX extended**: queue-at-down transition materialization (current
  assertions) **plus** post-up direct materialization — both tag modes.
- **New ED (daemon data links)**: UCI with two sections (vid 100, vid
  835) → two op15s at open (assert payloads incl. tag fields, both
  ACKed) → both `smuxctl` lines materialize at the transition (vids 100
  and 835) → both host VLANs created → teardown op16s both.
- **E2 flip**: currently asserts *no link-add before showtime* — now the
  contract is the opposite (link-adds REQUIRED at open, while down).
  E3/E5/E6 teardown assertions become per-link.
- Validation matrix E9 grows: duplicate vid, 9 sections, two untagged.

### F6 — Documentation

- `docs/xdsl/data_plane.md`: the full econet data-layer writeup (nas8
  bearer, MULIF/smux three modes, per-PVC topology, one-service-VLAN-
  per-link constraint, no-trunk explanation, 8-link/untagged limits).
- New "firmware quirks" section (data_plane.md or responses.md):
  `apiGetXdslStatus` ':' parse + tccicmd `"1:2"` literal table; hollow
  ACK semantics of queued op15; vid-dedup rules; queue-at-down vs
  direct-path decision table.
- `debug_sessions/fourth/FINDINGS.md`: final root cause (untagged
  bearer → BRAS drop), r2 dirty-build confirmation, corrected earlier
  conclusion about the direct path.
- CHANGELOG 0.5.0.

### F7 — Cleanup

- Revert the 0.4.2-r2 TEST-DIRTY hardcodes (worker.rs tag=100, mod.rs
  queue) — superseded by F1/F2. `PKG_RELEASE` back to 1 on 0.5.0.
- Selftest: keep post-up provisioning + teardown (it deliberately
  exercises the direct path as a diagnostic); align its log wording with
  the new "link type" phrasing if needed.

---

## Implementation order

1. F7 revert + F5 fake-format fix + EX both-paths (verify on real binary
   before any daemon changes) — commit A
2. F1 UCI schema + validation (+ unit tests, E9 cases) — commit B
3. F2 queue-at-open + per-link tracking + teardown + F3 warning — commit C
4. F4 auto swap logic — commit D
5. F5 ED scenario + E2/E3/E5/E6 rework — commit E
6. F6 docs + CHANGELOG + version 0.5.0 — release commit

Each step E2E-green before the next. No commit without explicit consent.
