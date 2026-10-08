use crate::assets::{DOCKERHUB_BETA_VALUES, VALUES_SCHEMA};
use anyhow::{Context, Result, anyhow, bail};
use ipnet::IpNet;
use regex::Regex;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, net::IpAddr, path::PathBuf, str::FromStr};

pub const TARGETS: [&str; 4] = ["dependencies", "control", "runtime", "observability"];

#[derive(Debug, Clone)]
pub struct DeploymentConfig {
    pub path: PathBuf,
    pub values: Value,
}

impl DeploymentConfig {
    pub fn load(path: impl Into<PathBuf>, run_id: Option<&str>) -> Result<Self> {
        let path = path.into();
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        };
        let path = path
            .canonicalize()
            .with_context(|| format!("values file does not exist: {}", path.display()))?;
        let yaml = std::fs::read_to_string(&path)?;
        Self::from_yaml(path, &yaml, run_id)
    }

    pub fn load_embedded_beta(run_id: Option<&str>) -> Result<Self> {
        Self::from_yaml(
            PathBuf::from("embedded:dockerhub-beta.yaml"),
            DOCKERHUB_BETA_VALUES,
            run_id,
        )
    }

    fn from_yaml(path: PathBuf, yaml: &str, run_id: Option<&str>) -> Result<Self> {
        let values: Value =
            serde_yaml::from_str(yaml).context("deployment values must be valid YAML")?;
        if !values.is_object() {
            bail!("deployment values must be a YAML object");
        }
        let schema: Value = serde_json::from_str(VALUES_SCHEMA)?;
        let validator =
            jsonschema::validator_for(&schema).context("embedded Values Schema is invalid")?;
        validator
            .validate(&values)
            .map_err(|error| anyhow!("values schema validation failed: {error}"))?;
        let config = Self { path, values }.scoped(run_id)?;
        config.validate_semantics()?;
        Ok(config)
    }

    pub fn environment(&self) -> &str {
        self.string("/global/environment")
            .expect("Values Schema guarantees global.environment")
    }

    pub fn string(&self, pointer: &str) -> Option<&str> {
        self.values.pointer(pointer).and_then(Value::as_str)
    }

    pub fn bool(&self, pointer: &str) -> Option<bool> {
        self.values.pointer(pointer).and_then(Value::as_bool)
    }

    pub fn u64(&self, pointer: &str) -> Option<u64> {
        self.values.pointer(pointer).and_then(Value::as_u64)
    }

    pub fn object(&self, pointer: &str) -> Option<&Map<String, Value>> {
        self.values.pointer(pointer).and_then(Value::as_object)
    }

    pub fn array(&self, pointer: &str) -> Option<&Vec<Value>> {
        self.values.pointer(pointer).and_then(Value::as_array)
    }

    pub fn namespaces(&self) -> BTreeMap<String, String> {
        self.object("/global/namespaces")
            .expect("Values Schema guarantees namespaces")
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
            .collect()
    }

    pub fn namespace(&self, target: &str) -> &str {
        let key = if target == "observability" {
            "runtime"
        } else {
            target
        };
        self.string(&format!("/global/namespaces/{key}"))
            .expect("Values Schema guarantees target namespace")
    }

    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(&self.values).context("serialize deployment values")
    }

    pub fn ingress_service_type(&self) -> &str {
        if self.environment() == "production" {
            return "LoadBalancer";
        }
        if self.environment() == "test" || self.namespace("dependencies").starts_with("agentx-e2e-")
        {
            "ClusterIP"
        } else {
            self.string("/global/ingress/serviceType")
                .unwrap_or("LoadBalancer")
        }
    }

    pub fn ingress_url(&self, plane: &str) -> String {
        let tls = !self
            .string(&format!("/global/ingress/{plane}TlsSecretName"))
            .unwrap_or("")
            .is_empty();
        let host = self
            .string(&format!("/global/ingress/{plane}Host"))
            .unwrap();
        let scheme = if tls { "https" } else { "http" };
        let port = if self.ingress_service_type() == "NodePort" {
            format!(
                ":{}",
                self.u64(if tls {
                    "/global/ingress/httpsNodePort"
                } else {
                    "/global/ingress/httpNodePort"
                })
                .unwrap()
            )
        } else {
            String::new()
        };
        format!("{scheme}://{host}{port}")
    }

    fn scoped(mut self, run_id: Option<&str>) -> Result<Self> {
        let Some(run_id) = run_id else {
            return Ok(self);
        };
        if self.environment() == "production" {
            bail!("--run-id is forbidden for production values");
        }
        let invalid = Regex::new("[^a-z0-9-]").unwrap();
        let normalized = invalid
            .replace_all(&run_id.to_ascii_lowercase(), "-")
            .trim_matches('-')
            .to_owned();
        if normalized.is_empty() || normalized.len() > 36 {
            bail!("run id must form a non-empty DNS-safe suffix up to 36 characters");
        }
        let original_namespaces = self.namespaces();
        let scoped_namespaces = BTreeMap::from([
            (
                "control".to_owned(),
                format!("agentx-e2e-control-{normalized}"),
            ),
            (
                "runtime".to_owned(),
                format!("agentx-e2e-runtime-{normalized}"),
            ),
            (
                "dependencies".to_owned(),
                format!("agentx-e2e-deps-{normalized}"),
            ),
        ]);
        {
            let namespaces = self
                .values
                .pointer_mut("/global/namespaces")
                .and_then(Value::as_object_mut)
                .unwrap();
            for (key, namespace) in &scoped_namespaces {
                namespaces.insert(key.clone(), namespace.clone().into());
            }
        }
        for (pointer, plane) in [
            ("/global/components/controlMysql/host", "control"),
            ("/global/components/runtimeMysql/host", "runtime"),
            ("/global/components/runtimeRedis/url", "runtime"),
            ("/global/components/secretProvider/endpoint", "dependencies"),
            ("/global/components/sandbox/endpoint", "dependencies"),
            ("/global/components/clickhouse/url", "runtime"),
            ("/global/components/objectStorage/endpoint", "dependencies"),
        ] {
            let original = &original_namespaces[plane];
            let scoped = &scoped_namespaces[plane];
            let value = self
                .values
                .pointer(pointer)
                .unwrap()
                .as_str()
                .unwrap()
                .replace(&format!(".{original}.svc"), &format!(".{scoped}.svc"))
                .into();
            *self.values.pointer_mut(pointer).unwrap() = value;
        }
        *self
            .values
            .pointer_mut("/global/ingress/className")
            .unwrap() = format!("agentx-e2e-{normalized}").into();
        if matches!(
            self.string("/global/network/egressGateway/sandboxAccess/mode"),
            Some("nodePort" | "privateLoadBalancer")
        ) {
            let port = deterministic_e2e_node_port(&normalized);
            *self
                .values
                .pointer_mut("/global/network/egressGateway/sandboxAccess/mode")
                .unwrap() = "nodePort".into();
            *self
                .values
                .pointer_mut("/global/network/egressGateway/sandboxAccess/endpoint")
                .unwrap() = format!("https://host.docker.internal:{port}").into();
            *self
                .values
                .pointer_mut("/global/network/egressGateway/sandboxAccess/port")
                .unwrap() = u64::from(port).into();
        }
        Ok(self)
    }

    fn validate_semantics(&self) -> Result<()> {
        let namespaces = self.namespaces();
        let unique: std::collections::BTreeSet<_> = namespaces.values().collect();
        if unique.len() != 3 {
            bail!("control, runtime, and dependencies namespaces must be distinct");
        }
        let dns = Regex::new(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$").unwrap();
        for namespace in namespaces.values() {
            if namespace.len() > 63 || !dns.is_match(namespace) {
                bail!("invalid Kubernetes namespace: {namespace}");
            }
        }
        if self.environment() == "production" {
            self.validate_production()?;
        }
        self.validate_tls()?;
        if self.string("/global/ingress/serviceType") == Some("NodePort") {
            let http = self.u64("/global/ingress/httpNodePort");
            let https = self.u64("/global/ingress/httpsNodePort");
            if http.is_none() || https.is_none() || http == https {
                bail!("NodePort ingress requires distinct httpNodePort and httpsNodePort");
            }
        }
        let ports = self
            .array("/global/network/egressGateway/allowedPublicPorts")
            .unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for port in ports.iter().filter_map(Value::as_u64) {
            if !seen.insert(port) {
                bail!("egress allowedPublicPorts must be unique and include 443");
            }
        }
        if !seen.contains(&443) {
            bail!("egress allowedPublicPorts must be unique and include 443");
        }
        let sandbox_endpoint = self
            .string("/global/network/egressGateway/sandboxAccess/endpoint")
            .unwrap();
        let sandbox_endpoint = url::Url::parse(sandbox_endpoint)
            .context("sandbox Egress endpoint must be a valid HTTPS URL")?;
        let sandbox_port = self
            .u64("/global/network/egressGateway/sandboxAccess/port")
            .unwrap();
        if self.string("/global/network/egressGateway/sandboxAccess/mode") == Some("nodePort")
            && !(30_000..=32_767).contains(&sandbox_port)
        {
            bail!("sandbox NodePort must be in the range 30000..32767");
        }
        if sandbox_endpoint.port_or_known_default() != Some(sandbox_port as u16) {
            bail!("sandbox Egress endpoint port must match sandboxAccess.port");
        }
        for path in [
            "/global/network/egressGateway/allowedPrivateCidrs",
            "/global/network/egressGateway/blockedCidrs",
        ] {
            for cidr in self.array(path).into_iter().flatten() {
                let network = IpNet::from_str(cidr.as_str().unwrap())?;
                if network.prefix_len() == 0 {
                    bail!("egress gateway CIDRs cannot contain an unrestricted network");
                }
            }
        }
        let protected = self
            .array("/global/network/egressGateway/blockedCidrs")
            .into_iter()
            .flatten()
            .map(|cidr| {
                IpNet::from_str(cidr.as_str().unwrap())
                    .map(|network| network.to_string())
                    .map_err(Into::into)
            })
            .collect::<Result<std::collections::BTreeSet<_>>>()?;
        for required in ["10.96.0.0/12", "10.244.0.0/16"] {
            if !protected.contains(required) {
                bail!("egress blockedCidrs must permanently include Kubernetes network {required}");
            }
        }
        if let Some(targets) = self.object("/global/network/externalEgress") {
            for target in targets.values() {
                for cidr in target
                    .get("cidrs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let network = IpNet::from_str(cidr.as_str().unwrap())?;
                    if network.prefix_len() == 0 {
                        bail!("external egress cannot allow an unrestricted CIDR");
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_tls(&self) -> Result<()> {
        for (name, field, scheme) in [
            ("runtimeRedis", "url", "rediss"),
            ("objectStorage", "endpoint", "https"),
            ("secretProvider", "endpoint", "https"),
            ("sandbox", "endpoint", "https"),
            ("clickhouse", "url", "https"),
        ] {
            let base = format!("/global/components/{name}");
            let endpoint = url::Url::parse(self.string(&format!("{base}/{field}")).unwrap())?;
            if self.string(&format!("{base}/caSecretName")).is_some() && endpoint.scheme() != scheme
            {
                bail!("{name} caSecretName requires a {scheme}:// endpoint");
            }
            let bundled = matches!(
                self.string(&format!("{base}/mode")),
                Some("bundled" | "bundled-minio")
            ) || name == "secretProvider" && self.environment() != "production";
            if bundled
                && endpoint.scheme() == scheme
                && self.string(&format!("{base}/caSecretName")).is_none()
            {
                bail!("bundled {name} TLS requires caSecretName");
            }
        }
        for name in ["controlMysql", "runtimeMysql"] {
            let base = format!("/global/components/{name}");
            if self.string(&format!("{base}/mode")) == Some("bundled") {
                let tls = self.string(&format!("{base}/caSecretName")).is_some();
                let mode = self.string(&format!("{base}/tlsMode"));
                if tls && mode != Some("verify_identity") || !tls && mode != Some("disabled") {
                    bail!("bundled {name} TLS requires both verify_identity and caSecretName");
                }
            }
        }
        if let Some(upstream) = self.string("/global/components/sandbox/localProxyUpstream") {
            let endpoint =
                url::Url::parse(self.string("/global/components/sandbox/endpoint").unwrap())?;
            let expected = format!("opensandbox.{}.svc", self.namespace("dependencies"));
            if self.environment() == "production"
                || endpoint.scheme() != "https"
                || endpoint.host_str() != Some(&expected)
                || endpoint.port_or_known_default() != Some(8443)
                || self
                    .string("/global/components/sandbox/caSecretName")
                    .is_none()
                || self.bool("/global/components/sandbox/secureAccess") != Some(true)
                || !upstream.starts_with("http://")
            {
                bail!(
                    "local OpenSandbox TLS proxy requires local/test, its namespace Service on HTTPS 8443, secureAccess, CA and an HTTP upstream"
                );
            }
        }
        Ok(())
    }

    fn validate_production(&self) -> Result<()> {
        if self
            .string("/global/ingress/serviceType")
            .is_some_and(|value| value != "LoadBalancer")
        {
            bail!("production ingress requires LoadBalancer");
        }
        if self.string("/global/secrets/mode") != Some("existing-kubernetes") {
            bail!("production requires existing-kubernetes secrets");
        }
        for key in ["controlTlsSecretName", "runtimeTlsSecretName"] {
            if self
                .string(&format!("/global/ingress/{key}"))
                .unwrap_or("")
                .is_empty()
            {
                bail!("production requires TLS secrets for both ingress hosts");
            }
        }
        let required_workloads = [
            "platformControl",
            "controlMigration",
            "runtimeGateway",
            "workflowRuntime",
            "workflowWorker",
            "sandboxManager",
            "egressGateway",
            "runtimeMigration",
            "observability",
            "observabilityMigration",
            "controlBackup",
            "runtimeBackup",
            "observabilityBackup",
        ];
        let workloads = self
            .object("/global/secrets/workloads")
            .cloned()
            .unwrap_or_default();
        let missing: Vec<_> = required_workloads
            .iter()
            .filter(|key| !workloads.contains_key(**key))
            .copied()
            .collect();
        if !missing.is_empty() {
            bail!(
                "production workload Secret mappings are missing: {}",
                missing.join(", ")
            );
        }
        if self.values.pointer("/global/backup").is_none() {
            bail!("production requires backup RPO/RTO settings");
        }
        for name in ["controlMysql", "runtimeMysql", "runtimeRedis", "clickhouse"] {
            if self.string(&format!("/global/components/{name}/mode")) != Some("external") {
                bail!("production requires external MySQL, Redis, and ClickHouse");
            }
        }
        if self.string("/global/components/objectStorage/mode") != Some("external-s3") {
            bail!("production requires external S3");
        }
        if self.bool("/global/components/objectStorage/allowHttp") == Some(true) {
            bail!("production object storage must use HTTPS");
        }
        for name in ["controlMysql", "runtimeMysql"] {
            if self.string(&format!("/global/components/{name}/tlsMode")) != Some("verify_identity")
                || self
                    .string(&format!("/global/components/{name}/caSecretName"))
                    .unwrap_or("")
                    .is_empty()
            {
                bail!("production {name} requires verify_identity and caSecretName");
            }
        }
        if !self
            .string("/global/components/runtimeRedis/url")
            .unwrap_or("")
            .starts_with("rediss://")
            || self
                .string("/global/components/runtimeRedis/caSecretName")
                .unwrap_or("")
                .is_empty()
        {
            bail!("production Runtime Redis requires rediss:// and caSecretName");
        }
        for (name, endpoint_key) in [
            ("objectStorage", "endpoint"),
            ("secretProvider", "endpoint"),
            ("sandbox", "endpoint"),
            ("clickhouse", "url"),
        ] {
            let base = format!("/global/components/{name}");
            if !self
                .string(&format!("{base}/{endpoint_key}"))
                .unwrap_or("")
                .starts_with("https://")
                || self
                    .string(&format!("{base}/caSecretName"))
                    .unwrap_or("")
                    .is_empty()
            {
                bail!("production {name} requires HTTPS and caSecretName");
            }
        }
        if self.bool("/global/components/sandbox/secureAccess") != Some(true) {
            bail!("production OpenSandbox requires secure access");
        }
        if self.string("/global/network/egressGateway/sandboxAccess/mode")
            != Some("privateLoadBalancer")
            || self
                .string("/global/network/egressGateway/sandboxAccess/caSecretName")
                .unwrap_or("")
                .is_empty()
        {
            bail!("production sandbox Egress requires privateLoadBalancer and caSecretName");
        }
        let annotations =
            self.object("/global/network/egressGateway/sandboxAccess/serviceAnnotations");
        let private = [
            (
                "service.beta.kubernetes.io/aws-load-balancer-internal",
                "true",
            ),
            (
                "service.beta.kubernetes.io/azure-load-balancer-internal",
                "true",
            ),
            ("networking.gke.io/load-balancer-type", "Internal"),
            ("cloud.google.com/load-balancer-type", "Internal"),
        ];
        let cloud_private = private.iter().any(|(key, value)| {
            annotations
                .and_then(|map| map.get(*key))
                .and_then(Value::as_str)
                == Some(*value)
        });
        let metallb_pool = annotations
            .and_then(|map| map.get("metallb.io/address-pool"))
            .and_then(Value::as_str);
        let metallb_ip = annotations
            .and_then(|map| map.get("metallb.io/loadBalancerIPs"))
            .and_then(Value::as_str);
        let valid_private = if metallb_pool.is_some() || metallb_ip.is_some() {
            let endpoint = url::Url::parse(
                self.string("/global/network/egressGateway/sandboxAccess/endpoint")
                    .unwrap(),
            )?;
            metallb_pool.is_some_and(|pool| !pool.trim().is_empty())
                && self
                    .string("/global/network/egressGateway/sandboxAccess/loadBalancerClass")
                    .is_some_and(|class| !class.trim().is_empty())
                && metallb_ip.is_some_and(|address| {
                    address.parse::<IpAddr>().is_ok_and(|ip| {
                        let private = match ip {
                            IpAddr::V4(ip) => ip.is_private(),
                            IpAddr::V6(ip) => ip.is_unique_local(),
                        };
                        let endpoint_ip = match endpoint.host() {
                            Some(url::Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
                            Some(url::Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
                            _ => None,
                        };
                        private && endpoint_ip == Some(ip)
                    })
                })
        } else {
            cloud_private
        };
        if !valid_private {
            bail!(
                "production sandbox Egress requires a supported internal load balancer annotation or a MetalLB class and pool with a matching private IP"
            );
        }
        let digests = self
            .object("/global/images/digests")
            .cloned()
            .unwrap_or_default();
        let services = self.array("/global/images/services").unwrap();
        if digests.len() != services.len()
            || services
                .iter()
                .any(|service| !digests.contains_key(service.as_str().unwrap()))
        {
            bail!("production requires an immutable digest for every Agentx image");
        }
        let source_commit = self.string("/global/images/sourceCommit").unwrap_or("");
        if !Regex::new(r"^[0-9a-f]{40}$")
            .unwrap()
            .is_match(source_commit)
        {
            bail!(
                "production requires global.images.sourceCommit as a 40-character lowercase Git SHA"
            );
        }
        Ok(())
    }
}

fn deterministic_e2e_node_port(run_id: &str) -> u16 {
    let hash = run_id
        .as_bytes()
        .iter()
        .fold(2_166_136_261_u32, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(16_777_619)
        });
    30_000 + (hash % 2_768) as u16
}

pub fn selected_targets(target: &str) -> Result<Vec<&'static str>> {
    if target == "all" {
        return Ok(TARGETS.to_vec());
    }
    TARGETS
        .iter()
        .copied()
        .find(|candidate| *candidate == target)
        .map(|target| vec![target])
        .ok_or_else(|| anyhow!("unsupported target: {target}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_order_is_stable() {
        assert_eq!(selected_targets("all").unwrap(), TARGETS);
        assert!(selected_targets("unknown").is_err());
    }

    #[test]
    fn repository_values_are_valid() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for name in [
            "local.yaml",
            "local-tls.yaml",
            "dockerhub-beta.yaml",
            "production.example.yaml",
        ] {
            DeploymentConfig::load(root.join("deploy/values").join(name), None).unwrap();
        }
    }

    #[test]
    fn kubernetes_service_and_pod_networks_cannot_be_removed_from_egress_protection() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values/local.yaml");
        let mut config = DeploymentConfig::load(path, None).unwrap();
        config
            .values
            .pointer_mut("/global/network/egressGateway/blockedCidrs")
            .and_then(Value::as_array_mut)
            .unwrap()
            .retain(|value| value.as_str() != Some("10.96.0.0/12"));
        assert!(
            config
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("Kubernetes network")
        );
    }

    #[test]
    fn embedded_beta_is_the_default_standalone_configuration() {
        let config = DeploymentConfig::load_embedded_beta(None).unwrap();
        assert_eq!(config.path, PathBuf::from("embedded:dockerhub-beta.yaml"));
        assert_eq!(config.environment(), "local");
        assert_eq!(
            config.string("/global/images/tag"),
            Some(concat!("v", env!("CARGO_PKG_VERSION")))
        );
    }

    #[test]
    fn local_tls_uses_scoped_identities_and_explicit_ingress_ports() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values/local-tls.yaml");
        let config = DeploymentConfig::load(&path, None).unwrap();
        assert_eq!(
            config.ingress_url("control"),
            "https://agentx.localhost:30443"
        );
        let config = DeploymentConfig::load(path, Some("tls-contract")).unwrap();
        assert_eq!(config.ingress_service_type(), "ClusterIP");
        assert_eq!(
            config.string("/global/components/sandbox/endpoint"),
            Some("https://opensandbox.agentx-e2e-deps-tls-contract.svc:8443")
        );
        assert_eq!(
            config.string("/global/network/egressGateway/sandboxAccess/mode"),
            Some("nodePort")
        );
    }

    #[test]
    fn production_rejects_nodeport_and_missing_internal_annotations() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/values/production.example.yaml");
        let base = DeploymentConfig::load(path, None).unwrap();
        let mut named_like_e2e = base.clone();
        *named_like_e2e
            .values
            .pointer_mut("/global/namespaces/dependencies")
            .unwrap() = "agentx-e2e-production-deps".into();
        assert_eq!(named_like_e2e.ingress_service_type(), "LoadBalancer");
        let mut ingress = base.clone();
        ingress
            .values
            .pointer_mut("/global/ingress")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("serviceType".into(), "NodePort".into());
        assert!(
            ingress
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("production ingress requires LoadBalancer")
        );
        let mut config = base.clone();
        *config
            .values
            .pointer_mut("/global/network/egressGateway/sandboxAccess/mode")
            .unwrap() = "nodePort".into();
        assert!(
            config
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("privateLoadBalancer")
        );
        let mut config = base;
        *config
            .values
            .pointer_mut("/global/network/egressGateway/sandboxAccess/serviceAnnotations")
            .unwrap() = serde_json::json!({});
        assert!(
            config
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("internal load balancer annotation")
        );
    }

    #[test]
    fn production_metallb_requires_an_explicit_matching_private_ip() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/values/production.example.yaml");
        let base = DeploymentConfig::load(path, None).unwrap();
        for (pool, ip, host, valid) in [
            ("agentx-private", "172.18.255.241", "172.18.255.241", true),
            ("agentx-private", "fd00::241", "[fd00::241]", true),
            ("agentx-private", "8.8.8.8", "8.8.8.8", false),
            ("agentx-private", "172.18.255.241", "172.18.255.242", false),
            ("", "172.18.255.241", "172.18.255.241", false),
            ("agentx-private", "", "172.18.255.241", false),
        ] {
            let mut config = base.clone();
            config
                .values
                .pointer_mut("/global/network/egressGateway/sandboxAccess")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("loadBalancerClass".into(), "metallb".into());
            *config
                .values
                .pointer_mut("/global/network/egressGateway/sandboxAccess/serviceAnnotations")
                .unwrap() = serde_json::json!({
                "metallb.io/address-pool":pool,
                "metallb.io/loadBalancerIPs":ip,
                "service.beta.kubernetes.io/aws-load-balancer-internal":"true"
            });
            *config
                .values
                .pointer_mut("/global/network/egressGateway/sandboxAccess/endpoint")
                .unwrap() = format!("https://{host}:3129").into();
            *config
                .values
                .pointer_mut("/global/network/egressGateway/sandboxAccess/port")
                .unwrap() = 3129.into();
            let result = config.validate_semantics();
            assert_eq!(result.is_ok(), valid, "{result:?}");
            if valid {
                config
                    .values
                    .pointer_mut("/global/network/egressGateway/sandboxAccess")
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("loadBalancerClass");
                assert!(config.validate_semantics().is_err());
            }
        }
    }

    #[test]
    fn bundled_tls_rejects_missing_ca_and_plaintext_urls() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values/local-tls.yaml");
        let base = DeploymentConfig::load(path, None).unwrap();
        let mut mysql = base.clone();
        *mysql
            .values
            .pointer_mut("/global/components/controlMysql/tlsMode")
            .unwrap() = "preferred".into();
        assert!(
            mysql
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("verify_identity and caSecretName")
        );
        let mut config = base.clone();
        config
            .values
            .pointer_mut("/global/components/runtimeRedis")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("caSecretName");
        assert!(
            config
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("TLS requires caSecretName")
        );
        let mut config = base;
        *config
            .values
            .pointer_mut("/global/components/objectStorage/endpoint")
            .unwrap() = "http://object-storage.agentx-tls-deps.svc:9000".into();
        assert!(
            config
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("https:// endpoint")
        );
    }

    #[test]
    fn run_id_scopes_namespaces() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values/local.yaml");
        let config = DeploymentConfig::load(path, Some("Test_Run")).unwrap();
        assert_eq!(config.namespace("control"), "agentx-e2e-control-test-run");
        assert_eq!(
            config.namespace("observability"),
            "agentx-e2e-runtime-test-run"
        );
        assert_eq!(
            config.string("/global/network/egressGateway/sandboxAccess/endpoint"),
            Some("https://host.docker.internal:30957")
        );
        assert_eq!(
            config.u64("/global/network/egressGateway/sandboxAccess/port"),
            Some(30_957)
        );
        assert_eq!(
            config.string("/global/components/controlMysql/host"),
            Some("control-mysql.agentx-e2e-control-test-run.svc")
        );
        assert_eq!(
            config.string("/global/components/runtimeRedis/url"),
            Some("redis://runtime-redis.agentx-e2e-runtime-test-run.svc:6379/")
        );
        assert_eq!(
            config.string("/global/components/secretProvider/endpoint"),
            Some("http://vault.agentx-e2e-deps-test-run.svc:8200")
        );
        assert_eq!(
            config.string("/global/components/sandbox/endpoint"),
            Some("http://host.docker.internal:18080")
        );
    }

    #[test]
    fn production_requires_image_source_commit() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let source =
            std::fs::read_to_string(root.join("deploy/values/production.example.yaml")).unwrap();
        let source = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("sourceCommit:"))
            .collect::<Vec<_>>()
            .join("\n");
        let directory = tempfile::tempdir().unwrap();
        let values = directory.path().join("production.yaml");
        std::fs::write(&values, source).unwrap();
        let error = DeploymentConfig::load(values, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("sourceCommit"));
    }

    #[test]
    fn run_id_rejects_empty_long_and_production_scopes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values");
        for value in ["___", "abcdefghijklmnopqrstuvwxyz-0123456789-too-long"] {
            let error = DeploymentConfig::load(root.join("local.yaml"), Some(value))
                .unwrap_err()
                .to_string();
            assert!(error.contains("DNS-safe suffix"));
        }
        let error = DeploymentConfig::load(root.join("production.example.yaml"), Some("forbidden"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("forbidden for production"));
    }

    #[test]
    fn production_rejects_mutable_images_and_generated_secrets() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/values/production.example.yaml");
        let base = DeploymentConfig::load(path, None).unwrap();
        let mut generated = base.clone();
        *generated
            .values
            .pointer_mut("/global/secrets/mode")
            .unwrap() = "generated-local".into();
        assert!(
            generated
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("existing-kubernetes")
        );

        let mut missing_digest = base.clone();
        let digests = missing_digest
            .values
            .pointer_mut("/global/images/digests")
            .unwrap()
            .as_object_mut()
            .unwrap();
        let first_digest = digests.keys().next().unwrap().clone();
        digests.remove(&first_digest);
        assert!(
            missing_digest
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("immutable digest")
        );

        let mut uppercase_commit = base;
        *uppercase_commit
            .values
            .pointer_mut("/global/images/sourceCommit")
            .unwrap() = "ABCDEF0123456789ABCDEF0123456789ABCDEF01".into();
        assert!(
            uppercase_commit
                .validate_semantics()
                .unwrap_err()
                .to_string()
                .contains("lowercase Git SHA")
        );
    }
}
