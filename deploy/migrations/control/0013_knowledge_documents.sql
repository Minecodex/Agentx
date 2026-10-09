-- plan7 P7-E: knowledge document management (upload -> external index ->
-- status -> hit-testing).
SET NAMES utf8mb4;
SET time_zone = '+00:00';

CREATE TABLE knowledge_documents (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    rag_resource_id BINARY(16) NOT NULL,
    name VARCHAR(255) NOT NULL,
    content_type VARCHAR(160) NOT NULL,
    size_bytes BIGINT UNSIGNED NOT NULL,
    sha256 CHAR(64) NOT NULL,
    artifact_id BINARY(16) NOT NULL,
    external_document_id VARCHAR(255) NULL,
    track_id VARCHAR(255) NULL,
    index_snapshot_json JSON NOT NULL,
    status ENUM('uploading', 'indexing', 'indexed', 'failed') NOT NULL DEFAULT 'uploading',
    error_code VARCHAR(64) NULL,
    error_message VARCHAR(1000) NULL,
    indexed_at TIMESTAMP(6) NULL,
    version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    next_attempt_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    locked_by BINARY(16) NULL,
    locked_until TIMESTAMP(6) NULL,
    fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0,
    created_by BINARY(16) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    UNIQUE KEY uq_knowledge_document_content (tenant_id, rag_resource_id, sha256),
    KEY idx_knowledge_document_resource (tenant_id, rag_resource_id, status, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE rag_retrieval_tests (
    id BINARY(16) NOT NULL,
    tenant_id BINARY(16) NOT NULL,
    rag_resource_id BINARY(16) NOT NULL,
    query VARCHAR(2000) NOT NULL,
    top_k INT UNSIGNED NOT NULL,
    hit_count INT UNSIGNED NOT NULL,
    error_code VARCHAR(64) NULL,
    duration_ms BIGINT UNSIGNED NOT NULL,
    created_by BINARY(16) NOT NULL,
    created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (id),
    KEY idx_rag_retrieval_tests (tenant_id, rag_resource_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
