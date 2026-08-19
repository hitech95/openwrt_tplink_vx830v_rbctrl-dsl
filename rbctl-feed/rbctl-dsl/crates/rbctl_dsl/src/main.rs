//! rbctl-dsl — EcoNet xDSL board configuration daemon.

mod board;
mod cli;
mod config;
mod constants;
mod daemon;
mod logging;
mod pcap;
mod selftest;
mod ubus_obj;
mod utils;

use clap::{Parser, Subcommand};
use config::CliOverrides;

#[derive(Parser)]
#[command(
    name = "rbctl-dsl",
    about = "EcoNet xDSL board configuration daemon",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the daemon
    Daemon(DaemonArgs),
    /// Print live line state and metrics
    Status,
    /// Reload UCI config in the running daemon
    Reload,
    /// Bounce the DSL line (down then up)
    RestartLine,
    /// Shut down the running daemon
    Stop,
    /// Upload firmware to the remote board
    FirmwareUpgrade {
        /// Path to the raw 2RDH firmware image
        path: String,
    },
    /// Exercise socket + VLAN + board opcodes, then exit
    Selftest(CommonArgs),
    /// Listen for 0x88B5 / 0x88B6 frames (passive, no send). Optionally write
    /// captured frames to a pcap file with `--dump <FILE>`.
    Sniff(SniffArgs),
}

/// Arguments shared by selftest and sniff modes.
#[derive(clap::Args)]
struct CommonArgs {
    /// Management VLAN interface
    #[arg(short = 'i', long, default_value = "lan0.500")]
    config_iface: String,
}

/// Arguments for sniff mode.
#[derive(clap::Args)]
struct SniffArgs {
    #[command(flatten)]
    common: CommonArgs,

    /// Write captured 0x88B5 / 0x88B6 frames to `FILE` as a pcap capture
    /// (`LINKTYPE_ETHERNET`, openable directly in Wireshark / tcpdump).
    #[arg(long, value_name = "FILE")]
    dump: Option<String>,
}

/// Arguments for daemon mode.
#[derive(clap::Args)]
struct DaemonArgs {
    /// Management VLAN interface
    #[arg(short = 'i', long, default_value = "lan0.500")]
    config_iface: String,

    /// Hotplug notify script path
    #[arg(short, long)]
    notify: Option<String>,

    /// Log to syslog instead of stdout (used by procd)
    #[arg(long)]
    syslog: bool,

    /// Override UCI annex (a, b, j, m)
    #[arg(long)]
    annex: Option<String>,

    /// Override UCI line_mode (adsl, vdsl)
    #[arg(long)]
    line_mode: Option<String>,

    /// Override UCI tone (av, 8a, 17a, ...)
    #[arg(long)]
    tone: Option<String>,

    /// Override UCI xfer_mode (ptm, atm)
    #[arg(long)]
    xfer_mode: Option<String>,

    /// Override ATM VPI
    #[arg(long)]
    vpi: Option<String>,

    /// Override ATM VCI
    #[arg(long)]
    vci: Option<String>,

    /// Override ATM encapsulation (llc, vcmux)
    #[arg(long)]
    encaps: Option<String>,

    /// Override ATM payload type (bridged, routed, pppoa)
    #[arg(long)]
    payload: Option<String>,

    /// Override bitswap enable (0 or 1)
    #[arg(long)]
    bitswap: Option<bool>,

    /// Override SRA enable (0 or 1)
    #[arg(long)]
    sra: Option<bool>,

    /// Override transport VLAN base index (0–7, VLAN id = base + 2000)
    #[arg(short = 't', long)]
    transport_vlan: Option<u8>,

    /// Monitor mode: only poll the board (op2/op4) and publish status.
    /// Never sends op1/op3/op5/op15 — the board keeps whatever config it
    /// booted with. Useful for observing a line the board trained on its
    /// own defaults without the daemon disrupting it.
    #[arg(long)]
    monitor: bool,
}

impl DaemonArgs {
    fn to_overrides(&self) -> CliOverrides {
        CliOverrides {
            annex: self.annex.clone(),
            line_mode: self.line_mode.clone(),
            tone: self.tone.clone(),
            xfer_mode: self.xfer_mode.clone(),
            vpi: self.vpi.clone(),
            vci: self.vci.clone(),
            encaps: self.encaps.clone(),
            payload: self.payload.clone(),
            bitswap: self.bitswap,
            sra: self.sra,
            transport_vlan: self.transport_vlan,
        }
    }
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Command::Daemon(args) => {
            if args.syslog {
                logging::init_syslog();
            } else {
                logging::init_stderr();
            }
        }
        _ => logging::init_stderr(),
    }

    match cli.command {
        Command::Daemon(args) => {
            let overrides = args.to_overrides();
            let code = daemon::run(
                &args.config_iface,
                args.notify.as_deref(),
                &overrides,
                args.monitor,
            );
            std::process::exit(code);
        }
        Command::Status => ipc_exit(daemon::ipc::IpcCmd::Status),
        Command::Reload => ipc_exit(daemon::ipc::IpcCmd::Reload),
        Command::RestartLine => ipc_exit(daemon::ipc::IpcCmd::Restart),
        Command::Stop => ipc_exit(daemon::ipc::IpcCmd::Stop),
        Command::FirmwareUpgrade { path } => {
            // Quick local header check before contacting the daemon
            match std::fs::read(&path) {
                Ok(data) => {
                    if let Err(e) = rbctl_proto::oem::firmware::validate_image(&data) {
                        eprintln!("error: {e}");
                        std::process::exit(1);
                    }
                    let hdr = rbctl_proto::oem::firmware::parse_header(&data).unwrap();
                    let ver = rbctl_proto::oem::firmware::version_str(&hdr);
                    eprintln!("firmware: {ver} ({} bytes, {:.1} MB)", data.len(), data.len() as f64 / 1048576.0);
                }
                Err(e) => {
                    eprintln!("error: cannot read '{path}': {e}");
                    std::process::exit(1);
                }
            }

            eprintln!("connecting to daemon...");
            match cli::send_firmware(&path, &mut |line| {
                eprintln!("  {line}");
            }) {
                Ok(result) if result.starts_with("DONE") => {
                    eprintln!("{result}");
                    std::process::exit(0);
                }
                Ok(result) => {
                    eprintln!("{result}");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("error: cannot connect to daemon: {e}");
                    std::process::exit(1);
                }
            }
        }
        Command::Selftest(args) => {
            let code = selftest::Selftest::run(&args.config_iface);
            log::info!("selftest exit code: {code}");
            std::process::exit(code);
        }
        Command::Sniff(args) => {
            selftest::sniff(&args.common.config_iface, args.dump.as_deref());
        }
    }
}

fn ipc_exit(cmd: daemon::ipc::IpcCmd) {
    match cli::send_command(daemon::SOCK_PATH, cmd) {
        Ok(response) => {
            print!("{response}");
            std::process::exit(if response.contains("ERR") { 1 } else { 0 });
        }
        Err(e) => {
            log::error!("cannot connect to daemon at {}: {e}", daemon::SOCK_PATH);
            std::process::exit(1);
        }
    }
}

