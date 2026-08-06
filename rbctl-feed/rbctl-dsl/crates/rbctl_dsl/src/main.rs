//! rbctl-dsl — EcoNet xDSL board configuration daemon.
//!
//! ## Usage
//!
//! **Daemon mode** (no existing instance):
//! ```text
//! rbctl-dsl [-i <iface>] [-n <script>] [-t <vlan>] [--annex b] [--line-mode vdsl] ...
//! ```
//!
//! **Control mode** (existing instance running):
//! ```text
//! rbctl-dsl status         → print live line state
//! rbctl-dsl reload         → reload UCI config
//! rbctl-dsl restart-line   → bounce the DSL line
//! rbctl-dsl stop           → shut down daemon
//! ```

mod board;
mod daemon;
mod hotplug;
mod ipc;
mod logger;
mod selftest;
mod transport;
mod ubus_obj;
mod uci_cfg;

use logger::Logger;
use uci_cfg::CliOverrides;

const HELP: &str = "\
rbctl-dsl — EcoNet xDSL board configuration daemon

Usage:
  rbctl-dsl [options]              Start as daemon (or control if running)
  rbctl-dsl <command>              Send command to running daemon

Commands (when daemon is running):
  status                           Print live line state and exit
  reload                           Reload UCI config
  restart-line                     Bounce the DSL line (down → up)
  stop                             Shut down the daemon

Options (daemon mode):
  -i, --config-iface <iface>       Management VLAN interface (default: lan0.500)
  -n, --notify <script>            Hotplug notify script (e.g. /sbin/dsl_notify.sh)
  -t, --transport-vlan <id>        Board transport VLAN id (default: 2001)
      --annex <a|b|j|m>            Override UCI annex
      --line-mode <adsl|vdsl>      Override UCI line_mode
      --tone <av|8a|17a|...>       Override UCI tone (VDSL2 profile bitmask)
      --xfer-mode <ptm|atm>        Override UCI xfer_mode
      --vpi <n>                    Override ATM VPI (default: 8)
      --vci <n>                    Override ATM VCI (default: 35)
      --encaps <llc|vcmux>         Override ATM encapsulation
      --payload <bridged|routed|pppoa>  Override ATM payload type

Other modes:
      --selftest                   Exercise socket + VLAN + board opcodes, then exit
      --sniff                      Listen for 0x88B5 frames (passive, no send)
  -h, --help                       Show this help
";

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Quick check: first positional arg might be a control command
    if args.len() == 2 {
        if let Some(cmd) = ipc::IpcCmd::from_arg(&args[1]) {
            // Control client mode
            match ipc::send_command(ipc::SOCK_PATH, cmd) {
                Ok(response) => {
                    print!("{response}");
                    if !response.contains("ERR") {
                        std::process::exit(0);
                    }
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Cannot connect to daemon at {}: {e}", ipc::SOCK_PATH);
                    std::process::exit(1);
                }
            }
        }
    }

    // Parse full arg list for daemon mode / selftest / sniff
    let mut config_iface = String::from("lan0.500");
    let mut notify_script: Option<String> = None;
    let mut transport_vlan: u16 = 2001;
    let mut selftest = false;
    let mut sniff = false;
    let mut overrides = CliOverrides::default();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-i" | "--config-iface" => { i += 1; if i < args.len() { config_iface = args[i].clone(); } }
            "-n" | "--notify" => { i += 1; if i < args.len() { notify_script = Some(args[i].clone()); } }
            "-t" | "--transport-vlan" => { i += 1; if i < args.len() { transport_vlan = args[i].parse().unwrap_or(2001); } }
            "--annex" => { i += 1; if i < args.len() { overrides.annex = Some(args[i].clone()); } }
            "--line-mode" => { i += 1; if i < args.len() { overrides.line_mode = Some(args[i].clone()); } }
            "--tone" => { i += 1; if i < args.len() { overrides.tone = Some(args[i].clone()); } }
            "--xfer-mode" => { i += 1; if i < args.len() { overrides.xfer_mode = Some(args[i].clone()); } }
            "--vpi" => { i += 1; if i < args.len() { overrides.vpi = Some(args[i].clone()); } }
            "--vci" => { i += 1; if i < args.len() { overrides.vci = Some(args[i].clone()); } }
            "--encaps" => { i += 1; if i < args.len() { overrides.encaps = Some(args[i].clone()); } }
            "--payload" => { i += 1; if i < args.len() { overrides.payload = Some(args[i].clone()); } }
            "--selftest" => selftest = true,
            "--sniff" => sniff = true,
            _ => {}
        }
        i += 1;
    }

    let log = Logger::new("rbctl");

    if sniff {
        let sniff_log = Logger::new("sniff");
        selftest::sniff(&sniff_log, &config_iface);
        std::process::exit(0);
    }

    if selftest {
        let st_log = Logger::new("selftest");
        let code = selftest::Selftest::run(&st_log, &config_iface);
        log.line(format!("selftest exit code: {code}"));
        std::process::exit(code);
    }

    // If a daemon is already running and no overrides specified, show status
    if overrides == CliOverrides::default() && ipc::daemon_running() {
        match ipc::send_command(ipc::SOCK_PATH, ipc::IpcCmd::Status) {
            Ok(response) => {
                print!("{response}");
                std::process::exit(0);
            }
            Err(_) => { /* socket exists but can't connect — stale, proceed to start */ }
        }
    }

    // Daemon mode
    let code = daemon::run(
        &config_iface,
        notify_script.as_deref(),
        transport_vlan,
        &overrides,
    );
    std::process::exit(code);
}
