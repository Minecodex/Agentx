-- plan7 P7-B: model deployment input modality capabilities (vision/audio).
SET NAMES utf8mb4;
SET time_zone = '+00:00';

ALTER TABLE model_deployments
    ADD COLUMN capabilities_json JSON NULL AFTER default_parameters;
