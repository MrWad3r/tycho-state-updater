use anyhow::Result;

use crate::cli_context::build_context;
use crate::{
    DownloadStateArgs, PersistentStateRequest, decode_node_id, overlay_client, persistent_state,
};

impl DownloadStateArgs {
    pub async fn run(self) -> Result<()> {
        let context = build_context(&self.global_config, self.bind_port).await?;
        let request = PersistentStateRequest::parse(&self.block, &self.masterchain_block)?;

        let target_node_id = {
            let node_id = decode_node_id(&self.node_id)?;
            let (_, target_node_id) =
                overlay_client::resolve_node(&node_id, context.clone()).await?;
            target_node_id
        };

        let prepared =
            persistent_state::prepare_persistent_state(&target_node_id, &request, context.clone())
                .await?;

        if !prepared {
            anyhow::bail!("persistent state is not available on the target node");
        }

        persistent_state::download_persistent_state(
            &target_node_id,
            &request,
            self.output_file_path.as_deref(),
            context,
        )
        .await
    }
}
