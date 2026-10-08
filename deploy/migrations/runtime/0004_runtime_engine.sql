-- V2-04 authoritative Runtime Engine state.
-- The migration runner requires an empty V2-03 business domain before this
-- destructive v1 contract rewrite is applied.
SET NAMES utf8mb4;
SET time_zone = '+00:00';

CREATE TABLE execution_runtime_state (
    execution_id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    state_version BIGINT UNSIGNED NOT NULL,
    context_version BIGINT UNSIGNED NOT NULL DEFAULT 0,
    delivery_sequence BIGINT UNSIGNED NOT NULL DEFAULT 0,
    activation_count BIGINT UNSIGNED NOT NULL DEFAULT 0,
    activation_budget BIGINT UNSIGNED NOT NULL,
    current_frontier_json JSON NOT NULL,
    context_json JSON NOT NULL,
    machine_state_json JSON NOT NULL,
    machine_state_hash CHAR(71) NOT NULL,
    terminal_result_json JSON NULL,
    terminal_result_hash CHAR(71) NULL,
    terminal_result_object_id BINARY(16) NULL,
    updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (execution_id),
    KEY idx_execution_runtime_state (tenant_id, updated_at),
    CONSTRAINT chk_runtime_activation_budget CHECK (activation_count <= activation_budget)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_resource_bindings (
    tenant_id BINARY(16) NOT NULL,
    binding_id BINARY(16) NOT NULL,
    bundle_id BINARY(16) NULL,
    work_package_id BINARY(16) NULL,
    resource_kind VARCHAR(32) NOT NULL,
    resource_id BINARY(16) NOT NULL,
    resource_version VARCHAR(160) NOT NULL,
    state_epoch BIGINT UNSIGNED NOT NULL,
    content_hash CHAR(71) NOT NULL,
    configuration_json JSON NOT NULL,
    object_ids_json JSON NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (binding_id),
    UNIQUE KEY uq_runtime_resource_binding (
        tenant_id, bundle_id, work_package_id, resource_kind, resource_id, resource_version
    ),
    KEY idx_runtime_binding_bundle (tenant_id, bundle_id, resource_kind),
    KEY idx_runtime_binding_package (tenant_id, work_package_id, resource_kind)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_resource_states (
    tenant_id BINARY(16) NOT NULL,
    resource_kind VARCHAR(32) NOT NULL,
    resource_id BINARY(16) NOT NULL,
    resource_version VARCHAR(160) NOT NULL,
    state_epoch BIGINT UNSIGNED NOT NULL,
    status ENUM('active','disabled','revoked') NOT NULL,
    content_hash CHAR(71) NOT NULL,
    applied_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (tenant_id, resource_kind, resource_id),
    KEY idx_runtime_resource_state (tenant_id, status, state_epoch)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_composite_snapshots (
    tenant_id BINARY(16) NOT NULL,
    bundle_id BINARY(16) NULL,
    work_package_id BINARY(16) NULL,
    binding_id BINARY(16) NOT NULL,
    workflow_version_id BINARY(16) NOT NULL,
    workflow_json JSON NOT NULL,
    definition_object_id BINARY(16) NOT NULL,
    ir_object_id BINARY(16) NOT NULL,
    definition_hash CHAR(71) NOT NULL,
    ir_hash CHAR(71) NOT NULL,
    definition_json JSON NOT NULL,
    compiled_ir_json JSON NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (binding_id),
    UNIQUE KEY uq_runtime_composite_bundle (tenant_id, bundle_id, workflow_version_id),
    UNIQUE KEY uq_runtime_composite_package (tenant_id, work_package_id, workflow_version_id),
    KEY idx_runtime_composite_definition (tenant_id, definition_object_id),
    CONSTRAINT chk_runtime_composite_owner CHECK (
        (bundle_id IS NULL AND work_package_id IS NOT NULL)
        OR (bundle_id IS NOT NULL AND work_package_id IS NULL)
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_work_packages (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    purpose ENUM('debug','evaluation') NOT NULL,
    call_purpose ENUM('debug','evaluation') NOT NULL,
    source_revision VARCHAR(160) NOT NULL,
    schema_version SMALLINT UNSIGNED NOT NULL,
    content_hash CHAR(71) NOT NULL,
    signature_key_id VARCHAR(128) NOT NULL,
    signature VARBINARY(96) NOT NULL,
    prepare_idempotency_key VARCHAR(192) NOT NULL,
    execute_idempotency_key VARCHAR(192) NULL,
    execute_request_hash CHAR(71) NULL,
    cancel_idempotency_key VARCHAR(192) NULL,
    payload_json JSON NOT NULL,
    worker_compatibility_json JSON NOT NULL,
    status ENUM('prepared','running','succeeded','failed','cancelled','expired') NOT NULL,
    version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    cancellation_version BIGINT UNSIGNED NOT NULL DEFAULT 0,
    result_json JSON NULL,
    result_hash CHAR(71) NULL,
    error_code VARCHAR(128) NULL,
    error_message VARCHAR(1000) NULL,
    expires_at TIMESTAMP(6) NOT NULL,
    started_at TIMESTAMP(6) NULL,
    completed_at TIMESTAMP(6) NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_runtime_work_package_hash (tenant_id, purpose, content_hash),
    UNIQUE KEY uq_runtime_work_package_prepare (tenant_id, prepare_idempotency_key),
    UNIQUE KEY uq_runtime_work_package_execute (tenant_id, execute_idempotency_key),
    UNIQUE KEY uq_runtime_work_package_cancel (tenant_id, cancel_idempotency_key),
    KEY idx_runtime_work_package_expiry (status, expires_at),
    CONSTRAINT chk_runtime_work_package_schema CHECK (schema_version = 1)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_work_package_objects (
    tenant_id BINARY(16) NOT NULL,
    work_package_id BINARY(16) NOT NULL,
    object_id BINARY(16) NOT NULL,
    content_hash CHAR(71) NOT NULL,
    reference_role VARCHAR(64) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (work_package_id, object_id, reference_role),
    KEY idx_runtime_package_object (tenant_id, object_id, work_package_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE execution_children (
    tenant_id BINARY(16) NOT NULL,
    parent_execution_id BINARY(16) NOT NULL,
    parent_node_execution_id BINARY(16) NOT NULL,
    child_execution_id BINARY(16) NOT NULL,
    child_bundle_id BINARY(16) NOT NULL,
    relationship ENUM('composite','evaluation','agent_tool') NOT NULL,
    context_overlay_json JSON NOT NULL,
    context_overlay_hash CHAR(71) NOT NULL,
    merge_status ENUM('pending','merged','discarded') NOT NULL DEFAULT 'pending',
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    merged_at TIMESTAMP(6) NULL,
    PRIMARY KEY (parent_execution_id, child_execution_id),
    UNIQUE KEY uq_runtime_child_execution (child_execution_id),
    KEY idx_runtime_children_parent (tenant_id, parent_execution_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE execution_forks (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    source_execution_id BINARY(16) NOT NULL,
    source_checkpoint_id BINARY(16) NOT NULL,
    fork_execution_id BINARY(16) NOT NULL,
    bundle_id BINARY(16) NOT NULL,
    mode ENUM('whole','node','to_node','from_node') NOT NULL,
    node_id VARCHAR(128) NULL,
    side_effect_resolution ENUM('execute','reuse_output','dry_run') NOT NULL,
    idempotency_key VARCHAR(192) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_runtime_fork_execution (tenant_id, fork_execution_id),
    UNIQUE KEY uq_runtime_fork_idempotency (tenant_id, idempotency_key),
    KEY idx_runtime_fork_source (tenant_id, source_execution_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE worker_result_receipts (
    attempt_id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    worker_id BINARY(16) NOT NULL,
    fencing_token BIGINT UNSIGNED NOT NULL,
    result_hash CHAR(71) NOT NULL,
    status ENUM('accepted','replayed','rejected') NOT NULL,
    response_json JSON NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (attempt_id),
    UNIQUE KEY uq_worker_result_hash (tenant_id, attempt_id, result_hash)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_retention_holds (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    aggregate_type VARCHAR(64) NOT NULL,
    aggregate_id BINARY(16) NOT NULL,
    reason VARCHAR(512) NOT NULL,
    version BIGINT UNSIGNED NOT NULL,
    released_at TIMESTAMP(6) NULL,
    expires_at TIMESTAMP(6) NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_runtime_retention_hold (tenant_id, aggregate_type, aggregate_id, id),
    KEY idx_runtime_retention_hold_live (tenant_id, aggregate_type, aggregate_id, released_at, expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE runtime_role_leases (
    role_key VARCHAR(64) NOT NULL,
    locked_by BINARY(16) NULL,
    locked_until TIMESTAMP(6) NULL,
    fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0,
    heartbeat_at TIMESTAMP(6) NULL,
    PRIMARY KEY (role_key),
    KEY idx_runtime_role_lease_claim (locked_until, role_key)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

INSERT INTO runtime_role_leases(role_key) VALUES ('quota_projection');

ALTER TABLE workflow_executions
    ADD COLUMN work_package_id BINARY(16) NULL AFTER bundle_id,
    ADD COLUMN parent_execution_id BINARY(16) NULL AFTER work_package_id,
    ADD COLUMN parent_node_execution_id BINARY(16) NULL AFTER parent_execution_id,
    ADD COLUMN terminal_result_json JSON NULL AFTER error_json,
    ADD COLUMN terminal_result_hash CHAR(71) NULL AFTER terminal_result_json,
    ADD COLUMN terminal_result_object_id BINARY(16) NULL AFTER terminal_result_hash,
    ADD COLUMN terminal_result_size_bytes BIGINT UNSIGNED GENERATED ALWAYS AS (JSON_STORAGE_SIZE(terminal_result_json)) STORED,
    ADD COLUMN input_size_bytes BIGINT UNSIGNED GENERATED ALWAYS AS (JSON_STORAGE_SIZE(input_json)) STORED,
    ADD KEY idx_runtime_terminal_externalize (terminal_result_object_id, terminal_result_size_bytes, id),
    ADD KEY idx_runtime_execution_input_size (input_size_bytes, tenant_id, id),
    ADD COLUMN retention_deleted_at TIMESTAMP(6) NULL AFTER terminal_result_object_id,
    ADD KEY idx_runtime_execution_package (tenant_id, work_package_id, created_at),
    ADD KEY idx_runtime_execution_parent (tenant_id, parent_execution_id, created_at);

ALTER TABLE execution_snapshots
    ADD COLUMN work_package_id BINARY(16) NULL AFTER bundle_id,
    ADD COLUMN authorization_snapshot_json JSON NOT NULL AFTER resource_snapshot_json,
    ADD COLUMN policy_snapshot_json JSON NOT NULL AFTER authorization_snapshot_json,
    ADD COLUMN worker_compatibility_json JSON NOT NULL AFTER policy_snapshot_json,
    ADD COLUMN object_manifest_json JSON NOT NULL AFTER worker_compatibility_json;

ALTER TABLE node_executions
    ADD COLUMN node_key VARCHAR(128) NOT NULL AFTER node_id,
    MODIFY capability VARCHAR(64) NOT NULL;

ALTER TABLE execution_outbox
    MODIFY capability VARCHAR(64) NULL,
    ADD COLUMN heartbeat_at TIMESTAMP(6) NULL AFTER locked_until,
    ADD COLUMN task_hash CHAR(71) NULL AFTER payload_json;

ALTER TABLE publish_receipts
    MODIFY operation VARCHAR(64) NOT NULL;

ALTER TABLE node_attempts
    ADD COLUMN capability VARCHAR(64) NOT NULL AFTER attempt_number,
    ADD COLUMN worker_protocol_version SMALLINT UNSIGNED NOT NULL DEFAULT 1 AFTER capability,
    ADD COLUMN ir_schema_version SMALLINT UNSIGNED NOT NULL DEFAULT 1 AFTER worker_protocol_version,
    ADD COLUMN compiler_version VARCHAR(64) NOT NULL AFTER ir_schema_version,
    ADD COLUMN manifest_version VARCHAR(64) NOT NULL AFTER compiler_version,
    ADD COLUMN heartbeat_at TIMESTAMP(6) NULL AFTER locked_until,
    ADD COLUMN result_hash CHAR(71) NULL AFTER output_json,
    ADD COLUMN result_object_id BINARY(16) NULL AFTER result_hash,
    ADD COLUMN outcome_unknown BOOLEAN NOT NULL DEFAULT FALSE AFTER result_object_id,
    ADD KEY idx_runtime_attempt_compatibility (
        status, capability, worker_protocol_version, ir_schema_version, locked_until
    );

ALTER TABLE worker_leases
    MODIFY capability VARCHAR(64) NOT NULL,
    ADD COLUMN worker_id BINARY(16) NOT NULL AFTER worker_instance_id,
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER lease_token,
    ADD COLUMN operation_deadline_at TIMESTAMP(6) NOT NULL AFTER expires_at,
    ADD COLUMN result_hash CHAR(71) NULL AFTER operation_deadline_at;

ALTER TABLE node_invocation_handles
    MODIFY resource_version VARCHAR(160) NULL,
    ADD COLUMN operation_key VARCHAR(64) NOT NULL AFTER handle_kind,
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL AFTER lease_token,
    ADD COLUMN operation_deadline_at TIMESTAMP(6) NOT NULL AFTER expires_at,
    ADD COLUMN revoked_at TIMESTAMP(6) NULL AFTER consumed_at,
    ADD UNIQUE KEY uq_runtime_handle_operation (tenant_id, attempt_id, operation_key, resource_id);

ALTER TABLE runtime_calls
    MODIFY call_kind VARCHAR(32) NOT NULL,
    MODIFY status ENUM('reserved','sent','succeeded','failed','cancelled','outcome_unknown') NOT NULL,
    MODIFY resource_version_id VARCHAR(160) NULL,
    ADD COLUMN plugin_parent_span_entity_id BINARY(16) NULL AFTER agent_run_id,
    ADD COLUMN provider_request_id VARCHAR(255) NULL AFTER request_fingerprint,
    ADD COLUMN request_json JSON NULL AFTER provider_request_id,
    ADD COLUMN response_json JSON NULL AFTER response_artifact_id,
    ADD COLUMN partial_output_object_id BINARY(16) NULL AFTER response_json,
    ADD COLUMN reconciliation_attempts INT UNSIGNED NOT NULL DEFAULT 0 AFTER partial_output_object_id;


ALTER TABLE checkpoints
    ADD COLUMN bundle_id BINARY(16) NOT NULL AFTER execution_id,
    ADD COLUMN work_package_id BINARY(16) NULL AFTER bundle_id,
    ADD COLUMN payload_hash CHAR(71) NOT NULL AFTER state_hash,
    ADD COLUMN state_version BIGINT UNSIGNED NOT NULL AFTER payload_hash,
    ADD KEY idx_runtime_checkpoint_bundle (tenant_id, bundle_id, created_at);

ALTER TABLE evaluation_runs
    ADD COLUMN work_package_id BINARY(16) NOT NULL AFTER id,
    ADD COLUMN bundle_id BINARY(16) NULL AFTER work_package_id,
    ADD COLUMN version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER parameters_json,
    ADD COLUMN cancellation_version BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER version,
    ADD COLUMN locked_by BINARY(16) NULL AFTER completed_at,
    ADD COLUMN locked_until TIMESTAMP(6) NULL AFTER locked_by,
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER locked_until,
    ADD COLUMN retention_deleted_at TIMESTAMP(6) NULL AFTER fencing_token,
    ADD UNIQUE KEY uq_runtime_evaluation_package (tenant_id, work_package_id),
    ADD KEY idx_runtime_evaluation_claim (status, locked_until, created_at);

ALTER TABLE application_messages
    ADD COLUMN retention_deleted_at TIMESTAMP(6) NULL AFTER created_at,
    ADD KEY idx_runtime_message_retention (tenant_id, retention_deleted_at, created_at);

ALTER TABLE quota_reservations
    ADD COLUMN idempotency_key VARCHAR(192) NOT NULL AFTER scope_id,
    ADD COLUMN settled_amount DECIMAL(24,6) NULL AFTER amount,
    ADD COLUMN release_reason VARCHAR(128) NULL AFTER status,
    ADD UNIQUE KEY uq_runtime_quota_idempotency (tenant_id, idempotency_key);

ALTER TABLE retention_runs
    ADD COLUMN policy_version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER dry_run,
    ADD COLUMN idempotency_key VARCHAR(192) NOT NULL AFTER policy_version,
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER locked_until,
    ADD UNIQUE KEY uq_runtime_retention_idempotency (tenant_id, idempotency_key);

ALTER TABLE retention_items
    MODIFY status ENUM('candidate','marked','deleting','blocked','deleted','failed') NOT NULL,
    ADD COLUMN object_id BINARY(16) NULL AFTER target_id,
    ADD COLUMN locked_by BINARY(16) NULL AFTER attempt_count,
    ADD COLUMN locked_until TIMESTAMP(6) NULL AFTER locked_by,
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER locked_until,
    ADD COLUMN deleted_at TIMESTAMP(6) NULL AFTER updated_at,
    ADD KEY idx_runtime_retention_item_claim (status, locked_until, created_at);

ALTER TABLE sandbox_leases
    ADD COLUMN provider_operation_id VARCHAR(255) NULL AFTER sandbox_id,
    ADD COLUMN provider_labels_json JSON NULL AFTER provider_operation_id,
    ADD COLUMN request_hash CHAR(71) NOT NULL AFTER provider_labels_json,
    ADD COLUMN result_json JSON NULL AFTER request_hash,
    ADD COLUMN locked_by BINARY(16) NULL AFTER status,
    ADD COLUMN locked_until TIMESTAMP(6) NULL AFTER locked_by,
    ADD COLUMN fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER locked_until,
    ADD COLUMN outcome_unknown BOOLEAN NOT NULL DEFAULT FALSE AFTER fencing_token,
    ADD KEY idx_runtime_sandbox_claim (status, locked_until, expires_at);

-- Runtime secrets are Vault references inside immutable bindings. These V2-01
-- compatibility tables are intentionally removed before any V2-04 data exists.
DROP TABLE credential_secret_versions;
DROP TABLE credentials;
