-- plan7 P7-A L1 channel auto-reply configuration (frozen into trigger
-- revisions on rebuild).
SET NAMES utf8mb4;
SET time_zone = '+00:00';

ALTER TABLE application_webhooks
    ADD COLUMN reply_config_json JSON NULL AFTER channel_config_json;
