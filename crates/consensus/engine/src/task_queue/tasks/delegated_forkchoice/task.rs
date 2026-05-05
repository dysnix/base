//! A follow-node task that applies delegated safe and finalized labels together.

use std::sync::Arc;

use async_trait::async_trait;
use base_common_genesis::RollupConfig;
use base_protocol::L2BlockInfo;
use derive_more::Constructor;

use crate::{DelegatedForkchoiceTaskError, Engine, EngineClient, EngineState, EngineTaskExt};

/// Delegated forkchoice labels from a remote follow source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegatedForkchoiceUpdate {
    /// The delegated safe L2 block.
    pub safe_l2: L2BlockInfo,
    /// The delegated finalized L2 block number, if available.
    pub finalized_l2_number: Option<u64>,
}

/// Applies delegated safe and finalized labels in engine-state order.
#[derive(Debug, Clone, Constructor)]
pub struct DelegatedForkchoiceTask<EngineClient_: EngineClient> {
    /// The engine client.
    pub client: Arc<EngineClient_>,
    /// The rollup config.
    pub cfg: Arc<RollupConfig>,
    /// The delegated labels to apply.
    pub update: DelegatedForkchoiceUpdate,
}

#[async_trait]
impl<EngineClient_: EngineClient> EngineTaskExt for DelegatedForkchoiceTask<EngineClient_> {
    type Output = ();
    type Error = DelegatedForkchoiceTaskError;

    async fn execute(&self, state: &mut EngineState) -> Result<(), Self::Error> {
        Engine::<EngineClient_>::delegated_forkchoice_with_state(
            state,
            Arc::clone(&self.client),
            Arc::clone(&self.cfg),
            self.update,
        )
        .await
    }
}
