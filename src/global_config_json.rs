use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value};
use tycho_types::cell::HashBytes;
use tycho_types::models::{BlockchainConfig, BlockchainConfigParams, ConfigParam0};

#[derive(Debug, Deserialize)]
pub struct GlobalConfigJson {
    params: Map<String, Value>,
}

pub fn make_blockchain_config(path: impl AsRef<Path>) -> Result<BlockchainConfig> {
    let path = path.as_ref();
    let file = File::open(path)
        .with_context(|| format!("failed to open zerostate json {}", path.display()))?;

    parse_blockchain_config(BufReader::new(file))
        .with_context(|| format!("failed to build blockchain config from {}", path.display()))
}

fn parse_blockchain_config(reader: impl Read) -> Result<BlockchainConfig> {
    let GlobalConfigJson { params } =
        serde_json::from_reader(reader).context("failed to deserialize zerostate json")?;
    let config_address = parse_config_address(&params)?;
    let params = deserialize_blockchain_config_params(params)?;

    let mut config = BlockchainConfig::new_empty(config_address);
    config.params = params;

    ensure!(
        config.params.get::<ConfigParam0>()? == Some(config_address),
        "config address mismatch after params import"
    );

    Ok(config)
}

fn parse_config_address(params: &Map<String, Value>) -> Result<HashBytes> {
    let config_address = params
        .get("0")
        .cloned()
        .context("config address is not set in params.0")?;
    serde_json::from_value(config_address).context("failed to parse config address from params.0")
}

fn deserialize_blockchain_config_params(
    params: Map<String, Value>,
) -> Result<BlockchainConfigParams> {
    serde_json::from_value(Value::Object(params))
        .context("failed to deserialize blockchain config params")
}
