//! Task wrapper for finalizing an L2 block.

use std::sync::Arc;

use async_trait::async_trait;
use base_common_genesis::RollupConfig;
use derive_more::Constructor;

use crate::{Engine, EngineClient, EngineState, EngineTaskExt, FinalizeTaskError};

/// The [`FinalizeTask`] fetches the [`L2BlockInfo`] at `block_number`, updates the [`EngineState`],
/// and dispatches a forkchoice update to finalize the block.
#[derive(Debug, Clone, Constructor)]
pub struct FinalizeTask<EngineClient_: EngineClient> {
    /// The engine client.
    pub client: Arc<EngineClient_>,
    /// The rollup config.
    pub cfg: Arc<RollupConfig>,
    /// The number of the L2 block to finalize.
    pub block_number: u64,
}

#[async_trait]
impl<EngineClient_: EngineClient> EngineTaskExt for FinalizeTask<EngineClient_> {
    type Output = ();

    type Error = FinalizeTaskError;

    async fn execute(&self, state: &mut EngineState) -> Result<(), FinalizeTaskError> {
        Engine::<EngineClient_>::finalize_with_state(
            state,
            Arc::clone(&self.client),
            Arc::clone(&self.cfg),
            self.block_number,
        )
        .await
    }
}
