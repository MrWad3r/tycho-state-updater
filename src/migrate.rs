use std::fs::File;
use std::io::{BufReader, BufWriter, Write};

use anyhow::{Context, Result};
use tycho_types::boc::ser::BocHeader;
use tycho_types::cell::{CellBuilder, HashBytes};
use tycho_types::cell::Lazy;
use tycho_types::models as tycho;
use tycho_types::models::ConfigParam34;
use serde::Serialize;

use crate::MigrateArgs;
use crate::migration::migrate_file;

#[derive(Debug, Serialize)]
struct ZerostateIdJson {
    seqno: u32,
    root_hash: HashBytes,
    file_hash: HashBytes,
}

struct HashingWriter<W> {
    inner: W,
    hasher: blake3::Hasher,
}

impl<W> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
        }
    }

    fn finalize(self) -> (W, HashBytes) {
        (self.inner, (*self.hasher.finalize().as_bytes()).into())
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

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
    println!("validator set overridden successfully");
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
            CellBuilder::build_from(migrated).context("failed to build migrated shard state")?;
        let zerostate_id = ZerostateIdJson {
            seqno: 0,
            root_hash: *migrated.repr_hash(),
            file_hash: HashBytes::ZERO,
        };

        let output = BufWriter::new(
            File::create(&self.output)
                .with_context(|| format!("failed to create {}", self.output.display()))?,
        );
        let mut output = HashingWriter::new(output);
        BocHeader::<std::collections::hash_map::RandomState>::with_root(migrated.as_ref())
            .encode_to_writer(&mut output)
            .with_context(|| format!("failed to write {}", self.output.display()))?;
        output.flush()?;
        let (mut output, file_hash) = output.finalize();
        output.flush()?;
        println!("migrated shard state written to {}", self.output.display());

        let zerostate_id = ZerostateIdJson {
            file_hash,
            ..zerostate_id
        };
        let zerostate_id_json = serde_json::to_string_pretty(&zerostate_id)
            .context("failed to serialize zerostate id json")?;
        println!("{zerostate_id_json}");

        if let Some(path) = self.zerostate_id_output.as_deref() {
            std::fs::write(path, format!("{zerostate_id_json}\n"))
                .with_context(|| format!("failed to write zerostate id {}", path.display()))?;
            println!("zerostate id written to {}", path.display());
        }
        Ok(())
    }
}
