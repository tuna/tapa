//! Global flags shared by every subcommand. Mirrors the
//! `entry_point` click group in `tapa/__main__.py`.

use std::path::PathBuf;

use clap::Parser;

use crate::chain::Step;
use crate::options::DEFAULT_CLANG_FORMAT_QUOTA;
use crate::steps::version::VERSION;

/// Top-level CLI: globals + an optional first chained step.
#[derive(Debug, Parser)]
#[command(
    name = "tapa",
    about = "The TAPA compiler.",
    version = VERSION,
    disable_help_subcommand = true,
    after_help = SUBCOMMAND_HELP,
)]
pub struct Cli {
    #[command(flatten)]
    pub globals: GlobalArgs,
    #[command(subcommand)]
    pub step: Option<Step>,
}

/// Global flags accepted before any subcommand. Field names map 1:1 to the
/// click options on `tapa/__main__.py::entry_point`.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "tapa",
    about = "The TAPA compiler.",
    version = VERSION,
    disable_help_subcommand = true,
    after_help = SUBCOMMAND_HELP,
)]
pub struct GlobalArgs {
    /// Increase logging verbosity.
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Decrease logging verbosity.
    #[arg(short = 'q', long = "quiet", action = clap::ArgAction::Count)]
    pub quiet: u8,

    /// Specify working directory.
    #[arg(
        short = 'w',
        long = "work-dir",
        value_name = "DIR",
        default_value = "./work.out/",
    )]
    pub work_dir: PathBuf,

    /// Specify temporary directory; cleaned up after execution.
    #[arg(long = "temp-dir", value_name = "DIR")]
    pub temp_dir: Option<PathBuf>,

    /// Limit clang-format to the first few bytes of code.
    #[arg(
        long = "clang-format-quota-in-bytes",
        default_value_t = DEFAULT_CLANG_FORMAT_QUOTA,
    )]
    pub clang_format_quota_in_bytes: u64,

    /// Remote Linux host for vendor tools (`user@host[:port]`).
    #[arg(long = "remote-host")]
    pub remote_host: Option<String>,

    /// Path to SSH private key for remote host authentication.
    #[arg(long = "remote-key-file")]
    pub remote_key_file: Option<String>,

    /// Path to Xilinx `settings64.sh` on the remote host.
    #[arg(long = "remote-xilinx-settings")]
    pub remote_xilinx_settings: Option<String>,

    /// Directory for OpenSSH multiplex control sockets.
    #[arg(long = "remote-ssh-control-dir")]
    pub remote_ssh_control_dir: Option<String>,

    /// OpenSSH `ControlPersist` duration (e.g. `30m`, `4h`).
    #[arg(long = "remote-ssh-control-persist")]
    pub remote_ssh_control_persist: Option<String>,

    /// Disable OpenSSH multiplexing for remote execution.
    #[arg(long = "remote-disable-ssh-mux")]
    pub remote_disable_ssh_mux: bool,
}

const SUBCOMMAND_HELP: &str = "\
Subcommands (chainable, processed left-to-right like click's chained group):

  analyze                       Analyze a TAPA program; persist graph.json + design.json.
  synth                         Synthesize the analyzed program into RTL.
  pack                          Pack the generated RTL into a Xilinx `.xo`.
  floorplan                     Apply a floorplan to the program.
  generate-floorplan            Run AutoBridge to generate floorplan solutions.
  compile                       analyze + synth + pack in one invocation.
  compile-with-floorplan-dse    Floorplan DSE driver chaining the above.
  g++                           Invoke g++ with TAPA include / link flags.
  version                       Print TAPA version.

Use `tapa <subcommand> --help` for per-subcommand options.
";
