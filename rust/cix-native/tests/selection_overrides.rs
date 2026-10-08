//! Selector override invariants that must hold before any payload encoder is
//! called.  The selector is included directly because it is shared by the
//! CLI binary and is deliberately not a public library API.

#[path = "../src/selection.rs"]
mod selection;

use selection::{generate_candidates, profile, Candidate};

fn non_raw(candidates: &[Candidate]) -> impl Iterator<Item = &Candidate> {
    candidates.iter().filter(|candidate| candidate.route != 0)
}

#[test]
fn intrinsic_route_names_are_exact_and_raw_is_the_only_raw_candidate() {
    let features = profile(
        b"selector override fixture with repeated repeated bytes",
        64,
    );
    for (route, backend) in [
        (0, "raw"),
        (1, "runs"),
        (8, "ppm"),
        (9, "mixture"),
        (10, "deflate"),
    ] {
        let candidates = generate_candidates(&features, "maximum", Some(route), Some(backend))
            .unwrap_or_else(|error| panic!("route {route}, backend {backend}: {error}"));
        assert_eq!(
            candidates[0],
            Candidate {
                route: 0,
                backend: "raw".into(),
                parameter: None,
            }
        );
        let materialized: Vec<_> = non_raw(&candidates).collect();
        if route == 0 {
            assert!(materialized.is_empty());
        } else {
            assert!(!materialized.is_empty());
            assert!(materialized
                .iter()
                .all(|candidate| candidate.route == route && candidate.backend == backend));
        }
    }
}

#[test]
fn automatic_candidates_do_not_mislabel_intrinsic_encoders() {
    let features = profile(b"abcdabcdabcdabcd", 16);
    let candidates = generate_candidates(&features, "maximum", None, None).unwrap();
    for (route, backend) in [(1, "runs"), (8, "ppm"), (9, "mixture"), (10, "deflate")] {
        assert!(candidates
            .iter()
            .filter(|candidate| candidate.route == route)
            .all(|candidate| candidate.backend == backend));
        assert!(candidates.iter().any(|candidate| candidate.route == route));
    }
}

#[test]
fn forced_route_rejects_all_incompatible_backends() {
    let features = profile(b"route/backend validation fixture", 64);
    for (route, backend) in [
        (0, "hybrid"),
        (1, "range"),
        (2, "runs"),
        (8, "count-range"),
        (9, "deflate"),
        (10, "cix"),
    ] {
        let error = generate_candidates(&features, "maximum", Some(route), Some(backend))
            .expect_err("incompatible forced route/backend pair must fail");
        assert!(error.contains("incompatible"), "{error}");
    }
}

#[test]
fn global_backend_only_keeps_compatible_routes_and_always_keeps_raw() {
    let features = profile(b"abcdabcdabcdabcd", 16);

    let range = generate_candidates(&features, "maximum", None, Some("range")).unwrap();
    assert_eq!(range[0].route, 0);
    assert_eq!(range[0].backend, "raw");
    assert!(non_raw(&range)
        .all(|candidate| { (2..=7).contains(&candidate.route) && candidate.backend == "range" }));

    // PPM and mixture are not guaranteed to be on a cheap shortlist.  Their
    // unique intrinsic backend override must still produce the one compatible
    // route, and must never attach that backend to another route.
    for (backend, route) in [("ppm", 8), ("mixture", 9), ("runs", 1)] {
        let candidates = generate_candidates(&features, "fast", None, Some(backend)).unwrap();
        assert_eq!(candidates[0].route, 0);
        assert!(non_raw(&candidates)
            .all(|candidate| candidate.route == route && candidate.backend == backend));
        assert!(non_raw(&candidates).next().is_some());
    }

    // Deflate legitimately has both meanings: it remains a valid nested
    // backend for stream routes and the sole backend for the intrinsic route.
    let deflate = generate_candidates(&features, "maximum", None, Some("deflate")).unwrap();
    assert!(non_raw(&deflate).all(|candidate| {
        ((2..=7).contains(&candidate.route) || candidate.route == 10)
            && candidate.backend == "deflate"
    }));
    assert!(non_raw(&deflate).any(|candidate| candidate.route == 10));
}

#[test]
fn invalid_ids_backends_and_effort_fail_before_candidate_generation() {
    let features = profile(b"validation", 16);
    assert!(generate_candidates(&features, "maximum", Some(11), None).is_err());
    assert!(generate_candidates(&features, "maximum", None, Some("invented")).is_err());
    assert!(generate_candidates(&features, "unknown", Some(0), Some("raw")).is_err());
}
