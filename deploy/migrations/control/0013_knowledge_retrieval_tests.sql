-- Query retrieval test audit. External providers own documents and indexes.
SET NAMES utf8mb4;
SET time_zone = '+00:00';

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
