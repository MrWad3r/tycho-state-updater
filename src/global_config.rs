use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Map, Value};
use tycho_types::cell::HashBytes;
use tycho_types::models::{BlockchainConfig, BlockchainConfigParams, ConfigParam0};

#[derive(Debug, Deserialize)]
struct GlobalConfigJson {
    params: Map<String, Value>,
    accounts: Map<String, Value>,
}

pub struct GlobalConfig {
    pub config: BlockchainConfig,
    pub validator_accounts: BTreeMap<HashBytes, String>,
}

const CONFIG_ACCOUNT_ADDRESS: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";

pub fn make_global_config(path: impl AsRef<Path>) -> Result<GlobalConfig> {
    let path = path.as_ref();
    let file = File::open(path)
        .with_context(|| format!("failed to open zerostate json {}", path.display()))?;

    parse_global_config(BufReader::new(file))
        .with_context(|| format!("failed to build global config from {}", path.display()))
}

fn parse_global_config(reader: impl Read) -> Result<GlobalConfig> {
    let GlobalConfigJson { params, accounts } =
        serde_json::from_reader(reader).context("failed to deserialize zerostate json")?;

    Ok(GlobalConfig {
        config: make_blockchain_config_from_params(params)?,
        validator_accounts: make_validator_accounts_from_map(accounts)?,
    })
}

fn make_validator_accounts_from_map(
    accounts: Map<String, Value>,
) -> Result<BTreeMap<HashBytes, String>> {
    let mut result = BTreeMap::new();
    for (address, account) in accounts {
        if address == CONFIG_ACCOUNT_ADDRESS {
            continue;
        }

        let address = address
            .parse::<HashBytes>()
            .with_context(|| format!("failed to parse validator account address {address}"))?;
        let account = match account {
            Value::String(account) => account,
            _ => bail!("validator account {address} must be encoded as a string"),
        };

        let prev = result.insert(address, account);
        ensure!(
            prev.is_none(),
            "duplicate validator account address after normalization: {address}"
        );
    }

    Ok(result)
}

fn make_blockchain_config_from_params(params: Map<String, Value>) -> Result<BlockchainConfig> {
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
