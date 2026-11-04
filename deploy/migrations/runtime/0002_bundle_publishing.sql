-- V2-02 Runtime Bundle, object, receipt, reference and GC state.
SET NAMES utf8mb4;
SET time_zone = '+00:00';

ALTER TABLE deployment_bundles
    ADD COLUMN deployment_id BINARY(16) NOT NULL AFTER application_id,
    ADD COLUMN workflow_id BINARY(16) NOT NULL AFTER deployment_id,
    ADD COLUMN workflow_version_id BINARY(16) NOT NULL AFTER workflow_id,
    ADD COLUMN signature_key_id VARCHAR(128) NOT NULL AFTER content_hash,
    ADD COLUMN object_manifest_json JSON NOT NULL AFTER payload_json,
    ADD COLUMN prepared_at TIMESTAMP(6) NULL AFTER created_at,
    ADD COLUMN activated_at TIMESTAMP(6) NULL AFTER prepared_at,
    ADD COLUMN superseded_at TIMESTAMP(6) NULL AFTER activated_at,
    ADD COLUMN disabled_at TIMESTAMP(6) NULL AFTER superseded_at,
    ADD COLUMN retained_until TIMESTAMP(6) NULL AFTER disabled_at,
    MODIFY status ENUM(
        'prepared', 'active', 'superseded', 'disabled', 'retained', 'garbage_collectable'
    ) NOT NULL DEFAULT 'prepared',
    ADD UNIQUE KEY uq_runtime_bundle_deployment (tenant_id, deployment_id),
    ADD KEY idx_runtime_bundle_gc (status, retained_until, created_at);

CREATE TABLE publish_receipts (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    operation ENUM('object_upload', 'admission', 'prepare', 'activate', 'rollback', 'disable') NOT NULL,
    idempotency_key VARCHAR(192) NOT NULL,
    request_hash CHAR(71) NOT NULL,
    bundle_id BINARY(16) NULL,
    application_id BINARY(16) NULL,
    deployment_id BINARY(16) NULL,
    status ENUM('accepted', 'rejected') NOT NULL,
    error_code VARCHAR(128) NULL,
    response_json JSON NOT NULL,
    object_version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_publish_receipt_idempotency (tenant_id, operation, idempotency_key),
    KEY idx_publish_receipt_bundle (tenant_id, bundle_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_objects (
    object_id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    object_key VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    content_hash CHAR(71) NOT NULL,
    size_bytes BIGINT UNSIGNED NOT NULL,
    media_type VARCHAR(255) NOT NULL,
    status ENUM('uploading', 'ready', 'deleting', 'deleted') NOT NULL DEFAULT 'uploading',
    temporary_key VARCHAR(1024) NULL,
    idempotency_key VARCHAR(192) NOT NULL,
    request_hash CHAR(71) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    ready_at TIMESTAMP(6) NULL,
    temporary_expires_at TIMESTAMP(6) NOT NULL,
    deleted_at TIMESTAMP(6) NULL,
    PRIMARY KEY (object_id),
    UNIQUE KEY uq_runtime_object_key (object_key),
    UNIQUE KEY uq_runtime_object_hash (tenant_id, object_id, content_hash),
    UNIQUE KEY uq_runtime_object_idempotency (tenant_id, idempotency_key),
    KEY idx_runtime_object_temporary (status, temporary_expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE bundle_objects (
    tenant_id BINARY(16) NOT NULL,
    bundle_id BINARY(16) NOT NULL,
    object_id BINARY(16) NOT NULL,
    content_hash CHAR(71) NOT NULL,
    size_bytes BIGINT UNSIGNED NOT NULL,
    media_type VARCHAR(255) NOT NULL,
    ordinal INT UNSIGNED NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (tenant_id, bundle_id, object_id),
    UNIQUE KEY uq_bundle_object_ordinal (tenant_id, bundle_id, ordinal),
    KEY idx_bundle_object_reference (tenant_id, object_id, bundle_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE bundle_references (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    bundle_id BINARY(16) NOT NULL,
    reference_kind ENUM(
        'active_execution', 'checkpoint_fork_source', 'pinned_session', 'pending_wait', 'retention_hold', 'application_page', 'application_mcp', 'application_batch', 'batch_page'
    ) NOT NULL,
    owner_id BINARY(16) NOT NULL,
    retained_until TIMESTAMP(6) NULL,
    released_at TIMESTAMP(6) NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_bundle_reference_owner (tenant_id, reference_kind, owner_id, bundle_id),
    KEY idx_bundle_reference_live (tenant_id, bundle_id, released_at, retained_until)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE bundle_retention_holds (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    bundle_id BINARY(16) NOT NULL,
    reason VARCHAR(512) NOT NULL,
    held_by VARCHAR(160) NOT NULL,
    expires_at TIMESTAMP(6) NULL,
    released_at TIMESTAMP(6) NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    KEY idx_bundle_hold_live (tenant_id, bundle_id, released_at, expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE bundle_gc_runs (
    id BINARY(16) NOT NULL,
    status ENUM('pending', 'marking', 'sweeping', 'completed', 'failed') NOT NULL DEFAULT 'pending',
    locked_by BINARY(16) NULL,
    locked_until TIMESTAMP(6) NULL,
    fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0,
    marked_count INT UNSIGNED NOT NULL DEFAULT 0,
    deleted_count INT UNSIGNED NOT NULL DEFAULT 0,
    failed_count INT UNSIGNED NOT NULL DEFAULT 0,
    started_at TIMESTAMP(6) NULL,
    completed_at TIMESTAMP(6) NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    KEY idx_bundle_gc_run_claim (status, locked_until, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE bundle_gc_items (
    id BINARY(16) NOT NULL,
    gc_run_id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    bundle_id BINARY(16) NULL,
    object_id BINARY(16) NULL,
    item_kind ENUM('bundle', 'object') NOT NULL,
    status ENUM('marked', 'deleting', 'deleted', 'failed', 'skipped_referenced') NOT NULL DEFAULT 'marked',
    locked_by BINARY(16) NULL,
    locked_until TIMESTAMP(6) NULL,
    fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0,
    attempt_count INT UNSIGNED NOT NULL DEFAULT 0,
    last_error VARCHAR(1000) NULL,
    deleted_at TIMESTAMP(6) NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_bundle_gc_bundle (gc_run_id, item_kind, bundle_id),
    UNIQUE KEY uq_bundle_gc_object (gc_run_id, item_kind, object_id),
    KEY idx_bundle_gc_item_claim (status, locked_until, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

ALTER TABLE application_invocations
    ADD COLUMN bundle_id BINARY(16) NULL AFTER workflow_version_id,
    ADD COLUMN admission_epoch BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER bundle_id,
    ADD COLUMN state_version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER admission_epoch,
    ADD COLUMN input_json JSON NULL AFTER state_version,
    ADD COLUMN result_json JSON NULL AFTER input_json,
    ADD COLUMN error_json JSON NULL AFTER result_json,
    ADD KEY idx_invocation_bundle (tenant_id, bundle_id, created_at);

ALTER TABLE workflow_executions
    ADD COLUMN application_id BINARY(16) NULL AFTER workflow_version_id,
    ADD COLUMN bundle_id BINARY(16) NULL AFTER application_id,
    ADD COLUMN admission_epoch BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER bundle_id,
    ADD COLUMN state_version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER admission_epoch,
    ADD COLUMN input_json JSON NULL AFTER state_version,
    ADD COLUMN output_json JSON NULL AFTER input_json,
    ADD COLUMN error_json JSON NULL AFTER output_json,
    ADD KEY idx_execution_query (tenant_id, application_id, created_at, id),
    ADD KEY idx_execution_bundle (tenant_id, bundle_id, created_at);

ALTER TABLE execution_snapshots
    ADD COLUMN bundle_id BINARY(16) NULL AFTER workflow_version_id,
    ADD COLUMN admission_epoch BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER bundle_id,
    ADD COLUMN state_version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER admission_epoch,
    ADD COLUMN output_json JSON NULL AFTER state_hash;

ALTER TABLE runtime_commands
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER locked_until;

ALTER TABLE execution_outbox
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER locked_until;
