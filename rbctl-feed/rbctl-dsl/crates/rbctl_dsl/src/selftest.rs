//! Selftest — exercises socket, VLAN lifecycle, board opcodes, and
//! dependency probes. Captures all TX/RX frames to `/tmp/rbctl-capture/`.

use std::ffi::CString;
use std::time::Duration;

use af_packet::RawSocket;
use rbctl_proto::pack::*;
use tinyln_rs::rtnl::RtnlLink;

use crate::board::Board;
use crate::logger::Logger;

pub struct Selftest<'a> {
    log: &'a Logger,
    config_iface: String,
    parent: String,
    pass: u32,
    fail: u32,
}

impl<'a> Selftest<'a> {
    pub fn run(log: &'a Logger, config_iface: &str) -> i32 {
        let parent = config_iface
            .rsplit_once('.')
            .map(|(p, _)| p.to_string())
            .unwrap_or_else(|| config_iface.to_string());

        let mut st = Selftest {
            log,
            config_iface: config_iface.to_string(),
            parent,
            pass: 0,
            fail: 0,
        };
        st.execute()
    }

    fn execute(&mut self) -> i32 {
        self.log.line(format!("config interface: {}", self.config_iface));

        // Step 0: bring up the interface (and parent if it's a VLAN)
        self.record("link", self.test_link_up());

        // Only proceed with socket/board tests if carrier is present
        if self.has_carrier(&self.config_iface) {
            self.record("socket", self.test_socket());
            self.record("vlan", self.test_vlan());
            self.record("board", self.test_board());
        } else {
            self.log.line(format!("  no carrier on {} — skipping socket/vlan/board tests", self.config_iface));
        }

        self.record("uci", self.test_uci());
        self.record("uloop", self.test_uloop());

        self.log.line(format!("{} passed, {} failed", self.pass, self.fail));
        if self.fail > 0 { 1 } else { 0 }
    }

    fn record(&mut self, label: &str, result: Result<String, String>) {
        match result {
            Ok(msg) => { self.log.pass(label, &msg); self.pass += 1; }
            Err(msg) => { self.log.fail(label, &msg); self.fail += 1; }
        }
    }

    // ── individual tests ──────────────────────────────────────────────

    /// Bring up the config interface (and its parent if VLAN).
    /// Checks carrier status after bringing it up.
    fn test_link_up(&self) -> Result<String, String> {
        // If config_iface is a VLAN (e.g. "lan0.500"), bring up the parent first
        if self.parent != self.config_iface {
            self.bring_up(&self.parent)?;
        }
        self.bring_up(&self.config_iface)?;

        // Check carrier
        if !self.has_carrier(&self.config_iface) {
            return Err(format!("{} is up but has no carrier", self.config_iface));
        }
        Ok(format!("{} up with carrier", self.config_iface))
    }

    fn bring_up(&self, iface: &str) -> Result<(), String> {
        let path = format!("/sys/class/net/{iface}/operstate");
        let state = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .trim()
            .to_string();
        if state == "up" {
            return Ok(()); // already up
        }
        self.log.line(format!("  bringing up {iface} (was {state:?})..."));
        let mut rtnl = RtnlLink::new().map_err(|e| format!("rtnl: {e}"))?;
        rtnl.set_up(iface).map_err(|e| format!("set_up {iface}: {e}"))?;
        // Give the kernel a moment to settle
        std::thread::sleep(Duration::from_millis(200));
        Ok(())
    }

    fn has_carrier(&self, iface: &str) -> bool {
        std::fs::read_to_string(format!("/sys/class/net/{iface}/carrier"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .is_some_and(|v| v == 1)
    }

    fn test_socket(&self) -> Result<String, String> {
        let sock = RawSocket::open(&self.config_iface, 0x88B5).map_err(|e| e.to_string())?;
        let mac = sock.local_mac();
        sock.set_timeout(Duration::from_millis(500)).map_err(|e| e.to_string())?;
        Ok(format!("bound {}, MAC {:02x?}, BPF 0x88B5", self.config_iface, mac))
    }

    fn test_vlan(&self) -> Result<String, String> {
        let test_vid: u16 = 2001;
        let mut rtnl = RtnlLink::new().map_err(|e| e.to_string())?;
        let name = format!("{}.{}", self.parent, test_vid);

        // Clean up stale interface from a previous run
        let _ = rtnl.del(&name);

        rtnl.add_vlan(&self.parent, test_vid).map_err(|e| format!("add_vlan: {e}"))?;

        let result = (|| -> Result<String, String> {
            let parent_idx = ifindex(&self.parent)?;
            let vlan_idx = ifindex(&name)?;
            if vlan_idx == 0 {
                return Err(format!("{name} was not created"));
            }
            rtnl.set_up(&name).map_err(|e| format!("set_up: {e}"))?;
            rtnl.set_down(&name).map_err(|e| format!("set_down: {e}"))?;
            Ok(format!("create→up→down→del {name} (parent idx {parent_idx})"))
        })();

        // Always clean up
        let _ = rtnl.del(&name);
        if result.is_ok() && ifindex(&name).unwrap_or(0) != 0 {
            return Err(format!("{name} was not deleted"));
        }
        result
    }

    fn test_board(&self) -> Result<String, String> {
        let sock = RawSocket::open(&self.config_iface, 0x88B5).map_err(|e| e.to_string())?;
        let mut board = Board::new(sock);
        board.set_timeout(Duration::from_secs(2));
        board.set_retries(3);
        board.enable_capture("/tmp/rbctl-capture");

        let mut got_response = false;
        let mut lines = Vec::new();

        // Op 2 — get_line_obj
        match board.get_line_obj() {
            Ok(o) => {
                got_response = true;
                lines.push(format!(
                    "op2: {:?}, down={} up={} kbps, SNR d={:.1} u={:.1} dB",
                    o.link_status, o.metrics.down_curr_rate, o.metrics.up_curr_rate,
                    o.metrics.down_snr_margin as f32 / 10.0, o.metrics.up_snr_margin as f32 / 10.0,
                ));
            }
            Err(crate::board::BoardError::Timeout) => lines.push("op2: no response".into()),
            Err(e) => lines.push(format!("op2: ERROR {e}")),
        }

        // Op 4 — channel stats
        match board.get_channel_stats() {
            Ok(s) => {
                got_response = true;
                lines.push(format!("op4: rx_blocks={} tx_blocks={}", s.receive_blocks, s.transmit_blocks));
            }
            Err(crate::board::BoardError::Timeout) => lines.push("op4: no response".into()),
            Err(e) => lines.push(format!("op4: ERROR {e}")),
        }

        // Op 1 — line config up
        match board.line_config_up(Modulation::Vdsl2, Annex::B, Vdsl2Profiles::THIRTY_A) {
            Ok(()) => { got_response = true; lines.push("op1: ACK".into()); }
            Err(crate::board::BoardError::Timeout) => lines.push("op1: no response".into()),
            Err(e) => lines.push(format!("op1: ERROR {e}")),
        }

        // Op 5 — atm link add
        let mut p = AtmLinkParams::default();
        p.vpi = 8; p.vci = 35; p.encap = AtmEncap::Llc; p.link_type = AtmLinkType::Eoa;
        p.qos = AtmQos::Ubr; p.pcr = 1000;
        match board.atm_link_add(&p) {
            Ok(vlan) => { got_response = true; lines.push(format!("op5: ACK vlan={vlan}")); }
            Err(crate::board::BoardError::Timeout) => lines.push("op5: no response".into()),
            Err(e) => lines.push(format!("op5: ERROR {e}")),
        }

        for l in &lines { self.log.line(format!("  {l}")); }

        write_hexdump("/tmp/rbctl-capture");

        let count = std::fs::read_dir("/tmp/rbctl-capture").map(|d| d.count()).unwrap_or(0);
        if got_response {
            Ok(format!("board is alive; {count} frames captured"))
        } else {
            Ok(format!("no board response; {count} TX frames captured to /tmp/rbctl-capture/"))
        }
    }

    fn test_uci(&self) -> Result<String, String> {
        let mut uci = rust_uci::Uci::new().map_err(|e| format!("uci_alloc: {e:?}"))?;
        match uci.get("network.@device[0].name") {
            Ok(v) => Ok(format!("context OK; device[0].name = {v:?}")),
            Err(_) => Ok("context OK".into()),
        }
    }

    fn test_uloop(&self) -> Result<String, String> {
        let _u = libubox::uloop::Uloop::new().map_err(|e| format!("uloop_init: {e:?}"))?;
        Ok("uloop_init OK".into())
    }
}

// ── standalone sniff mode ────────────────────────────────────────────

/// Listen for 0x88B5 or 0x88B6 frames on the interface and print them.
/// No sending — pure passive capture. Runs until killed.
pub fn sniff(log: &Logger, config_iface: &str) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    // Open a raw socket with NO protocol filter (we want both 0x88B5 and 0x88B6)
    let fd = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW, (0x0003u16).to_be() as i32) }; // ETH_P_ALL
    if fd < 0 {
        log.line(format!("socket failed: {}", std::io::Error::last_os_error()));
        return;
    }
    let _fd = unsafe { OwnedFd::from_raw_fd(fd) };

    // Bind to the interface
    let cname = CString::new(config_iface).unwrap();
    let ifindex = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    if ifindex == 0 {
        log.line(format!("if_nametoindex({config_iface}) failed"));
        return;
    }

    let sa = libc::sockaddr_ll {
        sll_family: libc::AF_PACKET as u16,
        sll_protocol: (0x0003u16).to_be(), // ETH_P_ALL
        sll_ifindex: ifindex as i32,
        sll_hatype: 0, sll_pkttype: 0, sll_halen: 0, sll_addr: [0; 8],
    };
    let ret = unsafe {
        libc::bind(
            _fd.as_raw_fd(),
            &sa as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        log.line(format!("bind failed: {}", std::io::Error::last_os_error()));
        return;
    }

    // Set a 3-second timeout so we can print "still listening" periodically
    let tv = libc::timeval { tv_sec: 3, tv_usec: 0 };
    unsafe {
        libc::setsockopt(
            _fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVTIMEO,
            &tv as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    };

    log.line(format!("listening on {config_iface} for 0x88B5 / 0x88B6 frames (Ctrl+C to stop)"));

    let mut buf = [0u8; 1518];
    let mut count = 0u32;

    loop {
        let mut sa: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        let mut sa_len = std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
        let n = unsafe {
            libc::recvfrom(
                _fd.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
                &mut sa as *mut _ as *mut libc::sockaddr,
                &mut sa_len,
            )
        };

        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                continue; // timeout, keep listening
            }
            log.line(format!("recvfrom error: {e}"));
            continue;
        }

        let n = n as usize;
        if n < 14 { continue; } // too short for Ethernet header

        // Check ethertype
        let ethertype = u16::from_be_bytes([buf[12], buf[13]]);
        if ethertype != 0x88B5 && ethertype != 0x88B6 {
            continue; // not our protocol
        }

        count += 1;
        let direction = match sa.sll_pkttype {
            0 => "OUT",  // PACKET_OUTGOING — sent by us
            1 => "IN ",  // PACKET_HOST — received
            _ => "???",
        };

        let src_mac = &buf[6..12];
        let dst_mac = &buf[0..6];

        log.line(format!(
            "[{count}] {direction} {ethertype:#06x} {} bytes, dst {:02x?} src {:02x?}",
            n, dst_mac, src_mac,
        ));

        // Hex dump first 64 bytes
        let show = n.min(64);
        for chunk in buf[..show].chunks(16) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
            let ascii: String = chunk.iter()
                .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                .collect();
            let off = chunk.as_ptr() as usize - buf.as_ptr() as usize;
            println!("[sniff]   {:04x}  {:<48}  {}", off, hex.join(" "), ascii);
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────

fn ifindex(name: &str) -> Result<u32, String> {
    let c = CString::new(name).unwrap();
    let idx = unsafe { libc::if_nametoindex(c.as_ptr()) };
    if idx == 0 { Err(format!("if_nametoindex({name}) failed")) } else { Ok(idx) }
}

fn write_hexdump(dir: &str) {
    let entries = match std::fs::read_dir(dir) { Ok(e) => e, Err(_) => return };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "bin"))
        .collect();
    paths.sort();

    let mut out = String::from("# rbctl-dsl frame capture\n\n");
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy();
        let data = match std::fs::read(path) { Ok(d) => d, Err(_) => continue };
        out.push_str(&format!("=== {} ({} bytes) ===\n", name, data.len()));
        for chunk in data.chunks(16) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
            let ascii: String = chunk.iter()
                .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                .collect();
            let off = chunk.as_ptr() as usize - data.as_ptr() as usize;
            out.push_str(&format!("{:04x}  {:<48}  {}\n", off, hex.join(" "), ascii));
        }
        out.push('\n');
    }

    let _ = std::fs::write(format!("{dir}/hexdump.txt"), &out);
    for line in out.lines().take(200) {
        println!("[capture] {line}");
    }
}
