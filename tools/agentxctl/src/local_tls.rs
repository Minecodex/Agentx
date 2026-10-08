use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

use crate::{
    config::DeploymentConfig,
    secrets::{Secret, apply_secret},
};

const CA_KEY: &str = "LOCAL_TLS_CA_CERTIFICATE_PEM";
const SUBJECTS_KEY: &str = "LOCAL_TLS_SUBJECTS_JSON";

struct Identity {
    name: String,
    secret: String,
    plane: &'static str,
    consumers: Vec<&'static str>,
    sans: Vec<String>,
}

fn identities(config: &DeploymentConfig) -> Result<Vec<Identity>> {
    let mut identities = Vec::new();
    for (name, mode, endpoint, plane, consumers) in [
        (
            "controlMysql",
            "bundled",
            "host",
            "control",
            vec!["control"],
        ),
        (
            "runtimeMysql",
            "bundled",
            "host",
            "runtime",
            vec!["runtime"],
        ),
        (
            "runtimeRedis",
            "bundled",
            "url",
            "runtime",
            vec!["runtime", "observability"],
        ),
        (
            "clickhouse",
            "bundled",
            "url",
            "observability",
            vec!["observability"],
        ),
        (
            "objectStorage",
            "bundled-minio",
            "endpoint",
            "dependencies",
            vec!["dependencies", "control", "runtime", "observability"],
        ),
        (
            "secretProvider",
            "vault_kv_v2",
            "endpoint",
            "dependencies",
            vec!["dependencies", "control", "runtime"],
        ),
        (
            "sandbox",
            "external_opensandbox",
            "endpoint",
            "dependencies",
            vec!["dependencies", "runtime"],
        ),
    ] {
        let base = format!("/global/components/{name}");
        let Some(secret) = config
            .string(&format!("{base}/caSecretName"))
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        if config.string(&format!("{base}/mode")) != Some(mode)
            || name == "sandbox"
                && config
                    .string(&format!("{base}/localProxyUpstream"))
                    .is_none()
        {
            continue;
        }
        let endpoint = config.string(&format!("{base}/{endpoint}")).unwrap();
        let host = if name.ends_with("Mysql") {
            endpoint.to_owned()
        } else {
            url::Url::parse(endpoint)?
                .host_str()
                .context("TLS endpoint has no host")?
                .to_owned()
        };
        let mut sans = vec![host.clone()];
        if host.ends_with(".svc") {
            sans.push(format!("{host}.cluster.local"));
        }
        identities.push(Identity {
            name: name.into(),
            secret: secret.into(),
            plane,
            consumers,
            sans,
        });
    }
    for (field, host, plane) in [
        ("controlTlsSecretName", "controlHost", "control"),
        ("runtimeTlsSecretName", "runtimeHost", "runtime"),
    ] {
        if let Some(secret) = config
            .string(&format!("/global/ingress/{field}"))
            .filter(|name| !name.is_empty())
        {
            identities.push(Identity {
                name: field.into(),
                secret: secret.into(),
                plane,
                consumers: vec![plane],
                sans: vec![
                    config
                        .string(&format!("/global/ingress/{host}"))
                        .unwrap()
                        .into(),
                ],
            });
        }
    }
    Ok(identities)
}

pub(crate) fn ensure_material(config: &DeploymentConfig, canonical: &mut Secret) -> Result<bool> {
    let subjects: BTreeMap<_, _> = identities(config)?
        .into_iter()
        .map(|identity| (identity.name, identity.sans))
        .collect();
    if subjects.is_empty() {
        return Ok(false);
    }
    let encoded = serde_json::to_string(&subjects)?;
    if canonical.contains_key(CA_KEY) {
        ensure!(
            canonical.get(SUBJECTS_KEY) == Some(&encoded),
            "local TLS identities changed; reinstall in fresh namespaces to issue new identities"
        );
        for name in subjects.keys() {
            for suffix in ["CERTIFICATE_PEM", "PRIVATE_KEY_PEM"] {
                ensure!(
                    canonical.contains_key(&format!("LOCAL_TLS_{name}_{suffix}")),
                    "local TLS identity is incomplete: {name}"
                );
            }
        }
        return Ok(false);
    }
    let material = agentx_key_material::local_tls_material(&subjects)?;
    canonical.insert(CA_KEY.into(), material.ca_certificate_pem);
    canonical.insert(SUBJECTS_KEY.into(), encoded);
    for (name, (key, cert)) in material.identities {
        canonical.insert(format!("LOCAL_TLS_{name}_CERTIFICATE_PEM"), cert);
        canonical.insert(format!("LOCAL_TLS_{name}_PRIVATE_KEY_PEM"), key);
    }
    Ok(true)
}

pub(crate) async fn publish(
    config: &DeploymentConfig,
    canonical: &Secret,
    targets: &BTreeSet<&str>,
) -> Result<()> {
    for identity in identities(config)? {
        let ca = canonical.get(CA_KEY).context("local TLS CA is missing")?;
        let cert = canonical
            .get(&format!("LOCAL_TLS_{}_CERTIFICATE_PEM", identity.name))
            .context("local TLS certificate is missing")?;
        let key = canonical
            .get(&format!("LOCAL_TLS_{}_PRIVATE_KEY_PEM", identity.name))
            .context("local TLS key is missing")?;
        let mut namespaces = BTreeSet::new();
        for consumer in identity
            .consumers
            .iter()
            .filter(|plane| targets.contains(**plane))
        {
            namespaces.insert(config.namespace(consumer));
        }
        for namespace in namespaces {
            let mut data = Secret::from([("ca.crt".into(), ca.clone())]);
            if namespace == config.namespace(identity.plane) {
                data.insert("tls.crt".into(), cert.clone());
                data.insert("tls.key".into(), key.clone());
            }
            apply_secret(namespace, &identity.secret, &data).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{
        RootCertStore,
        client::danger::ServerCertVerifier,
        pki_types::{ServerName, UnixTime},
    };
    use std::{io::Cursor, path::PathBuf, sync::Arc};

    #[test]
    fn local_certificates_verify_scoped_hosts_and_reuse_the_same_material() {
        let config = DeploymentConfig::load(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values/local-tls.yaml"),
            Some("tls-identity"),
        )
        .unwrap();
        let mut canonical = Secret::new();
        assert!(ensure_material(&config, &mut canonical).unwrap());
        let first = canonical.clone();
        assert!(!ensure_material(&config, &mut canonical).unwrap());
        assert_eq!(canonical, first);
        let mut roots = RootCertStore::empty();
        for certificate in rustls_pemfile::certs(&mut Cursor::new(&canonical[CA_KEY])) {
            roots.add(certificate.unwrap()).unwrap();
        }
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        )
        .build()
        .unwrap();
        for identity in identities(&config).unwrap() {
            let cert = rustls_pemfile::certs(&mut Cursor::new(
                &canonical[&format!("LOCAL_TLS_{}_CERTIFICATE_PEM", identity.name)],
            ))
            .next()
            .unwrap()
            .unwrap();
            for name in identity.sans {
                verifier
                    .verify_server_cert(
                        &cert,
                        &[],
                        &ServerName::try_from(name).unwrap(),
                        &[],
                        UnixTime::now(),
                    )
                    .unwrap();
            }
            assert!(
                verifier
                    .verify_server_cert(
                        &cert,
                        &[],
                        &ServerName::try_from("wrong.example").unwrap(),
                        &[],
                        UnixTime::now()
                    )
                    .is_err()
            );
        }
        let changed = DeploymentConfig::load(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values/local-tls.yaml"),
            Some("different-namespace"),
        )
        .unwrap();
        assert!(
            ensure_material(&changed, &mut canonical)
                .unwrap_err()
                .to_string()
                .contains("identities changed")
        );
    }
}
