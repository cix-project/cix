use super::*;
use std::time::Duration;

fn caps() -> ResourceCaps {
    ResourceCaps { input_bytes: 1024, output_bytes: 2048, memory_bytes: 4096,
        temporary_bytes: 0, workers: 2, deadline_ms: 1000 }
}
fn identity() -> Identity {
    Identity { provider: "host".into(), subject: "Zoë-日本語".into(), tenant: "acme".into() }
}
fn policy() -> Policy {
    Policy { version: 1, limits: caps(),
        roles: vec![Role { id: "compressor".into(), allow: vec![Rule::action("compress")], deny: vec![], limits: None }],
        bindings: vec![Binding { identity: identity(), roles: vec!["compressor".into()], limits: None }] }
}
fn request(action: &str) -> AccessRequest<'_> {
    AccessRequest { action, tenant: "acme", namespace: None, collection: None,
        profile: None, codec: None, root: None, job_owner: None, limits: caps() }
}

#[test]
fn default_denial_and_exact_identity_binding() {
    let p = policy();
    p.validate().unwrap();
    assert!(p.authorize(&identity(), &request("compress")).is_ok());
    assert_eq!(p.authorize(&identity(), &request("decompress")), Err(Error::PermissionDenied));
    for changed in [Identity { provider: "client-header".into(), ..identity() },
        Identity { subject: "zoë-日本語".into(), ..identity() },
        Identity { tenant: "other".into(), ..identity() }]
    {
        assert_eq!(p.authorize(&changed, &request("compress")), Err(Error::PermissionDenied));
    }
    let mut cross_tenant = request("compress");
    cross_tenant.tenant = "other";
    assert_eq!(p.authorize(&identity(), &cross_tenant), Err(Error::PermissionDenied));
}

#[test]
fn explicit_denial_beats_a_grant_in_any_role_order() {
    let mut p = policy();
    p.roles.push(Role { id: "deny".into(), allow: vec![], deny: vec![Rule::action("compress")], limits: None });
    p.bindings[0].roles.push("deny".into());
    for _ in 0..2 {
        p.validate().unwrap();
        assert_eq!(p.authorize(&identity(), &request("compress")), Err(Error::PermissionDenied));
        p.bindings[0].roles.reverse();
    }
}

#[test]
fn strictest_applicable_limits_and_no_temporary_storage() {
    let mut p = policy();
    p.roles[0].limits = Some(ResourceCaps { workers: 1, ..caps() });
    p.bindings[0].limits = Some(ResourceCaps { input_bytes: 512, ..caps() });
    p.validate().unwrap();
    assert_eq!(p.authorize(&identity(), &request("compress")), Err(Error::ResourceLimit("input_bytes")));
    let mut r = request("compress");
    r.limits.input_bytes = 512;
    assert_eq!(p.authorize(&identity(), &r), Err(Error::ResourceLimit("workers")));
    r.limits.workers = 1;
    assert!(p.authorize(&identity(), &r).is_ok());
    r.limits.temporary_bytes = 1;
    assert_eq!(p.authorize(&identity(), &r), Err(Error::ResourceLimit("temporary_bytes")));
}

#[test]
fn wildcard_requires_component_boundary() {
    let mut p = policy();
    p.roles[0].allow = vec![Rule::action("jobs:*")];
    p.validate().unwrap();
    assert!(p.authorize(&identity(), &request("jobs:read")).is_ok());
    for action in ["jobs", "jobs_other:read", "admin:jobs:read"] {
        assert_eq!(p.authorize(&identity(), &request(action)), Err(Error::PermissionDenied));
    }
    assert_eq!(p.authorize(&identity(), &request("jobs:*")), Err(Error::InvalidRequest("action")));
}

#[test]
fn resource_selectors_and_actual_job_owner_are_required() {
    let mut p = policy();
    p.roles[0].allow = vec![Rule { action: "jobs:cancel".into(), namespace: Some("reports".into()),
        collection: Some("2026".into()), profile: Some("fast".into()), codec: Some("native".into()),
        root: Some("output".into()), own_jobs_only: true }];
    let user = identity();
    let other = Identity { subject: "other".into(), ..identity() };
    let mut r = request("jobs:cancel");
    r.namespace = Some("reports"); r.collection = Some("2026"); r.profile = Some("fast");
    r.codec = Some("native"); r.root = Some("output"); r.job_owner = Some(&user);
    assert!(p.authorize(&user, &r).is_ok());
    r.job_owner = Some(&other);
    assert_eq!(p.authorize(&user, &r), Err(Error::PermissionDenied));
    r.job_owner = Some(&user); r.root = Some("output/../private");
    assert_eq!(p.authorize(&user, &r), Err(Error::PermissionDenied));
}

#[test]
fn invalid_policy_never_replaces_active_snapshot() {
    let rt = Runtime::new(policy()).unwrap();
    let session = rt.session_for_verified_identity(identity(), Duration::from_secs(60)).unwrap();
    let before = session.authorize(&request("compress")).unwrap();
    let mut invalid = policy(); invalid.version = 2;
    assert_eq!(rt.reload(invalid), Err(Error::InvalidConfiguration("policy_version")));
    assert_eq!(session.authorize(&request("compress")).unwrap(), before);
    let mut revoked = policy(); revoked.bindings.clear();
    assert_eq!(rt.reload(revoked), Ok(2));
    assert_eq!(session.authorize(&request("compress")), Err(Error::PermissionDenied));
}

#[test]
fn expired_session_and_invalid_references_fail_closed() {
    let rt = Runtime::new(policy()).unwrap();
    assert!(matches!(rt.session_for_verified_identity(identity(), Duration::ZERO), Err(Error::SessionExpired)));
    let mut p = policy(); p.bindings[0].roles = vec!["missing".into()];
    assert_eq!(p.validate(), Err(Error::InvalidConfiguration("binding_role")));
    p = policy(); p.bindings.push(p.bindings[0].clone());
    assert_eq!(p.validate(), Err(Error::InvalidConfiguration("duplicate_binding")));
    p = policy(); p.roles[0].allow[0].action = "jobs*".into();
    assert_eq!(p.validate(), Err(Error::InvalidConfiguration("role_rule")));
}

#[test]
fn stable_errors_do_not_render_identity_or_secret_material() {
    let e = Error::PermissionDenied;
    assert_eq!(e.to_string(), "forbidden");
    assert_eq!(e.message_id(), "managed.error.forbidden");
}
