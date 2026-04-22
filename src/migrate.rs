use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use tycho_types::boc::ser::BocHeader;
use tycho_types::cell::{CellBuilder, HashBytes};
use tycho_types::models as tycho;
use tycho_types::models::ShardStateUnsplit;

use crate::MigrateArgs;
use crate::global_config_json::make_blockchain_config;
use crate::migration::{ZerostateId, migrate_masterstate_file, migrate_shardstate_file};

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

impl MigrateArgs {
    pub fn run(self) -> Result<()> {
        let (migrates_shard_state, shard_balance) =
            migrate_shardstate_file(&self.shard_state, self.time)
                .with_context(|| format!("failed to migrate {}", self.shard_state.display()))?;

        let shard_state_id =
            self.write_state_to_file(migrates_shard_state, Path::new("0:8000000000000000.d.boc"))?;

        let config = make_blockchain_config(self.config.clone())?;
        let current_validator_set = load_current_validator_set(&self.current_validator_set)?;

        let (migrates_shard_state, _) = migrate_masterstate_file(
            &self.master_state,
            config,
            shard_state_id,
            shard_balance,
            current_validator_set,
            self.time,
        )
        .with_context(|| format!("failed to migrate {}", self.shard_state.display()))?;

        let _ =
            self.write_state_to_file(migrates_shard_state, Path::new("-1:8000000000000000.d.boc"))?;

        Ok(())
    }

    fn write_state_to_file(
        &self,
        state: ShardStateUnsplit,
        path: impl AsRef<Path>,
    ) -> Result<ZerostateId> {
        let seqno = state.seqno;
        let migrated =
            CellBuilder::build_from(state).context("failed to build migrated shard state")?;
        let zerostate_id = ZerostateId {
            seqno,
            root_hash: *migrated.repr_hash(),
            file_hash: HashBytes::ZERO,
        };

        let output = BufWriter::new(
            File::create(&self.output.as_path().join(path.as_ref()))
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

        let zerostate_id = ZerostateId {
            file_hash,
            ..zerostate_id
        };
        let zerostate_id_json = serde_json::to_string_pretty(&zerostate_id)
            .context("failed to serialize zerostate id json")?;
        println!("{zerostate_id_json}");

        Ok(zerostate_id)
    }
}
