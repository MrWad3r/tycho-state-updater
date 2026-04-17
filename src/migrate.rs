use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;
use tycho_types::boc::ser::BocHeader;
use tycho_types::cell::{CellBuilder, HashBytes};
use tycho_types::models as tycho;

use crate::MigrateArgs;
use crate::migration::{ShardStateHashes, ShardStateHashesEntry, migrate_file};

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

fn load_shard_state_hashes(paths: &[PathBuf]) -> Result<ShardStateHashes> {
    let mut result = ShardStateHashes::default();
    for path in paths {
        let data = std::fs::read(path)
            .with_context(|| format!("failed to read shard-state boc {}", path.display()))?;
        let root = tycho_types::boc::Boc::decode(data.as_slice())
            .with_context(|| format!("failed to decode shard-state boc {}", path.display()))?;
        let state: tycho::ShardStateUnsplit = tycho_types::boc::BocRepr::decode(data.as_slice())
            .with_context(|| format!("failed to parse shard-state boc {}", path.display()))?;
        let shard_ident = state.shard_ident;

        let prev = result.insert(
            shard_ident,
            ShardStateHashesEntry {
                root_hash: *root.repr_hash(),
                file_hash: (*blake3::hash(&data).as_bytes()).into(),
            },
        );
        anyhow::ensure!(
            prev.is_none(),
            "duplicate shard {} in shard-state files",
            shard_ident,
        );
    }

    Ok(result)
}

impl MigrateArgs {
    pub fn run(self) -> Result<()> {
        let shard_state_hashes = (!self.shard_state.is_empty())
            .then(|| load_shard_state_hashes(&self.shard_state))
            .transpose()?;
        let current_validator_set = self
            .current_validator_set
            .as_deref()
            .map(load_current_validator_set)
            .transpose()?;

        let migrated = migrate_file(
            &self.input,
            shard_state_hashes.as_ref(),
            current_validator_set.as_ref(),
            self.time,
        )
        .with_context(|| format!("failed to migrate {}", self.input.display()))?;

        let seqno = migrated.seqno;
        let migrated =
            CellBuilder::build_from(migrated).context("failed to build migrated shard state")?;
        let zerostate_id = ZerostateIdJson {
            seqno,
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
        Ok(())
    }
}
