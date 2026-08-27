#!/usr/bin/env python3
"""E2E scenario driver — runs INSIDE the harness namespace (see inner.sh).

Full harness documentation: docs/e2e.md

Board  = the real MIPS remote_board binary under qemu-mips (strace-wrapped for capture)
Daemon = the real aarch64 rbctl-dsl under qemu-aarch64
Wire   = veth pair eth0.1.500 (board) <-> lan0.1.500 (daemon)
Driver = this script: spawns/stops the peers, injects line state via fake
         /proc/tc3162, and asserts on wire captures + board traces + logs.

Scenarios (default set: all of them; subset via SCENARIOS):
  E1 board boot self-open      E5 injected showtime -> Up
  E2 daemon bring-up (no echo) E6 op16 delete ack 0 after materialization
  E3 daemon restart warm cache E7 warn-only watchdog (silent line)
  E8 session-2 fixture replay  (broken wire formats must misdecode/reject)
"""
import codecs, os, re, signal, subprocess, sys, threading, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import proto

ECONET = os.environ["ECONET_ROOTFS"]
AROOT = os.environ["AROOT"]
LOGDIR = os.environ["LOGDIR"]
SCENARIOS = os.environ.get("SCENARIOS", "E1 E2 E3 E5 E6 E7 E8 E9 ED EX EA").split()

BOARD_MAC = bytes.fromhex("00aabb012348")
HOST_MAC = bytes.fromhex("020000000099")   # harness raw client

# session-2 wire payloads (sniff2.pcap) — the bug, as permanent fixtures
OP1_ECHO_FIXTURE = bytes.fromhex("0106010101000001ff00000000")
OP16_ECHO_FIXTURE = bytes.fromhex("1007d003")


def log(msg):
    print(f"[e2e] {msg}", flush=True)


def path(name):
    return os.path.join(LOGDIR, name)


def wait_file_contains(name, needle, timeout, quiet=False):
    p = path(name)
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            if needle in open(p, errors="replace").read():
                return True
        except FileNotFoundError:
            pass
        time.sleep(0.2)
    if not quiet:
        excerpt = ""
        try:
            excerpt = open(p, errors="replace").read()[-500:]
        except FileNotFoundError:
            excerpt = "<missing>"
        raise AssertionError(f"timeout {timeout}s waiting for {needle!r} in {name}; tail:\n{excerpt}")
    return False


def start_board(tag):
    """Run the real board binary under qemu-mips, traced by strace.

    strace gives lossless capture of every write to /proc/tc3162/* (a
    tmpfs file would only ever hold the last write of an apply burst).
    The tailer thread converts the strace log into board_tcci.log.
    """
    env = dict(os.environ)
    env["PATH"] = os.environ["STUBBIN"] + ":" + env["PATH"]
    out = open(path(f"board_{tag}.log"), "w")
    return subprocess.Popen(
        ["strace", "-f", "-e", "trace=openat,write,execve", "-s", "8192",
         "-o", path(f"board_strace_{tag}.log"),
         "qemu-mips", "-L", ECONET, f"{ECONET}/userfs/bin/remote_board"],
        stdout=out, stderr=subprocess.STDOUT, env=env,
        start_new_session=True)  # own pgid: SIGTERM must reach qemu, not just strace


# ── strace tailer: openat/write → board_tcci.log ─────────────────────────
#
# strace line shapes:
#   [pid N] openat(AT_FDCWD, "/proc/tc3162/tcci_cmd", O_WRONLY|...) = 2
#   [pid N] write(2, "wan adsl open", 14)            = 14
# Track the tcci fd per pid; emit each write's decoded content.

OPEN_RE = re.compile(r'(?:\[pid (\d+)\] )?openat\([^,]*, "([^"]*)", ([^)]*)\)\s*= (\d+)$')
WRITE_RE = re.compile(r'(?:\[pid (\d+)\] )?write\((\d+), (.+), \d+\)\s*= \d+$')


def _unescape(s):
    return codecs.decode(s, "unicode_escape")


def _strace_tailer_once(lines, tcci_fds, emit):
    for line in lines:
        line = line.rstrip("\n")
        m = OPEN_RE.search(line)
        if m:
            pid, p, flags, fd = m.group(1) or "0", m.group(2), m.group(3), int(m.group(4))
            if "/proc/tc3162/" in p and "O_WRONLY" in flags:
                tcci_fds[(pid, fd)] = p
            continue
        m = WRITE_RE.search(line)
        if m:
            pid, fd = m.group(1) or "0", int(m.group(2))
            if (pid, fd) in tcci_fds:
                body = m.group(3)
                if body.startswith('"') and '", "' not in body[:1]:
                    # content is the quoted string up to the closing ",
                    end = body.rfind('"')
                    emit(_unescape(body[1:end]))
    return tcci_fds


def strace_tailer(tag):
    """Follow board_strace_<tag>.log files; append decoded writes."""
    seen = {}
    tcci_fds = {}
    out = open(path("board_tcci.log"), "a", buffering=1)
    # system() invocations (execve of the /bin/sh -c command line) —
    # captures the data-plane materialization commands, incl. absolute
    # paths (/usr/bin/smuxctl, /userfs/bin/vconfig) that bypass the
    # PATH stubs and never reach board_sys.log.
    exec_out = open(path("board_exec.log"), "a", buffering=1)
    while True:
        for f in sorted(os.listdir(LOGDIR)):
            if not f.startswith("board_strace_") or not f.endswith(".log"):
                continue
            full = path(f)
            off = seen.get(full, 0)
            size = os.path.getsize(full)
            if size <= off:
                continue
            with open(full, errors="replace") as fh:
                fh.seek(off)
                chunk = fh.read(size - off)
            seen[full] = size
            lines = (seen.get("_buf_" + full, "") + chunk).split("\n")
            seen["_buf_" + full] = lines.pop()  # keep the unterminated tail
            for line in lines:
                # 278340 execve("/bin/sh", ["/bin/sh", "-c", "smuxctl add ..."], ...) = 0
                if "execve(" not in line:
                    continue
                argv = re.findall(r'"((?:[^"\\]|\\.)*)"', line)
                if len(argv) >= 4 and argv[1] == "/bin/sh" and argv[2] == "-c":
                    exec_out.write(argv[3] + "\n")
            _strace_tailer_once(lines, tcci_fds,
                                lambda s: out.write(s + "\n"))
        time.sleep(0.1)


def start_daemon(tag, extra=()):
    env = dict(os.environ)
    env["UCI_CONFIG_DIR"] = os.path.join(AROOT, "etc/config")
    out = open(path(f"daemon_{tag}.log"), "w")
    return subprocess.Popen(
        ["qemu-aarch64", "-L", AROOT, f"{AROOT}/usr/sbin/rbctl-dsl",
         "daemon", "-i", "lan0.1.500", *extra],
        stdout=out, stderr=subprocess.STDOUT, env=env)


def start_sniffers():
    env = dict(os.environ)
    procs = []
    for iface, tag in (("eth0.1.500", "b_"), ("lan0.1.500", "h_")):
        procs.append(subprocess.Popen(
            ["python3", os.path.join(os.path.dirname(os.path.abspath(__file__)), "sniffer.py"),
             iface, path("wire.log"), tag], env=env))
    return procs


def set_line_state(state):
    """Rewrite the fake /proc state files (the board's next fopen sees it).

    The board's parser is needle-strict: lines must appear in the needle
    order, 'Opmode:' takes EXACTLY ONE space (the opmode table needles
    start with ' '), and the firmware spells 'ativetime' with one 'c'.
    Values keep the space after the colon (copied verbatim into slots).

    xdsl_stats line 1 uses the REAL tccicmd.ko format "<status>:<type>"
    (colon): "1:2" showtime-VDSL2, "1:1" showtime-ADSL, "0:3" down — the
    board's apiGetXdslStatus strtok's on ':' (session-4 finding; our old
    space-separated fake broke the op5/op15 direct materialization path).

    showtime_adsl additionally swaps /proc/modules to mt7510sar (the ATM
    driver) so dsl_isAtmConnection() is true — the other states restore
    mt7510ptm. Opmode " ITU G.992.5(ADSL2PLUS)" maps to modulation code 4
    (apiGetAdslType table @ 0x429074: needle strings need the LEADING
    SPACE, they strncmp against the value including it).
    """
    if state == "down":
        xdsl, adsl = "0:3\n", ("ADSL link status: down\nOpmode: Wrong opmode\n"
                               "Adsl Type: \n")
        modules = "mt7510ptm 12345 0 - Live 0x00000000\n"
    elif state == "showtime_ptm":
        xdsl = "1:2\n"
        # near-end = DOWNSTREAM, far-end = UPSTREAM (TrendChip convention;
        # empirically verified session 3): 40M down / 12M up.
        adsl = (
            "ADSL link status: up\nOpmode: ITU G.993.2(VDSL2)\n"
            "near-end interleaved channel bit rate: 40000\n"
            "near-end fast channel bit rate: 0\n"
            "far-end interleaved channel bit rate: 12000\n"
            "far-end fast channel bit rate: 0\n"
            + "".join(f"{k} 0\n" for k in [
                "near-end FEC error fast", "near-end FEC error interleaved",
                "near-end CRC error fast", "near-end CRC error interleaved",
                "near-end HEC error fast", "near-end HEC error interleaved",
                "far-end FEC error fast", "far-end FEC error interleaved",
                "far-end CRC error fast", "far-end CRC error interleaved",
                "far-end HEC error fast", "far-end HEC error interleaved"])
            + "Downstream\nrelative capacity occupation: 90\n"
              "noise margin downstream: 6.3\noutput power upstream: 12.5\n"
              "attenuation downstream: 20.1\n"
              "Upstream\nrelative capacity occupation: 85\n"
              "noise margin upstream: 6.1\noutput power downstream: 14.2\n"
              "attenuation upstream: 10.5\n"
              "attain upstream: 15000\nattain downstream: 45000\n"
               "ADSL activetime second: 5\nADSL total ativetime second: 6\n"
               "Adsl Type: vdsl2\n")
        modules = "mt7510ptm 12345 0 - Live 0x00000000\n"
    elif state == "showtime_adsl":
        # ADSL2+ showtime: "1:1" (status:type), ADSL opmode needle (code
        # 4), and the ATM driver loaded — trains the daemon's auto mode
        # to the ATM family (F4 swap scenario).
        xdsl = "1:1\n"
        adsl = (
            "ADSL link status: up\nOpmode: ITU G.992.5(ADSL2PLUS)\n"
            "near-end interleaved channel bit rate: 20000\n"
            "near-end fast channel bit rate: 0\n"
            "far-end interleaved channel bit rate: 1000\n"
            "far-end fast channel bit rate: 0\n"
            + "".join(f"{k} 0\n" for k in [
                "near-end FEC error fast", "near-end FEC error interleaved",
                "near-end CRC error fast", "near-end CRC error interleaved",
                "near-end HEC error fast", "near-end HEC error interleaved",
                "far-end FEC error fast", "far-end FEC error interleaved",
                "far-end CRC error fast", "far-end CRC error interleaved",
                "far-end HEC error fast", "far-end HEC error interleaved"])
            + "Downstream\nrelative capacity occupation: 80\n"
              "noise margin downstream: 9.0\noutput power upstream: 12.0\n"
              "attenuation downstream: 30.0\n"
              "Upstream\nrelative capacity occupation: 70\n"
              "noise margin upstream: 8.0\noutput power downstream: 12.0\n"
              "attenuation upstream: 20.0\n"
              "attain upstream: 1200\nattain downstream: 22000\n"
              "ADSL activetime second: 5\nADSL total ativetime second: 6\n"
              "Adsl Type: adsl2plus\n")
        modules = "mt7510sar 12345 0 - Live 0x00000000\n"
    else:
        raise ValueError(state)
    open("/proc/tc3162/xdsl_stats", "w").write(xdsl)
    open("/proc/tc3162/adsl_stats", "w").write(adsl)
    open("/proc/modules", "w").write(modules)


def wire():
    return proto.parse_wire_log(path("wire.log"))


def stop_daemon(proc, tag):
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=15)
    except subprocess.TimeoutExpired:
        proc.kill()
        raise AssertionError(f"daemon {tag} did not exit on SIGTERM")


def kill_board(proc, tag):
    # Signal the whole process group (strace + qemu): signaling strace alone
    # does not reliably terminate the tracee.
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=8)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()


# ── scenarios ─────────────────────────────────────────────────────────────

def e1_board_boot(board):
    """Fresh board applies boot default against zeroed cache and opens."""
    log("E1: board boot — expect boot apply burst ending in 'wan adsl open'")
    wait_file_contains("board_tcci.log", "wan adsl open", 20)
    tcci = open(path("board_tcci.log")).read()
    assert "wan adsl opencmd multimode" in tcci, tcci
    assert "wan ghs set annex auto" in tcci, tcci
    log("E1 PASS")


def e2_bringup(board):
    """Daemon double-apply: wire has bare 12-B op1s, board opens the line."""
    log("E2: daemon bring-up — double apply, no echo byte")
    d = start_daemon("e2")
    wait_file_contains("daemon_e2.log", "open-line: guaranteed double apply", 15)
    wait_file_contains("daemon_e2.log", "line_config_up: mod=Vdsl2 annex=B profiles=0x1ff bitswap=false", 10)
    # exactly two op1 REQs with bare descriptors + acks 0
    deadline = time.time() + 10
    while time.time() < deadline:
        op1s = [f for f in wire() if f["dir"] == "b_REQ" and f["op"] == 1]
        if len(op1s) >= 2:
            break
        time.sleep(0.3)
    assert len(op1s) >= 2, f"expected 2 op1 requests, got {len(op1s)}"
    for f in op1s[:2]:
        assert f["plen"] == 12, f"op1 payload_len {f['plen']} != 12 (echo bug!)"
        assert f["payload"][0] == 6, "modulation byte must be 6 (Vdsl2)"
        assert f["payload"][1] == 1, "annex byte must be 1 (B)"
        assert f["payload"][4:8] == b"\x00\x00\x01\xff", "profiles BE 0x1ff"
    acks = [f for f in wire() if f["dir"] == "h_RESP" and f["op"] == 1
            and f["seq"] in (op1s[0]["seq"], op1s[1]["seq"])]
    assert all(a["payload"] == b"\x00\x00\x00\x00" for a in acks), "op1 ack != 0"
    # Provisioning MUST be queued while the line is down (queue-at-open,
    # the OEM flow): exactly one op15 with the default untagged payload
    # (00 0000 0000 00 07d0) BEFORE any showtime. The startup cleanup
    # (op16+op6 dels) precedes it.
    adds = [f for f in wire() if f["dir"] == "b_REQ" and f["op"] in (15, 5)]
    assert adds, "queue-at-open: expected op15 while line is down"
    assert adds[0]["payload"] == bytes.fromhex("00000000" + "0000" + "07d0"), \
        f"default link must be untagged: {adds[0]['payload'].hex()}"
    # board-side: two apply bursts; second ends with open
    wait_file_contains("board_tcci.log", "wan adsl opencmd vdsl2", 10)
    log("E2 PASS (daemon left running for E3)")
    return d


def e3_restart(d):
    """Warm-cache restart: shutdown closes the line, re-start re-opens it."""
    log("E3: daemon restart with warm cache (the session-1 killer)")
    # Bring the line up first: provisioning runs at line-Up with the type
    # derived from the trained modulation (mod=6 VDSL2 → PTM link).
    set_line_state("showtime_ptm")
    wait_file_contains("daemon_e2.log", "board assigned transport vlan", 15)
    # stop: op16 ack 0 + op3 close (scoped to frames sent from here on —
    # earlier op16s are startup cleanups of absent links, ack status 1)
    n_before = len(wire())
    stop_daemon(d, "e2")
    wait_file_contains("board_tcci.log", "wan adsl close", 10)
    stop_frames = [f for f in wire()[n_before:] if f["dir"] == "b_REQ" and f["op"] in (16, 3)]
    stop_acks = {f["seq"]: f for f in wire()
                 if f["dir"] == "h_RESP" and f["op"] in (16, 3)}
    assert any(f["op"] == 16 for f in stop_frames), \
        "shutdown must tear down the provisioned link (op16)"
    for f in stop_frames:
        if f["op"] == 16:
            assert f["plen"] == 3, f"op16 payload_len {f['plen']} != 3"
            assert f["payload"][:2] == b"\x07\xd0", "op16 vlan BE 2000"
            ack = stop_acks.get(f["seq"])
            assert ack and ack["payload"] == b"\x00\x00\x00\x00", \
                f"op16 ack {ack and ack['payload'].hex()} != 0 (echo bug gave 2)"
    log("E3a PASS: shutdown = op16(ack 0) + op3 close")
    # restart with the SAME config — cache now holds exactly the intended cfg
    n_open = open(path("board_tcci.log")).read().count("wan adsl open")
    d = start_daemon("e3")
    wait_file_contains("daemon_e3.log", "line_config_up: mod=Vdsl2", 15)
    # line is still in showtime → provisioning re-runs at line-Up
    wait_file_contains("daemon_e3.log", "board assigned transport vlan", 15)
    # the second apply of the double-apply must re-open the line
    wait_file_contains("board_tcci.log", "wan adsl open", 10)
    assert open(path("board_tcci.log")).read().count("wan adsl open") > n_open, \
        "restart with identical config did NOT re-open the line"
    # board debug: second apply differed in exactly bitswap
    wait_file_contains("board_e2.log", "bitswap[1] != l_lastDslCfg.bitswap[0]", 5, quiet=True) \
        or wait_file_contains("board_e3.log", "bitswap[1] != l_lastDslCfg.bitswap[0]", 5, quiet=True)
    log("E3b PASS: identical-config restart re-opened via double apply")
    return d


def e5_showtime(board, d):
    """Showtime metrics: op2 replies carry the injected rates/SNR and the
    client renders them (showtime was injected in E3; provisioning ran
    there at line-Up with the PTM link derived from mod=6)."""
    log("E5: injected showtime -> line Up + link materialization")
    sys_before = open(path("board_sys.log")).read() if os.path.exists(path("board_sys.log")) else ""
    set_line_state("showtime_ptm")
    wait_file_contains("daemon_e3.log", "line state: ShowtimeTcSync", 15)
    time.sleep(2)   # let a poll cycle publish metrics

    # The op2 replies after showtime must carry the injected metrics:
    # near-end(=DS) 40000, far-end(=US) 12000, SNR 6.3/6.1 dB (×10).
    # This pins the empirically-verified DS/US pair order (session 3).
    import struct
    got = None
    for f in wire():
        if f["dir"] == "h_RESP" and f["op"] == 2 and f["plen"] == 63:
            p = f["payload"]
            if p[0x04] == 1:
                u32 = lambda o: struct.unpack(">I", p[o:o + 4])[0]
                if u32(0x08):
                    got = (u32(0x08), u32(0x0c), u32(0x18), u32(0x1c), p[0x05])
                    break
    assert got, "no post-showtime op2 reply with metrics"
    ds, us, snr_d, snr_u, mod = got
    assert (ds, us) == (40000, 12000), f"rates DS/US: {ds}/{us}"
    assert (snr_d, snr_u) == (63, 61), f"snr DS/US (dB×10): {snr_d}/{snr_u}"
    assert mod == 6, f"modulation: {mod}"

    # Client-side rendering: the IPC wire carries RAW dB×10 (63/61);
    # the `status` client must display dB (6.3/6.1). Retry — the daemon
    # snapshot refreshes on its poll cycle.
    rendered = None
    for _ in range(5):
        r = subprocess.run(
            ["qemu-aarch64", "-L", AROOT, f"{AROOT}/usr/sbin/rbctl-dsl", "status"],
            capture_output=True, text=True, timeout=20)
        assert r.returncode == 0, f"status client failed: {r.stdout}{r.stderr}"
        if "down_snr: 6.3" in r.stdout and "up_snr: 6.1" in r.stdout:
            rendered = r.stdout
            break
        if "down_snr: null" in r.stdout:
            time.sleep(2)   # snapshot not yet refreshed
            continue
        rendered = r.stdout   # got values but wrong rendering — fail below
        break
    assert rendered and "down_snr: 6.3" in rendered and "up_snr: 6.1" in rendered, \
        f"status client rendering (expected 6.3/6.1 dB):\n{rendered}"
    assert "down_rate: 40000" in rendered and "up_rate: 12000" in rendered, \
        f"status rates:\n{rendered}"
    # wire carries typed enum tokens, client renders names/labels
    assert "state: ShowtimeTcSync" not in rendered, "state token must be labelled client-side"
    assert "state: Showtime with TC-Layer sync" in rendered, f"status state:\n{rendered}"
    assert "modulation: VDSL2" in rendered, f"status modulation:\n{rendered}"
    assert "fw_status: idle" in rendered, f"status fw_status:\n{rendered}"
    log("E5 PASS: daemon saw Up with correctly-ordered metrics "
        f"(DS {ds} kbps @ {snr_d/10:.1f} dB), status renders enums client-side")
    return sys_before


def e6_delete_after_materialization(board, d, sys_before):
    """Stop daemon after showtime: op16 must ack 0 and the board must have
    materialized nas8 provisioning commands."""
    log("E6: teardown after showtime — materialized link deleted (ack 0)")
    sys_after = open(path("board_sys.log")).read() if os.path.exists(path("board_sys.log")) else ""
    assert "nas8" in sys_after, "board did not materialize the PTM link " \
        "(expected nas8 ifconfig/smuxctl commands):\n" + sys_after
    # scoped to the teardown: startup-cleanup op16s (absent link → ack
    # status 1) are legitimate earlier in the log
    n_before = len(wire())
    stop_daemon(d, "e3")
    frames = wire()[n_before:]
    assert any(f["dir"] == "b_REQ" and f["op"] == 16 for f in frames), \
        "teardown must remove the provisioned link (op16)"
    for i, f in enumerate(frames):
        if f["dir"] == "b_REQ" and f["op"] == 16:
            ack = next((a for a in frames if a["dir"] == "h_RESP" and a["op"] == 16
                        and a["seq"] == f["seq"]), None)
            assert ack and ack["payload"] == b"\x00\x00\x00\x00", \
                f"op16 ack after materialization: {ack and ack['payload'].hex()}"
    wait_file_contains("board_tcci.log", "wan adsl close", 10)
    log("E6 PASS")


def e7_watchdog(board):
    """Warn-only watchdog: 30 s of continuous NoSignal logs one diagnostic
    (with the restart-line hint) and the daemon sends NOTHING board-side."""
    log("E7: warn-only watchdog (silent line)")
    set_line_state("down")  # a fresh silent line (earlier scenarios may leave showtime)
    time.sleep(3)           # let the board's status thread observe the drop
    d = start_daemon("e7")
    # Silent line: no showtime → NO provisioning (the link type is
    # unknowable); sync instead on the FULL double apply (second apply
    # logs bitswap=true) — both of its opens must be inside the baseline.
    wait_file_contains(
        "daemon_e7.log",
        "line_config_up: mod=Vdsl2 annex=B profiles=0x1ff bitswap=true", 20)
    time.sleep(2)  # settle: board-side open strings land in the tcci log
    tcci = open(path("board_tcci.log")).read()
    n_open, n_close = tcci.count("wan adsl open"), tcci.count("wan adsl close")
    wait_file_contains("daemon_e7.log", "line silent (NoSignal)", 60)
    time.sleep(5)  # a full dwell past the warning — must stay quiet
    tcci = open(path("board_tcci.log")).read()
    assert tcci.count("wan adsl open") == n_open, \
        "warn-only watchdog must never re-apply"
    assert tcci.count("wan adsl close") == n_close, \
        "watchdog must never close the line"
    # exactly one warning, not a repeating log storm
    warns = open(path("daemon_e7.log")).read().count("line silent")
    assert warns == 1, f"expected exactly one warning, got {warns}"
    stop_daemon(d, "e7")
    log("E7 PASS")


def e8_fixture_replay(board):
    """Fresh board; replay the exact session-2 broken payloads and prove
    what the board made of them (the bug, on record)."""
    log("E8: session-2 wire fixtures against a fresh board")
    kill_board(board, "e2")
    time.sleep(1)
    board = start_board("e8")
    wait_file_contains("board_tcci.log", "wan adsl open", 20)
    n_tcci = open(path("board_tcci.log")).read()

    c = proto.RawClient("lan0.1.500", HOST_MAC)
    # op1 with the echo byte: board decodes mod=1 (gdmt), annex=6 (j)
    ack = c.request(1, OP1_ECHO_FIXTURE)
    assert ack == b"\x00\x00\x00\x00", f"board accepted? ack={ack.hex()}"
    time.sleep(0.5)
    tcci = open(path("board_tcci.log")).read()
    assert "wan adsl opencmd gdmt" in tcci[len(n_tcci):], \
        "echo fixture should configure G.DMT (the field shift)"
    assert "wan ghs set annex j" in tcci[len(n_tcci):]
    # op16 with the echo byte: board reads vlanId 0x1007 -> rejected
    ack = c.request(16, OP16_ECHO_FIXTURE)
    assert ack == b"\x00\x00\x00\x02", \
        f"echo op16 fixture must be rejected with status 2, got {ack.hex()}"
    log("E8 PASS: fixtures misdecode exactly as analyzed")
    return board


def e9_bad_config():
    """Static config validation through the shipped binary: a fixed ADSL
    line mode with forced PTM is physically impossible (ADSL standards
    are ATM-only) and must abort startup with a clear error. Injected
    via CLI overrides (the daemon's UCI context reads the target's
    /etc/config, not the host's). Legal overrides (ATM on VDSL2) are
    covered by unit tests."""
    log("E9: impossible transport combo (adsl+ptm) rejected at load")
    out = open(path("daemon_e9.log"), "w")
    p = subprocess.Popen(
        ["qemu-aarch64", "-L", AROOT, f"{AROOT}/usr/sbin/rbctl-dsl",
         "daemon", "-i", "lan0.1.500",
         "--line-mode", "adsl", "--xfer-mode", "ptm"],
        stdout=out, stderr=subprocess.STDOUT)
    code = p.wait(timeout=30)
    text = open(path("daemon_e9.log")).read()
    assert code != 0, f"impossible combo must abort startup:\n{text}"
    assert "ATM-only" in text, f"error must name the rule:\n{text}"
    log("E9 PASS")


def _run_daemon_uci(network_cfg, tag, extra=()):
    """Write $AROOT/etc/config/network and run the daemon to the config
    stage; returns (exit_code, log_text). Restores the previous file."""
    import shutil
    cfg = os.path.join(AROOT, "etc/config/network")
    backup = None
    if os.path.exists(cfg):
        backup = cfg + ".e9bak"
        shutil.move(cfg, backup)
    try:
        os.makedirs(os.path.dirname(cfg), exist_ok=True)
        open(cfg, "w").write(network_cfg)
        out = open(path(f"daemon_{tag}.log"), "w")
        p = subprocess.Popen(
            ["qemu-aarch64", "-L", AROOT, f"{AROOT}/usr/sbin/rbctl-dsl",
             "daemon", "-i", "lan0.1.500", *extra],
            stdout=out, stderr=subprocess.STDOUT)
        try:
            code = p.wait(timeout=30)
        except subprocess.TimeoutExpired:
            p.kill()
            code = 0  # still running = config accepted (caller decides)
            # SIGKILL leaves the daemon's queued links on the board —
            # remove them so later scenarios aren't polluted (the board
            # vid-dedup would reject same-vid links and skew assertions).
            try:
                c = proto.RawClient("lan0.1.500", HOST_MAC)
                for i in range(8):
                    c.request(16, (2000 + i).to_bytes(2, "big") + b"\x03")
            except Exception as e:
                log(f"e9b cleanup note: {e}")
        return code, open(path(f"daemon_{tag}.log")).read()
    finally:
        os.remove(cfg)
        if backup:
            shutil.move(backup, cfg)


def e9b_link_validation():
    """Data-link section validation (F1): named ptm/atm sections with the
    stable-index rule, board limits (dup vid / two untagged / dup index),
    and the ptp→ptm typo guard — all through the shipped binary."""
    log("E9b: data-link UCI validation")
    base = "config dsl 'dsl'\n  option line_mode 'vdsl'\n"

    # happy path: two indexed links — must reach the control loop (killed
    # by timeout above) and log both op15 payloads at open (F2 asserts the
    # wire in ED; here we only require config acceptance).
    ok_cfg = (base
        + "config ptm\n  option index '0'\n  option vid '100'\n"
        + "config ptm\n  option index '1'\n  option vid '836'\n")
    code, text = _run_daemon_uci(ok_cfg, "e9b_ok")
    assert "config load failed" not in text, f"valid config rejected:\n{text}"

    cases = [
        # missing index with >1 section
        ("e9b_noidx", base + "config ptm\n  option vid '100'\n"
                            "config ptm\n  option index '1'\n option vid '836'\n",
         "explicit 'index' required"),
        # duplicate index
        ("e9b_dupid", base + "config ptm\n  option index '1'\n option vid '100'\n"
                            "config ptm\n  option index '1'\n option vid '836'\n",
         "duplicate index 1"),
        # duplicate service vid (board vid-dedup)
        ("e9b_dupvid", base + "config ptm\n  option index '0'\n option vid '835'\n"
                             "config ptm\n  option index '1'\n option vid '835'\n",
         "duplicate service vid 835"),
        # two untagged (either rule fires: the vid-dedup sees two
        # None vids first, or the untagged-count rule — both correct)
        ("e9b_2untag", base + "config ptm\n  option index '0'\n"
                             "config ptm\n  option index '1'\n",
         "untagged"),
        # index out of range
        ("e9b_rng", base + "config ptm\n  option index '9'\n", "out of range"),
        # the ptp typo
        ("e9b_ptp", base + "config ptp\n  option index '0'\n",
         "'ptp' is IEEE-1588"),
        # pbit out of range
        ("e9b_pbit", base + "config ptm\n  option index '0'\n option vid '100'\n"
                           "  option pbit '9'\n", "pbit 9 out of range"),
    ]
    for tag, cfg, needle in cases:
        code, text = _run_daemon_uci(cfg, tag)
        assert code != 0 and needle in text, \
            f"{tag}: expected abort with {needle!r}:\n{text}"
    log("E9b PASS: 7 invalid configs rejected with named rules")


def ex_op15_materialization():
    """op15 materialization semantics against the real board binary.

    The board materializes a PTM data link in exactly ONE place per link:
    the line-up TRANSITION handler (dslHandleStatusUp walks the interface
    list built by op15s that arrived while the line was still DOWN). The
    op15 handler's direct path requires apiGetXdslStatus to parse
    "<status>:<type>" from xdsl_stats — a format the real driver never
    emits (space-separated), so an op15 sent AFTER showtime is only
    queued and never materialized (hollow status-0 ACK — verified
    session 4, debug_sessions/fourth/FINDINGS.md).

    This scenario pins the WORKING flow: queue op15 while the line is
    down, then inject showtime and assert the exact materialized
    commands (strace execve → board_exec.log):

      tag_enable=0         → smuxctl add bridge nas8 nas8_<pvc> 1 -1 0 0
      tag_enable=1 vid=100 → smuxctl add bridge nas8 nas8_<pvc> 3 100 <pbit> 0
                             + vconfig add eth0.1 <2000+pvc>
    """
    log("EX: op15 queued-at-down materializes at the up-transition")
    def exec_lines():
        return open(path("board_exec.log"), errors="replace").read().splitlines()

    set_line_state("down")
    time.sleep(3)   # board's status thread observes the drop

    c = proto.RawClient("lan0.1.500", HOST_MAC)
    # self-contained start: clear ALL data-link slots (earlier scenarios
    # may have left nodes; the board's vid-dedup would reject same-vid
    # links and silently skew the assertions below)
    for i in range(8):
        c.request(16, (2000 + i).to_bytes(2, "big") + b"\x03")
    time.sleep(1)
    # queue both variants while the line is DOWN (dedup: distinct vlans)
    n = len(exec_lines())
    pl_a = bytes([0]) + (0).to_bytes(2, "big") + (0).to_bytes(2, "big") \
        + b"\x00" + (2005).to_bytes(2, "big")
    r1 = c.request(15, pl_a)
    assert r1 == b"\x00\x00\x00\x00", f"op15 (untag) queue: {r1.hex()}"
    pl_b = bytes([1]) + (100).to_bytes(2, "big") + (0).to_bytes(2, "big") \
        + b"\x00" + (2006).to_bytes(2, "big")
    r2 = c.request(15, pl_b)
    assert r2 == b"\x00\x00\x00\x00", f"op15 (tag) queue: {r2.hex()}"
    time.sleep(1)

    set_line_state("showtime_ptm")     # ← the up-transition materializes
    time.sleep(5)                      # status thread polls every 2 s

    cmds = exec_lines()[n:]
    log(f"EX: materialized after transition: {cmds}")
    assert any("smuxctl add bridge nas8 nas8_5 1 -1 0 0" in l for l in cmds), \
        f"expected untag-mode smuxctl for vlan 2005 (nas8_5), got: {cmds}"
    assert any("smuxctl add bridge nas8 nas8_6 3 100 0 0" in l for l in cmds), \
        f"expected tag-mode smuxctl vid=100 for vlan 2006 (nas8_6), got: {cmds}"
    assert any("vconfig add eth0.1 2005" in l for l in cmds), \
        f"expected host-side vlan creation, got: {cmds}"

    # teardown both experiment vlans (line stays up)
    for v in (2005, 2006):
        c.request(16, v.to_bytes(2, "big") + b"\x03")

    # ── Part 2: the DIRECT path (op15 while line is UP) ──────────────
    # With the fake now writing real-format "1:2", apiGetXdslStatus
    # parses and the op15 handler materializes immediately — this is the
    # path the daemon uses for reload-added links (with a warning, F3).
    n2 = len(exec_lines())
    pl_c = bytes([1]) + (835).to_bytes(2, "big") + (0).to_bytes(2, "big") \
        + b"\x00" + (2007).to_bytes(2, "big")
    r3 = c.request(15, pl_c)
    # status 0 = materialized; 1 = queued-duplicate flavor (the board's
    # direct path both materializes AND reports the queued-node status).
    # The assertion that matters is the materialized command below.
    assert r3 in (b"\x00\x00\x00\x00", b"\x00\x00\x00\x01"), \
        f"op15 direct (tag 835): {r3.hex()}"
    time.sleep(2)
    cmds_c = [l for l in exec_lines()[n2:] if "smuxctl" in l]
    log(f"EX: direct-path materialized: {cmds_c}")
    assert any("smuxctl add bridge nas8 nas8_7 3 835 0 0" in l for l in cmds_c), \
        f"expected direct-path tag-mode smuxctl vid=835 (nas8_7), got: {cmds_c}"
    c.request(16, (2007).to_bytes(2, "big") + b"\x03")

    log("EX PASS: queue-at-down transition materialization + post-up "
        "direct path both verified (tag fields drive smuxctl mode 1/3)")


def ed_multi_link_daemon():
    """Multi-link data-plane through the DAEMON (F1+F2 end-to-end):
    two UCI ptm sections (vid 100 idx 0, vid 836 idx 1) → both op15s
    queued at open with correct tag payloads on the wire → both
    materialized at the up-transition (board_exec) → both host VLANs
    created → teardown removes both (op16s ack 0)."""
    log("ED: daemon multi-link (vid 100 + vid 836)")
    import shutil
    cfg = os.path.join(AROOT, "etc/config/network")
    backup = cfg + ".edbak"
    had = os.path.exists(cfg)
    if had:
        shutil.move(cfg, backup)
    open(cfg, "w").write(
        "config dsl 'dsl'\n  option line_mode 'vdsl'\n"
        + "config ptm\n  option index '0'\n  option vid '100'\n"
        + "config ptm\n  option index '1'\n  option vid '836'\n")
    try:
        set_line_state("down")
        time.sleep(3)   # board observes the drop
        n_wire = len(wire())
        n_exec = len(open(path("board_exec.log"), errors="replace").read().splitlines())
        d = start_daemon("ed")
        # both op15s queued at open, correct tag payloads, line DOWN
        deadline = time.time() + 20
        want = {
            (2000).to_bytes(2, "big"): bytes([1]) + (100).to_bytes(2, "big") + (0).to_bytes(2, "big") + b"\x00",
            (2001).to_bytes(2, "big"): bytes([1]) + (836).to_bytes(2, "big") + (0).to_bytes(2, "big") + b"\x00",
        }
        got = {}
        while time.time() < deadline and len(got) < 2:
            for f in wire()[n_wire:]:
                if f["dir"] == "b_REQ" and f["op"] == 15 and f["plen"] == 8:
                    got[f["payload"][6:8]] = f["payload"][:6]
            time.sleep(0.5)
        for vlan_be, tag in want.items():
            p = got.get(vlan_be)
            assert p == tag, \
                f"op15 for vlan {vlan_be.hex()}: payload {p.hex() if p else None} != {tag.hex()}"
        log("ED: both op15 queued at open with board-tag payloads")

        # up-transition materializes BOTH links
        set_line_state("showtime_ptm")
        time.sleep(5)
        cmds = open(path("board_exec.log"), errors="replace").read().splitlines()[n_exec:]
        assert any("smuxctl add bridge nas8 nas8_0 3 100 0 0" in l for l in cmds), \
            f"nas8_0 tag 100 missing: {cmds}"
        assert any("smuxctl add bridge nas8 nas8_1 3 836 0 0" in l for l in cmds), \
            f"nas8_1 tag 836 missing: {cmds}"
        wait_file_contains("daemon_ed.log", "board assigned transport vlan: 2000", 10)
        wait_file_contains("daemon_ed.log", "board assigned transport vlan: 2001", 10)
        log("ED: both links materialized + both host VLANs announced")

        # teardown removes both (scoped wire window)
        n2 = len(wire())
        stop_daemon(d, "ed")
        frames = wire()[n2:]
        dels = [f["payload"][:2] for f in frames
                if f["dir"] == "b_REQ" and f["op"] == 16]
        assert (2000).to_bytes(2, "big") in dels and (2001).to_bytes(2, "big") in dels, \
            f"teardown must op16 both vlans, got: {[d.hex() for d in dels]}"
        log("ED PASS: 2-link config → queue-at-open (tag payloads) → "
            "both materialized → both torn down")
    finally:
        os.remove(cfg)
        if had:
            shutil.move(backup, cfg)


def ea_auto_swap():
    """F4 end-to-end: auto line mode queues the implicit default link as
    optimistic PTM at open; an ADSL showtime must swap it to ATM post-up
    (op16 the queued PTM node + op5 with the ATM params) with the F3
    direct-path warning logged."""
    log("EA: auto mode — optimistic PTM queued, ADSL showtime swaps to ATM")
    import shutil
    cfg = os.path.join(AROOT, "etc/config/network")
    backup = cfg + ".eabak"
    had = os.path.exists(cfg)
    if had:
        shutil.move(cfg, backup)
    open(cfg, "w").write(
        "config dsl 'dsl'\n  option line_mode 'auto'\n  option annex 'auto'\n")
    try:
        set_line_state("down")
        time.sleep(3)   # board observes the drop
        n_wire = len(wire())
        d = start_daemon("ea")
        # optimistic PTM default link queued while down
        wait_file_contains("daemon_ea.log", "ptm_link_add: vlan=2000 untagged", 15)
        adds = [f for f in wire()[n_wire:] if f["dir"] == "b_REQ" and f["op"] == 15]
        assert adds and adds[0]["payload"] == \
            bytes.fromhex("00000000" + "0000" + "07d0"), \
            f"optimistic queue payload: {[f['payload'].hex() for f in adds]}"
        log("EA: optimistic untagged PTM queued at open")

        # ADSL2+ showtime (ATM family) → swap
        n_wire = len(wire())
        set_line_state("showtime_adsl")
        wait_file_contains("daemon_ea.log", "auto correction:", 15)
        text = open(path("daemon_ea.log")).read()
        assert "the default link is Ptm but the line trained Atm" in text, text
        assert "auto type correction" in text, \
            f"F3 post-up warning missing:\n{text}"
        log("EA: auto correction + F3 warning logged")

        # wire: op16(2000) teardown of the queued PTM node, then op5
        frames = wire()[n_wire:]
        assert any(f["dir"] == "b_REQ" and f["op"] == 16
                   and f["payload"][:2] == b"\x07\xd0" for f in frames), \
            "swap must op16 the optimistic PTM node"
        op5s = [f for f in frames if f["dir"] == "b_REQ" and f["op"] == 5]
        assert op5s and op5s[0]["plen"] == 24, "swap must send op5 (24-B)"
        p5 = op5s[0]["payload"]
        assert p5[0x12:0x14] == b"\x07\xd0", "op5 vlan 2000 BE"
        assert p5[0x01] == 8 and p5[0x02:0x04] == b"\x00\x23", \
            f"op5 default PVC 8/35: {p5.hex()}"   # atm_params defaults
        log("EA: op16(2000) + op5(vlan 2000, pvc 8/35) on the wire")

        stop_daemon(d, "ea")
        # cleanup: the swapped ATM node stays queued on the board (its
        # materialization needs real nas* ifaces) — clear all slots so
        # later runs start clean (op16 PTM + op6 ATM per vlan).
        c = proto.RawClient("lan0.1.500", HOST_MAC)
        for i in range(8):
            v = (2000 + i).to_bytes(2, "big") + b"\x03"
            c.request(16, v)
            c.request(6, v)
        log("EA PASS: optimistic queue → trained-ADSL swap (op16+op5, warned)")
    finally:
        os.remove(cfg)
        if had:
            shutil.move(backup, cfg)
        set_line_state("down")   # also restores the ptm driver module


def main():
    log(f"LOGDIR={LOGDIR} scenarios={SCENARIOS}")
    threading.Thread(target=strace_tailer, args=("all",), daemon=True).start()
    set_line_state("down")
    board = start_board("e2")
    sniffers = start_sniffers()
    # Reaper: an assertion crash must not orphan the qemu/strace/sniffer
    # children (they run in their own sessions and survive the driver).
    import atexit
    peers = {"board": board, "sniffers": sniffers, "daemon": None}

    def reap():
        d = peers["daemon"]
        if d is not None and d.poll() is None:
            try:
                os.kill(d.pid, signal.SIGTERM)
                d.wait(timeout=10)
            except Exception:
                try:
                    d.kill()
                except Exception:
                    pass
        for s in peers["sniffers"]:
            try:
                s.kill()
            except Exception:
                pass
        if peers["board"].poll() is None:
            kill_board(peers["board"], "reap")
    atexit.register(reap)

    d = None
    def track(proc):
        peers["daemon"] = proc
        return proc  # None = nothing running (scenario stopped it itself)
    if "E1" in SCENARIOS:
        e1_board_boot(board)
    if "E2" in SCENARIOS:
        d = track(e2_bringup(board))
    if "E3" in SCENARIOS:
        d = track(e3_restart(d))       # returns the NEW daemon
    if "E5" in SCENARIOS:
        sys_before = e5_showtime(board, d)
        if "E6" in SCENARIOS:
            e6_delete_after_materialization(board, d, sys_before)
            d = None
    if "E7" in SCENARIOS:
        d = track(e7_watchdog(board))
    if "E8" in SCENARIOS:
        if d is not None:
            stop_daemon(d, "last")
            d = None
        board = e8_fixture_replay(board)
    if "E9" in SCENARIOS:
        e9_bad_config()
        e9b_link_validation()
    if "ED" in SCENARIOS:
        ed_multi_link_daemon()
    if "EX" in SCENARIOS:
        if d is not None:
            stop_daemon(d, "ex")   # EX talks to the board directly
            d = None
        ex_op15_materialization()
    if "EA" in SCENARIOS:
        ea_auto_swap()
    peers["board"] = board

    for s in sniffers:
        s.kill()
    kill_board(board, "final")
    log("ALL PASS")


if __name__ == "__main__":
    main()
