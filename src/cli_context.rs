use std::net::SocketAddrV4;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use everscale_network::{NetworkBuilder, adnl, dht, overlay, rldp};
use global_config::GlobalConfig;

#[derive(Clone)]
pub struct CliContext {
    pub dht_key: Arc<adnl::Key>,
    pub overlay_key: Arc<adnl::Key>,
    pub adnl: Arc<adnl::Node>,
    pub dht: Arc<dht::Node>,
    pub rldp: Arc<rldp::Node>,
    pub overlay: Arc<overlay::Node>,
    pub global_config: Arc<GlobalConfig>,
}

pub async fn build_context(global_config_path: &Path, bind_port: u16) -> Result<Arc<CliContext>> {
    const TAG_DHT_KEY: usize = 1;
    const TAG_OVERLAY_KEY: usize = 2;

    let ipv4 = broxus_util::resolve_public_ip(None).await?;
    println!("using public ip: {ipv4}");

    let global_config = Arc::new(GlobalConfig::load(global_config_path)?);
    let keystore = adnl::Keystore::builder()
        .with_tagged_key(rand::random::<[u8; 32]>(), TAG_DHT_KEY)?
        .with_tagged_key(rand::random::<[u8; 32]>(), TAG_OVERLAY_KEY)?
        .build();

    let (adnl, dht, rldp, overlay) = NetworkBuilder::with_adnl(
        SocketAddrV4::new(ipv4, bind_port),
        keystore,
        adnl::NodeOptions::default(),
    )
    .with_dht(TAG_DHT_KEY, Default::default())
    .with_rldp(rldp::NodeOptions::default())
    .with_overlay(TAG_OVERLAY_KEY)
    .build()?;

    let dht_key = adnl.key_by_tag(TAG_DHT_KEY)?.clone();
    let overlay_key = adnl.key_by_tag(TAG_OVERLAY_KEY)?.clone();

    println!("using dht local id: {}", dht_key.id());
    println!("using overlay local id: {}", overlay_key.id());

    Ok(Arc::new(CliContext {
        dht_key,
        overlay_key,
        adnl,
        dht,
        rldp,
        overlay,
        global_config,
    }))
}
