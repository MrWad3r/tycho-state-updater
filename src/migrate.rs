use std::fs::File;
use std::io::{BufReader, BufWriter, Write};

use anyhow::{Context, Result};
use tycho_types::boc::BocRepr;
use tycho_types::cell::Lazy;
use tycho_types::models as tycho;
use tycho_types::models::ConfigParam34;

use crate::MigrateArgs;
use crate::migration::migrate_file;

fn load_current_validator_set(path: &std::path::Path) -> Result<tycho::ValidatorSet> {
    let file = File::open(path)
        .with_context(|| format!("failed to open validator set {}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("failed to parse validator set json {}", path.display()))
}

fn override_current_validator_set(
    state: &mut tycho::ShardStateUnsplit,
    validator_set: tycho::ValidatorSet,
) -> Result<()> {
    let Some(custom) = state.custom.take() else {
        anyhow::bail!("`--current-validator-set` requires a masterchain state");
    };
    let mut custom = custom
        .load()
        .context("failed to load migrated mc state extra")?;

    custom.config.params.remove(35)?;
    custom.config.params.set::<ConfigParam34>(&validator_set)?;

    let collation_config = custom
        .config
        .params
        .get_collation_config()
        .context("failed to load collation config after validator-set override")?;
    let session_seqno = custom.validator_info.catchain_seqno;
    let Some((_, validator_list_hash_short)) =
        validator_set.compute_mc_subset(session_seqno, collation_config.shuffle_mc_validators)
    else {
        anyhow::bail!("failed to compute validator subset for overridden current validator set");
    };
    custom.validator_info.validator_list_hash_short = validator_list_hash_short;

    state.custom = Some(Lazy::new(&custom).context("failed to rebuild mc state extra")?);
    Ok(())
}

impl MigrateArgs {
    pub fn run(self) -> Result<()> {
        let mut migrated = migrate_file(&self.input)
            .with_context(|| format!("failed to migrate {}", self.input.display()))?;

        if let Some(path) = self.current_validator_set.as_deref() {
            let validator_set = load_current_validator_set(path)?;
            override_current_validator_set(&mut migrated, validator_set).with_context(|| {
                format!(
                    "failed to apply validator-set override from {}",
                    path.display()
                )
            })?;
        }

        let migrated =
            BocRepr::encode(migrated).context("failed to encode migrated shard state")?;

        let mut output = BufWriter::new(
            File::create(&self.output)
                .with_context(|| format!("failed to create {}", self.output.display()))?,
        );
        output
            .write_all(migrated.as_slice())
            .with_context(|| format!("failed to write {}", self.output.display()))?;
        output.flush()?;
        println!("migrated shard state written to {}", self.output.display());
        Ok(())
    }
}
