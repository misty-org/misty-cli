mod artifacts;
mod checks;
mod cli;
mod cloud;
mod cloudflare;
mod config;
mod console;
mod desktop;
mod diagnostics;
mod environment;
mod guide;
mod hooks;
mod process;
mod release;
mod server;
mod setup;
mod website;
mod workspace;

use anyhow::Result;
use clap::Parser;

fn main() {
    if let Err(error) = run() {
        // A failed check passes its own exit code on; its output already said what failed.
        if let Some(hooks::Failed(code)) = error.downcast_ref::<hooks::Failed>() {
            std::process::exit(*code);
        }
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let arguments = cli::Cli::parse();
    let settings = config::Settings::load(arguments.workspace.as_deref())?;
    cli::dispatch(arguments, settings)
}
