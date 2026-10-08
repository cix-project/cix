use super::{Error, Result};
use std::collections::BTreeSet;

/// Exact, case-sensitive authenticated identity. Display names are not keys.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(feature = "server", derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema))]
#[cfg_attr(feature = "server", serde(deny_unknown_fields))]
pub struct Identity {
    pub provider: String,
    pub subject: String,
    pub tenant: String,
}

pub(crate) fn valid_identity_part(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

impl Identity {
    pub fn validate(&self) -> Result<()> {
        if [&self.provider, &self.subject, &self.tenant].iter().all(|s| valid_identity_part(s)) {
            Ok(())
        } else {
            Err(Error::InvalidConfiguration("identity"))
        }
    }
}

/// Per-operation ceilings. Zero temporary bytes explicitly forbids temporary
/// storage. These are admission limits, not a process-RSS guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "server", derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema))]
#[cfg_attr(feature = "server", serde(deny_unknown_fields))]
pub struct ResourceCaps {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub memory_bytes: u64,
    pub temporary_bytes: u64,
    pub workers: u32,
    pub deadline_ms: u64,
}

impl ResourceCaps {
    pub fn validate(self) -> Result<()> {
        if self.input_bytes == 0 || self.output_bytes == 0 || self.memory_bytes == 0
            || self.workers == 0 || self.deadline_ms == 0
        {
            Err(Error::InvalidConfiguration("resource_limits"))
        } else {
            Ok(())
        }
    }

    pub fn intersect(self, other: Self) -> Self {
        Self {
            input_bytes: self.input_bytes.min(other.input_bytes),
            output_bytes: self.output_bytes.min(other.output_bytes),
            memory_bytes: self.memory_bytes.min(other.memory_bytes),
            temporary_bytes: self.temporary_bytes.min(other.temporary_bytes),
            workers: self.workers.min(other.workers),
            deadline_ms: self.deadline_ms.min(other.deadline_ms),
        }
    }

    pub fn within(self, cap: Self) -> Result<()> {
        self.validate().map_err(|_| Error::InvalidRequest("resource_limits"))?;
        for (name, value, maximum) in [
            ("input_bytes", self.input_bytes, cap.input_bytes),
            ("output_bytes", self.output_bytes, cap.output_bytes),
            ("memory_bytes", self.memory_bytes, cap.memory_bytes),
            ("temporary_bytes", self.temporary_bytes, cap.temporary_bytes),
            ("deadline_ms", self.deadline_ms, cap.deadline_ms),
            ("workers", u64::from(self.workers), u64::from(cap.workers)),
        ] {
            if value > maximum { return Err(Error::ResourceLimit(name)); }
        }
        Ok(())
    }
}

/// An action is exact, `*`, or a trailing component wildcard such as `jobs:*`.
/// Optional selectors are exact. None explicitly matches all values for that
/// selector within the identity's tenant. There are no path-prefix matchers.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "server", derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema))]
#[cfg_attr(feature = "server", serde(deny_unknown_fields))]
pub struct Rule {
    pub action: String,
    pub namespace: Option<String>,
    pub collection: Option<String>,
    pub profile: Option<String>,
    pub codec: Option<String>,
    pub root: Option<String>,
    #[cfg_attr(feature = "server", serde(default))]
    pub own_jobs_only: bool,
}

fn valid_action(value: &str, pattern: bool) -> bool {
    let value = if pattern && value == "*" { return true; }
        else if pattern { value.strip_suffix(":*").unwrap_or(value) }
        else { value };
    !value.is_empty() && value.len() <= 128
        && value.split(':').all(|part| !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)))
}

impl Rule {
    pub fn action(action: impl Into<String>) -> Self {
        Self { action: action.into(), namespace: None, collection: None,
            profile: None, codec: None, root: None, own_jobs_only: false }
    }

    fn validate(&self) -> Result<()> {
        if !valid_action(&self.action, true)
            || [&self.namespace, &self.collection, &self.profile, &self.codec, &self.root]
                .iter().any(|s| s.as_ref().is_some_and(|v| !valid_identity_part(v)))
        {
            Err(Error::InvalidConfiguration("role_rule"))
        } else { Ok(()) }
    }

    fn matches(&self, identity: &Identity, request: &AccessRequest<'_>) -> bool {
        let action = self.action == "*" || self.action == request.action
            || self.action.strip_suffix('*').is_some_and(|prefix| request.action.starts_with(prefix));
        action && [
            (&self.namespace, request.namespace), (&self.collection, request.collection),
            (&self.profile, request.profile), (&self.codec, request.codec), (&self.root, request.root),
        ].iter().all(|(expected, actual)| expected.as_deref().is_none_or(|v| Some(v) == *actual))
            && (!self.own_jobs_only || request.job_owner == Some(identity))
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "server", derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema))]
#[cfg_attr(feature = "server", serde(deny_unknown_fields))]
pub struct Role {
    pub id: String,
    pub allow: Vec<Rule>,
    #[cfg_attr(feature = "server", serde(default))]
    pub deny: Vec<Rule>,
    pub limits: Option<ResourceCaps>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "server", derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema))]
#[cfg_attr(feature = "server", serde(deny_unknown_fields))]
pub struct Binding {
    pub identity: Identity,
    pub roles: Vec<String>,
    pub limits: Option<ResourceCaps>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "server", derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema))]
#[cfg_attr(feature = "server", serde(deny_unknown_fields))]
pub struct Policy {
    pub version: u32,
    pub limits: ResourceCaps,
    pub roles: Vec<Role>,
    pub bindings: Vec<Binding>,
}

/// Resource identity must be resolved by the trusted host, including the
/// actual job owner, codec/profile and root. Request headers are not evidence.
#[derive(Clone, Copy, Debug)]
pub struct AccessRequest<'a> {
    pub action: &'a str,
    pub tenant: &'a str,
    pub namespace: Option<&'a str>,
    pub collection: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub codec: Option<&'a str>,
    pub root: Option<&'a str>,
    pub job_owner: Option<&'a Identity>,
    pub limits: ResourceCaps,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Decision {
    /// Local snapshot generation, not a portable bearer capability.
    pub generation: u64,
    pub limits: ResourceCaps,
}

impl Policy {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 { return Err(Error::InvalidConfiguration("policy_version")); }
        self.limits.validate()?;
        let mut ids = BTreeSet::new();
        for role in &self.roles {
            if !valid_identity_part(&role.id) || !ids.insert(role.id.as_str()) {
                return Err(Error::InvalidConfiguration("role_id"));
            }
            for rule in role.allow.iter().chain(&role.deny) { rule.validate()?; }
            if let Some(limits) = role.limits { limits.validate()?; }
        }
        let mut identities = BTreeSet::new();
        for binding in &self.bindings {
            binding.identity.validate()?;
            if !identities.insert(&binding.identity) {
                return Err(Error::InvalidConfiguration("duplicate_binding"));
            }
            let mut assigned = BTreeSet::new();
            for role in &binding.roles {
                if !ids.contains(role.as_str()) || !assigned.insert(role) {
                    return Err(Error::InvalidConfiguration("binding_role"));
                }
            }
            if let Some(limits) = binding.limits { limits.validate()?; }
        }
        Ok(())
    }

    /// Evaluate only a validated snapshot. Runtime validates before installing
    /// it; direct callers should validate after constructing/changing policy.
    pub fn authorize(&self, identity: &Identity, request: &AccessRequest<'_>) -> Result<ResourceCaps> {
        if !valid_action(request.action, false) { return Err(Error::InvalidRequest("action")); }
        if identity.tenant != request.tenant { return Err(Error::PermissionDenied); }
        let binding = self.bindings.iter().find(|b| &b.identity == identity)
            .ok_or(Error::PermissionDenied)?;
        let mut effective = binding.limits.map_or(self.limits, |c| self.limits.intersect(c));
        let mut allowed = false;
        for id in &binding.roles {
            let role = self.roles.iter().find(|r| &r.id == id)
                .ok_or(Error::InvalidConfiguration("binding_role"))?;
            if role.deny.iter().any(|rule| rule.matches(identity, request)) {
                return Err(Error::PermissionDenied);
            }
            if role.allow.iter().any(|rule| rule.matches(identity, request)) {
                allowed = true;
                if let Some(limits) = role.limits { effective = effective.intersect(limits); }
            }
        }
        if !allowed { return Err(Error::PermissionDenied); }
        request.limits.within(effective)?;
        Ok(request.limits)
    }
}
