use std::fs::File;
use std::io::{BufWriter, Write};

use anyhow::{Context, Result};

use crate::MigrateArgs;
use crate::migration::migrate_file_to_boc;

impl MigrateArgs {
    pub fn run(self) -> Result<()> {
        let migrated = migrate_file_to_boc(&self.input)
            .with_context(|| format!("failed to migrate {}", self.input.display()))?;
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
