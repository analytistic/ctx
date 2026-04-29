mod cli;
mod commands;
mod display;
mod session;
mod tree;

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
    if let Err(e) = commands::dispatch(command) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
