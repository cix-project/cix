use cix_native::paq_bridge::{
    validate, PaqBridgeError, PaqExecutionScope, PaqInvocation, PaqVariant, PaqWorkerEnvironment,
    PAQ_VARIANTS,
};
use std::ffi::OsString;
use std::path::PathBuf;

fn request() -> PaqInvocation {
    PaqInvocation {
        scope: PaqExecutionScope::IsolatedWorker,
        variant: PaqVariant::V215,
        argv: vec![OsString::from("paq8px-v215"), OsString::from("-8")],
        input: PathBuf::from("missing-input"),
        output: PathBuf::from("fresh-output"),
        worker_environment: PaqWorkerEnvironment {
            entries: Vec::new(),
        },
        memory_limit_bytes: 1,
    }
}

#[test]
fn rejects_sdk_scope_before_any_codec_load() {
    let mut value = request();
    value.scope = PaqExecutionScope::NativeSdk;
    assert_eq!(validate(&value), Err(PaqBridgeError::NotSdkSafe));
}

#[test]
fn all_retained_variants_have_distinct_worker_bridge_identities() {
    assert_eq!(PAQ_VARIANTS.len(), 4);
    for descriptor in PAQ_VARIANTS {
        assert!(descriptor.worker_only);
        assert!(descriptor.library_basename.starts_with("cix_paq_"));
        assert!(descriptor.process_symbol.ends_with("_process"));
        assert_eq!(descriptor.variant.descriptor(), Some(&descriptor));
    }
}

#[test]
fn requires_worker_memory_and_a_real_input_before_the_ffi_boundary() {
    let mut value = request();
    value.memory_limit_bytes = 0;
    assert!(matches!(
        validate(&value),
        Err(PaqBridgeError::InvalidArgument(_))
    ));
    value.memory_limit_bytes = 1;
    assert_eq!(
        validate(&value),
        Err(PaqBridgeError::MissingInput(PathBuf::from("missing-input")))
    );
}
