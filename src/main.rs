use std::path::PathBuf;

use anyhow::{Context, Result};
use broxus_util::serde_hex_array;
use clap::{Parser, Subcommand};
use everscale_network::adnl::NodeIdShort;
use serde::{Deserialize, Deserializer};
use ton_block::{BlockIdExt, ShardIdent};
use ton_types::UInt256;

mod cli_context;
mod download_state;
mod migrate;
mod migration;
mod overlay_client;
mod persistent_state;
mod tl_models;

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
    #[arg(long = "node-id")]
    node_id: String,
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
    input: PathBuf,
    #[arg(long = "output", short = 'o')]
    output: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockIdJson {
    workchain_id: i32,
    #[serde(deserialize_with = "deserialize_hex_number")]
    shard: u64,
    seqno: u32,
    #[serde(with = "serde_hex_array")]
    root_hash: [u8; 32],
    #[serde(with = "serde_hex_array")]
    file_hash: [u8; 32],
}

struct PersistentStateRequest {
    block: BlockIdExt,
    masterchain_block: BlockIdExt,
}

impl BlockIdJson {
    fn to_block_id_ext(&self) -> Result<BlockIdExt> {
        Ok(BlockIdExt {
            shard_id: ShardIdent::with_tagged_prefix(self.workchain_id, self.shard)?,
            seq_no: self.seqno,
            root_hash: UInt256::from_be_bytes(self.root_hash.as_slice()),
            file_hash: UInt256::from_be_bytes(self.file_hash.as_slice()),
        })
    }
}

impl PersistentStateRequest {
    fn parse(block: &str, masterchain_block: &str) -> Result<Self> {
        let block: BlockIdJson =
            serde_json::from_str(block).context("failed to parse `--block` json")?;
        let masterchain_block: BlockIdJson = serde_json::from_str(masterchain_block)
            .context("failed to parse `--masterchain-block` json")?;

        Ok(Self {
            block: block.to_block_id_ext()?,
            masterchain_block: masterchain_block.to_block_id_ext()?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::DownloadState(args) => args.run().await,
        Command::Migrate(args) => args.run(),
    }
}

fn decode_node_id(node_id: &str) -> Result<NodeIdShort> {
    let bytes = hex::decode(node_id).context("node id must be hex")?;
    let hash: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("node id must decode to exactly 32 bytes"))?;
    Ok(NodeIdShort::new(hash))
}

fn deserialize_hex_number<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let data = String::deserialize(deserializer)?;
    let data = hex::decode(data).map_err(Error::custom)?;
    let bytes: [u8; 8] = data
        .as_slice()
        .try_into()
        .map_err(|_| Error::custom("shard must decode to exactly 8 bytes"))?;
    Ok(u64::from_be_bytes(bytes))
}
