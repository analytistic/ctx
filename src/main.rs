mod cli;
mod commands;
mod display;
mod session;
mod tree;
mod tui;

use clap::Parser;
use cli::Cli;

fn main() {
    let cli = Cli::parse();
    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            let _ = cmd.print_help();
            println!();
            return;
        }
    };
    if let Err(e) = commands::dispatch(command, cli.close_on_exit) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
