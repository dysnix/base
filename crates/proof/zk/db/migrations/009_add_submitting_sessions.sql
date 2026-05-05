-- Migration 009: Add durable submission stages for outbox-dispatched work.

ALTER TABLE proof_sessions
ALTER COLUMN backend_session_id DROP NOT NULL;

ALTER TABLE proof_sessions
ADD COLUMN IF NOT EXISTS updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW();

DROP TRIGGER IF EXISTS update_proof_sessions_updated_at ON proof_sessions;
CREATE TRIGGER update_proof_sessions_updated_at
    BEFORE UPDATE ON proof_sessions
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

DROP INDEX IF EXISTS idx_proof_sessions_active_stage;

CREATE UNIQUE INDEX idx_proof_sessions_active_stage
ON proof_sessions (proof_request_id, session_type)
WHERE status IN ('SUBMITTING', 'RUNNING');
