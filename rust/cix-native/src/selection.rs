//! CIXG byte profiler and bounded effort-controlled route shortlist.
//!
//! The original byte features mirror `cix.pipeline` at reference commit
//! `80b95427725ecf59d626b2982d50f6349abd56e1`. Probe adjacency is local to
//! each sampled region, while repeat and context state are shared between
//! regions, matching the Python implementation.

use std::collections::{HashMap, HashSet};

pub const ROUTES: [&str; 11] = [
    "raw",
    "runs",
    "composition",
    "lz",
    "predictor",
    "bwt",
    "stride",
    "phrases",
    "ppm",
    "mixture",
    "deflate",
];

pub const LAGS: [usize; 7] = [1, 2, 4, 8, 16, 32, 64];

/// One materializable CIXG1 encoding candidate. The representation and
/// backend are independent dimensions; future route parameters and packaging
/// can extend this spec without changing the CLI effort names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub route: u8,
    pub backend: String,
    /// Route-specific, self-describing parameter carried inside the payload.
    /// PPM order is stored in its payload header and checked against the
    /// decoder's versioned order bound.
    pub parameter: Option<u8>,
}

fn baseline_backend(route: u8) -> &'static str {
    match route {
        0 => "raw",
        1 => "runs",
        2 => "cix",
        8 => "ppm",
        9 => "mixture",
        10 => "deflate",
        _ => "hybrid",
    }
}

/// Backends that encode the generic byte substreams used by routes 2 through
/// 7.  These names must never be attached to an intrinsic route: those
/// encoders do not consume a nested byte-stream backend, so doing so would
/// make `--explain` and archive-selection records lie about what was written.
fn is_nested_backend(backend: &str) -> bool {
    matches!(
        backend,
        "cix"
            | "deflate"
            | "hybrid"
            | "range"
            | "count-range"
            | "context-range-o1-b4"
            | "context-range-o1-b8"
            | "context-range-o2-b8"
            | "huffman"
    )
}

/// The name recorded for each intrinsic route is part of the selector's
/// observable decision.  `deflate` is also a legal nested backend, but is
/// intrinsic only for route 10.
fn route_supports_backend(route: u8, backend: &str) -> bool {
    match route {
        0 => backend == "raw",
        1 => backend == "runs",
        2..=7 => is_nested_backend(backend),
        8 => backend == "ppm",
        9 => backend == "mixture",
        10 => backend == "deflate",
        _ => false,
    }
}

fn is_known_backend(backend: &str) -> bool {
    is_nested_backend(backend) || matches!(backend, "raw" | "runs" | "ppm" | "mixture")
}

fn intrinsic_route_for_backend(backend: &str) -> Option<u8> {
    match backend {
        "raw" => Some(0),
        "runs" => Some(1),
        "ppm" => Some(8),
        "mixture" => Some(9),
        // Deflate has both an intrinsic route and a nested byte-stream use.
        "deflate" => Some(10),
        _ => None,
    }
}

fn nested_backends(route: u8, effort_name: &str) -> &'static [&'static str] {
    if !(2..=7).contains(&route) {
        &[]
    } else {
        match effort_name {
            "fast" => &[],
            "balanced" => &["hybrid", "cix", "deflate"],
            // Context-range and Huffman are versioned CIXG2 substream
            // coders, not predictor-specific coders.  BEST must generate
            // each legal route (composition through phrases) × backend pair;
            // the existing worker memory permits and deadlines then prune
            // unaffordable trials with observable omissions.
            _ => &[
                "cix",
                "deflate",
                "hybrid",
                "range",
                "count-range",
                "context-range-o1-b4",
                "context-range-o1-b8",
                "context-range-o2-b8",
                "huffman",
            ],
        }
    }
}

/// Generate an ordered, finite CIXG1 candidate set. Python-reference route
/// order comes first; extension backends follow so strict-size ties preserve
/// the reference winner. `forced_route` and `forced_backend` constrain only
/// their named dimensions.
pub fn generate_candidates(
    features: &Features,
    effort_name: &str,
    forced_route: Option<u8>,
    forced_backend: Option<&str>,
) -> Result<Vec<Candidate>, String> {
    // Validate every externally supplied dimension here, rather than relying
    // on a caller's CLI parser.  Library callers and future frontends must
    // get the same compatibility errors and cannot manufacture a mislabeled
    // candidate.
    validate_constraints(effort_name, forced_route, forced_backend)?;
    let routes = candidate_routes(features, effort_name, forced_route, forced_backend)?;
    let mut candidates = vec![Candidate {
        route: 0,
        backend: "raw".into(),
        parameter: None,
    }];
    candidates.extend(baseline_candidates(&routes, effort_name, forced_backend));
    append_nested_candidates(&mut candidates, &routes, effort_name, forced_backend);
    // PPM order is independently serialized and the current CIXG1 decoder
    // supports orders 0..=8. Broader profiles include the complete legal
    // native order range; the execution budget decides how much is evaluated.
    append_ppm_candidates(&mut candidates, &routes, effort_name)?;
    Ok(candidates)
}

fn validate_constraints(
    effort_name: &str,
    forced_route: Option<u8>,
    forced_backend: Option<&str>,
) -> Result<(), String> {
    if effort(effort_name).is_none() {
        return Err(format!("unknown CIXG1 effort profile: {effort_name}"));
    }
    if forced_route.is_some_and(|route| usize::from(route) >= ROUTES.len()) {
        return Err(format!(
            "unsupported native route ID {}",
            forced_route.unwrap()
        ));
    }
    let Some(backend) = forced_backend else {
        return Ok(());
    };
    if !is_known_backend(backend) {
        return Err(format!("unsupported CIXG1 backend override: {backend}"));
    }
    if let Some(route) = forced_route {
        if !route_supports_backend(route, backend) {
            return Err(format!(
                "backend {backend} is incompatible with intrinsic route {}",
                ROUTES[route as usize]
            ));
        }
    }
    Ok(())
}

fn candidate_routes(
    features: &Features,
    effort_name: &str,
    forced_route: Option<u8>,
    forced_backend: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut routes = match forced_route {
        Some(route) => vec![route],
        None => shortlisted_route_ids(features, effort_name)?,
    };
    if let Some(backend) = forced_backend {
        add_intrinsic_route(&mut routes, backend, forced_route);
        routes.retain(|&route| route_supports_backend(route, backend));
    }
    Ok(routes)
}

fn shortlisted_route_ids(features: &Features, effort_name: &str) -> Result<Vec<u8>, String> {
    shortlist(features, effort_name)?
        .iter()
        .map(|name| {
            ROUTES
                .iter()
                .position(|candidate| candidate == name)
                .map(|id| id as u8)
                .ok_or_else(|| format!("unknown route {name}"))
        })
        .collect()
}

fn add_intrinsic_route(routes: &mut Vec<u8>, backend: &str, forced_route: Option<u8>) {
    if forced_route.is_none() {
        if let Some(route) = intrinsic_route_for_backend(backend) {
            if !routes.contains(&route) {
                routes.push(route);
            }
        }
    }
}

fn baseline_candidates(
    routes: &[u8],
    effort_name: &str,
    forced_backend: Option<&str>,
) -> Vec<Candidate> {
    routes
        .iter()
        .copied()
        .filter(|&route| route != 0)
        .map(|route| Candidate {
            route,
            backend: baseline_for(route, effort_name, forced_backend),
            parameter: (route == 8).then_some(3),
        })
        .collect()
}

fn baseline_for(route: u8, effort_name: &str, forced_backend: Option<&str>) -> String {
    forced_backend.map(str::to_owned).unwrap_or_else(|| {
        if effort_name == "fast" && (2..=7).contains(&route) {
            "deflate".into()
        } else {
            baseline_backend(route).into()
        }
    })
}

fn append_nested_candidates(
    candidates: &mut Vec<Candidate>,
    routes: &[u8],
    effort_name: &str,
    forced_backend: Option<&str>,
) {
    if forced_backend.is_some() {
        return;
    }
    for &route in routes {
        for &backend in nested_backends(route, effort_name) {
            let candidate = Candidate {
                route,
                backend: backend.into(),
                parameter: None,
            };
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
}

fn append_ppm_candidates(
    candidates: &mut Vec<Candidate>,
    routes: &[u8],
    effort_name: &str,
) -> Result<(), String> {
    if !routes.contains(&8) {
        return Ok(());
    }
    for &order in ppm_orders(effort_name)? {
        let candidate = Candidate {
            route: 8,
            backend: "ppm".into(),
            parameter: Some(order),
        };
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    Ok(())
}

fn ppm_orders(effort_name: &str) -> Result<&'static [u8], String> {
    match effort_name {
        "fast" => Ok(&[2]),
        "balanced" => Ok(&[1, 2, 3]),
        "maximum" => Ok(&[0, 1, 2, 3, 4, 5, 6, 7, 8]),
        _ => Err(format!("unknown CIXG1 effort profile: {effort_name}")),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effort {
    pub block_bytes: usize,
    pub sample_bytes: usize,
    pub candidate_limit: usize,
    pub search_seconds: f64,
    pub trial_seconds: f64,
    pub worker_memory_mib: usize,
}

pub const FAST: Effort = Effort {
    block_bytes: 65_536,
    sample_bytes: 2_048,
    candidate_limit: 4,
    search_seconds: 0.75,
    trial_seconds: 0.30,
    worker_memory_mib: 256,
};

pub const BALANCED: Effort = Effort {
    block_bytes: 32_768,
    sample_bytes: 4_096,
    candidate_limit: 7,
    search_seconds: 3.0,
    trial_seconds: 1.0,
    worker_memory_mib: 384,
};

pub const MAXIMUM: Effort = Effort {
    block_bytes: 65_536,
    sample_bytes: 8_192,
    candidate_limit: 10,
    search_seconds: 12.0,
    trial_seconds: 4.0,
    worker_memory_mib: 512,
};

pub const EFFORTS: [(&str, Effort); 3] =
    [("fast", FAST), ("balanced", BALANCED), ("maximum", MAXIMUM)];

pub fn effort(name: &str) -> Option<Effort> {
    EFFORTS
        .iter()
        .find_map(|(candidate, config)| (*candidate == name).then_some(*config))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Features {
    pub sample_bytes: usize,
    pub entropy: f64,
    pub symbols: usize,
    pub run_fraction: f64,
    pub repeat_fraction: f64,
    /// Entropies correspond positionally to [`LAGS`].
    pub lag_entropy: [f64; LAGS.len()],
    pub predictability: f64,
    /// Number of causal two-byte context predictions observed in the bounded
    /// profile.  This distinguishes a stable prediction signal from a tiny
    /// sample with one accidental hit.
    pub prediction_opportunities: usize,
    pub regional_entropy_range: f64,
}

struct Histogram {
    counts: [usize; 256],
    first_seen: Vec<u8>,
    len: usize,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            counts: [0; 256],
            first_seen: Vec::new(),
            len: 0,
        }
    }
}

impl Histogram {
    fn observe(&mut self, value: u8) {
        let count = &mut self.counts[usize::from(value)];
        if *count == 0 {
            self.first_seen.push(value);
        }
        *count += 1;
        self.len += 1;
    }

    fn entropy(&self) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        let n = self.len as f64;
        let weighted_log_sum = self.first_seen.iter().fold(0.0, |sum, &value| {
            let count = self.counts[usize::from(value)] as f64;
            sum + count * count.log2()
        });
        n.log2() - weighted_log_sum / n
    }
}

fn entropy(data: &[u8]) -> f64 {
    let mut histogram = Histogram::default();
    for &value in data {
        histogram.observe(value);
    }
    histogram.entropy()
}

/// Profile bounded, spread samples of one input block.
///
/// `cap` is the frozen total sample budget. As in Python, each of up to four
/// regions has width `max(1, cap / 4)`, and overlapping regions are not merged.
pub fn profile(data: &[u8], cap: usize) -> Features {
    let parts = profile_parts(data, cap);
    let (sample, run_equal) = sample_stats(&parts);
    let (repeats, keys) = repeat_stats(&parts);
    let lag_entropy = lag_entropies(&parts);
    let (hits, opportunities) = prediction_stats(&parts);
    let regional_entropy_range = regional_entropy_range(&parts);
    Features {
        sample_bytes: sample.len,
        entropy: sample.entropy(),
        symbols: sample.first_seen.len(),
        run_fraction: run_equal as f64 / sample.len.saturating_sub(parts.len()).max(1) as f64,
        repeat_fraction: repeats as f64 / keys.max(1) as f64,
        lag_entropy,
        predictability: hits as f64 / opportunities.max(1) as f64,
        prediction_opportunities: opportunities,
        regional_entropy_range,
    }
}

fn profile_parts(data: &[u8], cap: usize) -> Vec<&[u8]> {
    let width = (cap / 4).max(1);
    let two_thirds = (data.len() / 3) * 2 + ((data.len() % 3) * 2) / 3;
    let mut starts = vec![
        0,
        data.len() / 3,
        two_thirds,
        data.len().saturating_sub(width),
    ];
    starts.sort_unstable();
    starts.dedup();

    starts
        .into_iter()
        .map(|start| &data[start..start.saturating_add(width).min(data.len())])
        .collect()
}

fn sample_stats(parts: &[&[u8]]) -> (Histogram, usize) {
    let mut sample = Histogram::default();
    let mut run_equal = 0usize;
    for part in parts {
        for &value in *part {
            sample.observe(value);
        }
        run_equal += part.windows(2).filter(|pair| pair[0] == pair[1]).count();
    }
    (sample, run_equal)
}

fn repeat_stats(parts: &[&[u8]]) -> (usize, usize) {
    let mut seen = HashSet::<[u8; 4]>::new();
    let mut repeats = 0usize;
    let mut keys = 0usize;
    for part in parts {
        for key in part.windows(4) {
            let key = [key[0], key[1], key[2], key[3]];
            repeats += usize::from(!seen.insert(key));
            keys += 1;
        }
    }
    (repeats, keys)
}

fn lag_entropies(parts: &[&[u8]]) -> [f64; LAGS.len()] {
    LAGS.map(|lag| {
        let mut residuals = Histogram::default();
        for part in parts {
            for index in lag..part.len() {
                residuals.observe(part[index] ^ part[index - lag]);
            }
        }
        residuals.entropy()
    })
}

fn prediction_stats(parts: &[&[u8]]) -> (usize, usize) {
    let mut contexts = HashMap::<[u8; 2], u8>::new();
    let mut hits = 0usize;
    let mut opportunities = 0usize;
    for part in parts {
        for index in 2..part.len() {
            let key = [part[index - 2], part[index - 1]];
            if let Some(&prediction) = contexts.get(&key) {
                opportunities += 1;
                hits += usize::from(prediction == part[index]);
            }
            contexts.insert(key, part[index]);
        }
    }
    (hits, opportunities)
}

fn regional_entropy_range(parts: &[&[u8]]) -> f64 {
    let first_regional_entropy = entropy(parts[0]);
    let (minimum_regional_entropy, maximum_regional_entropy) = parts[1..].iter().fold(
        (first_regional_entropy, first_regional_entropy),
        |(minimum, maximum), part| {
            let value = entropy(part);
            (minimum.min(value), maximum.max(value))
        },
    );

    maximum_regional_entropy - minimum_regional_entropy
}

/// Return the ordered candidate route names for an effort profile.
///
/// The DEFAULT PPM promotion is native selector experiment
/// `cixg1-ppm-predictability-v2`: a measured causal two-byte prediction
/// signal with at least 64 opportunities makes the affordable bounded PPM
/// route eligible before the default candidate cap.  It is deliberately
/// input-derived and leaves FAST and the complete BEST route set unchanged.
pub fn shortlist(features: &Features, effort_name: &str) -> Result<Vec<&'static str>, String> {
    let config = effort(effort_name)
        .ok_or_else(|| format!("unknown CIXG1 effort profile: {effort_name}"))?;

    let mut routes = vec!["runs", "deflate"];
    let mut priorities = Vec::with_capacity(12);
    if features.repeat_fraction > 0.01 {
        priorities.push("lz");
    }

    // Python's min() compares in insertion order. Spell this out so even a
    // caller-provided NaN has the same behavior as the frozen implementation.
    let mut minimum_lag_entropy = features.lag_entropy[0];
    for &value in &features.lag_entropy[1..] {
        if value < minimum_lag_entropy {
            minimum_lag_entropy = value;
        }
    }
    if minimum_lag_entropy < features.entropy - 0.20 {
        priorities.push("stride");
    }
    if features.predictability > 0.1 {
        priorities.push("predictor");
        if effort_name == "balanced" && features.prediction_opportunities >= 64 {
            priorities.push("ppm");
        }
    }
    if features.symbols < 96 || features.entropy < 5.5 {
        priorities.push("composition");
    }
    priorities.extend([
        "lz",
        "bwt",
        "stride",
        "predictor",
        "phrases",
        "composition",
        "ppm",
        "mixture",
    ]);

    for route in priorities {
        if !routes.contains(&route) {
            routes.push(route);
        }
    }
    routes.truncate(config.candidate_limit);
    Ok(routes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_near(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-12,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn empty_profile_matches_frozen_python() {
        let features = profile(&[], 0);
        assert_eq!(features.sample_bytes, 0);
        assert_eq!(features.symbols, 0);
        assert_eq!(features.entropy, 0.0);
        assert_eq!(features.run_fraction, 0.0);
        assert_eq!(features.repeat_fraction, 0.0);
        assert_eq!(features.lag_entropy, [0.0; LAGS.len()]);
        assert_eq!(features.predictability, 0.0);
        assert_eq!(features.prediction_opportunities, 0);
        assert_eq!(features.regional_entropy_range, 0.0);
    }

    #[test]
    fn spread_profile_matches_frozen_python_vector() {
        let features = profile(b"abcdabcdabcdabcd", 16);
        assert_eq!(features.sample_bytes, 16);
        assert_eq!(features.symbols, 4);
        assert_near(features.entropy, 2.0);
        assert_near(features.run_fraction, 0.0);
        assert_near(features.repeat_fraction, 0.25);
        assert_near(features.lag_entropy[0], 1.959_147_917_027_244_8);
        assert_near(features.lag_entropy[1], 1.0);
        assert_eq!(&features.lag_entropy[2..], &[0.0; 5]);
        assert_near(features.predictability, 1.0);
        assert_eq!(features.prediction_opportunities, 4);
        assert_near(features.regional_entropy_range, 0.0);
    }

    #[test]
    fn shortlist_preserves_priority_and_effort_order() {
        let features = profile(b"abcdabcdabcdabcd", 16);
        assert_eq!(
            shortlist(&features, "fast").unwrap(),
            ["runs", "deflate", "lz", "stride"]
        );
        assert_eq!(
            shortlist(&features, "maximum").unwrap(),
            [
                "runs",
                "deflate",
                "lz",
                "stride",
                "predictor",
                "composition",
                "bwt",
                "phrases",
                "ppm",
                "mixture",
            ]
        );
    }

    #[test]
    fn unpredictable_default_fallback_retains_frozen_order() {
        // This vector has no bounded prediction signal, so the native
        // `cixg1-ppm-predictability-v1` promotion does not apply.  It remains
        // a frozen-order regression only for that fallback case.
        let features = Features {
            sample_bytes: 256,
            entropy: 8.0,
            symbols: 256,
            run_fraction: 0.0,
            repeat_fraction: 0.0,
            lag_entropy: [8.0; LAGS.len()],
            predictability: 0.0,
            prediction_opportunities: 0,
            regional_entropy_range: 0.0,
        };
        assert_eq!(
            shortlist(&features, "balanced").unwrap(),
            [
                "runs",
                "deflate",
                "lz",
                "bwt",
                "stride",
                "predictor",
                "phrases",
            ]
        );
        assert!(shortlist(&features, "unknown").is_err());
    }

    #[test]
    fn candidate_generation_keeps_dimensions_independent() {
        let features = profile(b"abcdabcdabcdabcd", 16);
        let best = generate_candidates(&features, "maximum", None, None).unwrap();
        assert_eq!(best.first().unwrap().route, 0);
        assert!(best.iter().any(|item| item.route == 8));
        assert!(best
            .iter()
            .any(|item| item.route == 4 && item.backend == "count-range"));
        let ppm_orders: Vec<_> = best
            .iter()
            .filter(|item| item.route == 8)
            .filter_map(|item| item.parameter)
            .collect();
        assert_eq!(ppm_orders, [3, 0, 1, 2, 4, 5, 6, 7, 8]);
        assert!(best
            .iter()
            .any(|item| item.route == 4 && item.backend == "context-range-o1-b4"));
        assert!(best
            .iter()
            .any(|item| item.route == 4 && item.backend == "huffman"));
        for route in 2..=7 {
            for backend in [
                "context-range-o1-b4",
                "context-range-o1-b8",
                "context-range-o2-b8",
                "huffman",
            ] {
                assert!(
                    best.iter()
                        .any(|item| item.route == route && item.backend == backend),
                    "BEST omitted legal route {} × {backend}",
                    ROUTES[route as usize]
                );
            }
        }

        let balanced = generate_candidates(&features, "balanced", None, None).unwrap();
        assert!(!balanced
            .iter()
            .any(|item| item.backend.starts_with("context-range-")));
        assert!(!balanced.iter().any(|item| item.backend == "huffman"));

        let route_locked = generate_candidates(&features, "maximum", Some(4), None).unwrap();
        assert!(route_locked
            .iter()
            .all(|item| item.route == 4 || item.route == 0));
        assert!(route_locked
            .iter()
            .any(|item| item.backend == "count-range"));

        let backend_locked =
            generate_candidates(&features, "maximum", None, Some("range")).unwrap();
        assert!(backend_locked
            .iter()
            .filter(|item| item.route != 0)
            .all(|item| item.backend == "range"));
        assert!(!backend_locked.iter().any(|item| item.route == 8));
        assert!(generate_candidates(&features, "maximum", Some(8), Some("range")).is_err());

        let fast = generate_candidates(&features, "fast", Some(8), None).unwrap();
        assert!(fast.iter().all(|item| item.backend != "count-range"));
        assert!(fast.len() < best.len());
        assert_eq!(
            fast.iter()
                .filter(|item| item.route == 8)
                .filter_map(|item| item.parameter)
                .collect::<Vec<_>>(),
            [3, 2]
        );
    }

    #[test]
    fn high_entropy_repetition_still_admits_lz() {
        // Entropy is not a duplicate-detection veto.  This repeats all byte
        // values (high global entropy) and must retain the input-derived LZ
        // opportunity rather than treating entropy as compressed-data proof.
        let mut input: Vec<u8> = (0..=255).collect();
        input.extend(0..=255);
        let features = profile(&input, input.len());
        assert!(features.entropy > 7.9);
        assert!(features.repeat_fraction > 0.01);
        let candidates = generate_candidates(&features, "maximum", None, None).unwrap();
        assert!(candidates.iter().any(|candidate| candidate.route == 3));
    }
}
