use std::{fmt, future::Future, sync::Arc, time::Duration};

use base_zk_client::ProveBlockRequest;
use base_zk_db::{ClaimedProofRequest, ProofRequest, ProofRequestRepo, SessionType};
use tracing::{debug, error, info, warn};

use crate::{backends::ProvingBackend, metrics};

const SUBMISSION_HEARTBEAT_SECS: u64 = 30;

/// Individual worker that processes a single proving task
pub struct ProverWorker {
    repo: ProofRequestRepo,
    backend: Arc<dyn ProvingBackend>,
    claimed: ClaimedProofRequest,
}

impl fmt::Debug for ProverWorker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProverWorker")
            .field("proof_request_id", &self.claimed.proof_request.id)
            .field("backend", &self.backend.name())
            .finish_non_exhaustive()
    }
}

impl ProverWorker {
    /// Creates a worker bound to one proof request (`proof_request_id=<uuid>`).
    pub fn new(
        repo: ProofRequestRepo,
        backend: Arc<dyn ProvingBackend>,
        claimed: ClaimedProofRequest,
    ) -> Self {
        Self { repo, backend, claimed }
    }

    /// Run the proving task
    pub async fn run(self) -> anyhow::Result<()> {
        let proof_request = &self.claimed.proof_request;
        info!(
            proof_request_id = %proof_request.id,
            session_type = %self.claimed.session_type,
            "Submitting durable proving stage"
        );

        info!(
            proof_request_id = %proof_request.id,
            backend = %self.backend.name(),
            "Starting backend submission"
        );

        debug!(
            proof_request_id = %proof_request.id,
            backend = %self.backend.name(),
            "Calling backend"
        );

        let pt_label = metrics::proof_type_label(proof_request.proof_type);

        let result = match self.claimed.session_type {
            SessionType::Stark => {
                let request = match proof_request_to_proto(proof_request) {
                    Ok(request) => request,
                    Err(e) => {
                        let error_msg = format!("Invalid proof request: {e}");
                        self.fail_submitting_request(proof_request, error_msg.clone()).await?;
                        return Err(anyhow::anyhow!(error_msg));
                    }
                };
                await_with_heartbeat(
                    self.repo.clone(),
                    self.claimed.proof_session_id,
                    self.backend.prove(&request),
                )
                .await
            }
            SessionType::Snark => {
                await_with_heartbeat(
                    self.repo.clone(),
                    self.claimed.proof_session_id,
                    self.backend.submit_snark(proof_request),
                )
                .await
            }
        };

        match result {
            Ok(prove_result) => {
                // Record witness generation duration on success
                if let Some(wg_ms) = prove_result.witness_gen_duration_ms {
                    metrics::record_witness_generation_duration(pt_label, true, wg_ms);
                }

                // Success path
                if let Some(session_id) = prove_result.session_id {
                    info!(
                        proof_request_id = %proof_request.id,
                        session_id = %session_id,
                        backend = %self.backend.name(),
                        "Got backend session ID"
                    );

                    match self
                        .repo
                        .mark_submitting_session_running(
                            self.claimed.proof_session_id,
                            &session_id,
                            prove_result.metadata,
                        )
                        .await
                    {
                        Ok(true) => {
                            info!(
                                proof_request_id = %proof_request.id,
                                backend_session_id = %session_id,
                                db_session_id = self.claimed.proof_session_id,
                                "Marked proof session RUNNING"
                            );
                        }
                        Ok(false) => {
                            error!(
                                proof_request_id = %proof_request.id,
                                backend_session_id = %session_id,
                                "Backend accepted job but session could not be marked RUNNING"
                            );
                            return Err(anyhow::anyhow!(
                                "Backend accepted session {session_id} for request {}, but DB state could not be marked RUNNING",
                                proof_request.id
                            ));
                        }
                        Err(e) => {
                            error!(
                                proof_request_id = %proof_request.id,
                                backend_session_id = %session_id,
                                backend = %self.backend.name(),
                                error = %e,
                                "Failed to persist session after successful prove — backend session may be orphaned"
                            );
                            return Err(anyhow::anyhow!(
                                "Failed to persist session {session_id} for request {}: {e}",
                                proof_request.id
                            ));
                        }
                    }
                } else {
                    let error_msg = "Backend returned no session ID".to_string();
                    error!(
                        proof_request_id = %proof_request.id,
                        "Backend submission returned without session ID"
                    );
                    self.fail_submitting_request(proof_request, error_msg.clone()).await?;
                    return Err(anyhow::anyhow!(error_msg));
                }

                Ok(())
            }
            Err(e) => {
                // Failure path
                let error_msg = format!("Backend error: {e}");
                warn!(
                    proof_request_id = %proof_request.id,
                    backend = %self.backend.name(),
                    error = %error_msg,
                    "Backend proving failed"
                );

                let was_failed = self
                    .repo
                    .fail_submitting_session_and_request(
                        self.claimed.proof_session_id,
                        proof_request.id,
                        error_msg.clone(),
                    )
                    .await?;

                if was_failed {
                    // Emit proof_requests_completed for early failures (PENDING → FAILED).
                    // These are never seen by the StatusPoller (which only queries RUNNING),
                    // so we emit directly here.
                    metrics::inc_proof_requests_completed("failed", pt_label);

                    info!(
                        proof_request_id = %proof_request.id,
                        "Updated proof request as FAILED"
                    );
                } else {
                    warn!(
                        proof_request_id = %proof_request.id,
                        "Could not transition submitting stage to FAILED"
                    );
                }

                Err(anyhow::anyhow!(error_msg))
            }
        }
    }

    async fn fail_submitting_request(
        &self,
        proof_request: &ProofRequest,
        error_msg: String,
    ) -> anyhow::Result<()> {
        let was_failed = self
            .repo
            .fail_submitting_session_and_request(
                self.claimed.proof_session_id,
                proof_request.id,
                error_msg,
            )
            .await?;

        if !was_failed {
            warn!(
                proof_request_id = %proof_request.id,
                proof_session_id = self.claimed.proof_session_id,
                "Could not transition submitting stage to FAILED"
            );
        }

        Ok(())
    }
}

async fn await_with_heartbeat<F, T>(
    repo: ProofRequestRepo,
    proof_session_id: i64,
    future: F,
) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    tokio::pin!(future);
    let mut interval = tokio::time::interval(Duration::from_secs(SUBMISSION_HEARTBEAT_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            result = &mut future => return result,
            _ = interval.tick() => {
                if let Err(e) = repo.touch_submitting_session(proof_session_id).await {
                    warn!(
                        proof_session_id,
                        error = %e,
                        "Failed to heartbeat submitting session"
                    );
                }
            }
        }
    }
}

fn proof_request_to_proto(proof_request: &ProofRequest) -> anyhow::Result<ProveBlockRequest> {
    Ok(ProveBlockRequest {
        start_block_number: u64::try_from(proof_request.start_block_number)?,
        number_of_blocks_to_prove: u64::try_from(proof_request.number_of_blocks_to_prove)?,
        sequence_window: proof_request.sequence_window.map(u64::try_from).transpose()?,
        proof_type: proof_request.proof_type.proto_i32(),
        session_id: None,
        prover_address: proof_request.prover_address.clone(),
        l1_head: proof_request.l1_head.clone(),
        intermediate_root_interval: proof_request
            .intermediate_root_interval
            .map(u64::try_from)
            .transpose()?,
    })
}
