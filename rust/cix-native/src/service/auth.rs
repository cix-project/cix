//! Verified identities and one atomic credential/policy authority.
use super::{
    config::{policy_problem, Config},
    error::{Problem, Result},
};
use crate::managed_sdk::{AccessRequest, Decision, Identity, Runtime as PolicyRuntime};
use argon2::{password_hash::PasswordHash, Argon2, PasswordVerifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, RwLock, Weak},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{OwnedRwLockReadGuard, RwLock as AsyncRwLock, Semaphore};
use zeroize::Zeroizing;

const CALLER_SECONDS: u64 = 3600;

#[derive(Clone)]
pub struct Authenticator {
    inner: Arc<Authority>,
}
struct Authority {
    snapshot: RwLock<Arc<Snapshot>>,
    password_slots: Arc<Semaphore>,
    password_checks: u32,
    publication: Arc<AsyncRwLock<()>>,
}
struct Snapshot {
    policy: PolicyRuntime,
    credentials: Vec<CredentialRecord>,
}
struct CredentialRecord {
    id: String,
    identity: Identity,
    expires: Option<u64>,
    verifier: Verifier,
}
enum Verifier {
    Key([u8; 32]),
    Password(Zeroizing<String>),
    Local,
}

/// Constructed only after verification by this authority. It contains no grants.
#[derive(Clone)]
pub struct VerifiedCaller {
    authority: Weak<Authority>,
    identity: Identity,
    credential_id: String,
    expires: u64,
    deadline: Instant,
}
impl std::fmt::Debug for VerifiedCaller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedCaller").finish_non_exhaustive()
    }
}
impl VerifiedCaller {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
}

/// Private, versioned durable proof reference, never accepted from a request.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueuedAuthority {
    pub(crate) version: u32,
    pub(crate) identity: Identity,
    credential_id: String,
    expires: u64,
}
impl std::fmt::Debug for QueuedAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueuedAuthority").finish_non_exhaustive()
    }
}

fn now_seconds() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| Problem::internal())
}
fn unauthorized() -> Problem {
    Problem::new("unauthorized", 401)
}
fn expired() -> Problem {
    Problem::new("session_expired", 401)
}
fn equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter()
        .zip(b)
        .fold(0u8, |difference, (x, y)| difference | (x ^ y))
        == 0
}
fn key_hash(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        return Err(Problem::invalid("credential"));
    }
    let mut result = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |x: u8| -> Option<u8> {
            match x {
                b'0'..=b'9' => Some(x - b'0'),
                b'a'..=b'f' => Some(x - b'a' + 10),
                b'A'..=b'F' => Some(x - b'A' + 10),
                _ => None,
            }
        };
        result[index] = (digit(chunk[0]).ok_or_else(|| Problem::invalid("credential"))? << 4)
            | digit(chunk[1]).ok_or_else(|| Problem::invalid("credential"))?;
    }
    Ok(result)
}
fn credential_id(identity: &Identity, verifier: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"CIX service credential reference v1\0");
    for value in [
        identity.provider.as_bytes(),
        identity.tenant.as_bytes(),
        identity.subject.as_bytes(),
        verifier,
    ] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value);
    }
    format!("{:x}", hash.finalize())
}
fn identity(provider: &str, tenant: &str, subject: &str) -> Identity {
    Identity {
        provider: provider.into(),
        tenant: tenant.into(),
        subject: subject.into(),
    }
}

/// Check every cost/format field before invoking the password KDF.
fn validate_password_hash(value: &str) -> Result<()> {
    if value.len() > 512 {
        return Err(Problem::invalid("password_verifier"));
    }
    let hash = PasswordHash::new(value).map_err(|_| Problem::invalid("password_verifier"))?;
    if hash.algorithm.as_str() != "argon2id"
        || hash.version != Some(19)
        || hash.params.iter().count() != 3
        || hash
            .params
            .iter()
            .any(|(name, _)| !matches!(name.as_str(), "m" | "t" | "p"))
    {
        return Err(Problem::invalid("password_verifier"));
    }
    let memory = hash
        .params
        .get_decimal("m")
        .ok_or_else(|| Problem::invalid("password_verifier"))?;
    let iterations = hash
        .params
        .get_decimal("t")
        .ok_or_else(|| Problem::invalid("password_verifier"))?;
    let lanes = hash
        .params
        .get_decimal("p")
        .ok_or_else(|| Problem::invalid("password_verifier"))?;
    if !(1..=4).contains(&lanes)
        || !(1..=3).contains(&iterations)
        || !(8 * lanes..=65536).contains(&memory)
    {
        return Err(Problem::invalid("password_verifier_cost"));
    }
    let mut buffer = [0u8; 64];
    let salt = hash
        .salt
        .ok_or_else(|| Problem::invalid("password_verifier"))?
        .decode_b64(&mut buffer)
        .map_err(|_| Problem::invalid("password_verifier"))?;
    let output = hash
        .hash
        .ok_or_else(|| Problem::invalid("password_verifier"))?;
    if !(16..=64).contains(&salt.len()) || !(32..=64).contains(&output.len()) {
        return Err(Problem::invalid("password_verifier"));
    }
    Ok(())
}

impl Snapshot {
    fn load(config: &Config) -> Result<Self> {
        config.validate()?;
        if config.credentials.len() > 4096 || config.local_users.len() > 1024 {
            return Err(Problem::limit("credentials"));
        }
        let policy = PolicyRuntime::new(config.effective_policy()?).map_err(policy_problem)?;
        let mut credentials = Vec::<CredentialRecord>::new();
        let now = now_seconds()?;
        for entry in &config.credentials {
            let digest = if let Some(hash) = &entry.key_sha256 {
                key_hash(hash)?
            } else {
                let token = Zeroizing::new(
                    std::env::var(&entry.token_env)
                        .map_err(|_| Problem::new("credential_unavailable", 503))?,
                );
                if !(32..=4096).contains(&token.len()) {
                    return Err(Problem::invalid("credential_length"));
                }
                Sha256::digest(token.as_bytes()).into()
            };
            if entry.expires_unix_seconds.is_some_and(|until| until <= now) {
                return Err(Problem::invalid("expired_credential"));
            }
            if credentials
                .iter()
                .any(|record| matches!(&record.verifier, Verifier::Key(old) if equal(old, &digest)))
            {
                return Err(Problem::invalid("duplicate_credential"));
            }
            let who = identity("api-key", &entry.tenant, &entry.subject);
            credentials.push(CredentialRecord {
                id: credential_id(&who, &digest),
                identity: who,
                expires: entry.expires_unix_seconds,
                verifier: Verifier::Key(digest),
            });
        }
        for entry in &config.local_users {
            validate_password_hash(&entry.password_hash)?;
            let who = identity("local-password", &entry.tenant, &entry.subject);
            if entry.expires_unix_seconds.is_some_and(|until| until <= now) {
                return Err(Problem::invalid("expired_credential"));
            }
            if credentials.iter().any(|record| record.identity == who) {
                return Err(Problem::invalid("duplicate_credential"));
            }
            credentials.push(CredentialRecord {
                id: credential_id(&who, entry.password_hash.as_bytes()),
                identity: who,
                expires: entry.expires_unix_seconds,
                verifier: Verifier::Password(Zeroizing::new(entry.password_hash.clone())),
            });
        }
        let who = identity("local-host", &config.stdio_tenant, &config.stdio_subject);
        credentials.push(CredentialRecord {
            id: credential_id(&who, b"trusted-local-host-v1"),
            identity: who,
            expires: None,
            verifier: Verifier::Local,
        });
        Ok(Self {
            policy,
            credentials,
        })
    }
}

impl Authenticator {
    pub fn load(config: &Config) -> Result<Self> {
        let snapshot = Arc::new(Snapshot::load(config)?);
        Ok(Self {
            inner: Arc::new(Authority {
                snapshot: RwLock::new(snapshot),
                password_slots: Arc::new(Semaphore::new(config.max_password_checks as usize)),
                password_checks: config.max_password_checks,
                publication: Arc::new(AsyncRwLock::new(())),
            }),
        })
    }
    fn snapshot(&self) -> Result<Arc<Snapshot>> {
        self.inner
            .snapshot
            .read()
            .map(|s| s.clone())
            .map_err(|_| Problem::internal())
    }
    /// Credential and policy replacement is atomic. Concurrency is startup-owned.
    pub(crate) async fn reload(&self, config: &Config) -> Result<()> {
        if config.max_password_checks != self.inner.password_checks {
            return Err(Problem::invalid("restart_required_password_checks"));
        }
        let next = Arc::new(Snapshot::load(config)?);
        let _exclusive = self.inner.publication.write().await;
        *self
            .inner
            .snapshot
            .write()
            .map_err(|_| Problem::internal())? = next;
        Ok(())
    }
    /// Serializes reload completion with authorized effects/publication. Workers
    /// hold this only for publication, never for their long codec computation.
    pub(crate) async fn operation(&self) -> OwnedRwLockReadGuard<()> {
        self.inner.publication.clone().read_owned().await
    }
    fn caller(&self, record: &CredentialRecord, until: Option<u64>) -> Result<VerifiedCaller> {
        let now = now_seconds()?;
        let expires = now
            .checked_add(CALLER_SECONDS)
            .ok_or_else(Problem::internal)?
            .min(record.expires.unwrap_or(u64::MAX))
            .min(until.unwrap_or(u64::MAX));
        if expires <= now {
            return Err(expired());
        }
        Ok(VerifiedCaller {
            authority: Arc::downgrade(&self.inner),
            identity: record.identity.clone(),
            credential_id: record.id.clone(),
            expires,
            deadline: Instant::now()
                .checked_add(Duration::from_secs(expires - now))
                .ok_or_else(Problem::internal)?,
        })
    }
    fn current_record<'a>(
        &self,
        snapshot: &'a Snapshot,
        caller: &VerifiedCaller,
    ) -> Result<&'a CredentialRecord> {
        let authority = caller.authority.upgrade().ok_or_else(unauthorized)?;
        if !Arc::ptr_eq(&authority, &self.inner) {
            return Err(unauthorized());
        }
        let now = now_seconds()?;
        if now >= caller.expires || Instant::now() >= caller.deadline {
            return Err(expired());
        }
        let record = snapshot
            .credentials
            .iter()
            .find(|r| r.id == caller.credential_id && r.identity == caller.identity)
            .ok_or_else(unauthorized)?;
        if record.expires.is_some_and(|until| now >= until) {
            return Err(expired());
        }
        Ok(record)
    }
    pub fn authenticate(&self, header: Option<&str>) -> Result<VerifiedCaller> {
        let token = header
            .and_then(|h| h.strip_prefix("Bearer "))
            .filter(|h| (32..=4096).contains(&h.len()))
            .ok_or_else(unauthorized)?;
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let snapshot = self.snapshot()?;
        let mut selected = None;
        for record in &snapshot.credentials {
            if let Verifier::Key(expected) = &record.verifier {
                if equal(expected, &digest) {
                    selected = Some(record);
                }
            }
        }
        self.caller(selected.ok_or_else(unauthorized)?, None)
    }
    pub async fn authenticate_password(
        &self,
        tenant: &str,
        subject: &str,
        password: String,
    ) -> Result<VerifiedCaller> {
        let password = Zeroizing::new(password);
        if password.is_empty() || password.len() > 1024 {
            return Err(unauthorized());
        }
        let snapshot = self.snapshot()?;
        let who = identity("local-password", tenant, subject);
        let record = snapshot
            .credentials
            .iter()
            .find(|r| r.identity == who)
            .ok_or_else(unauthorized)?;
        let caller = self.caller(record, None)?;
        let phc = match &record.verifier {
            Verifier::Password(value) => value.clone(),
            _ => return Err(unauthorized()),
        };
        let permit = self
            .inner
            .password_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Problem::new("authentication_busy", 429))?;
        let verified = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let parsed = PasswordHash::new(&phc).map_err(|_| unauthorized())?;
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .map_err(|_| unauthorized())
        })
        .await
        .map_err(|_| Problem::internal())?;
        verified?;
        let current = self.snapshot()?;
        self.current_record(&current, &caller)?;
        Ok(caller)
    }
    /// Only trusted in-process stdio/OS setup may invoke this, never a client identity.
    pub(crate) fn local_host(&self) -> Result<VerifiedCaller> {
        let snapshot = self.snapshot()?;
        let record = snapshot
            .credentials
            .iter()
            .find(|r| matches!(&r.verifier, Verifier::Local))
            .ok_or_else(unauthorized)?;
        self.caller(record, None)
    }
    pub(crate) fn authorize(
        &self,
        caller: &VerifiedCaller,
        request: &AccessRequest<'_>,
    ) -> Result<Decision> {
        let snapshot = self.snapshot()?;
        let record = self.current_record(&snapshot, caller)?;
        let session = snapshot
            .policy
            .session_for_verified_identity(record.identity.clone(), Duration::from_secs(1))
            .map_err(policy_problem)?;
        session.authorize(request).map_err(policy_problem)
    }
    pub(crate) fn queued(&self, caller: &VerifiedCaller) -> Result<QueuedAuthority> {
        let current = self.snapshot()?;
        self.current_record(&current, caller)?;
        Ok(QueuedAuthority {
            version: 1,
            identity: caller.identity.clone(),
            credential_id: caller.credential_id.clone(),
            expires: caller.expires,
        })
    }
    pub(crate) fn resume(&self, proof: &QueuedAuthority) -> Result<VerifiedCaller> {
        if proof.version != 1 {
            return Err(unauthorized());
        }
        let snapshot = self.snapshot()?;
        let record = snapshot
            .credentials
            .iter()
            .find(|r| r.id == proof.credential_id && r.identity == proof.identity)
            .ok_or_else(unauthorized)?;
        self.caller(record, Some(proof.expires))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        let mut c = Config::default();
        c.state_directory = std::env::temp_dir().join("cix-auth-unit-unused");
        c
    }
    #[tokio::test]
    async fn expired_and_foreign_callers_and_revoked_queued_proofs_fail() {
        let c = config();
        let auth = Authenticator::load(&c).unwrap();
        let mut caller = auth.local_host().unwrap();
        let proof = auth.queued(&caller).unwrap();
        assert!(auth.resume(&proof).is_ok());
        caller.expires = 0;
        assert_eq!(auth.queued(&caller).unwrap_err().code, "session_expired");
        let other = Authenticator::load(&c).unwrap();
        assert_eq!(
            other.queued(&auth.local_host().unwrap()).unwrap_err().code,
            "unauthorized"
        );
        let mut next = c;
        next.stdio_subject = "different".into();
        auth.reload(&next).await.unwrap();
        assert!(auth.resume(&proof).is_err());
    }
    #[tokio::test]
    async fn completed_reload_is_ordered_after_prior_publication_guard() {
        let config = config();
        let auth = Authenticator::load(&config).unwrap();
        let guard = auth.operation().await;
        let mut replacement = config;
        replacement.stdio_subject = "replacement".into();
        let other = auth.clone();
        let reload = tokio::spawn(async move { other.reload(&replacement).await });
        tokio::task::yield_now().await;
        assert!(!reload.is_finished());
        drop(guard);
        reload.await.unwrap().unwrap();
        assert_eq!(auth.local_host().unwrap().identity().subject, "replacement");
    }
    #[test]
    fn expired_proofs_cannot_refresh_their_lifetime() {
        let auth = Authenticator::load(&config()).unwrap();
        let caller = auth.local_host().unwrap();
        let mut proof = auth.queued(&caller).unwrap();
        proof.expires = 0;
        assert_eq!(auth.resume(&proof).unwrap_err().code, "session_expired");
    }
}
