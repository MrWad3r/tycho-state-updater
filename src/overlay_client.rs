use std::fmt::Debug;
use std::sync::Arc;

use anyhow::Result;
use everscale_network::adnl::{NewPeerContext, NodeIdShort};
use tl_proto::{TlRead, TlWrite};

use crate::cli_context::CliContext;

pub async fn resolve_node(
    node_id: &NodeIdShort,
    ctx: Arc<CliContext>,
) -> Result<(std::net::SocketAddrV4, NodeIdShort)> {
    for dht_node in &ctx.global_config.dht_nodes {
        let _ = ctx.dht.add_dht_peer(dht_node.clone());
    }
    ctx.dht.find_more_dht_nodes().await?;

    let (node_address, node_id_full) = ctx.dht.find_address(node_id).await?;
    let target_node_id = node_id_full.compute_short_id();
    ctx.rldp.adnl().add_peer(
        NewPeerContext::PublicOverlay,
        &ctx.local_id,
        &target_node_id,
        node_address,
        node_id_full,
    )?;

    Ok((node_address, target_node_id))
}

pub async fn adnl_query_node<Q, A>(
    target_node_id: &NodeIdShort,
    query: Q,
    ctx: Arc<CliContext>,
) -> Result<Option<A>>
where
    Q: TlWrite + Clone,
    for<'a> A: TlRead<'a, Repr = tl_proto::Boxed> + Debug + 'static,
{
    for overlay_id in &ctx.overlay_ids {
        let (shard, _) = ctx
            .overlay
            .add_public_overlay(overlay_id, Default::default());
        let reply: Result<Option<_>> = shard
            .adnl_query::<Q>(ctx.rldp.adnl(), target_node_id, query.clone(), None)
            .await;

        match reply {
            Ok(Some(reply)) => return Ok(Some(tl_proto::deserialize::<A>(&reply)?)),
            Ok(None) => println!("node reply timed out on overlay {overlay_id}"),
            Err(error) => eprintln!("failed to query overlay {overlay_id}: {error:?}"),
        }
    }

    Ok(None)
}

pub async fn rldp_query_node_raw<Q>(
    target_node_id: &NodeIdShort,
    query: Q,
    ctx: Arc<CliContext>,
) -> Result<Option<Vec<u8>>>
where
    Q: TlWrite + Clone,
{
    for overlay_id in &ctx.overlay_ids {
        let (shard, _) = ctx
            .overlay
            .add_public_overlay(overlay_id, Default::default());

        match shard
            .rldp_query(&ctx.rldp, target_node_id, query.clone(), None)
            .await
        {
            Ok((Some(reply), _)) => return Ok(Some(reply)),
            Ok((None, _)) => eprintln!("node reply timed out on overlay {overlay_id}"),
            Err(error) => eprintln!("failed to query overlay {overlay_id}: {error:?}"),
        }
    }

    Ok(None)
}
