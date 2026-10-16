-- V2-08A stores the immutable Workflow Definition and the mutable Studio
-- editor document independently. Baseline DDL is rebuilt during development.

ALTER TABLE workflow_drafts
    ADD COLUMN editor_json LONGTEXT NOT NULL CHECK (JSON_VALID(editor_json)) AFTER definition_json,
    ADD COLUMN editor_hash VARCHAR(80) NOT NULL AFTER content_hash;

ALTER TABLE workflow_draft_revisions
    ADD COLUMN editor_json LONGTEXT NOT NULL CHECK (JSON_VALID(editor_json)) AFTER definition_json,
    ADD COLUMN editor_hash VARCHAR(80) NOT NULL AFTER content_hash;

ALTER TABLE workflow_versions
    ADD COLUMN editor_json LONGTEXT NOT NULL CHECK (JSON_VALID(editor_json)) AFTER definition_json,
    ADD COLUMN editor_hash VARCHAR(80) NOT NULL AFTER content_hash;

-- V2 never stores credential plaintext or local ciphertext in Control MySQL.
-- The row records only an immutable Vault KV v2 reference.
ALTER TABLE credential_secret_versions
    DROP COLUMN algorithm,
    DROP COLUMN key_id,
    DROP COLUMN nonce,
    DROP COLUMN ciphertext,
    ADD COLUMN provider VARCHAR(32) NOT NULL DEFAULT 'vault_kv_v2' AFTER version_number,
    ADD COLUMN secret_ref VARCHAR(512) NOT NULL AFTER provider,
    ADD COLUMN provider_version VARCHAR(64) NOT NULL AFTER secret_ref;

ALTER TABLE skills
    ADD COLUMN alias VARCHAR(160) NOT NULL AFTER name,
    ADD UNIQUE KEY uq_skill_alias (tenant_id, alias);
