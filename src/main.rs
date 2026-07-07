mod platform;
mod recipe;
mod resolve;
mod state;
mod util;

use clap::{Parser, Subcommand};

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
        Cmd::Info { name } => resolve::info(&name),
        Cmd::Search { query } => resolve::search(&query),
        Cmd::SelfUpdate => resolve::self_update(),
    };
    if let Err(e) = res {
        eprintln!("\x1b[31merror:\x1b[0m {e:#}");
        std::process::exit(1);
    }
}
