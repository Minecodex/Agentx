-- V2-05 authoritative query, integration export and trace delivery schema.
-- The V2 migration runner applies this only to a recreated, empty V2 domain.
SET NAMES utf8mb4;
SET time_zone = '+00:00';

CREATE TABLE runtime_query_snapshots (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    subject_id BINARY(16) NOT NULL,
    query_kind ENUM('execution','invocation') NOT NULL,
    filter_hash CHAR(71) NOT NULL,
    upper_bound VARCHAR(160) NOT NULL,
    total_count BIGINT UNSIGNED NOT NULL,
    expires_at TIMESTAMP(6) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    KEY idx_runtime_query_snapshot_expiry (expires_at),
    KEY idx_runtime_query_snapshot_subject (tenant_id, subject_id, query_kind, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_query_snapshot_items (
    snapshot_id BINARY(16) NOT NULL,
    ordinal BIGINT UNSIGNED NOT NULL,
    object_id BINARY(16) NOT NULL,
    summary_json JSON NOT NULL,
    PRIMARY KEY (snapshot_id, ordinal),
    UNIQUE KEY uq_runtime_query_snapshot_object (snapshot_id, object_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_query_receipts (
    jti BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    subject_id BINARY(16) NOT NULL,
    scope VARCHAR(128) NOT NULL,
    request_hash CHAR(71) NOT NULL,
    status ENUM('accepted','rejected','completed','failed') NOT NULL,
    result_code VARCHAR(128) NULL,
    expires_at TIMESTAMP(6) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    completed_at TIMESTAMP(6) NULL,
    PRIMARY KEY (jti),
    KEY idx_runtime_query_receipt_expiry (expires_at),
    KEY idx_runtime_query_receipt_subject (tenant_id, subject_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE integration_event_sequence (
    sequence_key VARCHAR(32) NOT NULL,
    next_cursor BIGINT UNSIGNED NOT NULL,
    retention_floor_cursor BIGINT UNSIGNED NOT NULL DEFAULT 1,
    updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (sequence_key)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

INSERT INTO integration_event_sequence(sequence_key,next_cursor) VALUES ('runtime',1);

DROP TABLE integration_event_log;
CREATE TABLE integration_event_log (
    event_cursor BIGINT UNSIGNED NOT NULL,
    event_id BINARY(16) NOT NULL,
    source_outbox_id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    event_type VARCHAR(128) NOT NULL,
    aggregate_type VARCHAR(64) NOT NULL,
    aggregate_id VARCHAR(128) NOT NULL,
    aggregate_version BIGINT UNSIGNED NOT NULL,
    schema_version SMALLINT UNSIGNED NOT NULL,
    payload_json JSON NOT NULL,
    content_hash CHAR(71) NOT NULL,
    correlation_id BINARY(16) NOT NULL,
    causation_id BINARY(16) NULL,
    occurred_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    retention_until TIMESTAMP(6) NOT NULL DEFAULT (CURRENT_TIMESTAMP(6) + INTERVAL 7 DAY),
    PRIMARY KEY (event_cursor),
    UNIQUE KEY uq_integration_event_id (event_id),
    UNIQUE KEY uq_integration_event_source (source_outbox_id),
    KEY idx_integration_event_tenant (tenant_id, event_cursor),
    KEY idx_integration_event_retention (retention_until, event_cursor),
    CONSTRAINT chk_integration_event_schema_v205 CHECK (schema_version = 1),
    CONSTRAINT chk_integration_event_hash_v205 CHECK (content_hash REGEXP '^sha256:[0-9a-f]{64}$')
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_user_workflow_grants (
    tenant_id BINARY(16) NOT NULL,
    user_id BINARY(16) NOT NULL,
    workflow_id BINARY(16) NOT NULL,
    grant_version BIGINT UNSIGNED NOT NULL,
    status ENUM('active','revoked') NOT NULL,
    admission_epoch BIGINT UNSIGNED NOT NULL,
    updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (tenant_id,user_id,workflow_id),
    KEY idx_runtime_user_workflow_status (tenant_id,user_id,status,workflow_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

ALTER TABLE runtime_user_admission
    ADD COLUMN tenant_query_enabled BOOLEAN NOT NULL DEFAULT FALSE AFTER status,
    ADD COLUMN role_assignments_json JSON NOT NULL AFTER tenant_query_enabled;

ALTER TABLE runtime_user_application_grants
    ADD COLUMN can_invoke BOOLEAN NOT NULL DEFAULT FALSE AFTER status,
    ADD COLUMN can_query BOOLEAN NOT NULL DEFAULT FALSE AFTER can_invoke;

ALTER TABLE workflow_executions
    ADD COLUMN initiator_user_id BINARY(16) NULL AFTER trigger_type,
    ADD COLUMN initiator_user_name VARCHAR(255) NULL AFTER initiator_user_id,
    ADD COLUMN initiator_department_id BINARY(16) NULL AFTER initiator_user_name,
    ADD COLUMN initiator_department_name VARCHAR(255) NULL AFTER initiator_department_id,
    ADD COLUMN trigger_source_id BINARY(16) NULL AFTER initiator_department_name,
    ADD COLUMN trigger_name VARCHAR(255) NULL AFTER trigger_source_id,
    ADD KEY idx_runtime_execution_snapshot (tenant_id,created_at,id),
    ADD KEY idx_runtime_execution_app_snapshot (tenant_id,application_id,created_at,id),
    ADD KEY idx_runtime_execution_workflow_snapshot (tenant_id,workflow_id,created_at,id),
    ADD KEY idx_runtime_execution_status_snapshot (tenant_id,status,created_at,id),
    ADD KEY idx_runtime_execution_user_snapshot (tenant_id,initiator_user_id,created_at,id),
    ADD KEY idx_runtime_execution_department_snapshot (tenant_id,initiator_department_id,created_at,id),
    ADD KEY idx_runtime_execution_trigger_snapshot (tenant_id,trigger_type,trigger_source_id,created_at,id);

ALTER TABLE execution_snapshots
    ADD COLUMN execution_context_json JSON NOT NULL AFTER policy_snapshot_json;

ALTER TABLE application_invocations
    ADD KEY idx_runtime_invocation_snapshot (tenant_id,created_at,id),
    ADD KEY idx_runtime_invocation_status_snapshot (tenant_id,status,created_at,id);

ALTER TABLE node_executions
    ADD KEY idx_runtime_query_node (tenant_id,execution_id,run_index,iteration_index,id);

ALTER TABLE node_attempts
    ADD KEY idx_runtime_query_attempt (tenant_id,execution_id,node_execution_id,attempt_number,id);

ALTER TABLE runtime_calls
    ADD KEY idx_runtime_query_call (tenant_id,execution_id,node_execution_id,started_at,id);


ALTER TABLE checkpoints
    ADD KEY idx_runtime_query_checkpoint (tenant_id,execution_id,sequence_number,id);

ALTER TABLE artifacts
    ADD KEY idx_runtime_query_artifact (tenant_id,created_at,id);

ALTER TABLE execution_outbox
    MODIFY COLUMN execution_id BINARY(16) NULL,
    ADD COLUMN event_type VARCHAR(128) NULL AFTER message_type,
    ADD COLUMN aggregate_type VARCHAR(64) NULL AFTER event_type,
    ADD COLUMN aggregate_id VARCHAR(128) NULL AFTER aggregate_type,
    ADD COLUMN aggregate_version BIGINT UNSIGNED NULL AFTER aggregate_id,
    ADD COLUMN correlation_id BINARY(16) NULL AFTER aggregate_version,
    ADD COLUMN causation_id BINARY(16) NULL AFTER correlation_id;

ALTER TABLE trace_outbox
    ADD COLUMN execution_sequence BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER execution_id,
    ADD COLUMN content_hash CHAR(71) NULL AFTER payload_json,
    ADD COLUMN stream_id VARCHAR(64) NULL AFTER content_hash,
    ADD COLUMN heartbeat_at TIMESTAMP(6) NULL AFTER locked_until,
    ADD COLUMN streamed_at TIMESTAMP(6) NULL AFTER last_error,
    ADD UNIQUE KEY uq_trace_execution_sequence (tenant_id,execution_id,execution_sequence);

ALTER TABLE evaluation_runs
    ADD COLUMN last_event_cursor BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER version,
    ADD COLUMN projection_deleted BOOLEAN NOT NULL DEFAULT FALSE AFTER last_event_cursor,
    ADD KEY idx_runtime_evaluation_snapshot (tenant_id,last_event_cursor,id);

ALTER TABLE runtime_work_packages
    ADD COLUMN last_event_cursor BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER version,
    ADD COLUMN projection_deleted BOOLEAN NOT NULL DEFAULT FALSE AFTER last_event_cursor,
    ADD KEY idx_runtime_package_snapshot (tenant_id,purpose,last_event_cursor,id);

ALTER TABLE retention_runs
    ADD COLUMN last_event_cursor BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER policy_version,
    ADD COLUMN projection_deleted BOOLEAN NOT NULL DEFAULT FALSE AFTER last_event_cursor,
    ADD KEY idx_runtime_retention_snapshot (tenant_id,last_event_cursor,id);

ALTER TABLE notifications
    ADD COLUMN version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER tone,
    ADD COLUMN last_event_cursor BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER version,
    ADD COLUMN projection_deleted BOOLEAN NOT NULL DEFAULT FALSE AFTER last_event_cursor,
    ADD KEY idx_runtime_notification_snapshot (tenant_id,last_event_cursor,id);
