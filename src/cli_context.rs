use std::net::SocketAddrV4;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use everscale_crypto::ed25519;
use everscale_network::adnl::{ComputeNodeIds, NodeIdShort, NodeOptions};
use everscale_network::{NetworkBuilder, adnl, dht, overlay, rldp};
use global_config::GlobalConfig;

#[derive(Clone)]
pub struct CliContext {
    pub local_id: NodeIdShort,
    pub dht: Arc<dht::Node>,
    pub rldp: Arc<rldp::Node>,
    pub overlay: Arc<overlay::Node>,
    pub global_config: Arc<GlobalConfig>,
    pub overlay_ids: [overlay::IdShort; 2],
}

pub async fn build_context(global_config_path: &Path, bind_port: u16) -> Result<Arc<CliContext>> {
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
