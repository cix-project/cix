//! Explicit configuration. No credential values are included in diagnostics.
use super::{
    contracts::ResourceLimits,
    error::{Problem, Result},
};
use crate::managed_sdk::{Binding, Identity, Policy, ResourceCaps, Role, Rule};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub subject: String,
    pub tenant: String,
    /// Name of a runtime variable containing the bearer token, never its value.
    #[serde(default)]
    pub token_env: String,
    /// SHA-256 of a high-entropy API key; mutually exclusive with token_env.
    pub key_sha256: Option<String>,
    pub expires_unix_seconds: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub collections: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalUser {
    pub subject: String,
    pub tenant: String,
    pub password_hash: String,
    pub expires_unix_seconds: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub collections: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub listen: SocketAddr,
    pub state_directory: PathBuf,
    pub database_url_env: Option<String>,
    pub private_library_directory: Option<PathBuf>,
    pub credentials: Vec<Credential>,
    pub local_users: Vec<LocalUser>,
    /// When present, this is the sole grants source; legacy inline grants must
    /// be empty. Authentication providers never assign arbitrary caller roles.
    pub policy: Option<Policy>,
    pub stdio_tenant: String,
    pub stdio_subject: String,
    pub stdio_scopes: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub allowed_hosts: Vec<String>,
    pub limits: ResourceLimits,
    pub max_queued_jobs: u32,
    pub max_active_jobs: u32,
    pub max_json_bytes: usize,
    pub max_inline_bytes: usize,
    pub artifact_ttl_seconds: u64,
    pub worker_lease_seconds: u64,
    pub job_attempts: u32,
    pub max_password_checks: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            listen: ([127, 0, 0, 1], 7469).into(),
            state_directory: PathBuf::new(),
            database_url_env: None,
            private_library_directory: None,
            credentials: vec![],
            local_users: vec![],
            policy: None,
            stdio_tenant: "local".into(),
            stdio_subject: "local".into(),
            stdio_scopes: vec!["*".into()],
            allowed_origins: vec![],
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "[::1]".into()],
            limits: ResourceLimits::default(),
            max_queued_jobs: 64,
            max_active_jobs: 1,
            max_json_bytes: 1 << 20,
            max_inline_bytes: 64 << 10,
            artifact_ttl_seconds: 86400,
            worker_lease_seconds: 30,
            job_attempts: 3,
            max_password_checks: 2,
        }
    }
}
impl Config {
    pub fn read(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(Problem::invalid("configuration_file"));
        }
        let mut text = zeroize::Zeroizing::new(String::new());
        file.take((1 << 20) + 1).read_to_string(&mut text)?;
        Self::from_document(&text)
    }
    /// Explicit document loading also supports trusted host-owned settings.
    /// No ambient variable overrides or secondary files are consulted.
    pub fn from_document(text: &str) -> Result<Self> {
        if text.len() > 1 << 20 {
            return Err(Problem::limit("configuration"));
        }
        let c: Self = toml::from_str(text).map_err(|_| Problem::invalid("configuration"))?;
        c.validate()?;
        Ok(c)
    }
    pub fn redacted(&self) -> Self {
        let mut copy = self.clone();
        for credential in &mut copy.credentials {
            if let Some(value) = &mut credential.key_sha256 {
                zeroize::Zeroize::zeroize(value);
                *value = "[redacted]".into();
            }
        }
        for user in &mut copy.local_users {
            zeroize::Zeroize::zeroize(&mut user.password_hash);
            user.password_hash = "[redacted]".into();
        }
        copy
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Problem::invalid("configuration.version"));
        }
        if !self.state_directory.is_absolute() {
            return Err(Problem::invalid("state_directory"));
        }
        if self
            .private_library_directory
            .as_ref()
            .is_some_and(|p| !p.is_absolute())
        {
            return Err(Problem::invalid("private_library_directory"));
        }
        self.limits.within(&self.limits)?;
        if self.max_active_jobs == 0
            || self.max_queued_jobs == 0
            || self.max_json_bytes == 0
            || self.max_inline_bytes > 64 << 10
            || self.worker_lease_seconds < 5
            || self.worker_lease_seconds.checked_mul(1000).is_none()
            || self.job_attempts == 0
            || self.max_password_checks == 0
            || self.max_password_checks > 16
        {
            return Err(Problem::invalid("service_limits"));
        }
        if !safe_identity(&self.stdio_tenant) || !safe_identity(&self.stdio_subject) {
            return Err(Problem::invalid("stdio_identity"));
        }
        for item in &self.credentials {
            if !safe_identity(&item.tenant)
                || !safe_identity(&item.subject)
                || (item.token_env.is_empty() == item.key_sha256.is_none())
                || (!item.token_env.is_empty() && !valid_env_name(&item.token_env))
                || item
                    .key_sha256
                    .as_ref()
                    .is_some_and(|s| s.len() != 64 || !s.bytes().all(|c| c.is_ascii_hexdigit()))
            {
                return Err(Problem::invalid("credential"));
            }
        }
        for user in &self.local_users {
            if !safe_identity(&user.tenant)
                || !safe_identity(&user.subject)
                || user.password_hash.len() > 512
            {
                return Err(Problem::invalid("local_user"));
            }
        }
        if !self.local_users.is_empty()
            && u64::from(self.max_password_checks) * 64 * 1024 * 1024 > self.limits.memory_bytes.0
        {
            return Err(Problem::limit("authentication_memory"));
        }
        self.effective_policy()?
            .validate()
            .map_err(policy_problem)?;
        Ok(())
    }

    pub fn effective_policy(&self) -> Result<Policy> {
        if let Some(policy) = &self.policy {
            if self
                .credentials
                .iter()
                .any(|c| !c.scopes.is_empty() || !c.collections.is_empty())
                || self
                    .local_users
                    .iter()
                    .any(|u| !u.scopes.is_empty() || !u.collections.is_empty())
                || !self.stdio_scopes.is_empty()
            {
                return Err(Problem::invalid("ambiguous_policy_grants"));
            }
            let mut policy = policy.clone();
            policy.limits = policy.limits.intersect(managed_limits(&self.limits));
            return Ok(policy);
        }
        let mut policy = Policy {
            version: 1,
            limits: managed_limits(&self.limits),
            roles: vec![],
            bindings: vec![],
        };
        let entries = self
            .credentials
            .iter()
            .map(|c| ("api-key", &c.tenant, &c.subject, &c.scopes, &c.collections))
            .chain(self.local_users.iter().map(|u| {
                (
                    "local-password",
                    &u.tenant,
                    &u.subject,
                    &u.scopes,
                    &u.collections,
                )
            }));
        for (provider, tenant, subject, scopes, collections) in entries {
            add_grant(&mut policy, provider, tenant, subject, scopes, collections)?;
        }
        add_grant(
            &mut policy,
            "local-host",
            &self.stdio_tenant,
            &self.stdio_subject,
            &self.stdio_scopes,
            &[],
        )?;
        Ok(policy)
    }
}
pub fn safe_identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn valid_env_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !s.as_bytes()[0].is_ascii_digit()
}
pub fn managed_limits(limits: &ResourceLimits) -> ResourceCaps {
    ResourceCaps {
        input_bytes: limits.input_bytes.0,
        output_bytes: limits.output_bytes.0,
        memory_bytes: limits.memory_bytes.0,
        temporary_bytes: limits.temporary_bytes.0,
        workers: limits.workers,
        deadline_ms: limits.deadline_ms.0,
    }
}
pub fn policy_problem(error: crate::managed_sdk::Error) -> Problem {
    use crate::managed_sdk::Error;
    match error {
        Error::InvalidConfiguration(field) | Error::InvalidRequest(field) => {
            Problem::invalid(field)
        }
        Error::ResourceLimit(resource) => Problem::limit(resource),
        Error::PermissionDenied => Problem::new("forbidden", 403),
        Error::SessionExpired => Problem::new("session_expired", 401),
        Error::Internal => Problem::internal(),
    }
}
fn add_grant(
    policy: &mut Policy,
    provider: &str,
    tenant: &str,
    subject: &str,
    scopes: &[String],
    collections: &[String],
) -> Result<()> {
    let identity = Identity {
        provider: provider.into(),
        tenant: tenant.into(),
        subject: subject.into(),
    };
    let mut allow = Vec::new();
    for scope in scopes {
        let mut rule = Rule::action(scope);
        // Preserve legacy ownership restrictions while removing the old
        // admin:read shortcut for writes. Explicit Policy rules are unchanged.
        rule.own_jobs_only = scope.starts_with("jobs:") || scope.starts_with("artifacts:");
        let mut rules = vec![rule];
        if scopes.iter().any(|value| value == "admin:read") {
            for (family, read) in [("jobs:*", "jobs:read"), ("artifacts:*", "artifacts:read")] {
                if scope == family || scope == read {
                    rules.push(Rule::action(read));
                }
            }
        }
        for rule in rules {
            if collections.is_empty() {
                allow.push(rule);
            } else {
                for collection in collections {
                    let mut scoped = rule.clone();
                    scoped.collection = Some(collection.clone());
                    allow.push(scoped);
                }
            }
        }
    }
    if let Some(binding) = policy.bindings.iter().find(|b| b.identity == identity) {
        let role = policy
            .roles
            .iter()
            .find(|r| r.id == binding.roles[0])
            .ok_or_else(Problem::internal)?;
        if role.allow != allow {
            return Err(Problem::invalid("inconsistent_identity_grants"));
        }
        return Ok(());
    }
    let id = format!("inline-{}", policy.roles.len());
    policy.roles.push(Role {
        id: id.clone(),
        allow,
        deny: vec![],
        limits: None,
    });
    policy.bindings.push(Binding {
        identity,
        roles: vec![id],
        limits: None,
    });
    Ok(())
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("subject", &self.subject)
            .field("tenant", &self.tenant)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for LocalUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalUser")
            .field("subject", &self.subject)
            .field("tenant", &self.tenant)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Drop for Credential {
    fn drop(&mut self) {
        if let Some(value) = &mut self.key_sha256 {
            zeroize::Zeroize::zeroize(value);
        }
    }
}
impl Drop for LocalUser {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.password_hash);
    }
}
