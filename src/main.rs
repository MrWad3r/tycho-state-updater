use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::Deserializer;
mod global_config_json;
mod migrate;
mod migration;
mod old_models;

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    DownloadState(DownloadStateArgs),
    Migrate(MigrateArgs),
}

#[derive(Parser, Debug)]
struct DownloadStateArgs {
    #[arg(long = "global-config", short = 'g')]
    global_config: PathBuf,
    #[arg(long = "node-id", hide = true)]
    _node_id: Option<String>,
    #[arg(long = "block")]
    block: String,
    #[arg(long = "masterchain-block", alias = "m-block")]
    masterchain_block: String,
    #[arg(long = "output")]
    output_file_path: Option<PathBuf>,
    #[arg(long = "bind-port", default_value_t = 30088)]
    bind_port: u16,
}

#[derive(Parser, Debug)]
struct MigrateArgs {
    #[arg(long = "master", short = 'm')]
    master_state: PathBuf,
    #[arg(long = "shard", short = 's')]
    shard_state: PathBuf,
    #[arg(long = "output", short = 'o')]
    output: PathBuf,
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
