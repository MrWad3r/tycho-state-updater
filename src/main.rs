use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
mod global_config;
mod migrate;
mod migration;
mod models;

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Migrate(MigrateArgs),
}

#[derive(Parser, Debug)]
struct MigrateArgs {
    #[arg(long = "master", short = 'm')]
    master_state: PathBuf,
    #[arg(long = "shard", short = 's')]
    shard_state: PathBuf,
    #[arg(long = "config", short = 'c')]
    config: PathBuf,
    #[arg(long = "time", short = 't')]
    time: u64,
    #[arg(long = "current-validator-set", short = 'v')]
    current_validator_set: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Migrate(args) => args.run(),
        _ => Ok(()),
    }
}
