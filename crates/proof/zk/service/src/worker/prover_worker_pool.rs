use std::{fmt, sync::Arc};

use async_trait::async_trait;
use base_zk_db::{ProofRequestRepo, SessionType};
use base_zk_outbox::{OutboxTask, TaskQueue};
use tokio::{sync::Mutex, task::JoinHandle};
use tracing::{Instrument, error, info};

use crate::{
    backends::{BackendRegistry, BackendType},
    metrics,
    worker::prover_worker::ProverWorker,
};

const SUBMIT_SNARK_TASK: &str = "submit_snark";
const SUBMIT_STARK_TASK: &str = "submit_stark";

/// Pool that creates `ProverWorker` instances and implements `TaskQueue`.
///
/// Tracks spawned worker `JoinHandle`s for graceful shutdown support.
#[derive(Clone)]
pub struct ProverWorkerPool {
    repo: ProofRequestRepo,
    backend_registry: Arc<BackendRegistry>,
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl fmt::Debug for ProverWorkerPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProverWorkerPool")
            .field("handles_count", &"<locked>")
            .finish_non_exhaustive()
    }
}

impl ProverWorkerPool {
    /// Creates a new worker pool.
    pub fn new(repo: ProofRequestRepo, backend_registry: Arc<BackendRegistry>) -> Self {
        Self { repo, backend_registry, handles: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Waits for all spawned workers to complete.
    ///
    /// This can be used during graceful shutdown to drain in-flight proving tasks.
    pub async fn shutdown(&self) {
        let handles: Vec<_> = {
            let mut guard = self.handles.lock().await;
            std::mem::take(&mut *guard)
        };
        for handle in handles {
            let _ = handle.await;
        }
    }
}

#[async_trait]
impl TaskQueue for ProverWorkerPool {
    async fn submit(&self, task: OutboxTask) -> anyhow::Result<()> {
        let proof_request_id = task.proof_request_id;
        let task_type = task.params.get("task_type").and_then(serde_json::Value::as_str);
        let session_type = match session_type_for_task_type(task_type) {
            Ok(session_type) => session_type,
            Err(other) => {
                error!(
                    proof_request_id = %proof_request_id,
                    task_type = %other,
                    "Unknown outbox task type"
                );
                metrics::inc_outbox_tasks_processed("failed", "unknown");
                anyhow::bail!("Unknown outbox task_type: {other}");
            }
        };

        let Some(claimed) =
            self.repo.create_submitting_stage_for_outbox(proof_request_id, session_type).await?
        else {
            info!(
                proof_request_id = %proof_request_id,
                session_type = %session_type,
                "Outbox task already has an active stage"
            );
            return Ok(());
        };

        let proof_type = claimed.proof_request.proof_type;
        let pt_label = metrics::proof_type_label(proof_type);
        let backend_type: BackendType = proof_type.into();

        // Get backend from registry
        let backend = self.backend_registry.get(backend_type).ok_or_else(|| {
            let error_msg = format!("Backend not found: {backend_type:?}");
            error!(
                proof_request_id = %proof_request_id,
                backend_type = ?backend_type,
                "Backend not found"
            );
            metrics::inc_outbox_tasks_processed("failed", pt_label);
            anyhow::anyhow!(error_msg)
        })?;

        info!(
            proof_request_id = %proof_request_id,
            backend = %backend.name(),
            "ProverWorkerPool: creating and spawning worker"
        );

        // Clone dependencies for the worker
        let repo = self.repo.clone();

        // Capture backend name before moving the Arc into ProverWorker
        let backend_name = backend.name();

        // Create a new ProverWorker
        let worker = ProverWorker::new(repo, backend, claimed);

        // Create a tracing span that propagates proof_request_id to ALL nested log
        // calls — including witness generation, L1-head calculation, cluster submission,
        // and deep library code. With `tracing_subscriber::fmt().json()` the span
        // fields are automatically included in every JSON log event.
        let prove_span = tracing::info_span!(
            "prove_request",
            proof_request_id = %proof_request_id,
            backend = %backend_name,
        );

        // Spawn the worker task, instrumenting the future with the span
        let handle = tokio::spawn(
            async move {
                let result = worker.run().await;

                // Log the result (actual task completion is tracked in database)
                match result {
                    Ok(()) => {
                        info!("Worker completed successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "Worker failed");
                    }
                }
            }
            .instrument(prove_span),
        );

        let mut guard = self.handles.lock().await;
        guard.retain(|h| !h.is_finished());
        guard.push(handle);

        metrics::inc_outbox_tasks_processed("submitted", pt_label);

        // Return immediately - task has been successfully submitted to the worker
        Ok(())
    }
}

fn session_type_for_task_type(task_type: Option<&str>) -> Result<SessionType, String> {
    match task_type {
        Some(task) if task == SUBMIT_SNARK_TASK => Ok(SessionType::Snark),
        Some(task) if task == SUBMIT_STARK_TASK => Ok(SessionType::Stark),
        None => Ok(SessionType::Stark),
        Some(other) => Err(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_task_type_defaults_to_stark() {
        assert_eq!(session_type_for_task_type(None), Ok(SessionType::Stark));
    }

    #[test]
    fn explicit_snark_task_maps_to_snark() {
        assert_eq!(session_type_for_task_type(Some(SUBMIT_SNARK_TASK)), Ok(SessionType::Snark));
    }

    #[test]
    fn unknown_task_type_is_rejected() {
        assert_eq!(
            session_type_for_task_type(Some("submit_stork")),
            Err("submit_stork".to_string())
        );
    }
}
