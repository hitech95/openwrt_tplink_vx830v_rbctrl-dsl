//! rbctl-dsl — EcoNet xDSL board configuration daemon.

mod board;
mod daemon;
mod hotplug;
mod ipc;
mod selftest;
mod transport;
mod ubus_obj;
mod uci_cfg;

use clap::{Parser, Subcommand};
use uci_cfg::CliOverrides;

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
    /// Exercise socket + VLAN + board opcodes, then exit
    Selftest(CommonArgs),
    /// Listen for 0x88B5 frames (passive, no send)
    Sniff(CommonArgs),
}

/// Arguments shared by selftest and sniff modes.
#[derive(clap::Args)]
struct CommonArgs {
    /// Management VLAN interface
    #[arg(short = 'i', long, default_value = "lan0.500")]
    config_iface: String,
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

    /// Board transport VLAN id
    #[arg(short = 't', long, default_value_t = 2001)]
    transport_vlan: u16,

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
        }
    }
}

fn main() {
    let cli = Cli::parse();
    init_logging();

    match cli.command {
        Command::Daemon(args) => {
            let code = daemon::run(
                &args.config_iface,
                args.notify.as_deref(),
                args.transport_vlan,
                &args.to_overrides(),
            );
            std::process::exit(code);
        }
        Command::Status => ipc_exit(ipc::IpcCmd::Status),
        Command::Reload => ipc_exit(ipc::IpcCmd::Reload),
        Command::RestartLine => ipc_exit(ipc::IpcCmd::Restart),
        Command::Stop => ipc_exit(ipc::IpcCmd::Stop),
        Command::Selftest(args) => {
            let code = selftest::Selftest::run(&args.config_iface);
            log::info!("selftest exit code: {code}");
            std::process::exit(code);
        }
        Command::Sniff(args) => {
            selftest::sniff(&args.config_iface);
        }
    }
}

fn ipc_exit(cmd: ipc::IpcCmd) {
    match ipc::send_command(ipc::SOCK_PATH, cmd) {
        Ok(response) => {
            print!("{response}");
            std::process::exit(if response.contains("ERR") { 1 } else { 0 });
        }
        Err(e) => {
            log::error!("cannot connect to daemon at {}: {e}", ipc::SOCK_PATH);
            std::process::exit(1);
        }
    }
}

fn init_logging() {
    use std::io::Write;
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    )
    .format(|buf, record| {
        let target = record.target().rsplit("::").next().unwrap_or(record.target());
        writeln!(buf, "[{} {}] {}", record.level(), target, record.args())
    })
    .init();
}
