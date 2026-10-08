use crate::{config::DeploymentConfig, process};
use agentx_key_material::{generate_with_egress_tls_names, rsa_pair};
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::{Rng, distributions::Alphanumeric};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub(crate) type Secret = BTreeMap<String, String>;

fn password() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect()
}

fn egress_tls_names(config: &DeploymentConfig) -> Result<Vec<String>> {
    let endpoint = config
        .string("/global/network/egressGateway/sandboxAccess/endpoint")
        .context("Sandbox egress endpoint is required")?;
    let host = url::Url::parse(endpoint)
        .context("Sandbox egress endpoint is invalid")?
        .host_str()
        .context("Sandbox egress endpoint has no host")?
        .to_owned();
    Ok(vec!["agentx-egress-gateway".into(), host])
}

async fn get_secret(namespace: &str, name: &str) -> Result<Option<Secret>> {
    let result = process::run_command(
        [
            "kubectl", "-n", namespace, "get", "secret", name, "-o", "json",
        ],
        None,
        None,
        30,
        false,
        None,
    )
    .await?;
    if result.status != 0 {
        return Ok(None);
    }
    let payload: Value = serde_json::from_str(&result.stdout)?;
    let mut output = Secret::new();
    for (key, value) in payload
        .get("data")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let decoded = STANDARD.decode(
            value
                .as_str()
                .context("Secret data must be base64 strings")?,
        )?;
        output.insert(
            key.clone(),
            String::from_utf8(decoded).context("Secret data must be UTF-8")?,
        );
    }
    Ok(Some(output))
}

pub(crate) async fn apply_secret(namespace: &str, name: &str, data: &Secret) -> Result<()> {
    let payload = json!({
        "apiVersion": "v1", "kind": "Secret",
        "metadata": {"name": name, "namespace": namespace, "labels": {"app.kubernetes.io/managed-by": "agentxctl"}},
        "type": "Opaque", "stringData": data,
    });
    process::run_command(
        ["kubectl", "apply", "-f", "-"],
        None,
        Some(&serde_json::to_string(&payload)?),
        60,
        true,
        None,
    )
    .await?;
    Ok(())
}

pub async fn ensure_local_secrets(config: &DeploymentConfig, targets: &[&str]) -> Result<()> {
    if config.string("/global/secrets/mode") != Some("generated-local") {
        return ensure_existing_secrets(config).await;
    }
    let selected: BTreeSet<_> = targets.iter().copied().collect();
    let canonical_name = config.string("/global/secrets/dependencies").unwrap();
    let dependencies = config.namespace("dependencies");
    if let Some(mut existing) = get_secret(dependencies, canonical_name).await? {
        if crate::local_tls::ensure_material(config, &mut existing)? {
            apply_secret(dependencies, canonical_name, &existing).await?;
        }
        return publish_mirrors(config, &existing, &selected).await;
    }
    if !selected.contains("dependencies") {
        bail!("generated-local targets require an installed authoritative Dependencies Secret");
    }
    let material = generate_with_egress_tls_names(egress_tls_names(config)?)?;
    let mut egress_public = BTreeMap::new();
    egress_public.insert(
        "runtime-gateway-current",
        material.runtime_gateway_egress_public_key_pem.clone(),
    );
    egress_public.insert(
        "workflow-runtime-current",
        material.workflow_runtime_egress_public_key_pem.clone(),
    );
    egress_public.insert(
        "workflow-worker-current",
        material.workflow_worker_egress_public_key_pem.clone(),
    );
    egress_public.insert(
        "sandbox-manager-current",
        material.sandbox_egress_public_key_pem.clone(),
    );
    let mut canonical = Secret::new();
    for (key, value) in [
        ("MINIO_ROOT_USER", "agentx_admin".into()),
        ("MINIO_ROOT_PASSWORD", password()),
        ("CONTROL_OBJECT_PASSWORD", password()),
        ("RUNTIME_OBJECT_PASSWORD", password()),
        ("OBSERVABILITY_OBJECT_PASSWORD", password()),
        ("VAULT_DEV_ROOT_TOKEN_ID", password()),
        ("CONTROL_VAULT_TOKEN", password()),
        ("RUNTIME_VAULT_TOKEN", password()),
        ("OBSERVABILITY_REDIS_PASSWORD", password()),
        ("AGENTX_RUNTIME_REDIS_PASSWORD", password()),
        ("AGENTX_CONTROL_MYSQL_PASSWORD", password()),
        ("AGENTX_CONTROL_MYSQL_MIGRATE_PASSWORD", password()),
        ("AGENTX_CONTROL_MYSQL_ROOT_PASSWORD", password()),
        ("AGENTX_CONTROL_API_REFRESH_JWT_SIGNING_SECRET", password()),
        ("AGENTX_RUNTIME_MYSQL_PASSWORD", password()),
        ("AGENTX_RUNTIME_MYSQL_MIGRATE_PASSWORD", password()),
        ("AGENTX_RUNTIME_MYSQL_ROOT_PASSWORD", password()),
        ("AGENTX_CLICKHOUSE_QUERY_PASSWORD", password()),
        ("AGENTX_CLICKHOUSE_CONSUMER_PASSWORD", password()),
        ("AGENTX_CLICKHOUSE_MIGRATE_PASSWORD", password()),
        (
            "AGENTX_CONTROL_PUBLISHER_JWT_KID",
            "publisher-current".into(),
        ),
        (
            "AGENTX_CONTROL_PROJECTOR_JWT_KID",
            "projector-current".into(),
        ),
        ("AGENTX_CONTROL_BFF_JWT_KID", "bff-current".into()),
        ("AGENTX_CONTROL_USER_JWT_KID", "user-current".into()),
        ("AGENTX_CONTROL_BUNDLE_KEY_ID", "bundle-current".into()),
        (
            "AGENTX_CONTROL_WORK_PACKAGE_KEY_ID",
            "work-package-current".into(),
        ),
        (
            "AGENTX_CONTROL_PUBLISHER_JWT_PRIVATE_KEY_PEM",
            material.service_private_key_pem,
        ),
        (
            "AGENTX_CONTROL_PROJECTOR_JWT_PRIVATE_KEY_PEM",
            material.projector_private_key_pem,
        ),
        (
            "AGENTX_CONTROL_BFF_JWT_PRIVATE_KEY_PEM",
            material.bff_private_key_pem,
        ),
        (
            "AGENTX_CONTROL_USER_JWT_PRIVATE_KEY_PEM",
            material.user_private_key_pem,
        ),
        (
            "AGENTX_CONTROL_USER_JWT_PUBLIC_KEYS_JSON",
            serde_json::to_string(&json!({"user-current": material.user_public_key_pem}))?,
        ),
        (
            "AGENTX_RUNTIME_SERVICE_JWT_PUBLIC_KEYS_JSON",
            serde_json::to_string(&json!({
                "publisher-current": material.service_public_key_pem,
                "projector-current": material.projector_public_key_pem,
                "bff-current": material.bff_public_key_pem,
            }))?,
        ),
        (
            "AGENTX_OBSERVABILITY_BFF_JWT_PUBLIC_KEYS_JSON",
            serde_json::to_string(&json!({"bff-current": material.bff_public_key_pem}))?,
        ),
        (
            "AGENTX_RUNTIME_USER_JWT_PUBLIC_KEYS_JSON",
            serde_json::to_string(&json!({"user-current": material.user_public_key_pem}))?,
        ),
        (
            "AGENTX_CONTROL_BUNDLE_ED25519_PRIVATE_KEY_PEM",
            material.bundle_private_key_pem,
        ),
        (
            "AGENTX_RUNTIME_BUNDLE_PUBLIC_KEYS_JSON",
            serde_json::to_string(&json!({"bundle-current": material.bundle_public_key_base64}))?,
        ),
        (
            "AGENTX_CONTROL_WORK_PACKAGE_ED25519_PRIVATE_KEY_PEM",
            material.work_package_private_key_pem,
        ),
        (
            "AGENTX_RUNTIME_WORK_PACKAGE_PUBLIC_KEYS_JSON",
            serde_json::to_string(
                &json!({"work-package-current": material.work_package_public_key_base64}),
            )?,
        ),
        (
            "AGENTX_RUNTIME_GATEWAY_EGRESS_JWT_PRIVATE_KEY_PEM",
            material.runtime_gateway_egress_private_key_pem,
        ),
        (
            "AGENTX_RUNTIME_GATEWAY_EGRESS_JWT_KEY_ID",
            "runtime-gateway-current".into(),
        ),
        (
            "AGENTX_WORKFLOW_RUNTIME_EGRESS_JWT_PRIVATE_KEY_PEM",
            material.workflow_runtime_egress_private_key_pem,
        ),
        (
            "AGENTX_WORKFLOW_RUNTIME_EGRESS_JWT_KEY_ID",
            "workflow-runtime-current".into(),
        ),
        (
            "AGENTX_WORKFLOW_WORKER_EGRESS_JWT_PRIVATE_KEY_PEM",
            material.workflow_worker_egress_private_key_pem,
        ),
        (
            "AGENTX_WORKFLOW_WORKER_EGRESS_JWT_KEY_ID",
            "workflow-worker-current".into(),
        ),
        (
            "AGENTX_SANDBOX_EGRESS_JWT_PRIVATE_KEY_PEM",
            material.sandbox_egress_private_key_pem,
        ),
        (
            "AGENTX_SANDBOX_EGRESS_JWT_KEY_ID",
            "sandbox-manager-current".into(),
        ),
        (
            "AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON",
            serde_json::to_string(&egress_public)?,
        ),
        (
            "AGENTX_EGRESS_TLS_CERTIFICATE_PEM",
            material.egress_tls_certificate_pem,
        ),
        (
            "AGENTX_EGRESS_TLS_PRIVATE_KEY_PEM",
            material.egress_tls_private_key_pem,
        ),
    ] {
        canonical.insert(key.into(), value);
    }
    crate::local_tls::ensure_material(config, &mut canonical)?;
    apply_secret(dependencies, canonical_name, &canonical).await?;
    publish_mirrors(config, &canonical, &selected).await
}

async fn publish_mirrors(
    config: &DeploymentConfig,
    canonical: &Secret,
    targets: &BTreeSet<&str>,
) -> Result<()> {
    let secret_name = |plane: &str| config.string(&format!("/global/secrets/{plane}")).unwrap();
    if targets.contains("control") {
        let mut control = Secret::from([
            (
                "AGENTX_CONTROL_MYSQL_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_CONTROL_MYSQL_PASSWORD"),
            ),
            (
                "AGENTX_CONTROL_MYSQL_MIGRATE_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_CONTROL_MYSQL_MIGRATE_PASSWORD"),
            ),
            (
                "AGENTX_CONTROL_MYSQL_ROOT_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_CONTROL_MYSQL_ROOT_PASSWORD"),
            ),
            (
                "AGENTX_CONTROL_S3_ACCESS_KEY".into(),
                config
                    .string("/global/components/objectStorage/domains/control/user")
                    .unwrap()
                    .into(),
            ),
            (
                "AGENTX_CONTROL_S3_SECRET_KEY".into(),
                required(canonical, "CONTROL_OBJECT_PASSWORD")?,
            ),
            (
                "AGENTX_CONTROL_API_REFRESH_JWT_SIGNING_SECRET".into(),
                value_or_password(canonical, "AGENTX_CONTROL_API_REFRESH_JWT_SIGNING_SECRET"),
            ),
            (
                "AGENTX_CONTROL_VAULT_TOKEN".into(),
                required(canonical, "CONTROL_VAULT_TOKEN")?,
            ),
        ]);
        copy_prefixes(canonical, &mut control, &["AGENTX_CONTROL_"]);
        apply_secret(
            config.namespace("control"),
            secret_name("control"),
            &control,
        )
        .await?;
    }
    if targets.contains("runtime") {
        let redis = required(canonical, "AGENTX_RUNTIME_REDIS_PASSWORD")?;
        let observability = required(canonical, "OBSERVABILITY_REDIS_PASSWORD")?;
        let mut runtime = Secret::from([
            (
                "AGENTX_RUNTIME_MYSQL_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_RUNTIME_MYSQL_PASSWORD"),
            ),
            (
                "AGENTX_RUNTIME_MYSQL_MIGRATE_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_RUNTIME_MYSQL_MIGRATE_PASSWORD"),
            ),
            (
                "AGENTX_RUNTIME_MYSQL_ROOT_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_RUNTIME_MYSQL_ROOT_PASSWORD"),
            ),
            ("AGENTX_RUNTIME_REDIS_PASSWORD".into(), redis.clone()),
            (
                "AGENTX_RUNTIME_REDIS_ACL_FILE".into(),
                format!(
                    "user default on >{redis} ~* &agentx:v2:invocation:wakeup:* +@all\nuser observability on >{observability} ~agentx:v2:trace:v1 ~agentx:v2:observability:jti:* +ping +xgroup +xreadgroup +xpending +xautoclaim +xack +set +get +del +exists"
                ),
            ),
            (
                "AGENTX_RUNTIME_S3_ACCESS_KEY".into(),
                config
                    .string("/global/components/objectStorage/domains/runtime/user")
                    .unwrap()
                    .into(),
            ),
            (
                "AGENTX_RUNTIME_S3_SECRET_KEY".into(),
                required(canonical, "RUNTIME_OBJECT_PASSWORD")?,
            ),
            (
                "AGENTX_RUNTIME_VAULT_TOKEN".into(),
                required(canonical, "RUNTIME_VAULT_TOKEN")?,
            ),
            (
                "AGENTX_OPENSANDBOX_API_KEY".into(),
                "agentx-local-opensandbox-key".into(),
            ),
        ]);
        copy_prefixes(
            canonical,
            &mut runtime,
            &["AGENTX_RUNTIME_", "AGENTX_WORKFLOW_", "AGENTX_SANDBOX_"],
        );
        apply_secret(
            config.namespace("runtime"),
            secret_name("runtime"),
            &runtime,
        )
        .await?;
    }
    if targets.contains("observability") {
        let observability = Secret::from([
            (
                "AGENTX_CLICKHOUSE_QUERY_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_CLICKHOUSE_QUERY_PASSWORD"),
            ),
            (
                "AGENTX_CLICKHOUSE_CONSUMER_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_CLICKHOUSE_CONSUMER_PASSWORD"),
            ),
            (
                "AGENTX_CLICKHOUSE_MIGRATE_PASSWORD".into(),
                value_or_password(canonical, "AGENTX_CLICKHOUSE_MIGRATE_PASSWORD"),
            ),
            (
                "AGENTX_OBSERVABILITY_REDIS_PASSWORD".into(),
                required(canonical, "OBSERVABILITY_REDIS_PASSWORD")?,
            ),
            (
                "AGENTX_OBSERVABILITY_BFF_JWT_PUBLIC_KEYS_JSON".into(),
                required(canonical, "AGENTX_OBSERVABILITY_BFF_JWT_PUBLIC_KEYS_JSON")?,
            ),
            (
                "AGENTX_OBSERVABILITY_S3_ACCESS_KEY".into(),
                config
                    .string("/global/components/objectStorage/domains/observability/user")
                    .unwrap()
                    .into(),
            ),
            (
                "AGENTX_OBSERVABILITY_S3_SECRET_KEY".into(),
                required(canonical, "OBSERVABILITY_OBJECT_PASSWORD")?,
            ),
        ]);
        apply_secret(
            config.namespace("observability"),
            secret_name("observability"),
            &observability,
        )
        .await?;
    }
    if targets.contains("dependencies") {
        apply_secret(
            config.namespace("dependencies"),
            "agentx-egress-gateway-secrets",
            &Secret::from([(
                "AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON".into(),
                required(canonical, "AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON")?,
            )]),
        )
        .await?;
        let tls_name = config
            .string("/global/network/egressGateway/sandboxAccess/tlsSecretName")
            .unwrap();
        let cert = required(canonical, "AGENTX_EGRESS_TLS_CERTIFICATE_PEM")?;
        apply_secret(
            config.namespace("dependencies"),
            tls_name,
            &Secret::from([
                ("tls.crt".into(), cert.clone()),
                (
                    "tls.key".into(),
                    required(canonical, "AGENTX_EGRESS_TLS_PRIVATE_KEY_PEM")?,
                ),
                ("ca.crt".into(), cert),
            ]),
        )
        .await?;
    }
    if targets.contains("runtime") {
        let ca_name = config
            .string("/global/network/egressGateway/sandboxAccess/caSecretName")
            .unwrap();
        apply_secret(
            config.namespace("runtime"),
            ca_name,
            &Secret::from([(
                "ca.crt".into(),
                required(canonical, "AGENTX_EGRESS_TLS_CERTIFICATE_PEM")?,
            )]),
        )
        .await?;
    }
    crate::local_tls::publish(config, canonical, targets).await?;
    Ok(())
}

fn copy_prefixes(source: &Secret, target: &mut Secret, prefixes: &[&str]) {
    for (key, value) in source {
        if prefixes.iter().any(|prefix| key.starts_with(prefix)) {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn required(secret: &Secret, key: &str) -> Result<String> {
    secret
        .get(key)
        .cloned()
        .with_context(|| format!("canonical Secret is missing {key}"))
}

fn value_or_password(secret: &Secret, key: &str) -> String {
    secret.get(key).cloned().unwrap_or_else(password)
}

async fn ensure_existing_secrets(config: &DeploymentConfig) -> Result<()> {
    for (plane, namespace) in [
        ("control", "control"),
        ("runtime", "runtime"),
        ("observability", "runtime"),
        ("dependencies", "dependencies"),
    ] {
        let name = config.string(&format!("/global/secrets/{plane}")).unwrap();
        if get_secret(config.namespace(namespace), name)
            .await?
            .is_none()
        {
            bail!(
                "required existing secret is missing: {}/{name}",
                config.namespace(namespace)
            );
        }
    }
    Ok(())
}

pub async fn ensure_existing_secret_references(
    config: &DeploymentConfig,
    manifests: &[String],
) -> Result<Value> {
    if config.string("/global/secrets/mode") != Some("existing-kubernetes") {
        return Ok(json!({"status":"not-required","secrets":0}));
    }
    ensure_secret_references(config, manifests).await
}

pub async fn ensure_secret_references(
    config: &DeploymentConfig,
    manifests: &[String],
) -> Result<Value> {
    let mut required: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let canonical = config.string("/global/secrets/dependencies").unwrap();
    required
        .entry((config.namespace("dependencies").into(), canonical.into()))
        .or_default();
    for manifest in manifests {
        for document in serde_yaml::Deserializer::from_str(manifest) {
            let value = Value::deserialize(document)?;
            if !value.is_object() {
                continue;
            }
            let namespace = value
                .pointer("/metadata/namespace")
                .and_then(Value::as_str)
                .unwrap_or("");
            inspect_secret_references(&value, namespace, &mut required);
        }
    }
    let mut missing = Vec::new();
    for ((namespace, name), keys) in &required {
        match get_secret(namespace, name).await? {
            None => missing.push(format!("{namespace}/{name}")),
            Some(secret) => {
                for key in keys {
                    if !secret.contains_key(key) {
                        missing.push(format!("{namespace}/{name}:{key}"));
                    }
                }
            }
        }
    }
    if !missing.is_empty() {
        bail!(
            "required existing Secret data is missing: {}",
            missing.join(", ")
        );
    }
    Ok(json!({"status":"ready","secrets":required.len()}))
}

fn inspect_secret_references(
    value: &Value,
    namespace: &str,
    required: &mut BTreeMap<(String, String), BTreeSet<String>>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                inspect_secret_references(value, namespace, required);
            }
        }
        Value::Object(object) => {
            for field in ["secretKeyRef", "secretRef"] {
                if let Some(reference) = object.get(field).and_then(Value::as_object) {
                    if reference.get("optional").and_then(Value::as_bool) != Some(true) {
                        add_reference(
                            required,
                            namespace,
                            reference.get("name").and_then(Value::as_str),
                            reference.get("key").and_then(Value::as_str),
                        );
                    }
                }
            }
            if let Some(secret) = object.get("secret").and_then(Value::as_object) {
                if secret.get("optional").and_then(Value::as_bool) != Some(true) {
                    let name = secret.get("secretName").and_then(Value::as_str);
                    add_reference(required, namespace, name, None);
                    for item in secret
                        .get("items")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        add_reference(
                            required,
                            namespace,
                            name,
                            item.get("key").and_then(Value::as_str),
                        );
                    }
                }
            }
            for field in ["imagePullSecrets", "tls"] {
                for item in object
                    .get(field)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let key = if field == "tls" { "secretName" } else { "name" };
                    add_reference(
                        required,
                        namespace,
                        item.get(key).and_then(Value::as_str),
                        None,
                    );
                }
            }
            for value in object.values() {
                inspect_secret_references(value, namespace, required);
            }
        }
        _ => {}
    }
}

fn add_reference(
    required: &mut BTreeMap<(String, String), BTreeSet<String>>,
    namespace: &str,
    name: Option<&str>,
    key: Option<&str>,
) {
    if namespace.is_empty() || name.unwrap_or("").is_empty() {
        return;
    }
    let keys = required
        .entry((namespace.into(), name.unwrap().into()))
        .or_default();
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        keys.insert(key.into());
    }
}

pub async fn sync_existing_mirrors(config: &DeploymentConfig) -> Result<Vec<String>> {
    if config.string("/global/secrets/mode") != Some("existing-kubernetes") {
        ensure_local_secrets(
            config,
            &["dependencies", "control", "runtime", "observability"],
        )
        .await?;
        return Ok(["control", "runtime", "observability"]
            .iter()
            .map(|plane| {
                config
                    .string(&format!("/global/secrets/{plane}"))
                    .unwrap()
                    .into()
            })
            .collect());
    }
    let canonical_name = config.string("/global/secrets/dependencies").unwrap();
    let canonical = get_secret(config.namespace("dependencies"), canonical_name)
        .await?
        .context("the authoritative Dependencies Secret is missing or empty")?;
    let namespaces = BTreeMap::from([
        ("platformControl", "control"),
        ("controlMigration", "control"),
        ("controlBackup", "control"),
        ("runtimeGateway", "runtime"),
        ("workflowRuntime", "runtime"),
        ("workflowWorker", "runtime"),
        ("sandboxManager", "runtime"),
        ("runtimeMigration", "runtime"),
        ("runtimeBackup", "runtime"),
        ("observability", "runtime"),
        ("observabilityMigration", "runtime"),
        ("observabilityBackup", "runtime"),
        ("egressGateway", "dependencies"),
    ]);
    let mut updated = Vec::new();
    for (field, name) in config
        .object("/global/secrets/workloads")
        .into_iter()
        .flatten()
    {
        let Some(namespace_key) = namespaces.get(field.as_str()) else {
            continue;
        };
        let name = name.as_str().unwrap();
        let namespace = config.namespace(namespace_key);
        let mut mirror = get_secret(namespace, name)
            .await?
            .with_context(|| format!("workload Secret is missing: {namespace}/{name}"))?;
        let shared: Vec<_> = mirror
            .keys()
            .filter(|key| canonical.contains_key(*key))
            .cloned()
            .collect();
        if !shared.is_empty() {
            for key in shared {
                mirror.insert(key.clone(), canonical[&key].clone());
            }
            apply_secret(namespace, name, &mirror).await?;
            updated.push(format!("{namespace}/{name}"));
        }
    }
    Ok(updated)
}

pub async fn rotate_egress_keys(config: &DeploymentConfig, apply: bool) -> Result<Value> {
    let dependencies = config.namespace("dependencies");
    let runtime = config.namespace("runtime");
    let canonical_name = config.string("/global/secrets/dependencies").unwrap();
    let workload = config.object("/global/secrets/workloads");
    let gateway_name = workload
        .and_then(|map| map.get("egressGateway"))
        .and_then(Value::as_str)
        .unwrap_or("agentx-egress-gateway-secrets");
    let roles = [
        (
            "runtime-gateway",
            "runtimeGateway",
            "AGENTX_RUNTIME_GATEWAY_EGRESS_JWT_PRIVATE_KEY_PEM",
            "AGENTX_RUNTIME_GATEWAY_EGRESS_JWT_KEY_ID",
        ),
        (
            "workflow-runtime",
            "workflowRuntime",
            "AGENTX_WORKFLOW_RUNTIME_EGRESS_JWT_PRIVATE_KEY_PEM",
            "AGENTX_WORKFLOW_RUNTIME_EGRESS_JWT_KEY_ID",
        ),
        (
            "workflow-worker",
            "workflowWorker",
            "AGENTX_WORKFLOW_WORKER_EGRESS_JWT_PRIVATE_KEY_PEM",
            "AGENTX_WORKFLOW_WORKER_EGRESS_JWT_KEY_ID",
        ),
        (
            "sandbox-manager",
            "sandboxManager",
            "AGENTX_SANDBOX_EGRESS_JWT_PRIVATE_KEY_PEM",
            "AGENTX_SANDBOX_EGRESS_JWT_KEY_ID",
        ),
    ];
    let mut canonical = get_secret(dependencies, canonical_name)
        .await?
        .context("canonical Secret must exist before key rotation")?;
    let old_canonical = canonical.clone();
    let old_gateway = get_secret(dependencies, gateway_name)
        .await?
        .context("gateway Secret must exist before key rotation")?;
    let mut caller_names = BTreeMap::new();
    let mut old_callers = BTreeMap::new();
    for (deployment, field, _, _) in roles {
        let name = workload
            .and_then(|map| map.get(field))
            .and_then(Value::as_str)
            .unwrap_or_else(|| config.string("/global/secrets/runtime").unwrap());
        caller_names.insert(deployment, name.to_owned());
        if !old_callers.contains_key(name) {
            old_callers.insert(
                name.to_owned(),
                get_secret(runtime, name)
                    .await?
                    .with_context(|| format!("caller Secret is missing: {name}"))?,
            );
        }
    }
    let active: BTreeMap<String, String> =
        serde_json::from_str(required(&canonical, "AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON")?.as_str())?;
    let phases = json!([
        "publish-overlap",
        "restart-gateway",
        "roll-callers",
        "remove-previous",
        "commit-canonical"
    ]);
    if !apply {
        return Ok(
            json!({"status":"ready","activeKids":active.keys().collect::<Vec<_>>(),"phases":phases}),
        );
    }
    let lock_name = "agentx-egress-key-rotation-lock";
    let lock_owner = ["--from-literal=owner=", &Uuid::now_v7().to_string()].concat();
    let lock = process::run_command(
        [
            "kubectl",
            "-n",
            dependencies,
            "create",
            "configmap",
            lock_name,
            &lock_owner,
        ],
        None,
        None,
        30,
        false,
        None,
    )
    .await?;
    if lock.status != 0 {
        bail!("another Egress key rotation holds the cluster lock");
    }
    let result = rotate_locked(
        config,
        &roles,
        gateway_name,
        &caller_names,
        &mut canonical,
        active,
    )
    .await;
    if result.is_err() {
        let _ = apply_secret(dependencies, canonical_name, &old_canonical).await;
        let _ = apply_secret(dependencies, gateway_name, &old_gateway).await;
        for (name, secret) in &old_callers {
            let _ = apply_secret(runtime, name, secret).await;
        }
        let _ = restart_and_wait(dependencies, "agentx-egress-gateway", false).await;
        for (deployment, _, _, _) in roles {
            let _ = restart_and_wait(runtime, deployment, false).await;
        }
    }
    let _ = process::run_command(
        [
            "kubectl",
            "-n",
            dependencies,
            "delete",
            "configmap",
            lock_name,
            "--ignore-not-found",
        ],
        None,
        None,
        30,
        false,
        None,
    )
    .await;
    result
}

async fn rotate_locked(
    config: &DeploymentConfig,
    roles: &[(&str, &str, &str, &str)],
    gateway_name: &str,
    caller_names: &BTreeMap<&str, String>,
    canonical: &mut Secret,
    mut overlap: BTreeMap<String, String>,
) -> Result<Value> {
    let dependencies = config.namespace("dependencies");
    let runtime = config.namespace("runtime");
    let rotation = Uuid::now_v7().simple().to_string()[..10].to_owned();
    let mut generated = BTreeMap::new();
    let mut new_public = BTreeMap::new();
    for (deployment, _, private_key, kid_key) in roles {
        let (private, public) = rsa_pair()?;
        let kid = format!("{deployment}-{rotation}");
        canonical.insert((*private_key).into(), private.clone());
        canonical.insert((*kid_key).into(), kid.clone());
        overlap.insert(kid.clone(), public.clone());
        new_public.insert(kid.clone(), public);
        generated.insert(*deployment, (private, kid));
    }
    apply_secret(
        dependencies,
        gateway_name,
        &Secret::from([(
            "AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON".into(),
            serde_json::to_string(&overlap)?,
        )]),
    )
    .await?;
    restart_and_wait(dependencies, "agentx-egress-gateway", true).await?;
    for (deployment, _, private_key, kid_key) in roles {
        let name = &caller_names[deployment];
        let mut caller = get_secret(runtime, name)
            .await?
            .with_context(|| format!("caller Secret disappeared during rotation: {name}"))?;
        caller.insert((*private_key).into(), generated[deployment].0.clone());
        caller.insert((*kid_key).into(), generated[deployment].1.clone());
        apply_secret(runtime, name, &caller).await?;
        restart_and_wait(runtime, deployment, true).await?;
    }
    canonical.insert(
        "AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON".into(),
        serde_json::to_string(&new_public)?,
    );
    apply_secret(
        dependencies,
        config.string("/global/secrets/dependencies").unwrap(),
        canonical,
    )
    .await?;
    apply_secret(
        dependencies,
        gateway_name,
        &Secret::from([(
            "AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON".into(),
            serde_json::to_string(&new_public)?,
        )]),
    )
    .await?;
    restart_and_wait(dependencies, "agentx-egress-gateway", true).await?;
    Ok(
        json!({"status":"rotated","rotationId":rotation,"activeKids":new_public.keys().collect::<Vec<_>>() }),
    )
}

async fn restart_and_wait(namespace: &str, deployment: &str, check: bool) -> Result<()> {
    let restarted = process::run_command(
        [
            "kubectl",
            "-n",
            namespace,
            "rollout",
            "restart",
            &format!("deployment/{deployment}"),
        ],
        None,
        None,
        60,
        check,
        None,
    )
    .await?;
    if restarted.status == 0 {
        process::run_command(
            [
                "kubectl",
                "-n",
                namespace,
                "rollout",
                "status",
                &format!("deployment/{deployment}"),
                "--timeout=300s",
            ],
            None,
            None,
            330,
            check,
            None,
        )
        .await?;
    }
    Ok(())
}

use serde::Deserialize;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{CommandExecutor, CommandRequest, CommandResult};
    use agentx_key_material::rsa_public_key_pem;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct SecretCluster {
        secrets: Mutex<BTreeMap<(String, String), Secret>>,
        requests: Mutex<Vec<CommandRequest>>,
        failure: Mutex<Option<(String, usize)>>,
        failure_matches: Mutex<usize>,
    }

    impl SecretCluster {
        fn with_secrets(secrets: BTreeMap<(String, String), Secret>) -> Self {
            Self {
                secrets: Mutex::new(secrets),
                ..Self::default()
            }
        }

        fn fail_on(&self, value: &str, occurrence: usize) {
            *self.failure.lock().unwrap() = Some((value.into(), occurrence));
            *self.failure_matches.lock().unwrap() = 0;
        }

        fn snapshot(&self) -> BTreeMap<(String, String), Secret> {
            self.secrets.lock().unwrap().clone()
        }

        fn requests(&self) -> Vec<CommandRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CommandExecutor for SecretCluster {
        async fn execute(&self, request: CommandRequest) -> Result<CommandResult> {
            self.requests.lock().unwrap().push(request.clone());
            let rendered = format!(
                "{} {}",
                request.command.join(" "),
                request.input.as_deref().unwrap_or("")
            );
            let should_fail = if let Some((needle, occurrence)) = &*self.failure.lock().unwrap() {
                if rendered.contains(needle) {
                    let mut matches = self.failure_matches.lock().unwrap();
                    *matches += 1;
                    *matches == *occurrence
                } else {
                    false
                }
            } else {
                false
            };
            if should_fail {
                return Ok(CommandResult {
                    command: request.command,
                    stdout: String::new(),
                    stderr: "injected failure".into(),
                    status: 1,
                });
            }
            let mut status = 0;
            let mut stdout = String::new();
            if request.command.len() >= 7
                && request.command[0] == "kubectl"
                && request.command[1] == "-n"
                && request.command[3..5] == ["get", "secret"]
            {
                let key = (request.command[2].clone(), request.command[5].clone());
                if let Some(secret) = self.secrets.lock().unwrap().get(&key) {
                    let data = secret
                        .iter()
                        .map(|(key, value)| (key.clone(), STANDARD.encode(value)))
                        .collect::<BTreeMap<_, _>>();
                    stdout = json!({"data":data}).to_string();
                } else {
                    status = 1;
                }
            } else if request.command == ["kubectl", "apply", "-f", "-"] {
                let payload: Value = serde_json::from_str(request.input.as_deref().unwrap())?;
                let namespace = payload["metadata"]["namespace"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let name = payload["metadata"]["name"].as_str().unwrap().to_owned();
                let secret = payload["stringData"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
                    .collect();
                self.secrets
                    .lock()
                    .unwrap()
                    .insert((namespace, name), secret);
            }
            Ok(CommandResult {
                command: request.command,
                stdout,
                stderr: String::new(),
                status,
            })
        }
    }

    fn local_config() -> DeploymentConfig {
        DeploymentConfig::load(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values/local.yaml"),
            None,
        )
        .unwrap()
    }

    #[test]
    fn generated_passwords_satisfy_bundled_database_initializers() {
        for _ in 0..64 {
            let value = password();
            assert_eq!(value.len(), 48);
            assert!(value.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn generated_egress_tls_names_include_the_configured_sandbox_endpoint() {
        assert_eq!(
            egress_tls_names(&local_config()).unwrap(),
            vec![
                "agentx-egress-gateway".to_owned(),
                "host.docker.internal".to_owned(),
            ]
        );
    }

    #[test]
    fn secret_reference_scanner_keeps_image_pull_secrets_keyless() {
        let manifest = json!({
            "metadata":{"namespace":"agentx-runtime"},
            "spec":{
                "imagePullSecrets":[{"name":"registry-auth"}],
                "containers":[{"env":[{"valueFrom":{"secretKeyRef":{"name":"runtime","key":"TOKEN"}}}]}],
                "optional":{"secretRef":{"name":"ignored","optional":true}}
            }
        });
        let mut required = BTreeMap::new();
        inspect_secret_references(&manifest, "agentx-runtime", &mut required);
        assert_eq!(
            required[&("agentx-runtime".into(), "registry-auth".into())],
            BTreeSet::new()
        );
        assert_eq!(
            required[&("agentx-runtime".into(), "runtime".into())],
            BTreeSet::from(["TOKEN".into()])
        );
        assert!(!required.keys().any(|(_, name)| name == "ignored"));
    }

    #[tokio::test]
    async fn generated_secrets_are_created_once_and_reused() {
        let config = local_config();
        let cluster = Arc::new(SecretCluster::default());
        process::with_command_executor(
            cluster.clone(),
            ensure_local_secrets(
                &config,
                &["dependencies", "control", "runtime", "observability"],
            ),
        )
        .await
        .unwrap();
        let canonical_key = (
            config.namespace("dependencies").to_owned(),
            config
                .string("/global/secrets/dependencies")
                .unwrap()
                .to_owned(),
        );
        let first = cluster.snapshot()[&canonical_key].clone();
        assert!(first.contains_key("AGENTX_CONTROL_PUBLISHER_JWT_PRIVATE_KEY_PEM"));
        assert!(first.contains_key("AGENTX_EGRESS_TLS_PRIVATE_KEY_PEM"));

        let runtime_trust: BTreeMap<String, String> =
            serde_json::from_str(first["AGENTX_RUNTIME_SERVICE_JWT_PUBLIC_KEYS_JSON"].as_str())
                .unwrap();
        for (kid_key, private_key, expected_kid) in [
            (
                "AGENTX_CONTROL_PUBLISHER_JWT_KID",
                "AGENTX_CONTROL_PUBLISHER_JWT_PRIVATE_KEY_PEM",
                "publisher-current",
            ),
            (
                "AGENTX_CONTROL_PROJECTOR_JWT_KID",
                "AGENTX_CONTROL_PROJECTOR_JWT_PRIVATE_KEY_PEM",
                "projector-current",
            ),
            (
                "AGENTX_CONTROL_BFF_JWT_KID",
                "AGENTX_CONTROL_BFF_JWT_PRIVATE_KEY_PEM",
                "bff-current",
            ),
        ] {
            assert_eq!(first[kid_key], expected_kid);
            assert_eq!(
                runtime_trust.get(&first[kid_key]).unwrap(),
                &rsa_public_key_pem(&first[private_key]).unwrap()
            );
        }

        process::with_command_executor(
            cluster.clone(),
            ensure_local_secrets(
                &config,
                &["dependencies", "control", "runtime", "observability"],
            ),
        )
        .await
        .unwrap();
        assert_eq!(cluster.snapshot()[&canonical_key], first);
        let canonical_applies = cluster
            .requests()
            .iter()
            .filter(|request| {
                request.input.as_deref().is_some_and(|input| {
                    input.contains("\"name\":\"agentx-dependencies-secrets\"")
                        && input.contains("AGENTX_CONTROL_PUBLISHER_JWT_PRIVATE_KEY_PEM")
                })
            })
            .count();
        assert_eq!(canonical_applies, 1);
    }

    #[tokio::test]
    async fn existing_secret_validation_reports_the_exact_missing_key() {
        let mut config = local_config();
        *config.values.pointer_mut("/global/secrets/mode").unwrap() = "existing-kubernetes".into();
        let canonical = (
            config.namespace("dependencies").to_owned(),
            config
                .string("/global/secrets/dependencies")
                .unwrap()
                .to_owned(),
        );
        let cluster = Arc::new(SecretCluster::with_secrets(BTreeMap::from([
            (canonical, Secret::new()),
            (
                ("agentx-runtime".into(), "runtime-secret".into()),
                Secret::new(),
            ),
        ])));
        let manifest = "apiVersion: v1\nkind: Pod\nmetadata:\n  namespace: agentx-runtime\nspec:\n  containers:\n  - env:\n    - valueFrom:\n        secretKeyRef:\n          name: runtime-secret\n          key: REQUIRED_TOKEN\n".to_owned();
        let error = process::with_command_executor(
            cluster,
            ensure_existing_secret_references(&config, &[manifest]),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("agentx-runtime/runtime-secret:REQUIRED_TOKEN"));
    }

    #[tokio::test]
    async fn rotation_failure_restores_every_secret_and_releases_the_lock() {
        let config = local_config();
        let dependencies = config.namespace("dependencies").to_owned();
        let runtime = config.namespace("runtime").to_owned();
        let canonical_name = config
            .string("/global/secrets/dependencies")
            .unwrap()
            .to_owned();
        let runtime_name = config.string("/global/secrets/runtime").unwrap().to_owned();
        let gateway_name = "agentx-egress-gateway-secrets".to_owned();
        let old_public = serde_json::to_string(&BTreeMap::from([(
            "old".to_owned(),
            "old-public".to_owned(),
        )]))
        .unwrap();
        let initial = BTreeMap::from([
            (
                (dependencies.clone(), canonical_name),
                Secret::from([(
                    "AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON".into(),
                    old_public.clone(),
                )]),
            ),
            (
                (dependencies.clone(), gateway_name),
                Secret::from([("AGENTX_EGRESS_JWT_PUBLIC_KEYS_JSON".into(), old_public)]),
            ),
            (
                (runtime, runtime_name),
                Secret::from([("UNCHANGED".into(), "value".into())]),
            ),
        ]);
        for (needle, occurrence) in [
            ("\"name\":\"agentx-egress-gateway-secrets\"", 1),
            ("rollout status deployment/agentx-egress-gateway", 1),
            ("rollout status deployment/workflow-runtime", 1),
            ("\"name\":\"agentx-dependencies-secrets\"", 1),
            ("\"name\":\"agentx-egress-gateway-secrets\"", 2),
        ] {
            let cluster = Arc::new(SecretCluster::with_secrets(initial.clone()));
            cluster.fail_on(needle, occurrence);
            let error =
                process::with_command_executor(cluster.clone(), rotate_egress_keys(&config, true))
                    .await
                    .unwrap_err()
                    .to_string();
            assert!(error.contains("injected failure"), "{needle}: {error}");
            assert_eq!(cluster.snapshot(), initial, "{needle}");
            assert!(cluster.requests().iter().any(|request| {
                request.command.iter().any(|argument| argument == "delete")
                    && request
                        .command
                        .iter()
                        .any(|argument| argument == "agentx-egress-key-rotation-lock")
            }));
        }
    }
}
