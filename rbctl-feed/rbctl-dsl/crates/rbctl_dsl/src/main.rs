//! rbctl-dsl — EcoNet xDSL board configuration daemon.
//!
//! Usage:
//!   rbctl-dsl [--config-iface <iface>] [--selftest]
//!
//! Without `--selftest`, starts the daemon (Phase 3 — not yet implemented).

mod board;
mod logger;
mod selftest;
mod transport;

use logger::Logger;

const HELP: &str = "\
rbctl-dsl — EcoNet xDSL board configuration daemon

Usage: rbctl-dsl [-i <iface>] [--selftest] [--sniff]

Options:
  -i, --config-iface <iface>  Management VLAN interface (default: lan0.500)
  --selftest                  Exercise socket + VLAN + board opcodes, then exit
  --sniff                     Listen for 0x88B5/0x88B6 frames (passive, no send)
  -h, --help                  Show this help
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut config_iface = String::from("lan0.500");
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

    // Phase 3: daemon mode (UCI config → board init → uloop + ubus)
    log.line("daemon mode not yet implemented. Use --selftest.");
    std::process::exit(1);
}
