mod platform;
mod recipe;
mod resolve;
mod state;
mod util;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "nobrew",
    version,
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
}

fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Install { name } => resolve::install(&name),
        Cmd::Uninstall { name } => resolve::uninstall(&name),
        Cmd::List => resolve::list(),
        Cmd::Info { name } => resolve::info(&name),
        Cmd::Search { query } => resolve::search(&query),
    };
    if let Err(e) = res {
        eprintln!("\x1b[31merror:\x1b[0m {e:#}");
        std::process::exit(1);
    }
}
