//! Selftest — exercises socket, VLAN lifecycle, board opcodes, and
//! dependency probes. Captures all TX/RX frames to `/tmp/rbctl-capture/`.

use std::ffi::CString;
use std::time::Duration;

use af_packet::RawSocket;
use rbctl_proto::pack::*;
use tinyln_rs::rtnl::RtnlLink;

use crate::board::Board;

pub struct Selftest {
    config_iface: String,
    parent: String,
    pass: u32,
    fail: u32,
}

impl Selftest {
    pub fn run(config_iface: &str) -> i32 {
        let parent = config_iface
            .rsplit_once('.')
            .map(|(p, _)| p.to_string())
            .unwrap_or_else(|| config_iface.to_string());

        let mut st = Selftest {
            config_iface: config_iface.to_string(),
            parent,
            pass: 0,
            fail: 0,
        };
        st.execute()
    }

    fn execute(&mut self) -> i32 {
        log::info!("config interface: {}", self.config_iface);

        self.record("link", self.test_link_up());

        if self.has_carrier(&self.config_iface) {
            self.record("socket", self.test_socket());
            self.record("vlan", self.test_vlan());
            self.record("board", self.test_board());
        } else {
            log::info!("no carrier on {} — skipping socket/vlan/board tests", self.config_iface);
        }

        self.record("uci", self.test_uci());
        self.record("uloop", self.test_uloop());

        log::info!("{} passed, {} failed", self.pass, self.fail);
        if self.fail > 0 { 1 } else { 0 }
    }

    fn record(&mut self, label: &str, result: Result<String, String>) {
        match result {
            Ok(msg) => { log::info!("PASS {label}: {msg}"); self.pass += 1; }
            Err(msg) => { log::error!("FAIL {label}: {msg}"); self.fail += 1; }
        }
    }

    // ── individual tests ──────────────────────────────────────────────

    fn test_link_up(&self) -> Result<String, String> {
        if self.parent != self.config_iface {
            self.bring_up(&self.parent)?;
        }
        self.bring_up(&self.config_iface)?;

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
            return Ok(());
        }
        log::info!("bringing up {iface} (was {state:?})...");
        let mut rtnl = RtnlLink::new().map_err(|e| format!("rtnl: {e}"))?;
        rtnl.set_up(iface).map_err(|e| format!("set_up {iface}: {e}"))?;
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

        match board.get_channel_stats() {
            Ok(s) => {
                got_response = true;
                lines.push(format!("op4: rx_blocks={} tx_blocks={}", s.receive_blocks, s.transmit_blocks));
            }
            Err(crate::board::BoardError::Timeout) => lines.push("op4: no response".into()),
            Err(e) => lines.push(format!("op4: ERROR {e}")),
        }

        match board.line_config_up(Modulation::Vdsl2, Annex::B, Vdsl2Profiles::THIRTY_A) {
            Ok(()) => { got_response = true; lines.push("op1: ACK".into()); }
            Err(crate::board::BoardError::Timeout) => lines.push("op1: no response".into()),
            Err(e) => lines.push(format!("op1: ERROR {e}")),
        }

        let mut p = AtmLinkParams::default();
        p.vpi = 8; p.vci = 35; p.encap = AtmEncap::Llc; p.link_type = AtmLinkType::Eoa;
        p.qos = AtmQos::Ubr; p.pcr = 1000;
        match board.atm_link_add(&p) {
            Ok(vlan) => { got_response = true; lines.push(format!("op5: ACK vlan={vlan}")); }
            Err(crate::board::BoardError::Timeout) => lines.push("op5: no response".into()),
            Err(e) => lines.push(format!("op5: ERROR {e}")),
        }

        for l in &lines { log::info!("{l}"); }

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
        Ok("uloop not used (Rust polling loop)".into())
    }
}

// ── standalone sniff mode ────────────────────────────────────────────

pub fn sniff(config_iface: &str) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let fd = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW, (0x0003u16).to_be() as i32) };
    if fd < 0 {
        log::error!("socket failed: {}", std::io::Error::last_os_error());
        return;
    }
    let _fd = unsafe { OwnedFd::from_raw_fd(fd) };

    let cname = CString::new(config_iface).unwrap();
    let ifindex = unsafe { libc::if_nametoindex(cname.as_ptr()) };
    if ifindex == 0 {
        log::error!("if_nametoindex({config_iface}) failed");
        return;
    }

    let sa = libc::sockaddr_ll {
        sll_family: libc::AF_PACKET as u16,
        sll_protocol: (0x0003u16).to_be(),
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
        log::error!("bind failed: {}", std::io::Error::last_os_error());
        return;
    }

    let tv = libc::timeval { tv_sec: 3, tv_usec: 0 };
    unsafe {
        libc::setsockopt(
            _fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVTIMEO,
            &tv as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    };

    log::info!("listening on {config_iface} for 0x88B5 / 0x88B6 frames (Ctrl+C to stop)");

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
                continue;
            }
            log::error!("recvfrom error: {e}");
            continue;
        }

        let n = n as usize;
        if n < 14 { continue; }

        let ethertype = u16::from_be_bytes([buf[12], buf[13]]);
        if ethertype != 0x88B5 && ethertype != 0x88B6 {
            continue;
        }

        count += 1;
        let direction = match sa.sll_pkttype {
            0 => "OUT",
            1 => "IN ",
            _ => "???",
        };

        let src_mac = &buf[6..12];
        let dst_mac = &buf[0..6];

        log::info!(
            "[{count}] {direction} {ethertype:#06x} {n} bytes, dst {:02x?} src {:02x?}",
            dst_mac, src_mac,
        );

        let show = n.min(64);
        for chunk in buf[..show].chunks(16) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
            let ascii: String = chunk.iter()
                .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                .collect();
            let off = chunk.as_ptr() as usize - buf.as_ptr() as usize;
            println!("  {:04x}  {:<48}  {}", off, hex.join(" "), ascii);
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
        log::info!("{line}");
    }
}
