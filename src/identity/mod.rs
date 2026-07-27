// Tencent is pleased to support the open source community by making Pole available.
//
// Copyright (C) 2019 THL A29 Limited, a Tencent company. All rights reserved.
//
// Licensed under the BSD 3-Clause License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://opensource.org/licenses/BSD-3-Clause

//! 数据面 workload 身份适配。
//!
//! Pole SDK 不拥有业务 HTTP/gRPC server，也不发送普通业务请求。业务必须将本模块
//! 提供的 injector/interceptor 显式挂载到自己的客户端和服务端网络栈。

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use http::{header::HeaderValue, HeaderMap, Request as HttpRequest};
use serde::Deserialize;
use tonic::{metadata::MetadataValue, service::Interceptor, Request as TonicRequest, Status};

use crate::core::model::error::{ErrorCode, PoleError};

/// WorkloadCredential 使用独立 header，避免占用业务的 Authorization header。
pub const WORKLOAD_CREDENTIAL_HEADER: &str = "x-pole-workload-credential";
const MAX_CREDENTIAL_SIZE: usize = 16 * 1024;

#[derive(Clone)]
pub struct AuthenticatedCaller {
    subject: String,
    namespace: String,
    service: String,
    trust_domain: String,
    issued_at: i64,
    expires_at: i64,
    key_id: String,
}

impl std::fmt::Debug for AuthenticatedCaller {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticatedCaller")
            .field("subject", &self.subject)
            .field("namespace", &self.namespace)
            .field("service", &self.service)
            .field("trust_domain", &self.trust_domain)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("key_id", &self.key_id)
            .finish()
    }
}

impl AuthenticatedCaller {
    #[cfg(test)]
    pub(crate) fn for_test(namespace: &str, service: &str) -> Self {
        Self {
            subject: format!("pole://service/{namespace}/{service}"),
            namespace: namespace.to_string(),
            service: service.to_string(),
            trust_domain: "pole.test".to_string(),
            issued_at: 1,
            expires_at: i64::MAX,
            key_id: "test-key".to_string(),
        }
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn trust_domain(&self) -> &str {
        &self.trust_domain
    }

    pub fn issued_at(&self) -> i64 {
        self.issued_at
    }

    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

#[derive(Clone)]
pub(crate) struct CredentialSnapshot {
    pub(crate) token: String,
    pub(crate) credential_id: String,
    pub(crate) expires_at: i64,
    pub(crate) renew_after: i64,
    pub(crate) identity_revision: String,
}

impl std::fmt::Debug for CredentialSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialSnapshot")
            .field("token", &"[REDACTED]")
            .field("credential_id", &self.credential_id)
            .field("expires_at", &self.expires_at)
            .field("renew_after", &self.renew_after)
            .field("identity_revision", &self.identity_revision)
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct TrustKey {
    pub(crate) key_id: String,
    pub(crate) public_key: [u8; 32],
    pub(crate) not_before: i64,
    pub(crate) not_after: i64,
}

#[derive(Clone, Default)]
pub(crate) struct TrustBundleSnapshot {
    pub(crate) trust_domain: String,
    pub(crate) issuer: String,
    pub(crate) version: String,
    pub(crate) sequence: u64,
    pub(crate) expires_at: i64,
    pub(crate) keys: Vec<TrustKey>,
    pub(crate) subject_revocations: HashMap<String, i64>,
    pub(crate) credential_revocations: HashSet<String>,
}

#[derive(Default)]
pub(crate) struct IdentityState {
    credential: RwLock<Option<CredentialSnapshot>>,
    trust_bundle: RwLock<TrustBundleSnapshot>,
}

impl IdentityState {
    pub(crate) fn replace_credential(&self, credential: CredentialSnapshot) {
        *self.credential.write().unwrap() = Some(credential);
    }

    pub(crate) fn clear_credential(&self) {
        *self.credential.write().unwrap() = None;
    }

    pub(crate) fn replace_trust_bundle(
        &self,
        bundle: TrustBundleSnapshot,
    ) -> Result<(), PoleError> {
        let mut current = self.trust_bundle.write().unwrap();
        if bundle.sequence < current.sequence {
            return Err(identity_error(
                ErrorCode::InvalidServerResponse,
                "workload trust bundle rollback detected",
            ));
        }
        if bundle.sequence == current.sequence && !current.version.is_empty() {
            if bundle.version != current.version {
                return Err(identity_error(
                    ErrorCode::InvalidServerResponse,
                    "workload trust bundle rollback detected",
                ));
            }
            if !same_trust_bundle_security_content(&current, &bundle) {
                return Err(identity_error(
                    ErrorCode::InvalidServerResponse,
                    "workload trust bundle content changed without sequence advance",
                ));
            }
            // control-plane 会在相同 sequence/version 下续租 bundle。安全关键内容一致时，
            // 允许刷新 bundle 与 key 的时间窗口，避免本地快照永久过期。
            *current = bundle;
            return Ok(());
        }
        *current = bundle;
        Ok(())
    }

    pub(crate) fn credential_for_renewal(&self) -> Option<(String, i64, i64, String)> {
        self.credential.read().unwrap().as_ref().map(|credential| {
            (
                credential.token.clone(),
                credential.expires_at,
                credential.renew_after,
                credential.identity_revision.clone(),
            )
        })
    }
}

fn same_trust_bundle_security_content(
    current: &TrustBundleSnapshot,
    candidate: &TrustBundleSnapshot,
) -> bool {
    current.trust_domain == candidate.trust_domain
        && current.issuer == candidate.issuer
        && current.version == candidate.version
        && current.keys.len() == candidate.keys.len()
        && current.keys.iter().all(|current_key| {
            candidate.keys.iter().any(|candidate_key| {
                current_key.key_id == candidate_key.key_id
                    && current_key.public_key == candidate_key.public_key
            })
        })
        && current.subject_revocations == candidate.subject_revocations
        && current.credential_revocations == candidate.credential_revocations
}

/// 一个 Engine 对应的 workload 身份句柄。
///
/// 该句柄不会公开原始凭证；业务只能通过注入器或验签器使用凭证。
#[derive(Clone)]
pub struct WorkloadIdentity {
    pub(crate) state: Arc<IdentityState>,
}

impl WorkloadIdentity {
    pub(crate) fn new(state: Arc<IdentityState>) -> Self {
        Self { state }
    }

    pub fn inject_http(&self, headers: &mut HeaderMap) -> Result<(), PoleError> {
        if headers.contains_key(WORKLOAD_CREDENTIAL_HEADER) {
            return Err(identity_error(
                ErrorCode::ApiInvalidArgument,
                "workload credential header already exists",
            ));
        }
        let token = self.current_credential()?;
        let value = HeaderValue::from_str(&token).map_err(|_| {
            identity_error(
                ErrorCode::InternalError,
                "workload credential is not a valid HTTP header value",
            )
        })?;
        headers.insert(WORKLOAD_CREDENTIAL_HEADER, value);
        Ok(())
    }

    pub fn tonic_client_interceptor(&self) -> WorkloadCredentialClientInterceptor {
        WorkloadCredentialClientInterceptor {
            identity: self.clone(),
        }
    }

    pub fn tonic_server_interceptor(
        &self,
        expected_audience: impl Into<String>,
    ) -> WorkloadCredentialServerInterceptor {
        WorkloadCredentialServerInterceptor {
            identity: self.clone(),
            expected_audience: expected_audience.into(),
        }
    }

    /// 验证 HTTP 请求并将可信 caller 写入 request extensions。
    pub fn authenticate_http<B>(
        &self,
        request: &mut HttpRequest<B>,
        expected_audience: &str,
    ) -> Result<AuthenticatedCaller, PoleError> {
        let caller = self.verify_headers(request.headers(), expected_audience)?;
        request.extensions_mut().insert(caller.clone());
        Ok(caller)
    }

    pub fn verify_headers(
        &self,
        headers: &HeaderMap,
        expected_audience: &str,
    ) -> Result<AuthenticatedCaller, PoleError> {
        let mut values = headers.get_all(WORKLOAD_CREDENTIAL_HEADER).iter();
        let value = values.next().ok_or_else(|| {
            identity_error(ErrorCode::UNAUTHORIZED, "workload credential is missing")
        })?;
        if values.next().is_some() {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "multiple workload credentials are not allowed",
            ));
        }
        let token = value.to_str().map_err(|_| {
            identity_error(ErrorCode::UNAUTHORIZED, "workload credential is malformed")
        })?;
        self.verify_jwt_at(token, expected_audience, unix_seconds())
    }

    fn current_credential(&self) -> Result<String, PoleError> {
        let now = unix_seconds();
        let credential = self.state.credential.read().unwrap();
        let credential = credential.as_ref().ok_or_else(|| {
            identity_error(ErrorCode::UNAUTHORIZED, "workload credential is not ready")
        })?;
        if credential.expires_at <= now {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential has expired",
            ));
        }
        Ok(credential.token.clone())
    }

    fn verify_jwt_at(
        &self,
        token: &str,
        expected_audience: &str,
        now: i64,
    ) -> Result<AuthenticatedCaller, PoleError> {
        if token.is_empty() || token.len() > MAX_CREDENTIAL_SIZE {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential size is invalid",
            ));
        }
        let mut segments = token.split('.');
        let encoded_header = segments.next().unwrap_or_default();
        let encoded_claims = segments.next().unwrap_or_default();
        let encoded_signature = segments.next().unwrap_or_default();
        if encoded_header.is_empty()
            || encoded_claims.is_empty()
            || encoded_signature.is_empty()
            || segments.next().is_some()
        {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential must be a compact JWT",
            ));
        }

        let header: JwtHeader = decode_json_segment(encoded_header)?;
        if header.alg != "EdDSA"
            || header.typ.as_deref() != Some("pole-workload+jwt")
            || header.kid.trim().is_empty()
        {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential algorithm or key id is invalid",
            ));
        }
        let claims: WorkloadClaims = decode_json_segment(encoded_claims)?;
        let signature_bytes = URL_SAFE_NO_PAD.decode(encoded_signature).map_err(|_| {
            identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential signature is malformed",
            )
        })?;
        let signature = Signature::from_slice(&signature_bytes).map_err(|_| {
            identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential signature length is invalid",
            )
        })?;

        let bundle = self.state.trust_bundle.read().unwrap();
        if bundle.version.trim().is_empty()
            || bundle.trust_domain.trim().is_empty()
            || bundle.issuer.trim().is_empty()
        {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload trust bundle is not ready",
            ));
        }
        if bundle.expires_at <= now {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload trust bundle has expired",
            ));
        }
        let key = bundle
            .keys
            .iter()
            .find(|key| key.key_id == header.kid)
            .ok_or_else(|| {
                identity_error(
                    ErrorCode::UNAUTHORIZED,
                    "workload credential key is unknown",
                )
            })?;
        if key.not_before > now || key.not_after <= now {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential key is outside its validity window",
            ));
        }
        let verifying_key = VerifyingKey::from_bytes(&key.public_key).map_err(|_| {
            identity_error(ErrorCode::UNAUTHORIZED, "workload trust key is invalid")
        })?;
        let signing_input = format!("{encoded_header}.{encoded_claims}");
        verifying_key
            .verify_strict(signing_input.as_bytes(), &signature)
            .map_err(|_| {
                identity_error(
                    ErrorCode::UNAUTHORIZED,
                    "workload credential signature is invalid",
                )
            })?;

        const CLOCK_SKEW_SECONDS: i64 = 30;
        if claims.issuer != bundle.issuer
            || claims.pole_trust_domain != bundle.trust_domain
            || claims.pole_ver != 1
            || claims.pole_identity_revision.trim().is_empty()
            || claims.pole_binding_type != "SERVICE_TOKEN"
            || claims.credential_id.trim().is_empty()
            || claims.subject.trim().is_empty()
            || claims.pole_namespace.trim().is_empty()
            || claims.pole_service.trim().is_empty()
            || claims.issued_at > now + CLOCK_SKEW_SECONDS
            || claims.not_before > now + CLOCK_SKEW_SECONDS
            || claims.expires_at <= now
            || claims.expires_at <= claims.issued_at
            || !claims.aud.matches(expected_audience)
        {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential claims are invalid",
            ));
        }
        if bundle
            .credential_revocations
            .contains(&claims.credential_id)
            || bundle
                .subject_revocations
                .get(&claims.subject)
                .map(|issued_before| claims.issued_at <= *issued_before)
                .unwrap_or(false)
        {
            return Err(identity_error(
                ErrorCode::UNAUTHORIZED,
                "workload credential has been revoked",
            ));
        }

        Ok(AuthenticatedCaller {
            subject: claims.subject,
            namespace: claims.pole_namespace,
            service: claims.pole_service,
            trust_domain: claims.pole_trust_domain,
            issued_at: claims.issued_at,
            expires_at: claims.expires_at,
            key_id: header.kid,
        })
    }
}

#[derive(Clone)]
pub struct WorkloadCredentialClientInterceptor {
    identity: WorkloadIdentity,
}

impl Interceptor for WorkloadCredentialClientInterceptor {
    fn call(&mut self, mut request: TonicRequest<()>) -> Result<TonicRequest<()>, Status> {
        if request.metadata().contains_key(WORKLOAD_CREDENTIAL_HEADER) {
            return Err(Status::invalid_argument(
                "workload credential metadata already exists",
            ));
        }
        let token = self
            .identity
            .current_credential()
            .map_err(|_| Status::unauthenticated("workload credential is unavailable"))?;
        let value = MetadataValue::try_from(token)
            .map_err(|_| Status::internal("workload credential metadata is invalid"))?;
        request
            .metadata_mut()
            .insert(WORKLOAD_CREDENTIAL_HEADER, value);
        Ok(request)
    }
}

#[derive(Clone)]
pub struct WorkloadCredentialServerInterceptor {
    identity: WorkloadIdentity,
    expected_audience: String,
}

impl Interceptor for WorkloadCredentialServerInterceptor {
    fn call(&mut self, mut request: TonicRequest<()>) -> Result<TonicRequest<()>, Status> {
        let mut headers = HeaderMap::new();
        let mut values = request
            .metadata()
            .get_all(WORKLOAD_CREDENTIAL_HEADER)
            .iter();
        let value = values
            .next()
            .ok_or_else(|| Status::unauthenticated("workload credential is missing"))?;
        if values.next().is_some() {
            return Err(Status::unauthenticated(
                "multiple workload credentials are not allowed",
            ));
        }
        let value = value
            .to_str()
            .map_err(|_| Status::unauthenticated("workload credential is malformed"))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| Status::unauthenticated("workload credential is malformed"))?;
        headers.insert(WORKLOAD_CREDENTIAL_HEADER, value);
        let caller = self
            .identity
            .verify_headers(&headers, &self.expected_audience)
            .map_err(|_| Status::unauthenticated("workload credential is invalid"))?;
        request.extensions_mut().insert(caller);
        Ok(request)
    }
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    typ: Option<String>,
    kid: String,
}

#[derive(Deserialize)]
struct WorkloadClaims {
    #[serde(rename = "iss")]
    issuer: String,
    #[serde(rename = "sub")]
    subject: String,
    aud: Audience,
    #[serde(rename = "jti")]
    credential_id: String,
    pole_ver: u32,
    pole_trust_domain: String,
    pole_namespace: String,
    pole_service: String,
    pole_identity_revision: String,
    pole_binding_type: String,
    #[serde(rename = "iat")]
    issued_at: i64,
    #[serde(rename = "nbf")]
    not_before: i64,
    #[serde(rename = "exp")]
    expires_at: i64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn matches(&self, expected: &str) -> bool {
        !expected.is_empty()
            && match self {
                Audience::One(value) => value == expected,
                Audience::Many(values) => values.iter().any(|value| value == expected),
            }
    }
}

fn decode_json_segment<T: for<'de> Deserialize<'de>>(encoded: &str) -> Result<T, PoleError> {
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| {
        identity_error(
            ErrorCode::UNAUTHORIZED,
            "workload credential encoding is malformed",
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        identity_error(
            ErrorCode::UNAUTHORIZED,
            "workload credential JSON is malformed",
        )
    })
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn identity_error(code: ErrorCode, message: &str) -> PoleError {
    PoleError::new(code, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    fn fixture() -> (WorkloadIdentity, SigningKey) {
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let state = Arc::new(IdentityState::default());
        state
            .replace_trust_bundle(TrustBundleSnapshot {
                trust_domain: "pole.local".to_string(),
                issuer: "https://issuer.pole.local".to_string(),
                version: "bundle-1".to_string(),
                sequence: 1,
                expires_at: i64::MAX,
                keys: vec![TrustKey {
                    key_id: "key-1".to_string(),
                    public_key: signing_key.verifying_key().to_bytes(),
                    not_before: 0,
                    not_after: i64::MAX,
                }],
                subject_revocations: HashMap::new(),
                credential_revocations: HashSet::new(),
            })
            .unwrap();
        (WorkloadIdentity::new(state), signing_key)
    }

    fn jwt(signing_key: &SigningKey, audience: &str, now: i64) -> String {
        jwt_with(
            signing_key,
            "key-1",
            "https://issuer.pole.local",
            audience,
            now,
        )
    }

    fn jwt_with(
        signing_key: &SigningKey,
        key_id: &str,
        issuer: &str,
        audience: &str,
        now: i64,
    ) -> String {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "alg": "EdDSA",
                "typ": "pole-workload+jwt",
                "kid": key_id
            }))
            .unwrap(),
        );
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "iss": issuer,
                "sub": "pole://service/orders",
                "aud": audience,
                "jti": "credential-1",
                "pole_ver": 1,
                "pole_trust_domain": "pole.local",
                "pole_namespace": "production",
                "pole_service": "orders",
                "pole_identity_revision": "identity-1",
                "pole_binding_type": "SERVICE_TOKEN",
                "iat": now,
                "nbf": now,
                "exp": now + 300
            }))
            .unwrap(),
        );
        let input = format!("{header}.{claims}");
        let signature = signing_key.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    #[test]
    fn verifies_ed25519_jwt_and_exposes_typed_caller() {
        let (identity, signing_key) = fixture();
        let token = jwt(&signing_key, "payments", 1_700_000_000);

        let caller = identity
            .verify_jwt_at(&token, "payments", 1_700_000_010)
            .unwrap();

        assert_eq!(caller.subject(), "pole://service/orders");
        assert_eq!(caller.namespace(), "production");
        assert_eq!(caller.service(), "orders");
        assert_eq!(caller.key_id(), "key-1");
    }

    #[test]
    fn rejects_tampering_wrong_audience_and_expiry() {
        let (identity, signing_key) = fixture();
        let token = jwt(&signing_key, "payments", 1_700_000_000);
        let mut tampered = token.clone();
        tampered.push('x');

        assert!(identity
            .verify_jwt_at(&tampered, "payments", 1_700_000_010)
            .is_err());
        assert!(identity
            .verify_jwt_at(&token, "inventory", 1_700_000_010)
            .is_err());
        assert!(identity
            .verify_jwt_at(&token, "payments", 1_700_000_400)
            .is_err());
    }

    #[test]
    fn rejects_unknown_key_and_wrong_issuer() {
        let (identity, signing_key) = fixture();

        assert!(identity
            .verify_jwt_at(
                &jwt_with(
                    &signing_key,
                    "unknown-key",
                    "https://issuer.pole.local",
                    "payments",
                    1_700_000_000,
                ),
                "payments",
                1_700_000_010,
            )
            .is_err());
        assert!(identity
            .verify_jwt_at(
                &jwt_with(
                    &signing_key,
                    "key-1",
                    "https://evil.example",
                    "payments",
                    1_700_000_000,
                ),
                "payments",
                1_700_000_010,
            )
            .is_err());
    }

    #[test]
    fn trust_bundle_sequence_never_rolls_back() {
        let state = IdentityState::default();
        let bundle = |sequence, version: &str| TrustBundleSnapshot {
            trust_domain: "pole.local".to_string(),
            issuer: "https://issuer.pole.local".to_string(),
            version: version.to_string(),
            sequence,
            expires_at: i64::MAX,
            keys: Vec::new(),
            subject_revocations: HashMap::new(),
            credential_revocations: HashSet::new(),
        };
        state.replace_trust_bundle(bundle(2, "bundle-2")).unwrap();

        assert!(state.replace_trust_bundle(bundle(1, "bundle-1")).is_err());
        assert!(state
            .replace_trust_bundle(bundle(2, "different-content-version"))
            .is_err());
        assert!(state.replace_trust_bundle(bundle(3, "bundle-3")).is_ok());
    }

    #[test]
    fn same_bundle_sequence_refreshes_only_validity_windows() {
        let state = IdentityState::default();
        let bundle = |public_key, expires_at, not_before, not_after| TrustBundleSnapshot {
            trust_domain: "pole.local".to_string(),
            issuer: "https://issuer.pole.local".to_string(),
            version: "bundle-1".to_string(),
            sequence: 1,
            expires_at,
            keys: vec![TrustKey {
                key_id: "key-1".to_string(),
                public_key,
                not_before,
                not_after,
            }],
            subject_revocations: HashMap::from([("pole://service/orders".to_string(), 100)]),
            credential_revocations: HashSet::from(["credential-1".to_string()]),
        };
        state
            .replace_trust_bundle(bundle([7; 32], 200, 10, 200))
            .unwrap();

        state
            .replace_trust_bundle(bundle([7; 32], 400, 20, 400))
            .unwrap();
        let current = state.trust_bundle.read().unwrap();
        assert_eq!(current.expires_at, 400);
        assert_eq!(current.keys[0].not_before, 20);
        assert_eq!(current.keys[0].not_after, 400);
        drop(current);

        assert!(state
            .replace_trust_bundle(bundle([8; 32], 500, 30, 500))
            .is_err());
        assert_eq!(state.trust_bundle.read().unwrap().expires_at, 400);
    }

    #[test]
    fn injection_is_fail_closed_and_never_overwrites_existing_header() {
        let (identity, signing_key) = fixture();
        let mut headers = HeaderMap::new();
        assert!(identity.inject_http(&mut headers).is_err());

        identity.state.replace_credential(CredentialSnapshot {
            token: jwt(&signing_key, "payments", unix_seconds()),
            credential_id: "credential-1".to_string(),
            expires_at: unix_seconds() + 300,
            renew_after: unix_seconds() + 200,
            identity_revision: "identity-1".to_string(),
        });
        identity.inject_http(&mut headers).unwrap();
        assert!(headers.contains_key(WORKLOAD_CREDENTIAL_HEADER));
        assert!(identity.inject_http(&mut headers).is_err());
    }

    #[test]
    fn http_authentication_inserts_authenticated_caller_extension() {
        let (identity, signing_key) = fixture();
        let token = jwt(&signing_key, "payments", unix_seconds());
        let mut request = HttpRequest::builder()
            .header(WORKLOAD_CREDENTIAL_HEADER, token)
            .body(())
            .unwrap();

        identity
            .authenticate_http(&mut request, "payments")
            .unwrap();

        assert_eq!(
            request
                .extensions()
                .get::<AuthenticatedCaller>()
                .unwrap()
                .service(),
            "orders"
        );
    }

    #[test]
    fn tonic_interceptors_inject_and_verify_explicitly() {
        let (identity, signing_key) = fixture();
        let now = unix_seconds();
        let token = jwt(&signing_key, "payments", now);
        identity.state.replace_credential(CredentialSnapshot {
            token: token.clone(),
            credential_id: "credential-1".to_string(),
            expires_at: now + 300,
            renew_after: now + 200,
            identity_revision: "identity-1".to_string(),
        });

        let mut client = identity.tonic_client_interceptor();
        let outbound = client.call(TonicRequest::new(())).unwrap();
        assert_eq!(
            outbound
                .metadata()
                .get(WORKLOAD_CREDENTIAL_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            token.as_str()
        );

        let mut server = identity.tonic_server_interceptor("payments");
        let inbound = server.call(outbound).unwrap();
        assert_eq!(
            inbound
                .extensions()
                .get::<AuthenticatedCaller>()
                .unwrap()
                .service(),
            "orders"
        );
    }

    #[test]
    fn tonic_server_rejects_duplicate_credentials() {
        let (identity, signing_key) = fixture();
        let token = jwt(&signing_key, "payments", unix_seconds());
        let value = MetadataValue::try_from(token).unwrap();
        let mut request = TonicRequest::new(());
        request
            .metadata_mut()
            .append(WORKLOAD_CREDENTIAL_HEADER, value.clone());
        request
            .metadata_mut()
            .append(WORKLOAD_CREDENTIAL_HEADER, value);

        let mut server = identity.tonic_server_interceptor("payments");
        let error = server.call(request).unwrap_err();

        assert_eq!(error.code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn credential_debug_is_redacted() {
        let snapshot = CredentialSnapshot {
            token: "secret.jwt".to_string(),
            credential_id: "credential-1".to_string(),
            expires_at: 42,
            renew_after: 21,
            identity_revision: "identity-1".to_string(),
        };

        let debug = format!("{snapshot:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret.jwt"));
    }
}
