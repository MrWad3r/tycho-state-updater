use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use everscale_network::adnl::NodeIdShort;
use everscale_network::overlay;
use futures::stream::{FuturesOrdered, StreamExt};

use crate::PersistentStateRequest;
use crate::cli_context::CliContext;

use super::overlay_client;
use super::tl_models::RpcDownloadPersistentStateSlice;

const MAX_RLDP_SIZE: u64 = 2_000_000;
const BATCH_SIZE: usize = 10;
const BATCH_CAPACITY: usize = MAX_RLDP_SIZE as usize * BATCH_SIZE;
const RETRY_DELAY: Duration = Duration::from_secs(1);

pub async fn prepare_persistent_state(
    request: &PersistentStateRequest,
    ctx: Arc<CliContext>,
) -> Result<Vec<overlay_client::PersistentStatePeer>> {
    overlay_client::find_persistent_state_peers(&request.block, &request.masterchain_block, ctx)
        .await
}

pub async fn download_persistent_state(
    peers: Vec<overlay_client::PersistentStatePeer>,
    request: &PersistentStateRequest,
    output_file_path: Option<&Path>,
    ctx: Arc<CliContext>,
) -> Result<()> {
    let mut peers = peers;
    let mut writer = open_output(output_file_path)?;
    let mut offset = 0u64;
    loop {
        let bytes = download_batch_with_failover(
            &mut peers,
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

async fn download_batch_with_failover(
    peers: &mut Vec<overlay_client::PersistentStatePeer>,
    block: &ton_block::BlockIdExt,
    masterchain_block: &ton_block::BlockIdExt,
    start_offset: u64,
    ctx: Arc<CliContext>,
) -> Result<Vec<u8>> {
    loop {
        let Some(peer) = peers.first().cloned() else {
            anyhow::bail!("all persistent-state peers failed at offset {start_offset}");
        };

        match download_batch(
            &peer.overlay,
            &peer.peer_id,
            block,
            masterchain_block,
            start_offset,
            ctx.clone(),
        )
        .await
        {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                println!(
                    "batch download failed at offset {start_offset} on peer {}: {error}",
                    peer.peer_id
                );
                peers.remove(0);

                if let Some(next_peer) = peers.first() {
                    println!("switching to peer {}", next_peer.peer_id);
                } else {
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }
}

async fn download_batch(
    overlay: &Arc<overlay::Overlay>,
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
            let result = overlay_client::rldp_query_node_raw_in_overlay(
                overlay,
                &node_id,
                RpcDownloadPersistentStateSlice {
                    block,
                    masterchain_block,
                    offset,
                    max_size: MAX_RLDP_SIZE,
                },
                ctx,
            )
            .await;
            (offset, result)
        });
    }

    let mut merged = Vec::with_capacity(BATCH_CAPACITY);
    let mut eof_offset = None;
    let mut trailing_empty_chunks = 0usize;

    while let Some(chunk) = futures.next().await {
        let (offset, chunk) = chunk;
        match chunk? {
            Some(bytes) => {
                if bytes.len() < MAX_RLDP_SIZE as usize {
                    eof_offset = Some(
                        offset
                            + u64::try_from(bytes.len())
                                .context("slice size does not fit into u64")?,
                    );
                }
                merged.extend_from_slice(bytes.as_slice());
            }
            None if eof_offset.is_some() => {
                trailing_empty_chunks += 1;
            }
            None => anyhow::bail!("node returned an empty slice before EOF at offset {offset}"),
        }
    }

    if let Some(eof_offset) = eof_offset.filter(|_| trailing_empty_chunks > 0) {
        println!(
            "ignoring {trailing_empty_chunks} trailing empty replies after EOF at offset {eof_offset}"
        );
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
