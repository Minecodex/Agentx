-- plan7 P7-B: time-to-first-token for streaming model calls. Written when the
-- first delta frame is parsed and surfaced on the runtime_call Trace span.
SET NAMES utf8mb4;
SET time_zone = '+00:00';

ALTER TABLE runtime_calls
    ADD COLUMN first_token_ms INT UNSIGNED NULL AFTER usage_estimated;

-- Debug executions have no Invocation. Their live preview uses an independent
-- cursor and never reserves Trace watermarks or sends tokens to ClickHouse.
CREATE TABLE execution_model_deltas (
    sequence_number BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    tenant_id BINARY(16) NOT NULL,
    execution_id BINARY(16) NOT NULL,
    payload_json JSON NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (sequence_number),
    KEY idx_execution_model_delta_cursor (tenant_id, execution_id, sequence_number)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
