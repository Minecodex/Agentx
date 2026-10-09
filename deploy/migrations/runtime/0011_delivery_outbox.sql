-- plan7 P7-A outbound delivery outbox: authoritative delivery records for
-- IM channel replies, claimed by the delivery loop role with MySQL lease +
-- fencing, mirroring execution_outbox semantics.
SET NAMES utf8mb4;
SET time_zone = '+00:00';

ALTER TABLE webhook_bindings
    ADD COLUMN reply_config_json JSON NULL AFTER fixed_inputs_json;

CREATE TABLE delivery_outbox (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    application_id BINARY(16) NOT NULL,
    invocation_id BINARY(16) NULL,
    execution_id BINARY(16) NOT NULL,
    channel_binding_id BINARY(16) NOT NULL,
    provider VARCHAR(32) NOT NULL,
    origin VARCHAR(400) NOT NULL,
    target_json JSON NOT NULL,
    credential_ref_json JSON NULL,
    payload_json JSON NOT NULL,
    status ENUM('pending','delivering','delivered','failed','dead') NOT NULL DEFAULT 'pending',
    attempt_count INT UNSIGNED NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    locked_by BINARY(16) NULL,
    locked_until TIMESTAMP(6) NULL,
    fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0,
    last_error_code VARCHAR(64) NULL,
    last_error_message VARCHAR(1000) NULL,
    provider_message_id VARCHAR(255) NULL,
    idempotency_key VARCHAR(191) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_delivery_idempotency (tenant_id, idempotency_key),
    KEY idx_delivery_claim (status, next_attempt_at, locked_until, created_at),
    KEY idx_delivery_invocation (tenant_id, invocation_id, created_at),
    KEY idx_delivery_execution (tenant_id, execution_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE delivery_dead_letters (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    application_id BINARY(16) NOT NULL,
    invocation_id BINARY(16) NULL,
    execution_id BINARY(16) NOT NULL,
    channel_binding_id BINARY(16) NOT NULL,
    provider VARCHAR(32) NOT NULL,
    origin VARCHAR(400) NOT NULL,
    target_json JSON NOT NULL,
    credential_ref_json JSON NULL,
    payload_json JSON NOT NULL,
    attempt_count INT UNSIGNED NOT NULL,
    last_error_code VARCHAR(64) NULL,
    last_error_message VARCHAR(1000) NULL,
    provider_message_id VARCHAR(255) NULL,
    idempotency_key VARCHAR(191) NOT NULL,
    dead_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    created_at TIMESTAMP(6) NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_delivery_dead_idempotency (tenant_id, idempotency_key),
    KEY idx_delivery_dead_invocation (tenant_id, invocation_id, dead_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
