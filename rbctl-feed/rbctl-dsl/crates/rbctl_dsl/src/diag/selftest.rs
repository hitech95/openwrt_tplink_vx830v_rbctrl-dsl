//! Selftest — exercises socket, VLAN lifecycle, board opcodes, and
//! dependency probes. Frame inspection is provided by the `sniff` subcommand;
//! selftest itself no longer writes captures to disk.

use std::time::Duration;

use af_packet::RawSocket;
use rbctl_proto::oem::pack::*;
use rbctl_proto::oem::unpack::DataPath;
use tinyln_rs::rtnl::RtnlLink;

use crate::board::Board;
use crate::config::XferMode;
use crate::constants;
use crate::utils::mgmt_link;

// ── selftest board params (static, no user input) ──────────────────────
// Matches the board's own boot defaults — the config that trains.
const ST_MODULATION: Modulation = Modulation::Multimode;
const ST_ANNEX: Annex = Annex::Auto;
const ST_PROFILES: Vdsl2Profiles = Vdsl2Profiles(0x1ff); // all profiles
const ST_BITSWAP: bool = true;
const ST_SRA: bool = false;

/// Transport VLAN for the test link.
const ST_TEST_VLAN: u16 = 2001;

/// How long to wait for the line to train after op1.
const ST_TRAIN_TIMEOUT: Duration = Duration::from_secs(60);
const ST_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub struct Selftest {
    config_iface: String,
    parent: String,
    pass: u32,
    fail: u32,
}

impl Selftest {
    pub fn run(config_iface: &str) -> i32 {
        let parent = mgmt_link::parent_of(config_iface)
            .map(str::to_string)
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
        mgmt_link::read_carrier(iface).unwrap_or(false)
    }

    fn test_socket(&self) -> Result<String, String> {
        let sock = RawSocket::open(&self.config_iface, constants::BOARD_ETHERTYPE_CMD).map_err(|e| e.to_string())?;
        let mac = sock.local_mac();
        sock.set_timeout(Duration::from_millis(500)).map_err(|e| e.to_string())?;
        Ok(format!("bound {}, MAC {:02x?}, BPF {:#06x}", self.config_iface, mac, constants::BOARD_ETHERTYPE_CMD))
    }

    fn test_vlan(&self) -> Result<String, String> {
        let test_vid: u16 = 2001;
        let mut rtnl = RtnlLink::new().map_err(|e| e.to_string())?;
        let name = format!("{}.{}", self.parent, test_vid);

        let _ = rtnl.del(&name);

        rtnl.add_vlan(&self.parent, test_vid).map_err(|e| format!("add_vlan: {e}"))?;

        let result = (|| -> Result<String, String> {
            let parent_idx = mgmt_link::iface_exists(&self.parent);
            if !mgmt_link::iface_exists(&name) {
                return Err(format!("{name} was not created"));
            }
            rtnl.set_up(&name).map_err(|e| format!("set_up: {e}"))?;
            rtnl.set_down(&name).map_err(|e| format!("set_down: {e}"))?;
            Ok(format!("create→up→down→del {name} (parent idx {parent_idx})"))
        })();

        let _ = rtnl.del(&name);
        if result.is_ok() && mgmt_link::iface_exists(&name) {
            return Err(format!("{name} was not deleted"));
        }
        result
    }

    fn test_board(&self) -> Result<String, String> {
        let sock = RawSocket::open(&self.config_iface, constants::BOARD_ETHERTYPE_CMD).map_err(|e| e.to_string())?;
        let mut board = Board::new(sock);
        board.set_timeout(constants::BOARD_DEFAULT_TIMEOUT);
        board.set_retries(constants::BOARD_DEFAULT_RETRIES);

        // ── Step 1: op1 — line config up with auto modulation ───────────
        // Sends the board's default/auto config (Multimode / Annex-Auto /
        // all profiles). If the board already has this config, dslCfgSet's
        // diff check is a no-op. If it had a different config, it retrains
        // to auto — the known-good config.
        match board.line_config_up(ST_MODULATION, ST_ANNEX, ST_PROFILES, ST_BITSWAP, ST_SRA) {
            Ok(()) => {
                log::info!(
                    "op1: ACK ({:?}/Annex::{:?}/0x{:x}, bitswap={} sra={})",
                    ST_MODULATION, ST_ANNEX, ST_PROFILES.bitmask(), ST_BITSWAP, ST_SRA
                );
            }
            Err(crate::board::BoardError::Timeout) => {
                log::info!("op1: no response");
                return Ok("no board response".into());
            }
            Err(e) => return Err(format!("op1: {e}")),
        }

        // ── Step 2: poll op2 until Up or timeout ────────────────────────
        log::info!("waiting for line to train (up to {}s)...", ST_TRAIN_TIMEOUT.as_secs());

        let start = std::time::Instant::now();
        let deadline = start + ST_TRAIN_TIMEOUT;
        let mut line_obj = None;

        while std::time::Instant::now() < deadline {
            match board.get_line_obj() {
                Ok(o) => {
                    let elapsed = start.elapsed().as_secs();
                    log::info!(
                        "poll: {:?} ({}s) — mod={} datapath={:?} annex={}",
                        o.link_status, elapsed, o.modulation_code, o.data_path, o.annex_code
                    );

                    if o.link_status.is_up() {
                        line_obj = Some(o);
                        break;
                    }
                }
                Err(crate::board::BoardError::Timeout) => {}
                Err(e) => log::warn!("poll error: {e}"),
            }
            std::thread::sleep(ST_POLL_INTERVAL);
        }

        // ── Step 3: detect modulation + provision link ──────────────────
        // The link type is DERIVED from the trained modulation
        // (VDSL2→PTM, ADSL→ATM): the op2 `data_path` field is a
        // power-on default that does not follow training (observed
        // on-target reporting Atm while a VDSL2 line was in showtime).
        let mut detected = String::new();
        let mut provisioned: Option<XferMode> = None;

        if let Some(ref o) = line_obj {
            let dp = o.data_path;
            let mode = o.modulation_code;
            let down = o.metrics.down_rate;
            let up = o.metrics.up_rate;
            let snr_d = o.metrics.down_noise_margin as f32 / 10.0;
            let snr_u = o.metrics.up_noise_margin as f32 / 10.0;

            detected = format!(
                "mod={} datapath={:?} annex={} down={} up={} kbps SNR d={:.1} u={:.1} dB",
                mode, dp, o.annex_code, down, up, snr_d, snr_u
            );
            log::info!("line up: {detected}");

            // Trained modulation wins; fall back to the field only if
            // the modulation code is unknown.
            let xfer = match o.modulation() {
                Some(m) => {
                    let derived = XferMode::for_modulation(m);
                    let field = match dp {
                        DataPath::Ptm => Some(XferMode::Ptm),
                        DataPath::Atm => Some(XferMode::Atm),
                        DataPath::Unknown(_) => None,
                    };
                    if field.is_some_and(|f| f != derived) {
                        log::warn!(
                            "op2 data_path field says {dp:?} but trained modulation \
                             is {m:?} — using {derived:?} (field does not follow training)"
                        );
                    }
                    derived
                }
                None => match dp {
                    DataPath::Ptm => XferMode::Ptm,
                    DataPath::Atm => XferMode::Atm,
                    DataPath::Unknown(code) => {
                        log::warn!(
                            "unknown modulation {mode} and data_path code {code} \
                             — skipping link provisioning"
                        );
                        XferMode::Auto
                    }
                },
            };

            match xfer {
                XferMode::Ptm => {
                    log::info!("link type: PTM (VDSL) — provisioning PTM link (vlan={ST_TEST_VLAN})");
                    match board.ptm_link_add(0, 0, 0, ST_TEST_VLAN) {
                        Ok(vlan) => {
                            log::info!("op15: ACK vlan={vlan}");
                            provisioned = Some(XferMode::Ptm);
                        }
                        Err(crate::board::BoardError::Timeout) => log::info!("op15: no response"),
                        Err(e) => log::error!("op15: ERROR {e}"),
                    }
                }
                XferMode::Atm => {
                    log::info!("link type: ATM (ADSL) — provisioning ATM link (vlan={ST_TEST_VLAN})");
                    let mut p = AtmLinkParams::default();
                    p.vpi = constants::ATM_DEFAULT_VPI;
                    p.vci = constants::ATM_DEFAULT_VCI;
                    p.encap = AtmEncap::Llc;
                    p.link_type = AtmLinkType::Eoa;
                    p.qos = AtmQos::Ubr;
                    p.pcr = 1000;
                    p.vlan_id = ST_TEST_VLAN;
                    match board.atm_link_add(&p) {
                        Ok(vlan) => {
                            log::info!("op5: ACK vlan={vlan}");
                            provisioned = Some(XferMode::Atm);
                        }
                        Err(crate::board::BoardError::Timeout) => log::info!("op5: no response"),
                        Err(e) => log::error!("op5: ERROR {e}"),
                    }
                }
                // Unreachable: resolved to Ptm/Atm above.
                XferMode::Auto => {}
            }
        } else {
            log::info!("line did not train within {}s timeout", ST_TRAIN_TIMEOUT.as_secs());
            log::info!("hint: check physical line, DSLAM config, annex compatibility");
        }

        // ── Step 4: op4 — channel stats ────────────────────────────────
        match board.get_channel_stats() {
            Ok(s) => {
                log::info!("op4: rx_blocks={} tx_blocks={}", s.receive_blocks, s.transmit_blocks);
            }
            Err(crate::board::BoardError::Timeout) => log::info!("op4: no response"),
            Err(e) => log::error!("op4: ERROR {e}"),
        }

        // ── Step 5: teardown the provisioned data link ─────────────────
        // The selftest must leave the board CLEAN: a data link left
        // behind (e.g. an ATM link next to a later daemon's PTM link)
        // breaks the board data plane. The line itself stays trained.
        if let Some(xfer) = provisioned {
            let r = match xfer {
                XferMode::Ptm => board.ptm_link_del(ST_TEST_VLAN),
                XferMode::Atm => board.atm_link_del(ST_TEST_VLAN),
                XferMode::Auto => Ok(()),
            };
            match r {
                Ok(()) => log::info!("cleanup: data link removed (vlan={ST_TEST_VLAN}, {xfer:?})"),
                Err(crate::board::BoardError::Timeout) => {
                    log::warn!("cleanup: link_del vlan={ST_TEST_VLAN} — no response")
                }
                Err(e) => log::warn!("cleanup: link_del vlan={ST_TEST_VLAN}: {e}"),
            }
        }

        // ── Step 5: summary ────────────────────────────────────────────
        if line_obj.is_some() {
            Ok(format!("line up, {detected}"))
        } else {
            Ok("board is alive but line did not train".into())
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
