use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use everscale_network::adnl::NodeIdShort;
use futures::stream::{FuturesOrdered, StreamExt};

use super::overlay_client;
use super::tl_models::{PreparedState, RpcDownloadPersistentStateSlice, RpcPreparePersistentState};
use super::{CliContext, PersistentStateRequest};

const MAX_RLDP_SIZE: u64 = 2_000_000;
const BATCH_SIZE: usize = 10;
const BATCH_CAPACITY: usize = MAX_RLDP_SIZE as usize * BATCH_SIZE;
const RETRY_DELAY: Duration = Duration::from_secs(1);

pub async fn prepare_persistent_state(
    node_id: &NodeIdShort,
    request: &PersistentStateRequest,
    ctx: Arc<CliContext>,
) -> Result<bool> {
    let reply = overlay_client::adnl_query_node::<RpcPreparePersistentState, PreparedState>(
        node_id,
        RpcPreparePersistentState {
            block: request.block.clone(),
            masterchain_block: request.masterchain_block.clone(),
        },
        ctx,
    )
    .await?;

    match reply {
        Some(PreparedState::Found) => Ok(true),
        Some(PreparedState::NotFound) => Ok(false),
        None => anyhow::bail!("failed to get `preparePersistentState` response from node"),
    }
}

pub async fn download_persistent_state(
    node_id: &NodeIdShort,
    request: &PersistentStateRequest,
    output_file_path: Option<&Path>,
    ctx: Arc<CliContext>,
) -> Result<()> {
    let mut writer = open_output(output_file_path)?;
    let mut offset = 0u64;
    loop {
        let bytes = download_batch_with_retry(
            node_id,
            &request.block,
            &request.masterchain_block,
            offset,
            ctx.clone(),
        )
        .await?;

        if bytes.is_empty() {
            break;
        }

        offset = advance_offset(offset, bytes.len())?;
        writer.write_all(bytes.as_slice())?;

        if bytes.len() < BATCH_CAPACITY {
            break;
        }
    }

    writer.flush()?;
    println!("persistent state dump finished");
    Ok(())
}

async fn download_batch_with_retry(
    node_id: &NodeIdShort,
    block: &ton_block::BlockIdExt,
    masterchain_block: &ton_block::BlockIdExt,
    start_offset: u64,
    ctx: Arc<CliContext>,
) -> Result<Vec<u8>> {
    loop {
        match download_batch(node_id, block, masterchain_block, start_offset, ctx.clone()).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                // Persistent-state downloads can run for hours, so a retry loop must not spin
                // aggressively on transient network failures.
                println!("batch download failed at offset {start_offset}: {error}");
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }
    }
}

async fn download_batch(
    node_id: &NodeIdShort,
    block: &ton_block::BlockIdExt,
    masterchain_block: &ton_block::BlockIdExt,
    start_offset: u64,
    ctx: Arc<CliContext>,
) -> Result<Vec<u8>> {
    let mut futures = FuturesOrdered::new();

    for index in 0..BATCH_SIZE {
        let ctx = ctx.clone();
        let node_id = *node_id;
        let block = block.clone();
        let masterchain_block = masterchain_block.clone();
        futures.push_back(async move {
            let offset = chunk_offset(start_offset, index);
            println!("downloading offset {offset}");
            overlay_client::rldp_query_node_raw(
                &node_id,
                RpcDownloadPersistentStateSlice {
                    block,
                    masterchain_block,
                    offset,
                    max_size: MAX_RLDP_SIZE,
                },
                ctx,
            )
            .await
        });
    }

    let mut merged = Vec::with_capacity(BATCH_CAPACITY);
    let mut saw_short_chunk = false;

    while let Some(chunk) = futures.next().await {
        match chunk? {
            Some(bytes) => {
                if bytes.len() < MAX_RLDP_SIZE as usize {
                    saw_short_chunk = true;
                }
                merged.extend_from_slice(bytes.as_slice());
            }
            None if saw_short_chunk => {}
            None => anyhow::bail!("node returned an empty slice before EOF"),
        }
    }

    Ok(merged)
}

fn open_output(output_file_path: Option<&Path>) -> Result<Box<dyn Write>> {
    match output_file_path {
        Some(path) => Ok(Box::new(BufWriter::new(
            File::create(path).context("failed to create output file")?,
        ))),
        None => Ok(Box::new(BufWriter::new(io::stdout()))),
    }
}

fn advance_offset(offset: u64, bytes_len: usize) -> Result<u64> {
    let bytes_len = u64::try_from(bytes_len).context("slice size does not fit into u64")?;
    offset
        .checked_add(bytes_len)
        .context("persistent state offset overflowed u64")
}

fn chunk_offset(start_offset: u64, index: usize) -> u64 {
    start_offset + u64::try_from(index).expect("batch index fits into u64") * MAX_RLDP_SIZE
}
