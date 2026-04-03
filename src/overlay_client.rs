use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use everscale_network::adnl::{NodeIdFull, NodeIdShort, PeersSet};
use everscale_network::{adnl, dht, overlay};
use tl_proto::TlWrite;
use ton_block::BlockIdExt;

use crate::cli_context::CliContext;
use crate::tl_models::{PreparedState, RpcPreparePersistentState};

const DISCOVERY_ATTEMPTS: u32 = 10;
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(1);
const CACHED_PEERS_LIMIT: u32 = 50;
const ADDRESS_BROADCAST_INTERVAL: Duration = Duration::from_secs(500);

#[derive(Clone)]
struct OverlayTarget {
    full_id: overlay::IdFull,
    short_id: overlay::IdShort,
}

#[derive(Clone)]
pub struct PersistentStatePeer {
    pub overlay: Arc<overlay::Overlay>,
    pub peer_id: NodeIdShort,
}

pub fn start_network_tasks(ctx: Arc<CliContext>) {
    start_broadcasting_our_ip(ctx.dht.clone(), ctx.dht_key.clone());
    start_broadcasting_our_ip(ctx.dht.clone(), ctx.overlay_key.clone());
}

pub async fn find_persistent_state_peers(
    block: &BlockIdExt,
    masterchain_block: &BlockIdExt,
    ctx: Arc<CliContext>,
) -> Result<Vec<PersistentStatePeer>> {
    let overlays = discover_overlay_peers(&ctx).await?;
    find_persistent_state_peers_in_overlays(block, masterchain_block, &ctx, &overlays).await
}

pub async fn rldp_query_node_raw_in_overlay<Q>(
    overlay: &Arc<overlay::Overlay>,
    target_node_id: &NodeIdShort,
    query: Q,
    ctx: Arc<CliContext>,
) -> Result<Option<Vec<u8>>>
where
    Q: TlWrite + Clone,
{
    match overlay
        .rldp_query(&ctx.rldp, target_node_id, query, None)
        .await
    {
        Ok((Some(reply), _)) => Ok(Some(reply)),
        Ok((None, _)) => Ok(None),
        Err(error) => {
            eprintln!("failed to query overlay {}: {error:?}", overlay.id());
            Ok(None)
        }
    }
}

async fn scan_dht_table(ctx: &CliContext) -> Result<Arc<everscale_network::dht::Node>> {
    for dht_node in &ctx.global_config.dht_nodes {
        let _ = ctx.dht.add_dht_peer(dht_node.clone());
    }

    ctx.dht.find_more_dht_nodes().await?;
    Ok(ctx.dht.clone())
}

fn cached_peers(overlay: &Arc<overlay::Overlay>) -> Vec<NodeIdShort> {
    let peers = PeersSet::with_capacity(CACHED_PEERS_LIMIT);
    overlay.write_cached_peers(CACHED_PEERS_LIMIT, &peers);
    peers.into_iter().collect()
}

#[derive(Clone)]
struct DiscoveredOverlay {
    overlay: Arc<overlay::Overlay>,
    peers: Vec<NodeIdShort>,
}

async fn discover_overlay_peers(ctx: &CliContext) -> Result<Vec<DiscoveredOverlay>> {
    let _ = scan_dht_table(ctx).await?;
    let targets = overlay_targets(ctx.global_config.as_ref());

    let mut overlays = Vec::with_capacity(targets.len());
    for target in targets {
        let overlay = bootstrap_overlay(ctx, &ctx.dht, &target).await?;
        overlays.push(DiscoveredOverlay {
            peers: cached_peers(&overlay),
            overlay,
        });
    }

    Ok(overlays)
}

async fn bootstrap_overlay(
    ctx: &CliContext,
    dht: &Arc<dht::Node>,
    target: &OverlayTarget,
) -> Result<Arc<overlay::Overlay>> {
    let (overlay, _) = ctx
        .overlay
        .add_public_overlay(&target.short_id, Default::default());

    let local_node = overlay.sign_local_node();
    let _ = dht
        .store_overlay_node(&target.full_id, local_node.as_equivalent_ref())
        .await;

    for attempt in 0..DISCOVERY_ATTEMPTS {
        println!(
            "scanning overlay {}. Attempt {}/{}",
            target.short_id,
            attempt + 1,
            DISCOVERY_ATTEMPTS,
        );

        import_overlay_nodes(ctx, &overlay).await?;
        process_new_overlay_peers(ctx, &overlay).await?;
        exchange_random_peers(ctx, &overlay).await;
        process_new_overlay_peers(ctx, &overlay).await?;

        if !cached_peers(&overlay).is_empty() {
            break;
        }

        if attempt + 1 < DISCOVERY_ATTEMPTS {
            tokio::time::sleep(DISCOVERY_INTERVAL).await;
        }
    }

    Ok(overlay)
}

async fn import_overlay_nodes(ctx: &CliContext, overlay: &Arc<overlay::Overlay>) -> Result<()> {
    let found = ctx
        .dht
        .find_overlay_nodes(overlay.id())
        .await
        .context("failed to find overlay nodes")?;

    overlay.add_public_peers(
        ctx.adnl.as_ref(),
        found
            .iter()
            .map(|(addr, node)| (*addr, node.as_equivalent_ref())),
    )?;

    Ok(())
}

async fn process_new_overlay_peers(
    ctx: &CliContext,
    overlay: &Arc<overlay::Overlay>,
) -> Result<()> {
    for peer in overlay.take_new_peers().into_values() {
        let peer_id = match NodeIdFull::try_from(peer.id.as_equivalent_ref()) {
            Ok(full_id) => full_id.compute_short_id(),
            Err(error) => {
                eprintln!("invalid overlay peer id: {error:?}");
                continue;
            }
        };

        let addr = match ctx.dht.find_address(&peer_id).await {
            Ok((addr, _)) => addr,
            Err(_) => continue,
        };

        overlay.add_public_peer(ctx.adnl.as_ref(), addr, peer.as_equivalent_ref())?;
    }

    Ok(())
}

async fn exchange_random_peers(ctx: &CliContext, overlay: &Arc<overlay::Overlay>) {
    for peer_id in cached_peers(overlay) {
        if let Err(error) = overlay
            .exchange_random_peers(ctx.adnl.as_ref(), &peer_id, None)
            .await
        {
            eprintln!("failed to exchange overlay peers: {error:?}");
        }
    }
}

async fn find_persistent_state_peers_in_overlays(
    block: &BlockIdExt,
    masterchain_block: &BlockIdExt,
    ctx: &CliContext,
    overlays: &[DiscoveredOverlay],
) -> Result<Vec<PersistentStatePeer>> {
    let mut peers_with_state = Vec::new();

    for discovered in overlays {
        if discovered.peers.is_empty() {
            println!("no discovered peers in overlay {}", discovered.overlay.id());
        }
        for peer_id in &discovered.peers {
            println!("Checking peer {} for available persistent state", peer_id);
            match query_prepared_state(&discovered.overlay, peer_id, block, masterchain_block, ctx)
                .await
            {
                Ok(Some(PreparedState::Found)) => {
                    peers_with_state.push(PersistentStatePeer {
                        overlay: discovered.overlay.clone(),
                        peer_id: *peer_id,
                    });
                }
                Ok(Some(PreparedState::NotFound)) => {
                    println!("peer {} not found", peer_id);
                    continue;
                }
                Ok(None) => {
                    println!("peer {} has no response", peer_id);
                    continue;
                }
                Err(error) => {
                    eprintln!("failed to query peer: {error:?}");
                    continue;
                }
            }
        }
    }

    Ok(peers_with_state)
}

async fn query_prepared_state(
    overlay: &Arc<overlay::Overlay>,
    peer_id: &NodeIdShort,
    block: &BlockIdExt,
    masterchain_block: &BlockIdExt,
    ctx: &CliContext,
) -> Result<Option<PreparedState>> {
    let reply = overlay
        .adnl_query(
            ctx.adnl.as_ref(),
            peer_id,
            RpcPreparePersistentState {
                block: block.clone(),
                masterchain_block: masterchain_block.clone(),
            },
            None,
        )
        .await?;

    match reply {
        Some(reply) => Ok(Some(tl_proto::deserialize::<PreparedState>(&reply)?)),
        None => Ok(None),
    }
}

fn overlay_targets(global_config: &global_config::GlobalConfig) -> [OverlayTarget; 2] {
    let file_hash = global_config.zero_state.file_hash();

    [
        {
            let full_id = overlay::IdFull::for_workchain_overlay(-1, file_hash.as_slice());
            let short_id = full_id.compute_short_id();
            OverlayTarget { full_id, short_id }
        },
        {
            let full_id = overlay::IdFull::for_workchain_overlay(0, file_hash.as_slice());
            let short_id = full_id.compute_short_id();
            OverlayTarget { full_id, short_id }
        },
    ]
}

fn start_broadcasting_our_ip(dht: Arc<dht::Node>, key: Arc<adnl::Key>) {
    let addr = dht.adnl().socket_addr();

    tokio::spawn(async move {
        loop {
            if let Err(error) = dht.store_address(&key, addr).await {
                eprintln!("failed to store address in dht: {error:?}");
            }

            tokio::time::sleep(ADDRESS_BROADCAST_INTERVAL).await;
        }
    });
}
