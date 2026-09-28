-- plan7 P7-B: time-to-first-token for streaming model calls. Written when the
-- first delta frame is parsed and surfaced on the runtime_call Trace span.
SET NAMES utf8mb4;
SET time_zone = '+00:00';

ALTER TABLE runtime_calls
    ADD COLUMN first_token_ms INT UNSIGNED NULL AFTER usage_estimated;
