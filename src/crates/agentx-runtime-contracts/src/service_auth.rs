use std::collections::{BTreeSet, HashMap};

use ipnet::IpNet;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ContentHash;

pub const SERVICE_TOKEN_TTL_SECONDS: i64 = 300;
pub const DELEGATION_TOKEN_TTL_SECONDS: i64 = 60;
pub const USER_ACCESS_TOKEN_TTL_SECONDS: i64 = 900;
pub const EGRESS_RUNTIME_TOKEN_TTL_SECONDS: i64 = 60;
pub const EGRESS_SANDBOX_TOKEN_MAX_TTL_SECONDS: i64 = 3_600;
pub const EGRESS_TOKEN_ISSUER: &str = "agentx-runtime";
pub const EGRESS_TOKEN_AUDIENCE: &str = "agentx-egress-gateway";

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ControlRole {
    Api,
    Publisher,
    Projector,
    Retention,
    DebugOrchestrator,
    EvaluationOrchestrator,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum EgressRole {
    RuntimeGateway,
    WorkflowRuntime,
    WorkflowWorker,
    Sandbox,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressMode {
    TcpProxy,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressPortRangeV1 {
    pub from: u16,
    pub to: u16,
}

impl EgressPortRangeV1 {
    #[must_use]
    pub fn contains(&self, port: u16) -> bool {
        self.from <= port && port <= self.to
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressDestinationV1 {
    pub target: String,
    pub ports: Vec<EgressPortRangeV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressConnectClaimsV1 {
    pub iss: String,
    pub aud: String,
    pub role: EgressRole,
    pub tenant_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Uuid>,
    pub egress_mode: EgressMode,
    pub destinations: Vec<EgressDestinationV1>,
    pub policy_hash: ContentHash,
    pub iat: i64,
    pub exp: i64,
    pub jti: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceClaimsV1 {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub role: ControlRole,
    pub scope: BTreeSet<String>,
    pub iat: i64,
    pub exp: i64,
    pub jti: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegationClaimsV1 {
    pub iss: String,
    pub aud: String,
    pub sub: Uuid,
    pub tenant_id: Uuid,
    pub token_version: u64,
    pub tenant_wide: bool,
    pub scope: BTreeSet<String>,
    pub application_ids: BTreeSet<Uuid>,
    pub workflow_ids: BTreeSet<Uuid>,
    pub execution_ids: BTreeSet<Uuid>,
    pub session_ids: BTreeSet<Uuid>,
    pub request_hash: ContentHash,
    pub iat: i64,
    pub exp: i64,
    pub jti: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserAccessClaimsV1 {
    pub iss: String,
    pub aud: String,
    pub sub: Uuid,
    pub tenant_id: Uuid,
    pub token_version: u64,
    pub kind: String,
    pub origin: crate::ExecutionOriginV1,
    pub iat: i64,
    pub exp: i64,
    pub jti: Uuid,
}

#[derive(Debug, Error)]
pub enum ServiceJwtError {
    #[error("JWT key id is missing")]
    MissingKeyId,
    #[error("JWT key id is not trusted")]
    UnknownKeyId,
    #[error("JWT role is not allowed")]
    RoleNotAllowed,
    #[error("JWT scope is not allowed")]
    ScopeNotAllowed,
    #[error("JWT lifetime exceeds the contract")]
    LifetimeExceeded,
    #[error("JWT is invalid or expired")]
    Invalid,
    #[error("JWT signing failed")]
    Signing,
}

pub fn issue_service_token(
    key_id: &str,
    private_key_pem: &[u8],
    claims: &ServiceClaimsV1,
) -> Result<String, ServiceJwtError> {
    ensure_lifetime(claims.iat, claims.exp, SERVICE_TOKEN_TTL_SECONDS)?;
    issue(key_id, private_key_pem, claims)
}

pub fn issue_delegation_token(
    key_id: &str,
    private_key_pem: &[u8],
    claims: &DelegationClaimsV1,
) -> Result<String, ServiceJwtError> {
    ensure_lifetime(claims.iat, claims.exp, DELEGATION_TOKEN_TTL_SECONDS)?;
    issue(key_id, private_key_pem, claims)
}

pub fn issue_user_access_token(
    key_id: &str,
    private_key_pem: &[u8],
    claims: &UserAccessClaimsV1,
) -> Result<String, ServiceJwtError> {
    ensure_lifetime(claims.iat, claims.exp, USER_ACCESS_TOKEN_TTL_SECONDS)?;
    if claims.kind != "access" {
        return Err(ServiceJwtError::RoleNotAllowed);
    }
    issue(key_id, private_key_pem, claims)
}

pub fn issue_egress_connect_token(
    key_id: &str,
    private_key_pem: &[u8],
    claims: &EgressConnectClaimsV1,
) -> Result<String, ServiceJwtError> {
    validate_egress_claims(claims, &BTreeSet::from([claims.role]), None)?;
    issue(key_id, private_key_pem, claims)
}

pub fn verify_egress_connect_token(
    token: &str,
    trusted_keys: &HashMap<String, Vec<u8>>,
    allowed_roles: &BTreeSet<EgressRole>,
    expected_target: Option<(&str, u16)>,
) -> Result<EgressConnectClaimsV1, ServiceJwtError> {
    let key_id = jsonwebtoken::decode_header(token)
        .map_err(|_| ServiceJwtError::Invalid)?
        .kid
        .ok_or(ServiceJwtError::MissingKeyId)?;
    let claims = verify::<EgressConnectClaimsV1>(
        token,
        trusted_keys,
        EGRESS_TOKEN_ISSUER,
        EGRESS_TOKEN_AUDIENCE,
    )?;
    if !egress_key_id_matches_role(&key_id, claims.role) {
        return Err(ServiceJwtError::RoleNotAllowed);
    }
    validate_egress_claims(&claims, allowed_roles, expected_target)?;
    Ok(claims)
}

fn egress_key_id_matches_role(key_id: &str, role: EgressRole) -> bool {
    let prefix = match role {
        EgressRole::RuntimeGateway => "runtime-gateway-",
        EgressRole::WorkflowRuntime => "workflow-runtime-",
        EgressRole::WorkflowWorker => "workflow-worker-",
        EgressRole::Sandbox => "sandbox-",
    };
    key_id.starts_with(prefix) && key_id.len() > prefix.len()
}

fn validate_egress_claims(
    claims: &EgressConnectClaimsV1,
    allowed_roles: &BTreeSet<EgressRole>,
    expected_target: Option<(&str, u16)>,
) -> Result<(), ServiceJwtError> {
    let now = now_unix();
    if claims.iss != EGRESS_TOKEN_ISSUER
        || claims.aud != EGRESS_TOKEN_AUDIENCE
        || claims.egress_mode != EgressMode::TcpProxy
        || !allowed_roles.contains(&claims.role)
    {
        return Err(ServiceJwtError::RoleNotAllowed);
    }
    let maximum = if claims.role == EgressRole::Sandbox {
        EGRESS_SANDBOX_TOKEN_MAX_TTL_SECONDS
    } else {
        EGRESS_RUNTIME_TOKEN_TTL_SECONDS
    };
    ensure_lifetime(claims.iat, claims.exp, maximum)?;
    if claims.iat > now + 5 || claims.exp <= now {
        return Err(ServiceJwtError::Invalid);
    }
    if let Some((host, port)) = expected_target
        && !claims
            .destinations
            .iter()
            .any(|destination| destination_matches(destination, host, port))
    {
        return Err(ServiceJwtError::ScopeNotAllowed);
    }
    if claims.destinations.is_empty()
        || claims.destinations.len() > 32
        || claims.destinations.iter().any(|destination| {
            destination.target.trim().is_empty()
                || destination.target.len() > 253
                || destination.ports.is_empty()
                || destination.ports.len() > 8
                || destination
                    .ports
                    .iter()
                    .any(|range| range.from == 0 || range.to == 0 || range.from > range.to)
        })
        || crate::content_hash(&claims.destinations).ok().as_ref() != Some(&claims.policy_hash)
    {
        return Err(ServiceJwtError::ScopeNotAllowed);
    }
    Ok(())
}

fn destination_matches(destination: &EgressDestinationV1, host: &str, port: u16) -> bool {
    if !destination.ports.iter().any(|range| range.contains(port)) {
        return false;
    }
    let expected = destination
        .target
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let actual = host
        .trim()
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if let Ok(network) = expected.parse::<IpNet>() {
        return actual
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| network.contains(&ip));
    }
    if let Some(suffix) = expected.strip_prefix("*.") {
        return actual.ends_with(&format!(".{suffix}")) && actual != suffix;
    }
    actual == expected
}

pub fn verify_user_access_token(
    token: &str,
    trusted_keys: &HashMap<String, Vec<u8>>,
    issuer: &str,
    audience: &str,
) -> Result<UserAccessClaimsV1, ServiceJwtError> {
    let claims = verify::<UserAccessClaimsV1>(token, trusted_keys, issuer, audience)?;
    ensure_lifetime(claims.iat, claims.exp, USER_ACCESS_TOKEN_TTL_SECONDS)?;
    if claims.kind != "access" {
        return Err(ServiceJwtError::RoleNotAllowed);
    }
    Ok(claims)
}

fn issue<T: Serialize>(
    key_id: &str,
    private_key_pem: &[u8],
    claims: &T,
) -> Result<String, ServiceJwtError> {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(key_id.to_owned());
    let key = EncodingKey::from_rsa_pem(private_key_pem).map_err(|_| ServiceJwtError::Signing)?;
    encode(&header, claims, &key).map_err(|_| ServiceJwtError::Signing)
}

pub fn verify_service_token(
    token: &str,
    trusted_keys: &HashMap<String, Vec<u8>>,
    issuer: &str,
    audience: &str,
    allowed_roles: &BTreeSet<ControlRole>,
    required_scopes: &BTreeSet<String>,
) -> Result<ServiceClaimsV1, ServiceJwtError> {
    let claims = verify::<ServiceClaimsV1>(token, trusted_keys, issuer, audience)?;
    ensure_lifetime(claims.iat, claims.exp, SERVICE_TOKEN_TTL_SECONDS)?;
    if !allowed_roles.contains(&claims.role) {
        return Err(ServiceJwtError::RoleNotAllowed);
    }
    if !required_scopes.is_subset(&claims.scope) {
        return Err(ServiceJwtError::ScopeNotAllowed);
    }
    Ok(claims)
}

pub fn verify_delegation_token(
    token: &str,
    trusted_keys: &HashMap<String, Vec<u8>>,
    issuer: &str,
    audience: &str,
    tenant_id: Uuid,
    required_scopes: &BTreeSet<String>,
) -> Result<DelegationClaimsV1, ServiceJwtError> {
    let claims = verify::<DelegationClaimsV1>(token, trusted_keys, issuer, audience)?;
    ensure_lifetime(claims.iat, claims.exp, DELEGATION_TOKEN_TTL_SECONDS)?;
    if claims.tenant_id != tenant_id {
        return Err(ServiceJwtError::ScopeNotAllowed);
    }
    if !required_scopes.is_subset(&claims.scope) {
        return Err(ServiceJwtError::ScopeNotAllowed);
    }
    Ok(claims)
}

fn verify<T: for<'de> Deserialize<'de>>(
    token: &str,
    trusted_keys: &HashMap<String, Vec<u8>>,
    issuer: &str,
    audience: &str,
) -> Result<T, ServiceJwtError> {
    let header = jsonwebtoken::decode_header(token).map_err(|_| ServiceJwtError::Invalid)?;
    let key_id = header.kid.ok_or(ServiceJwtError::MissingKeyId)?;
    let public_key = trusted_keys
        .get(&key_id)
        .ok_or(ServiceJwtError::UnknownKeyId)?;
    let key = DecodingKey::from_rsa_pem(public_key).map_err(|_| ServiceJwtError::Invalid)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.leeway = 0;
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    decode::<T>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|_| ServiceJwtError::Invalid)
}

fn ensure_lifetime(iat: i64, exp: i64, maximum: i64) -> Result<(), ServiceJwtError> {
    if exp <= iat || exp - iat > maximum {
        return Err(ServiceJwtError::LifetimeExceeded);
    }
    Ok(())
}

#[must_use]
pub fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use crate::content_hash;
    use uuid::Uuid;

    use super::{
        ControlRole, DELEGATION_TOKEN_TTL_SECONDS, DelegationClaimsV1, EGRESS_TOKEN_AUDIENCE,
        EGRESS_TOKEN_ISSUER, EgressConnectClaimsV1, EgressDestinationV1, EgressMode,
        EgressPortRangeV1, EgressRole, SERVICE_TOKEN_TTL_SECONDS, ServiceClaimsV1,
        destination_matches, issue_delegation_token, issue_egress_connect_token,
        issue_service_token, verify_delegation_token, verify_egress_connect_token,
        verify_service_token,
    };

    const PRIVATE_KEY: &[u8] = include_bytes!("../tests/fixtures/service-private.pem");
    const PUBLIC_KEY: &[u8] = include_bytes!("../tests/fixtures/service-public.pem");

    fn destinations() -> Vec<EgressDestinationV1> {
        vec![EgressDestinationV1 {
            target: "api.example.com".into(),
            ports: vec![EgressPortRangeV1 { from: 443, to: 443 }],
        }]
    }

    #[test]
    fn egress_destinations_match_domains_addresses_cidrs_and_port_ranges() {
        let ports = vec![EgressPortRangeV1 {
            from: 8_000,
            to: 8_010,
        }];
        let matches = |target: &str, host: &str, port: u16| {
            destination_matches(
                &EgressDestinationV1 {
                    target: target.into(),
                    ports: ports.clone(),
                },
                host,
                port,
            )
        };

        assert!(matches("api.example.com", "API.EXAMPLE.COM.", 8_000));
        assert!(matches("*.example.com", "db.internal.example.com", 8_010));
        assert!(!matches("*.example.com", "example.com", 8_000));
        assert!(!matches("*.example.com", "api.example.com", 8_011));
        assert!(matches("10.24.0.0/16", "10.24.8.9", 8_005));
        assert!(!matches("10.24.0.0/16", "10.25.8.9", 8_005));
        assert!(matches("2001:db8:abcd::/48", "[2001:db8:abcd::7]", 8_005));
        assert!(!matches("2001:db8:abcd::/48", "2001:db8:abce::7", 8_005));
    }

    fn claims() -> ServiceClaimsV1 {
        ServiceClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: "publisher".into(),
            role: ControlRole::Publisher,
            scope: BTreeSet::from(["bundle:prepare".into()]),
            iat: super::now_unix(),
            exp: super::now_unix() + SERVICE_TOKEN_TTL_SECONDS,
            jti: Uuid::now_v7(),
        }
    }

    #[test]
    fn rs256_validates_kid_role_audience_and_scope() {
        let token = issue_service_token("current", PRIVATE_KEY, &claims()).unwrap();
        let keys = HashMap::from([
            ("previous".into(), PUBLIC_KEY.to_vec()),
            ("current".into(), PUBLIC_KEY.to_vec()),
        ]);
        let verified = verify_service_token(
            &token,
            &keys,
            "agentx-control",
            "agentx-runtime-internal",
            &BTreeSet::from([ControlRole::Publisher]),
            &BTreeSet::from(["bundle:prepare".into()]),
        )
        .unwrap();
        assert_eq!(verified.role, ControlRole::Publisher);
        assert!(
            verify_service_token(
                &token,
                &keys,
                "agentx-control",
                "wrong-audience",
                &BTreeSet::from([ControlRole::Publisher]),
                &BTreeSet::new(),
            )
            .is_err()
        );
        assert!(
            verify_service_token(
                &token,
                &keys,
                "wrong-issuer",
                "agentx-runtime-internal",
                &BTreeSet::from([ControlRole::Publisher]),
                &BTreeSet::new(),
            )
            .is_err()
        );
        assert!(
            verify_service_token(
                &token,
                &keys,
                "agentx-control",
                "agentx-runtime-internal",
                &BTreeSet::from([ControlRole::Api]),
                &BTreeSet::new(),
            )
            .is_err()
        );
        assert!(
            verify_service_token(
                &token,
                &keys,
                "agentx-control",
                "agentx-runtime-internal",
                &BTreeSet::from([ControlRole::Publisher]),
                &BTreeSet::from(["deployment:activate".into()]),
            )
            .is_err()
        );
    }

    #[test]
    fn token_lifetime_is_bounded() {
        let mut claims = claims();
        claims.exp += 1;
        assert!(issue_service_token("current", PRIVATE_KEY, &claims).is_err());
    }

    #[test]
    fn removed_and_expired_keys_are_rejected() {
        let token = issue_service_token("removed", PRIVATE_KEY, &claims()).unwrap();
        let keys = HashMap::from([("current".into(), PUBLIC_KEY.to_vec())]);
        assert!(
            verify_service_token(
                &token,
                &keys,
                "agentx-control",
                "agentx-runtime-internal",
                &BTreeSet::from([ControlRole::Publisher]),
                &BTreeSet::new(),
            )
            .is_err()
        );

        let mut expired = claims();
        expired.iat = super::now_unix() - SERVICE_TOKEN_TTL_SECONDS;
        expired.exp = super::now_unix() - 1;
        let token = issue_service_token("current", PRIVATE_KEY, &expired).unwrap();
        assert!(
            verify_service_token(
                &token,
                &keys,
                "agentx-control",
                "agentx-runtime-internal",
                &BTreeSet::from([ControlRole::Publisher]),
                &BTreeSet::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn delegation_is_short_lived_and_tenant_scoped() {
        let now = super::now_unix();
        let tenant_id = Uuid::now_v7();
        let claims = DelegationClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-query".into(),
            sub: Uuid::now_v7(),
            tenant_id,
            token_version: 1,
            tenant_wide: false,
            scope: BTreeSet::from(["query:execution".into()]),
            application_ids: BTreeSet::new(),
            workflow_ids: BTreeSet::new(),
            execution_ids: BTreeSet::new(),
            session_ids: BTreeSet::new(),
            request_hash: crate::content_hash(&"delegation-test").unwrap(),
            iat: now,
            exp: now + DELEGATION_TOKEN_TTL_SECONDS,
            jti: Uuid::now_v7(),
        };
        let token = issue_delegation_token("current", PRIVATE_KEY, &claims).unwrap();
        let keys = HashMap::from([("current".into(), PUBLIC_KEY.to_vec())]);
        verify_delegation_token(
            &token,
            &keys,
            "agentx-control",
            "agentx-runtime-query",
            tenant_id,
            &BTreeSet::from(["query:execution".into()]),
        )
        .unwrap();
        assert!(
            verify_delegation_token(
                &token,
                &keys,
                "agentx-control",
                "agentx-runtime-query",
                Uuid::now_v7(),
                &BTreeSet::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn egress_token_is_short_lived_role_and_target_scoped() {
        let now = super::now_unix();
        let claims = EgressConnectClaimsV1 {
            iss: EGRESS_TOKEN_ISSUER.into(),
            aud: EGRESS_TOKEN_AUDIENCE.into(),
            role: EgressRole::WorkflowWorker,
            tenant_id: Uuid::now_v7(),
            execution_id: Some(Uuid::now_v7()),
            request_id: None,
            egress_mode: EgressMode::TcpProxy,
            destinations: destinations(),
            policy_hash: content_hash(&destinations()).unwrap(),
            iat: now,
            exp: now + 60,
            jti: Uuid::now_v7(),
        };
        let token =
            issue_egress_connect_token("workflow-worker-current", PRIVATE_KEY, &claims).unwrap();
        let keys = HashMap::from([("workflow-worker-current".into(), PUBLIC_KEY.to_vec())]);
        let verified = verify_egress_connect_token(
            &token,
            &keys,
            &BTreeSet::from([EgressRole::WorkflowWorker]),
            Some(("API.EXAMPLE.COM", 443)),
        )
        .unwrap();
        assert_eq!(verified.tenant_id, claims.tenant_id);
        assert!(
            verify_egress_connect_token(
                &token,
                &keys,
                &BTreeSet::from([EgressRole::RuntimeGateway]),
                Some(("api.example.com", 443)),
            )
            .is_err()
        );
        assert!(
            verify_egress_connect_token(
                &token,
                &keys,
                &BTreeSet::from([EgressRole::WorkflowWorker]),
                Some(("api.example.com", 8443)),
            )
            .is_err()
        );
    }

    #[test]
    fn runtime_egress_token_cannot_exceed_sixty_seconds() {
        let now = super::now_unix();
        let claims = EgressConnectClaimsV1 {
            iss: EGRESS_TOKEN_ISSUER.into(),
            aud: EGRESS_TOKEN_AUDIENCE.into(),
            role: EgressRole::RuntimeGateway,
            tenant_id: Uuid::now_v7(),
            execution_id: None,
            request_id: Some(Uuid::now_v7()),
            egress_mode: EgressMode::TcpProxy,
            destinations: destinations(),
            policy_hash: content_hash(&destinations()).unwrap(),
            iat: now,
            exp: now + 61,
            jti: Uuid::now_v7(),
        };
        assert!(issue_egress_connect_token("current", PRIVATE_KEY, &claims).is_err());
    }

    #[test]
    fn egress_key_rotation_accepts_overlap_then_rejects_the_removed_key() {
        let now = super::now_unix();
        let mut claims = EgressConnectClaimsV1 {
            iss: EGRESS_TOKEN_ISSUER.into(),
            aud: EGRESS_TOKEN_AUDIENCE.into(),
            role: EgressRole::WorkflowRuntime,
            tenant_id: Uuid::now_v7(),
            execution_id: Some(Uuid::now_v7()),
            request_id: None,
            egress_mode: EgressMode::TcpProxy,
            destinations: destinations(),
            policy_hash: content_hash(&destinations()).unwrap(),
            iat: now,
            exp: now + 60,
            jti: Uuid::now_v7(),
        };
        let previous =
            issue_egress_connect_token("workflow-runtime-previous", PRIVATE_KEY, &claims).unwrap();
        claims.jti = Uuid::now_v7();
        let current =
            issue_egress_connect_token("workflow-runtime-current", PRIVATE_KEY, &claims).unwrap();
        let roles = BTreeSet::from([EgressRole::WorkflowRuntime]);
        let overlap = HashMap::from([
            ("workflow-runtime-previous".into(), PUBLIC_KEY.to_vec()),
            ("workflow-runtime-current".into(), PUBLIC_KEY.to_vec()),
        ]);
        assert!(
            verify_egress_connect_token(
                &previous,
                &overlap,
                &roles,
                Some(("api.example.com", 443))
            )
            .is_ok()
        );
        assert!(
            verify_egress_connect_token(&current, &overlap, &roles, Some(("api.example.com", 443)))
                .is_ok()
        );
        let finalized = HashMap::from([("workflow-runtime-current".into(), PUBLIC_KEY.to_vec())]);
        assert!(
            verify_egress_connect_token(
                &previous,
                &finalized,
                &roles,
                Some(("api.example.com", 443))
            )
            .is_err()
        );
    }

    #[test]
    fn expired_and_unknown_kid_egress_tokens_are_rejected() {
        let now = super::now_unix();
        let claims = EgressConnectClaimsV1 {
            iss: EGRESS_TOKEN_ISSUER.into(),
            aud: EGRESS_TOKEN_AUDIENCE.into(),
            role: EgressRole::RuntimeGateway,
            tenant_id: Uuid::now_v7(),
            execution_id: None,
            request_id: Some(Uuid::now_v7()),
            egress_mode: EgressMode::TcpProxy,
            destinations: destinations(),
            policy_hash: content_hash(&destinations()).unwrap(),
            iat: now - 61,
            exp: now - 1,
            jti: Uuid::now_v7(),
        };
        let expired = super::issue("runtime-gateway-expired", PRIVATE_KEY, &claims).unwrap();
        let roles = BTreeSet::from([EgressRole::RuntimeGateway]);
        let keys = HashMap::from([("runtime-gateway-expired".into(), PUBLIC_KEY.to_vec())]);
        assert!(
            verify_egress_connect_token(&expired, &keys, &roles, Some(("api.example.com", 443)))
                .is_err()
        );

        let mut current = claims;
        current.iat = now;
        current.exp = now + 60;
        current.jti = Uuid::now_v7();
        let unknown =
            issue_egress_connect_token("runtime-gateway-unknown", PRIVATE_KEY, &current).unwrap();
        assert!(
            verify_egress_connect_token(
                &unknown,
                &HashMap::new(),
                &roles,
                Some(("api.example.com", 443))
            )
            .is_err()
        );
    }

    #[test]
    fn egress_key_id_cannot_impersonate_another_role() {
        let now = super::now_unix();
        let claims = EgressConnectClaimsV1 {
            iss: EGRESS_TOKEN_ISSUER.into(),
            aud: EGRESS_TOKEN_AUDIENCE.into(),
            role: EgressRole::RuntimeGateway,
            tenant_id: Uuid::now_v7(),
            execution_id: None,
            request_id: Some(Uuid::now_v7()),
            egress_mode: EgressMode::TcpProxy,
            destinations: destinations(),
            policy_hash: content_hash(&destinations()).unwrap(),
            iat: now,
            exp: now + 60,
            jti: Uuid::now_v7(),
        };
        let token =
            issue_egress_connect_token("workflow-worker-current", PRIVATE_KEY, &claims).unwrap();
        let keys = HashMap::from([("workflow-worker-current".into(), PUBLIC_KEY.to_vec())]);
        assert!(
            verify_egress_connect_token(
                &token,
                &keys,
                &BTreeSet::from([EgressRole::RuntimeGateway]),
                Some(("api.example.com", 443))
            )
            .is_err()
        );
    }
}
