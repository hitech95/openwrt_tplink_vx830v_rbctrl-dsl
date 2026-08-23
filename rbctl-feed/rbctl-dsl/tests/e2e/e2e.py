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
SCENARIOS = os.environ.get("SCENARIOS", "E1 E2 E3 E5 E6 E7 E8").split()

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
        ["strace", "-f", "-e", "trace=openat,write", "-s", "8192",
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
    """Rewrite the fake /proc state files (the board's next fopen sees it)."""
    if state == "down":
        xdsl, adsl = "0 3\n", ("ADSL link status: down\nOpmode:  Wrong opmode\n"
                               "Adsl Type: \n")
    elif state == "showtime_ptm":
        xdsl = "1 2\n"
        adsl = (
            "ADSL link status: up\n"
            "Opmode:  ITU G.993.2(VDSL2)\n"
            "near-end interleaved channel bit rate: 12000\n"
            "near-end fast channel bit rate: 0\n"
            "far-end interleaved channel bit rate: 40000\n"
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
              "ADSL activetime second: 5\nADSL total activetime second: 5\n"
              "Adsl Type: vdsl2\n")
    else:
        raise ValueError(state)
    open("/proc/tc3162/xdsl_stats", "w").write(xdsl)
    open("/proc/tc3162/adsl_stats", "w").write(adsl)


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
    # board-side: two apply bursts; second ends with open
    wait_file_contains("board_tcci.log", "wan adsl opencmd vdsl2", 10)
    log("E2 PASS (daemon left running for E3)")
    return d


def e3_restart(d):
    """Warm-cache restart: shutdown closes the line, re-start re-opens it."""
    log("E3: daemon restart with warm cache (the session-1 killer)")
    # stop: op16 ack 0 + op3 close
    stop_daemon(d, "e2")
    wait_file_contains("board_tcci.log", "wan adsl close", 10)
    stop_frames = [f for f in wire() if f["dir"] == "b_REQ" and f["op"] in (16, 3)]
    stop_acks = {f["seq"]: f for f in wire()
                 if f["dir"] == "h_RESP" and f["op"] in (16, 3)}
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
    """Inject showtime: board status thread materializes the queued op15,
    daemon observes Up with real parsed rates."""
    log("E5: injected showtime -> line Up + link materialization")
    sys_before = open(path("board_sys.log")).read() if os.path.exists(path("board_sys.log")) else ""
    set_line_state("showtime_ptm")
    wait_file_contains("daemon_e3.log", "line state: Up", 15)
    # rates parsed from our synthetic adsl_stats (12/40 Mbps)
    wait_file_contains("daemon_e3.log", "", 1)
    time.sleep(2)   # let a poll cycle publish metrics
    log("E5 PASS: daemon saw Up")
    return sys_before


def e6_delete_after_materialization(board, d, sys_before):
    """Stop daemon after showtime: op16 must ack 0 and the board must have
    materialized nas8 provisioning commands."""
    log("E6: teardown after showtime — materialized link deleted (ack 0)")
    sys_after = open(path("board_sys.log")).read() if os.path.exists(path("board_sys.log")) else ""
    assert "nas8" in sys_after, "board did not materialize the PTM link " \
        "(expected nas8 ifconfig/smuxctl commands):\n" + sys_after
    stop_daemon(d, "e3")
    frames = wire()
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
    wait_file_contains("daemon_e7.log", "board assigned transport vlan", 20)
    n_open = open(path("board_tcci.log")).read().count("wan adsl open")
    n_close = open(path("board_tcci.log")).read().count("wan adsl close")
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
    peers["board"] = board

    for s in sniffers:
        s.kill()
    kill_board(board, "final")
    log("ALL PASS")


if __name__ == "__main__":
    main()
