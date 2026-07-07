mod platform;
mod recipe;
mod resolve;
mod state;
mod util;

use clap::{Parser, Subcommand};

const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (build ",
    env!("NOBREW_BUILD_ID"),
    ")"
);

#[derive(Parser)]
#[command(
    name = "nobrew",
    version = VERSION,
    about = "One install/uninstall over brew, pacman, and recipes (macOS + Arch)."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install a package (auto-selects brew / pacman / recipe)
    Install { name: String },
    /// Uninstall a package (routes to whatever backend installed it)
    Uninstall { name: String },
    /// List packages nobrew has installed
    List,
    /// Show a package and how it would be installed here
    Info { name: String },
    /// Search brew, pacman, and recipes
    Search { query: String },
    /// Update nobrew itself to the latest build
    SelfUpdate,
}

fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Install { name } => resolve::install(&name),
        Cmd::Uninstall { name } => resolve::uninstall(&name),
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
