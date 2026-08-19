//! Passive sniffer — listens for `0x88B5` / `0x88B6` frames on the
//! management VLAN and prints a hex+ascii dump of each one. Runs forever
//! until interrupted (Ctrl+C).
//!
//! Opens the socket with `ETH_P_ALL` and filters the two management
//! ethertypes in userspace — this is a debug tool and we deliberately want
//! to see every direction of traffic. Reuses [`af_packet::RawSocket`] so
//! all of the `AF_PACKET` plumbing lives in one place.
//!
//! When `dump_file` is `Some`, each captured management frame is also
//! appended to that path as a pcap capture (`LINKTYPE_ETHERNET`,
//! Wireshark-readable) — see [`crate::diag::pcap`].

use std::time::Duration;

use af_packet::RawSocket;

use crate::constants;

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
        Some(path) => match super::pcap::create(path) {
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

    let mut buf = [0u8; constants::BUF_ETH_MTU];
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
        if ethertype != constants::BOARD_ETHERTYPE_CMD && ethertype != constants::BOARD_ETHERTYPE_RSP {
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

        let show = n.min(constants::BUF_HEX_DUMP_MAX);
        for chunk in pkt.data[..show].chunks(constants::BUF_HEX_DUMP_ROW_WIDTH) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
            let ascii: String = chunk.iter()
                .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                .collect();
            let off = chunk.as_ptr() as usize - pkt.data.as_ptr() as usize;
            println!("  {:04x}  {:<48}  {}", off, hex.join(" "), ascii);
        }

        // Optional pcap dump (sniffer-only, gated by --dump).
        if let Some(w) = pcap.as_mut() {
            if let Err(e) = super::pcap::write(w, std::time::SystemTime::now(), pkt.data) {
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
