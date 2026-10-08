//! Process-free, transport-independent policy for managed CIX integrations.
//!
//! The embedding host authenticates identities and supplies explicit policy.
//! Nothing here reads environment variables/files, starts threads, opens a
//! listener, or treats a caller-provided identity as authentication evidence.
//! The low-level codec API remains available separately; native host code is
//! inside the trust boundary and can deliberately bypass this facade.

mod error;
mod policy;
mod runtime;

pub use error::{Error, Result};
pub use policy::{AccessRequest, Binding, Decision, Identity, Policy, ResourceCaps, Role, Rule};
pub use runtime::{Runtime, Session};

#[cfg(test)]
mod tests;
