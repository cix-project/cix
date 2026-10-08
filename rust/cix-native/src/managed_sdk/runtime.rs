use super::{AccessRequest, Decision, Error, Identity, Policy, Result};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

struct Snapshot { generation: u64, policy: Policy }

/// An explicit host-owned policy context. Clones share atomic policy reloads.
#[derive(Clone)]
pub struct Runtime { snapshot: Arc<RwLock<Snapshot>> }

impl Runtime {
    pub fn new(policy: Policy) -> Result<Self> {
        policy.validate()?;
        Ok(Self { snapshot: Arc::new(RwLock::new(Snapshot { generation: 1, policy })) })
    }

    /// A bad replacement has no effect. No callback or I/O runs under the lock.
    pub fn reload(&self, policy: Policy) -> Result<u64> {
        policy.validate()?;
        let mut state = self.snapshot.write().map_err(|_| Error::Internal)?;
        let generation = state.generation.checked_add(1).ok_or(Error::Internal)?;
        *state = Snapshot { generation, policy };
        Ok(generation)
    }

    /// Only the embedding host may call this after authenticating the caller.
    /// This is not an authentication protocol or an untrusted-input endpoint.
    pub fn session_for_verified_identity(&self, identity: Identity, lifetime: Duration) -> Result<Session> {
        identity.validate()?;
        if lifetime.is_zero() { return Err(Error::SessionExpired); }
        let expires = Instant::now().checked_add(lifetime)
            .ok_or(Error::InvalidRequest("session_lifetime"))?;
        Ok(Session { runtime: self.clone(), identity, expires })
    }
}

/// Sessions retain identity, never a cached list of granted permissions.
pub struct Session { runtime: Runtime, identity: Identity, expires: Instant }

impl Session {
    pub fn identity(&self) -> &Identity { &self.identity }

    /// Call at each operation boundary. A decision cannot authorize later
    /// operations after reload. The host owns admission, execution and safe
    /// cancellation of already-running work; this method performs no codec I/O.
    pub fn authorize(&self, request: &AccessRequest<'_>) -> Result<Decision> {
        if Instant::now() >= self.expires { return Err(Error::SessionExpired); }
        let state = self.runtime.snapshot.read().map_err(|_| Error::Internal)?;
        let limits = state.policy.authorize(&self.identity, request)?;
        Ok(Decision { generation: state.generation, limits })
    }
}
