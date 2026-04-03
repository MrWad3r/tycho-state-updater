use anyhow::Result;

use crate::cli_context::build_context;
use crate::overlay_client;
use crate::{DownloadStateArgs, PersistentStateRequest, persistent_state};

impl DownloadStateArgs {
    pub async fn run(self) -> Result<()> {
        let context = build_context(&self.global_config, self.bind_port).await?;
        overlay_client::start_network_tasks(context.clone());
        let request = PersistentStateRequest::parse(&self.block, &self.masterchain_block)?;
        let peers = persistent_state::prepare_persistent_state(&request, context.clone()).await?;
        if peers.is_empty() {
            anyhow::bail!("persistent state peer was not found in discovered overlays");
        }

        persistent_state::download_persistent_state(
            peers,
            &request,
            self.output_file_path.as_deref(),
            context,
        )
        .await
    }
}
