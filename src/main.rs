mod manifest;
mod platform;
mod recipe;
mod resolve;
mod state;
mod util;
mod version;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (build ",
    env!("XBREW_BUILD_ID"),
    ")"
);

#[derive(Parser)]
#[command(
    name = "xbrew",
    version = VERSION,
    about = "One install/uninstall over brew, pacman, and recipes (macOS + Arch)."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install one or more packages (auto-selects the right backend)
    Install {
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
    },
    /// Uninstall one or more packages (routes to whatever installed them)
    Uninstall {
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,
    },
    /// List packages xbrew has installed
    List,
    /// Install everything in one or more YAML manifests (Brewfile-style),
    /// honoring `trust:` taps and `name >= x` version constraints
    Bundle {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
    },
    /// Print the installed version of a tracked package
    Version { name: String },
    /// Check installed vs latest for tracked packages and update them (y/n/all)
    Update {
        #[arg(num_args = 0..)]
        names: Vec<String>,
    },
    /// Show a package and how it would be installed here
    Info { name: String },
    /// Search brew, pacman, and recipes
    Search { query: String },
    /// Update xbrew itself to the latest build
    SelfUpdate,
}

fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Install { names } => resolve::install_many(&names),
        Cmd::Uninstall { names } => resolve::uninstall_many(&names),
        Cmd::List => resolve::list(),
        Cmd::Bundle { files } => resolve::bundle(&files),
        Cmd::Version { name } => resolve::version(&name),
        Cmd::Update { names } => resolve::update(&names),
        Cmd::Info { name } => resolve::info(&name),
        Cmd::Search { query } => resolve::search(&query),
        Cmd::SelfUpdate => resolve::self_update(),
    };
    if let Err(e) = res {
        eprintln!("\x1b[31merror:\x1b[0m {e:#}");
        std::process::exit(1);
    }
}
