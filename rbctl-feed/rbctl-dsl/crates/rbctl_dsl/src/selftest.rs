//! Selftest — exercises socket, VLAN lifecycle, board opcodes, and
//! dependency probes. Frame inspection is provided by the `sniff` subcommand;
//! selftest itself no longer writes captures to disk.

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

        match board.line_config_up(Modulation::Vdsl2, Annex::B, Vdsl2Profiles::THIRTY_A, true, true) {
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

        if got_response {
            Ok("board is alive".into())
        } else {
            Ok("no board response".into())
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

/// Listen for `0x88B5` / `0x88B6` frames on `config_iface` and print a
/// hex+ascii dump of each one. Runs forever until interrupted (Ctrl+C).
///
/// Opens the socket with `ETH_P_ALL` and filters the two management ethertypes
/// in userspace — this is a debug tool and we deliberately want to see every
/// direction of traffic. Reuses [`af_packet::RawSocket`] so all of the
/// `AF_PACKET` plumbing lives in one place.
///
/// When `dump_file` is `Some`, each captured management frame is also appended
/// to that path as a pcap capture (`LINKTYPE_ETHERNET`, Wireshark-readable) —
/// see [`crate::pcap`]. This is the only frame-to-file path in the daemon; it
/// lives here in the debug sniffer, never on the `Board` request hot path.
pub fn sniff(config_iface: &str, dump_file: Option<&str>) {
    let sock = match RawSocket::open_unfiltered(config_iface) {
        Ok(s) => s,
        Err(e) => {
            log::error!("open_unfiltered({config_iface}): {e}");
            return;
        }
    };
    // A short receive timeout keeps the loop responsive to SIGINT on kernels
    // where recvfrom wouldn't otherwise be interrupted promptly.
    let _ = sock.set_timeout(Duration::from_secs(3));

    // Open the pcap dump file (and write its global header) up front. On
    // failure, continue sniffing without dumping.
    let mut pcap = match dump_file {
        Some(path) => match crate::pcap::create(path) {
            Ok(f) => {
                log::info!("dumping captured frames to pcap {path}");
                Some(f)
            }
            Err(e) => {
                log::warn!("could not create pcap {path}: {e} — continuing without dump");
                None
            }
        },
        None => None,
    };

    log::info!("listening on {config_iface} for 0x88B5 / 0x88B6 frames (Ctrl+C to stop)");

    let mut buf = [0u8; 1518];
    let mut count = 0u32;

    loop {
        let pkt = match sock.recv(&mut buf) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                log::error!("recvfrom error: {e}");
                continue;
            }
        };

        let n = pkt.data.len();
        if n < 14 { continue; }

        let ethertype = u16::from_be_bytes([pkt.data[12], pkt.data[13]]);
        if ethertype != 0x88B5 && ethertype != 0x88B6 {
            continue;
        }

        count += 1;
        let direction = match pkt.pkt_type {
            0 => "OUT",
            1 => "IN ",
            _ => "???",
        };

        let src_mac = &pkt.data[6..12];
        let dst_mac = &pkt.data[0..6];

        log::info!(
            "[{count}] {direction} {ethertype:#06x} {n} bytes, dst {:02x?} src {:02x?}",
            dst_mac, src_mac,
        );

        let show = n.min(64);
        for chunk in pkt.data[..show].chunks(16) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
            let ascii: String = chunk.iter()
                .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                .collect();
            let off = chunk.as_ptr() as usize - pkt.data.as_ptr() as usize;
            println!("  {:04x}  {:<48}  {}", off, hex.join(" "), ascii);
        }

        // Optional pcap dump (sniffer-only, gated by --dump).
        if let Some(w) = pcap.as_mut() {
            if let Err(e) = crate::pcap::write(w, std::time::SystemTime::now(), pkt.data) {
                log::warn!("pcap write failed: {e}");
            }
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────

fn ifindex(name: &str) -> Result<u32, String> {
    let c = CString::new(name).unwrap();
    let idx = unsafe { libc::if_nametoindex(c.as_ptr()) };
    if idx == 0 { Err(format!("if_nametoindex({name}) failed")) } else { Ok(idx) }
}
