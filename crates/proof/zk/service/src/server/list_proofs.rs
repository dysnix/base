//! Implementation of the `ListProofs` gRPC endpoint.

use base_zk_client::{
    ListProofsRequest, ListProofsResponse, ProofJobStatus, ProofSummary, get_proof_response,
};
use base_zk_db::ProofStatus;
use tonic::{Request, Response, Status};
use tracing::debug;

use crate::{metrics, server::ProverServiceServer};

const MAX_LIMIT: u64 = 100;
const DEFAULT_LIMIT: u64 = 50;

impl ProverServiceServer {
    /// Returns a paginated list of proof summaries for the given filter.
    pub async fn list_proofs_impl(
        &self,
        request: Request<ListProofsRequest>,
    ) -> Result<Response<ListProofsResponse>, Status> {
        let start = std::time::Instant::now();
        let result = self.list_proofs_inner(request).await;

        let (success, status_code) = match &result {
            Ok(_) => (true, "OK"),
            Err(s) => (false, metrics::grpc_status_code_str(s.code())),
        };
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        metrics::inc_requests("ListProofs", success, status_code);
        metrics::record_response_latency("ListProofs", success, elapsed_ms);

        result
    }

    async fn list_proofs_inner(
        &self,
        request: Request<ListProofsRequest>,
    ) -> Result<Response<ListProofsResponse>, Status> {
        let req = request.into_inner();

        let limit = match req.limit {
            0 => DEFAULT_LIMIT,
            n if n > MAX_LIMIT => MAX_LIMIT,
            n => n,
        };

        let offset = req.offset;
        if offset > i64::MAX as u64 {
            return Err(Status::invalid_argument("offset exceeds maximum supported value"));
        }

        let status_filter = match req.status_filter {
            None => None,
            Some(v) => {
                let proto_status = get_proof_response::Status::try_from(v).map_err(|_| {
                    Status::invalid_argument(format!("invalid status_filter value: {v}"))
                })?;
                match proto_status {
                    get_proof_response::Status::Unspecified => None,
                    get_proof_response::Status::Created => Some(ProofStatus::Created),
                    get_proof_response::Status::Pending => Some(ProofStatus::Pending),
                    get_proof_response::Status::Running => Some(ProofStatus::Running),
                    get_proof_response::Status::Succeeded => Some(ProofStatus::Succeeded),
                    get_proof_response::Status::Failed => Some(ProofStatus::Failed),
                }
            }
        };

        debug!(
            limit = limit,
            offset = offset,
            status_filter = ?status_filter,
            "listing proofs"
        );

        let (proofs, total_count) = self
            .repo
            .list_with_offset(status_filter, limit as i64, offset as i64)
            .await
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        let summaries: Vec<ProofSummary> = proofs
            .into_iter()
            .map(|p| ProofSummary {
                id: p.id.to_string(),
                start_block_number: p.start_block_number.max(0) as u64,
                number_of_blocks_to_prove: p.number_of_blocks_to_prove.max(0) as u64,
                proof_type: p.proof_type.proto_i32(),
                status: proto_status(p.status).into(),
                created_at: p.created_at.to_rfc3339(),
                updated_at: p.updated_at.to_rfc3339(),
                completed_at: p.completed_at.map(|t| t.to_rfc3339()),
                error_message: p.error_message,
            })
            .collect();

        Ok(Response::new(ListProofsResponse { proofs: summaries, total_count }))
    }
}

const fn proto_status(status: ProofStatus) -> ProofJobStatus {
    match status {
        ProofStatus::Created => ProofJobStatus::Created,
        ProofStatus::Pending => ProofJobStatus::Pending,
        ProofStatus::Running => ProofJobStatus::Running,
        ProofStatus::Succeeded => ProofJobStatus::Succeeded,
        ProofStatus::Failed => ProofJobStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proto_status_maps_all_variants() {
        assert_eq!(proto_status(ProofStatus::Created), ProofJobStatus::Created);
        assert_eq!(proto_status(ProofStatus::Pending), ProofJobStatus::Pending);
        assert_eq!(proto_status(ProofStatus::Running), ProofJobStatus::Running);
        assert_eq!(proto_status(ProofStatus::Succeeded), ProofJobStatus::Succeeded);
        assert_eq!(proto_status(ProofStatus::Failed), ProofJobStatus::Failed);
    }

    #[test]
    fn limit_clamping_zero_uses_default() {
        let limit = match 0u64 {
            0 => DEFAULT_LIMIT,
            n if n > MAX_LIMIT => MAX_LIMIT,
            n => n,
        };
        assert_eq!(limit, 50);
    }

    #[test]
    fn limit_clamping_exceeds_max_caps_at_100() {
        let limit = match 500u64 {
            0 => DEFAULT_LIMIT,
            n if n > MAX_LIMIT => MAX_LIMIT,
            n => n,
        };
        assert_eq!(limit, MAX_LIMIT);
    }

    #[test]
    fn limit_clamping_valid_value_passthrough() {
        let limit = match 25u64 {
            0 => DEFAULT_LIMIT,
            n if n > MAX_LIMIT => MAX_LIMIT,
            n => n,
        };
        assert_eq!(limit, 25);
    }
}
