use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use broxus_util::serde_hex_array;
use clap::Parser;
use everscale_crypto::ed25519;
use everscale_network::adnl::{ComputeNodeIds, NodeIdShort, NodeOptions};
use everscale_network::{NetworkBuilder, adnl, dht, overlay, rldp};
use global_config::GlobalConfig;
use serde::{Deserialize, Deserializer};
use ton_block::{BlockIdExt, ShardIdent};
use ton_types::UInt256;

mod overlay_client;
mod persistent_state;
mod tl_models;

#[derive(Clone)]
struct CliContext {
    local_id: NodeIdShort,
    dht: Arc<dht::Node>,
    rldp: Arc<rldp::Node>,
    overlay: Arc<overlay::Node>,
    global_config: Arc<GlobalConfig>,
    overlay_ids: [overlay::IdShort; 2],
}

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
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
    let context = build_context(&cli.global_config, cli.bind_port).await?;
    let request = PersistentStateRequest::parse(&cli.block, &cli.masterchain_block)?;

    let target_node_id = {
        let node_id = decode_node_id(&cli.node_id)?;
        let (_, target_node_id) = overlay_client::resolve_node(&node_id, context.clone()).await?;
        target_node_id
    };

    let prepared =
        persistent_state::prepare_persistent_state(&target_node_id, &request, context.clone())
            .await?;

    if !prepared {
        anyhow::bail!("persistent state is not available on the target node");
    }

    persistent_state::download_persistent_state(
        &target_node_id,
        &request,
        cli.output_file_path.as_deref(),
        context,
    )
    .await
}

async fn build_context(global_config_path: &Path, bind_port: u16) -> Result<Arc<CliContext>> {
    let ipv4 = broxus_util::resolve_public_ip(None).await?;
    println!("using public ip: {ipv4}");

    let global_config = Arc::new(GlobalConfig::load(global_config_path)?);
    let file_hash = global_config.zero_state.file_hash();
    let overlay_ids = [
        overlay::IdFull::for_workchain_overlay(-1, file_hash.as_slice()).compute_short_id(),
        overlay::IdFull::for_workchain_overlay(0, file_hash.as_slice()).compute_short_id(),
    ];

    let key = rand::random::<[u8; 32]>();
    let secret_key = ed25519::SecretKey::from_bytes(key);
    let (_, local_id) = secret_key.compute_node_ids();
    println!("using local id: {local_id}");

    let keystore = adnl::Keystore::builder().with_tagged_key(key, 0)?.build();

    let (_, dht, rldp, overlay) = NetworkBuilder::with_adnl(
        SocketAddrV4::new(ipv4, bind_port),
        keystore,
        NodeOptions {
            use_loopback_for_neighbours: true,
            version: Some(1),
            ..Default::default()
        },
    )
    .with_dht(0, Default::default())
    .with_rldp(rldp::NodeOptions {
        force_compression: true,
        ..Default::default()
    })
    .with_overlay(0)
    .build()?;

    Ok(Arc::new(CliContext {
        local_id,
        dht,
        rldp,
        overlay,
        global_config,
        overlay_ids,
    }))
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
