//! rbctl-dsl — EcoNet xDSL board configuration daemon.
//!
//! Usage:
//!   rbctl-dsl [-i <iface>] [-n <script>] [-t <vlan>] [--selftest] [--sniff]
//!
//! Without `--selftest` or `--sniff`, starts the daemon.

mod board;
mod daemon;
mod hotplug;
mod logger;
mod selftest;
mod transport;
mod ubus_obj;
mod uci_cfg;

use logger::Logger;

const HELP: &str = "\
rbctl-dsl — EcoNet xDSL board configuration daemon

Usage: rbctl-dsl [-i <iface>] [-n <script>] [-t <vlan>] [--selftest] [--sniff]

Options:
  -i, --config-iface <iface>  Management VLAN interface (default: lan0.500)
  -n, --notify <script>       Hotplug notify script (e.g. /sbin/dsl_notify.sh)
  -t, --transport-vlan <id>   Board transport VLAN id (default: 2001)
  --selftest                  Exercise socket + VLAN + board opcodes, then exit
  --sniff                     Listen for 0x88B5/0x88B6 frames (passive, no send)
  -h, --help                  Show this help
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut config_iface = String::from("lan0.500");
    let mut notify_script: Option<String> = None;
    let mut transport_vlan: u16 = 2001;
    let mut selftest = false;
    let mut sniff = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-i" | "--config-iface" => {
                i += 1;
                if i < args.len() {
                    config_iface = args[i].clone();
                }
            }
            "-n" | "--notify" => {
                i += 1;
                if i < args.len() {
                    notify_script = Some(args[i].clone());
                }
            }
            "-t" | "--transport-vlan" => {
                i += 1;
                if i < args.len() {
                    transport_vlan = args[i].parse().unwrap_or(2001);
                }
            }
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

    // Daemon mode
    let code = daemon::run(
        &config_iface,
        notify_script.as_deref(),
        transport_vlan,
    );
    std::process::exit(code);
}
