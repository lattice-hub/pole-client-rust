// Tencent is pleased to support the open source community by making Pole available.
//
// Copyright (C) 2019 THL A29 Limited, a Tencent company. All rights reserved.
//
// Licensed under the BSD 3-Clause License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://opensource.org/licenses/BSD-3-Clause

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pole_specification::v1::{
    Code, ServiceIdentityDescriptor, VerificationKeyState, WorkloadBindingType, WorkloadCredential,
    WorkloadCredentialFormat, WorkloadCredentialIssueRequest, WorkloadCredentialRenewRequest,
    WorkloadCredentialResponse, WorkloadSigningAlgorithm, WorkloadTrustBundle,
};
use tokio::{runtime::Runtime, sync::watch, task::JoinHandle};

use crate::core::config::global::ServiceIdentityDiscoveryConfig;
use crate::core::model::cache::{EventType, RemoteData, ResourceEventKey};
use crate::core::model::error::{ErrorCode, PoleError};
use crate::core::plugin::connector::{Connector, ResourceHandler};
use crate::identity::{
    CredentialSnapshot, IdentityState, TrustBundleSnapshot, TrustKey, WorkloadIdentity,
};
use crate::{error, info};

/// 一个 Engine 内的身份、trust bundle 与短期凭证生命周期。
pub(crate) struct IdentityRuntime {
    _descriptor: Arc<RwLock<Option<ServiceIdentityDescriptor>>>,
    workload_identity: WorkloadIdentity,
    stopped: Arc<AtomicBool>,
    credential_worker: JoinHandle<()>,
}

impl IdentityRuntime {
    pub(crate) fn start(
        runtime: Arc<Runtime>,
        connector: Arc<Box<dyn Connector>>,
        config: ServiceIdentityDiscoveryConfig,
    ) -> Self {
        let descriptor = Arc::new(RwLock::new(None));
        let state = Arc::new(IdentityState::default());
        let workload_identity = WorkloadIdentity::new(state.clone());
        let (descriptor_tx, descriptor_rx) = watch::channel(None);

        let identity_handler = ServiceIdentityResourceHandler {
            namespace: config.namespace.clone(),
            service: config.service.clone(),
            descriptor: descriptor.clone(),
            descriptor_tx,
        };
        let bundle_handler = TrustBundleResourceHandler {
            namespace: config.namespace,
            service: config.service,
            state: state.clone(),
        };
        let registration_connector = connector.clone();
        runtime.spawn(async move {
            if let Err(err) = registration_connector
                .register_resource_handler(Box::new(identity_handler))
                .await
            {
                error!(
                    "[pole][identity] register service identity watch failed: {}",
                    err
                );
            }
            if let Err(err) = registration_connector
                .register_resource_handler(Box::new(bundle_handler))
                .await
            {
                error!(
                    "[pole][identity] register trust bundle watch failed: {}",
                    err
                );
            }
        });

        let stopped = Arc::new(AtomicBool::new(false));
        let credential_worker = runtime.spawn(run_credential_worker(
            connector,
            descriptor_rx,
            state,
            stopped.clone(),
        ));

        Self {
            _descriptor: descriptor,
            workload_identity,
            stopped,
            credential_worker,
        }
    }

    pub(crate) fn workload_identity(&self) -> WorkloadIdentity {
        self.workload_identity.clone()
    }
}

impl Drop for IdentityRuntime {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.credential_worker.abort();
        self.workload_identity.state.clear_credential();
    }
}

struct ServiceIdentityResourceHandler {
    namespace: String,
    service: String,
    descriptor: Arc<RwLock<Option<ServiceIdentityDescriptor>>>,
    descriptor_tx: watch::Sender<Option<ServiceIdentityDescriptor>>,
}

impl ResourceHandler for ServiceIdentityResourceHandler {
    fn handle_event(&self, event: RemoteData) {
        let Some(response) = event.discover_value else {
            return;
        };
        let Some(descriptor) = response.service_identity else {
            return;
        };
        if descriptor.namespace != self.namespace || descriptor.service != self.service {
            error!(
                "[pole][identity] ignore mismatched descriptor: expected={}/{} actual={}/{}",
                self.namespace, self.service, descriptor.namespace, descriptor.service
            );
            return;
        }
        if descriptor.subject.trim().is_empty()
            || descriptor.revision.trim().is_empty()
            || descriptor.trust_bundle_version.trim().is_empty()
            || descriptor.trust_domain.trim().is_empty()
            || descriptor.audience.trim().is_empty()
            || descriptor.credential_endpoint.trim().is_empty()
            || descriptor.identity_protocol_version != 1
            || !descriptor
                .credential_formats
                .contains(&(WorkloadCredentialFormat::JwtEd25519 as i32))
        {
            error!(
                "[pole][identity] ignore unsupported service identity descriptor for {}/{}",
                self.namespace, self.service
            );
            return;
        }
        *self.descriptor.write().unwrap() = Some(descriptor.clone());
        let _ = self.descriptor_tx.send(Some(descriptor));
    }

    fn interest_resource(&self) -> ResourceEventKey {
        ResourceEventKey {
            namespace: self.namespace.clone(),
            event_type: EventType::ServiceIdentity,
            filter: HashMap::from([("service".to_string(), self.service.clone())]),
        }
    }
}

struct TrustBundleResourceHandler {
    namespace: String,
    service: String,
    state: Arc<IdentityState>,
}

impl ResourceHandler for TrustBundleResourceHandler {
    fn handle_event(&self, event: RemoteData) {
        let Some(response) = event.discover_value else {
            return;
        };
        let Some(bundle) = response.service_identity_bundle else {
            return;
        };
        match convert_trust_bundle(bundle) {
            Ok(bundle) => {
                if let Err(err) = self.state.replace_trust_bundle(bundle) {
                    error!("[pole][identity] reject trust bundle update: {}", err);
                }
            }
            Err(err) => error!("[pole][identity] reject invalid trust bundle: {}", err),
        }
    }

    fn interest_resource(&self) -> ResourceEventKey {
        ResourceEventKey {
            namespace: self.namespace.clone(),
            event_type: EventType::ServiceIdentityBundle,
            filter: HashMap::from([
                ("service".to_string(), self.service.clone()),
                ("sequence".to_string(), "0".to_string()),
            ]),
        }
    }
}

async fn run_credential_worker(
    connector: Arc<Box<dyn Connector>>,
    mut descriptor_rx: watch::Receiver<Option<ServiceIdentityDescriptor>>,
    state: Arc<IdentityState>,
    stopped: Arc<AtomicBool>,
) {
    let mut backoff = Duration::from_secs(1);
    while !stopped.load(Ordering::Relaxed) {
        let descriptor = loop {
            if let Some(descriptor) = descriptor_rx.borrow().clone() {
                break descriptor;
            }
            if descriptor_rx.changed().await.is_err() {
                return;
            }
        };

        let action = credential_refresh_action(
            state.credential_for_renewal(),
            &descriptor.revision,
            unix_seconds(),
        );
        if let CredentialRefreshAction::Wait(wait) = &action {
            tokio::select! {
                _ = tokio::time::sleep(*wait) => {},
                changed = descriptor_rx.changed() => {
                    if changed.is_err() { return; }
                }
            }
            continue;
        }

        let response = match action {
            CredentialRefreshAction::Renew(credential) => {
                connector
                    .renew_workload_credential(WorkloadCredentialRenewRequest {
                        protocol_version: 1,
                        request_id: uuid::Uuid::new_v4().to_string(),
                        current_credential: credential,
                        accepted_formats: vec![WorkloadCredentialFormat::JwtEd25519 as i32],
                        evidence: None,
                    })
                    .await
            }
            CredentialRefreshAction::Issue => {
                state.clear_credential();
                connector
                    .issue_workload_credential(WorkloadCredentialIssueRequest {
                        protocol_version: 1,
                        request_id: uuid::Uuid::new_v4().to_string(),
                        expected_identity_revision: descriptor.revision.clone(),
                        accepted_formats: vec![WorkloadCredentialFormat::JwtEd25519 as i32],
                        evidence: None,
                    })
                    .await
            }
            CredentialRefreshAction::Wait(_) => unreachable!(),
        };

        let descriptor_is_current = descriptor_rx
            .borrow()
            .as_ref()
            .map(|current| current.revision == descriptor.revision)
            .unwrap_or(false);
        if !descriptor_is_current {
            state.clear_credential();
            continue;
        }

        match response.and_then(|response| credential_snapshot(response, &descriptor)) {
            Ok(credential) => {
                state.replace_credential(credential);
                backoff = Duration::from_secs(1);
                info!("[pole][identity] workload credential is ready");
            }
            Err(err) => {
                // 不记录请求或响应对象，避免泄露 current/serialized credential。
                error!(
                    "[pole][identity] workload credential refresh failed: {}",
                    err
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

enum CredentialRefreshAction {
    Wait(Duration),
    Issue,
    // Secret: do not derive Debug for this enum.
    Renew(String),
}

fn credential_refresh_action(
    current: Option<(String, i64, i64, String)>,
    descriptor_revision: &str,
    now: i64,
) -> CredentialRefreshAction {
    match current {
        Some((credential, expires_at, renew_after, identity_revision))
            if identity_revision == descriptor_revision && expires_at > now =>
        {
            if renew_after > now {
                CredentialRefreshAction::Wait(Duration::from_secs(
                    (renew_after - now).min(30) as u64
                ))
            } else {
                CredentialRefreshAction::Renew(credential)
            }
        }
        _ => CredentialRefreshAction::Issue,
    }
}

fn credential_snapshot(
    response: WorkloadCredentialResponse,
    descriptor: &ServiceIdentityDescriptor,
) -> Result<CredentialSnapshot, PoleError> {
    if response.code != Code::ExecuteSuccess as u32 {
        return Err(PoleError::new(ErrorCode::ServerError, response.info));
    }
    let credential = response.credential.ok_or_else(|| {
        PoleError::new(
            ErrorCode::InvalidServerResponse,
            "credential response has no credential".to_string(),
        )
    })?;
    validate_credential(credential, descriptor)
}

fn validate_credential(
    credential: WorkloadCredential,
    descriptor: &ServiceIdentityDescriptor,
) -> Result<CredentialSnapshot, PoleError> {
    let expires_at = timestamp_seconds(credential.expires_at.as_ref())?;
    let renew_after = timestamp_seconds(credential.renew_after.as_ref())?;
    let issued_at = timestamp_seconds(credential.issued_at.as_ref())?;
    let not_before = timestamp_seconds(credential.not_before.as_ref())?;
    if credential.format() != WorkloadCredentialFormat::JwtEd25519
        || credential.binding_type() != WorkloadBindingType::WorkloadBindingServiceToken
        || credential.serialized.trim().is_empty()
        || credential.credential_id.trim().is_empty()
        || credential.key_id.trim().is_empty()
        || credential.trust_bundle_version != descriptor.trust_bundle_version
        || not_before > issued_at
        || renew_after < issued_at
        || renew_after >= expires_at
        || expires_at <= unix_seconds()
    {
        return Err(PoleError::new(
            ErrorCode::InvalidServerResponse,
            "credential response is invalid".to_string(),
        ));
    }
    Ok(CredentialSnapshot {
        token: credential.serialized,
        credential_id: credential.credential_id,
        expires_at,
        renew_after,
        identity_revision: descriptor.revision.clone(),
    })
}

fn convert_trust_bundle(bundle: WorkloadTrustBundle) -> Result<TrustBundleSnapshot, PoleError> {
    if bundle.schema_version != 1
        || bundle.sequence == 0
        || bundle.trust_domain.trim().is_empty()
        || bundle.version.trim().is_empty()
        || bundle.issuer.trim().is_empty()
    {
        return Err(invalid_bundle());
    }
    let expires_at = timestamp_seconds(bundle.expires_at.as_ref())?;
    if expires_at <= unix_seconds() {
        return Err(invalid_bundle());
    }

    let mut keys = Vec::new();
    for key in bundle.keys {
        if key.algorithm() != WorkloadSigningAlgorithm::Ed25519
            || !matches!(
                key.state(),
                VerificationKeyState::Active | VerificationKeyState::Retiring
            )
            || key.key_id.trim().is_empty()
            || key.public_key.len() != 32
        {
            continue;
        }
        let public_key: [u8; 32] = key.public_key.try_into().map_err(|_| invalid_bundle())?;
        let key_not_before = key
            .not_before
            .as_ref()
            .map(|timestamp| timestamp_seconds(Some(timestamp)))
            .transpose()?
            .unwrap_or_else(|| {
                bundle
                    .issued_at
                    .as_ref()
                    .map(|timestamp| timestamp.seconds)
                    .unwrap_or_default()
            });
        let key_not_after = key
            .not_after
            .as_ref()
            .map(|timestamp| timestamp_seconds(Some(timestamp)))
            .transpose()?
            .unwrap_or(expires_at);
        keys.push(TrustKey {
            key_id: key.key_id,
            public_key,
            not_before: key_not_before,
            not_after: key_not_after,
        });
    }
    if keys.is_empty() {
        return Err(invalid_bundle());
    }

    let mut subject_revocations = HashMap::new();
    for revocation in bundle.subject_revocations {
        if revocation.subject.trim().is_empty() {
            return Err(invalid_bundle());
        }
        let issued_before = timestamp_seconds(revocation.credentials_issued_before.as_ref())?;
        subject_revocations.insert(revocation.subject, issued_before);
    }
    let mut credential_revocations = HashSet::new();
    for revocation in bundle.credential_revocations {
        if revocation.credential_id.trim().is_empty() {
            return Err(invalid_bundle());
        }
        credential_revocations.insert(revocation.credential_id);
    }

    Ok(TrustBundleSnapshot {
        trust_domain: bundle.trust_domain,
        issuer: bundle.issuer,
        version: bundle.version,
        sequence: bundle.sequence,
        expires_at,
        keys,
        subject_revocations,
        credential_revocations,
    })
}

fn timestamp_seconds(timestamp: Option<&prost_types::Timestamp>) -> Result<i64, PoleError> {
    let timestamp = timestamp.ok_or_else(|| {
        PoleError::new(
            ErrorCode::InvalidServerResponse,
            "identity timestamp is missing".to_string(),
        )
    })?;
    if timestamp.seconds < 0 || !(0..1_000_000_000).contains(&timestamp.nanos) {
        return Err(PoleError::new(
            ErrorCode::InvalidServerResponse,
            "identity timestamp is invalid".to_string(),
        ));
    }
    Ok(timestamp.seconds)
}

fn invalid_bundle() -> PoleError {
    PoleError::new(
        ErrorCode::InvalidServerResponse,
        "workload trust bundle is invalid".to_string(),
    )
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use pole_specification::v1::{
        discover_response::DiscoverResponseType, CredentialRevocation, DiscoverResponse,
        SubjectRevocation, WorkloadVerificationKey,
    };

    fn descriptor() -> ServiceIdentityDescriptor {
        ServiceIdentityDescriptor {
            subject: "pole://service/orders".to_string(),
            namespace: "production".to_string(),
            service: "orders".to_string(),
            revision: "identity-1".to_string(),
            trust_bundle_version: "bundle-1".to_string(),
            trust_domain: "pole.local".to_string(),
            audience: "pole-data-plane".to_string(),
            credential_endpoint: "/v1.WorkloadCredentialService".to_string(),
            identity_protocol_version: 1,
            credential_formats: vec![WorkloadCredentialFormat::JwtEd25519 as i32],
            ..ServiceIdentityDescriptor::default()
        }
    }

    #[test]
    fn identity_handler_rejects_wrong_service_and_accepts_supported_descriptor() {
        let state = Arc::new(RwLock::new(None));
        let (tx, _rx) = watch::channel(None);
        let handler = ServiceIdentityResourceHandler {
            namespace: "production".to_string(),
            service: "orders".to_string(),
            descriptor: state.clone(),
            descriptor_tx: tx,
        };
        let mut wrong = descriptor();
        wrong.service = "payments".to_string();
        handler.handle_event(identity_event(wrong));
        assert!(state.read().unwrap().is_none());

        handler.handle_event(identity_event(descriptor()));
        assert_eq!(
            state.read().unwrap().as_ref().unwrap().subject,
            "pole://service/orders"
        );
    }

    #[test]
    fn trust_bundle_conversion_keeps_only_usable_ed25519_keys() {
        let now = unix_seconds();
        let converted = convert_trust_bundle(WorkloadTrustBundle {
            schema_version: 1,
            trust_domain: "pole.local".to_string(),
            issuer: "https://issuer.pole.local".to_string(),
            version: "bundle-1".to_string(),
            sequence: 1,
            issued_at: Some(timestamp(now - 1)),
            expires_at: Some(timestamp(now + 300)),
            keys: vec![WorkloadVerificationKey {
                key_id: "key-1".to_string(),
                algorithm: WorkloadSigningAlgorithm::Ed25519.into(),
                public_key: vec![7; 32],
                state: VerificationKeyState::Active.into(),
                // control-plane 可省略 key 窗口，SDK 回退到 bundle 生命周期。
                not_before: None,
                not_after: None,
            }],
            ..WorkloadTrustBundle::default()
        })
        .unwrap();

        assert_eq!(converted.keys.len(), 1);
        assert_eq!(converted.sequence, 1);
        assert_eq!(converted.keys[0].not_before, now - 1);
        assert_eq!(converted.keys[0].not_after, now + 300);
    }

    #[test]
    fn trust_bundle_rejects_malformed_revocations_instead_of_ignoring_them() {
        let now = unix_seconds();
        let bundle = |subject_revocations, credential_revocations| WorkloadTrustBundle {
            schema_version: 1,
            trust_domain: "pole.local".to_string(),
            issuer: "https://issuer.pole.local".to_string(),
            version: "bundle-1".to_string(),
            sequence: 1,
            issued_at: Some(timestamp(now - 1)),
            expires_at: Some(timestamp(now + 300)),
            keys: vec![WorkloadVerificationKey {
                key_id: "key-1".to_string(),
                algorithm: WorkloadSigningAlgorithm::Ed25519.into(),
                public_key: vec![7; 32],
                state: VerificationKeyState::Active.into(),
                not_before: None,
                not_after: None,
            }],
            subject_revocations,
            credential_revocations,
            ..WorkloadTrustBundle::default()
        };

        assert!(convert_trust_bundle(bundle(
            vec![SubjectRevocation {
                subject: "pole://service/orders".to_string(),
                credentials_issued_before: None,
                ..SubjectRevocation::default()
            }],
            Vec::new(),
        ))
        .is_err());
        assert!(convert_trust_bundle(bundle(
            Vec::new(),
            vec![CredentialRevocation {
                credential_id: String::new(),
                ..CredentialRevocation::default()
            }],
        ))
        .is_err());
    }

    #[test]
    fn credential_response_requires_service_token_binding_and_matching_bundle() {
        let now = unix_seconds();
        let valid = WorkloadCredential {
            format: WorkloadCredentialFormat::JwtEd25519.into(),
            serialized: "header.claim.signature".to_string(),
            credential_id: "credential-1".to_string(),
            key_id: "key-1".to_string(),
            trust_bundle_version: "bundle-1".to_string(),
            issued_at: Some(timestamp(now)),
            not_before: Some(timestamp(now - 30)),
            expires_at: Some(timestamp(now + 300)),
            renew_after: Some(timestamp(now + 200)),
            binding_type: WorkloadBindingType::WorkloadBindingServiceToken.into(),
        };
        assert!(validate_credential(valid.clone(), &descriptor()).is_ok());

        let mut wrong = valid;
        wrong.binding_type = WorkloadBindingType::WorkloadBindingKubernetesSa.into();
        assert!(validate_credential(wrong, &descriptor()).is_err());
    }

    #[test]
    fn refresh_lifecycle_issues_waits_renews_and_reissues_on_revision_change() {
        assert!(matches!(
            credential_refresh_action(None, "identity-1", 100),
            CredentialRefreshAction::Issue
        ));
        assert!(matches!(
            credential_refresh_action(
                Some(("secret".to_string(), 300, 200, "identity-1".to_string())),
                "identity-1",
                100,
            ),
            CredentialRefreshAction::Wait(_)
        ));
        assert!(matches!(
            credential_refresh_action(
                Some(("secret".to_string(), 300, 200, "identity-1".to_string())),
                "identity-1",
                200,
            ),
            CredentialRefreshAction::Renew(_)
        ));
        assert!(matches!(
            credential_refresh_action(
                Some(("secret".to_string(), 300, 200, "identity-1".to_string())),
                "identity-2",
                100,
            ),
            CredentialRefreshAction::Issue
        ));
    }

    fn identity_event(value: ServiceIdentityDescriptor) -> RemoteData {
        RemoteData {
            event_key: ResourceEventKey {
                namespace: "production".to_string(),
                event_type: EventType::ServiceIdentity,
                filter: HashMap::from([("service".to_string(), "orders".to_string())]),
            },
            discover_value: Some(DiscoverResponse {
                code: Code::ExecuteSuccess as u32,
                r#type: DiscoverResponseType::ServiceIdentity.into(),
                service_identity: Some(value),
                ..DiscoverResponse::default()
            }),
            config_value: None,
        }
    }

    fn timestamp(seconds: i64) -> prost_types::Timestamp {
        prost_types::Timestamp { seconds, nanos: 0 }
    }
}
