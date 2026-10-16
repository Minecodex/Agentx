-- V2-04 Control intents and receipts for the Runtime Engine.
-- This is a destructive contract boundary: the migration runner refuses to
-- apply it over a populated V2-03 business database.
SET NAMES utf8mb4;
SET time_zone = '+00:00';

CREATE TABLE runtime_work_package_publications (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    package_id BINARY(16) NOT NULL,
    purpose ENUM('debug','evaluation') NOT NULL,
    source_type VARCHAR(64) NOT NULL,
    source_id BINARY(16) NOT NULL,
    source_revision VARCHAR(160) NOT NULL,
    content_hash CHAR(71) NOT NULL,
    signature_key_id VARCHAR(128) NOT NULL,
    package_json JSON NOT NULL,
    object_manifest_json JSON NOT NULL,
    status ENUM('building','prepared','running','completed','failed','cancelled','expired') NOT NULL,
    runtime_version BIGINT UNSIGNED NOT NULL DEFAULT 0,
    prepare_idempotency_key VARCHAR(192) NOT NULL,
    request_hash CHAR(71) NOT NULL,
    cancel_idempotency_key VARCHAR(192) NULL,
    prepare_receipt_json JSON NULL,
    cancel_receipt_json JSON NULL,
    expires_at TIMESTAMP(6) NOT NULL,
    created_by BINARY(16) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_control_work_package (tenant_id, package_id),
    UNIQUE KEY uq_control_work_package_prepare (tenant_id, prepare_idempotency_key),
    UNIQUE KEY uq_control_work_package_cancel (tenant_id, cancel_idempotency_key),
    KEY idx_control_work_package_source (tenant_id, source_type, source_id, created_at),
    KEY idx_control_work_package_expiry (status, expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE workflow_debug_runs (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    workflow_id BINARY(16) NOT NULL,
    source_json JSON NOT NULL,
    work_package_id BINARY(16) NOT NULL,
    command_id BINARY(16) NULL,
    command_version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    status ENUM('building','prepared','running','succeeded','failed','cancelled','expired') NOT NULL,
    result_receipt_json JSON NULL,
    error_code VARCHAR(128) NULL,
    error_message VARCHAR(1000) NULL,
    expires_at TIMESTAMP(6) NOT NULL,
    created_by BINARY(16) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    completed_at TIMESTAMP(6) NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_control_debug_package (tenant_id, work_package_id),
    KEY idx_control_debug_workflow (tenant_id, workflow_id, created_at),
    KEY idx_control_debug_expiry (status, expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE execution_fork_requests (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    source_execution_id BINARY(16) NOT NULL,
    checkpoint_id BINARY(16) NOT NULL,
    runtime_command_id BINARY(16) NOT NULL,
    idempotency_key VARCHAR(192) NOT NULL,
    mode ENUM('whole','node','to_node','from_node') NOT NULL,
    node_id VARCHAR(128) NULL,
    side_effect_resolution ENUM('execute','reuse_output','dry_run') NOT NULL,
    status ENUM('requested','accepted','rejected','completed','failed') NOT NULL,
    runtime_execution_id BINARY(16) NULL,
    receipt_json JSON NULL,
    requested_by BINARY(16) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    completed_at TIMESTAMP(6) NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_control_fork_idempotency (tenant_id, idempotency_key),
    UNIQUE KEY uq_control_fork_command (tenant_id, runtime_command_id),
    KEY idx_control_fork_source (tenant_id, source_execution_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_projection_cursors (
    projection_name VARCHAR(96) NOT NULL,
    partition_key VARCHAR(96) NOT NULL,
    last_cursor BIGINT UNSIGNED NOT NULL DEFAULT 0,
    snapshot_version BIGINT UNSIGNED NOT NULL DEFAULT 0,
    locked_by BINARY(16) NULL,
    locked_until TIMESTAMP(6) NULL,
    fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0,
    updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (projection_name, partition_key),
    KEY idx_control_projection_claim (locked_until, projection_name)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

ALTER TABLE evaluation_runs
    ADD COLUMN work_package_id BINARY(16) NULL AFTER evaluation_profile_version_id,
    ADD COLUMN runtime_command_id BINARY(16) NULL AFTER work_package_id,
    ADD COLUMN intent_version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER runtime_command_id,
    ADD COLUMN runtime_receipt_json JSON NULL AFTER parameters_json,
    ADD UNIQUE KEY uq_control_evaluation_package (tenant_id, work_package_id),
    ADD UNIQUE KEY uq_control_evaluation_command (tenant_id, runtime_command_id);

ALTER TABLE retention_runs
    ADD COLUMN runtime_command_id BINARY(16) NULL AFTER dry_run,
    ADD COLUMN policy_version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER runtime_command_id,
    ADD COLUMN idempotency_key VARCHAR(192) NULL AFTER policy_version,
    ADD COLUMN runtime_receipt_json JSON NULL AFTER idempotency_key,
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER locked_until,
    ADD UNIQUE KEY uq_control_retention_command (tenant_id, runtime_command_id),
    ADD UNIQUE KEY uq_control_retention_idempotency (tenant_id, idempotency_key);

ALTER TABLE outbox
    ADD COLUMN object_version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER aggregate_id,
    ADD COLUMN correlation_id BINARY(16) NULL AFTER idempotency_key,
    ADD COLUMN causation_id BINARY(16) NULL AFTER correlation_id;
