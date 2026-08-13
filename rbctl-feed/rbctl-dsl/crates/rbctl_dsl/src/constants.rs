//! Centralized constants for the rbctl-dsl daemon.
//!
//! All shared numeric literals live here with identifying prefixes:
//! - `BOARD_`  — board protocol (timeouts, retries, ethertypes)
//! - `POLL_`   — polling intervals and staleness thresholds
//! - `VLAN_`   — VLAN configuration
//! - `IPC_`    — IPC control socket
//! - `FW_`     — firmware upgrade
//! - `ATM_`    — ATM defaults
//! - `BUF_`    — buffer sizes
//! - `DAEMON_` — daemon lifecycle
//! - `UBUS_`   — ubus connection

use std::time::Duration;

// ── BOARD: board protocol ───────────────────────────────────────────────

/// Default per-request timeout for board opcodes (matches OEM binary).
pub const BOARD_DEFAULT_TIMEOUT: Duration = Duration::from_millis(2000);

/// Default retransmit count for board requests.
pub const BOARD_DEFAULT_RETRIES: u8 = 3;

/// Retry budget dropped to this at shutdown for best-effort line_config_down.
pub const BOARD_SHUTDOWN_RETRIES: u8 = 0;

/// Management ethertype for local-command board protocol (0x88B5).
pub const BOARD_ETHERTYPE_CMD: u16 = 0x88B5;

/// Management ethertype for remote-command protocol (0x88B6, used by sniff).
pub const BOARD_ETHERTYPE_RSP: u16 = 0x88B6;

/// Link-delete type byte sent in op6/op16 payloads (3 = delete).
pub const BOARD_LINK_DEL_TYPE_DELETE: u8 = 3;

// ── POLL: polling and timing ────────────────────────────────────────────

/// Interval between board status polls (op2) in the worker thread.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Main control-loop and worker tick — both run at this cadence.
pub const POLL_TICK: Duration = Duration::from_millis(50);

/// If no successful op2 response within this duration, the board is
/// considered unreachable (line_obj cleared, status shows error).
pub const POLL_STALE_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between line_config_down and line_config_up during restart-line.
pub const POLL_RESTART_DELAY: Duration = Duration::from_secs(1);

// ── VLAN: VLAN configuration ────────────────────────────────────────────

/// Transport VLAN id = base index + this offset (e.g. base 0 → VLAN 2000).
pub const VLAN_TRANSPORT_BASE_OFFSET: u16 = 2000;

/// Maximum transport_vlan_base value (0–7, yielding VLANs 2000–2007).
pub const VLAN_TRANSPORT_BASE_MAX: u8 = 7;

// ── IPC: IPC control socket ─────────────────────────────────────────────

/// Unix socket path for the daemon's IPC control channel.
pub const IPC_SOCK_PATH: &str = "/var/run/rbctl-dsl.sock";

/// Read timeout for IPC command/reply (both server and client side).
pub const IPC_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum bytes accepted on a single IPC command line — bounds `read_line`
/// so a client cannot grow memory by withholding `\n` (D8).
pub const IPC_MAX_CMD_LINE: usize = 4096;

/// Read timeout for firmware upload progress streaming (long — upload
/// can take minutes).
pub const IPC_FW_UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Write timeout for streaming firmware progress back to the IPC client.
pub const IPC_FW_STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

// ── FW: firmware upgrade ────────────────────────────────────────────────

/// 2RDH firmware container header size — content starts at this offset.
pub const FW_HEADER_SIZE: usize = 0x100;

/// Progress callback fires at roughly every this many percent during stream.
pub const FW_PROGRESS_STEP_PCT: u8 = 10;

// ── ATM: ATM link defaults ──────────────────────────────────────────────

/// Default ATM VPI when no UCI atm-bridge section is present.
pub const ATM_DEFAULT_VPI: u8 = 8;

/// Default ATM VCI when no UCI atm-bridge section is present.
pub const ATM_DEFAULT_VCI: u16 = 35;

// ── BUF: buffer sizes ───────────────────────────────────────────────────

/// Standard Ethernet MTU including header (used for sniff recv buffer).
pub const BUF_ETH_MTU: usize = 1518;

/// ubus transport read chunk size.
pub const BUF_UBUS_READ_CHUNK: usize = 2048;

/// Maximum bytes displayed in selftest hex dump.
pub const BUF_HEX_DUMP_MAX: usize = 64;

/// Row width (bytes per line) in selftest hex dump.
pub const BUF_HEX_DUMP_ROW_WIDTH: usize = 16;

/// Initial capacity for ubus transport rx_buf.
pub const BUF_UBUS_RX_CAPACITY: usize = 1024;

// ── DAEMON: daemon lifecycle ────────────────────────────────────────────

/// How long the main thread waits for the board worker to finish its
/// best-effort shutdown before abandoning it.
pub const DAEMON_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(3);

// ── UBUS: ubus connection ───────────────────────────────────────────────

/// Candidate ubus socket paths, tried in order.
pub const UBUS_SOCKET_PATHS: &[&str] = &[
    "/var/run/ubus.sock",
    "/var/run/ubus/ubus.sock",
    "/tmp/run/ubus.sock",
    "/tmp/run/ubus/ubus.sock",
];
