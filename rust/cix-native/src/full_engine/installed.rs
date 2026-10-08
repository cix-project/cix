//! Explicit installed-provider layout discovery.
//!
//! This module only resolves paths and records what is present.  It never
//! searches `PATH` or environment variables, loads a DSO, starts a worker, or
//! creates the supplied temporary directory.  ABI/version qualification and
//! provider loading remain separate operations.

use super::{backend_provider::TrustedBackendPaths, selection::ProviderConfig};
use crate::paq_bridge::{PaqVariant, PAQ_VARIANTS};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

const JXL_BRIDGE_BASENAME: &str = "cix_jxl_bridge";
const SPATIAL_BRIDGE_BASENAME: &str = "cix_spatial_bridge";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstalledCapabilityKind {
    Paq(PaqVariant),
    JxlBridge,
    SpatialBridge,
}

/// One known installed artifact.  The fixed six-entry array in
/// [`InstalledProviders`] prevents an unbounded directory inventory from
/// becoming an accidental discovery/search mechanism.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledCapability {
    pub id: &'static str,
    pub kind: InstalledCapabilityKind,
    pub expected_path: PathBuf,
    pub available: bool,
    pub reason: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct InstalledProviders {
    pub config: ProviderConfig,
    pub capabilities: [InstalledCapability; 6],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstalledProviderError {
    ExecutableNotAbsolute(PathBuf),
    ExecutableUnavailable(PathBuf),
    ExecutableNotRegular(PathBuf),
    ExecutableHasNoPrefix(PathBuf),
    LibraryDirectoryNotAbsolute(PathBuf),
    TemporaryRootNotAbsolute(PathBuf),
}

impl fmt::Display for InstalledProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExecutableNotAbsolute(path) => {
                write!(f, "CIX executable path is not absolute: {}", path.display())
            }
            Self::ExecutableUnavailable(path) => write!(
                f,
                "CIX executable cannot be canonicalized: {}",
                path.display()
            ),
            Self::ExecutableNotRegular(path) => write!(
                f,
                "CIX executable is not a regular file: {}",
                path.display()
            ),
            Self::ExecutableHasNoPrefix(path) => write!(
                f,
                "CIX executable has no install prefix: {}",
                path.display()
            ),
            Self::LibraryDirectoryNotAbsolute(path) => write!(
                f,
                "CIX private library directory is not absolute: {}",
                path.display()
            ),
            Self::TemporaryRootNotAbsolute(path) => {
                write!(f, "CIX temporary root is not absolute: {}", path.display())
            }
        }
    }
}
impl std::error::Error for InstalledProviderError {}

/// Return the one package filename for a CIX-owned shared bridge on this target.
///
/// Callers supply a fixed CIX descriptor basename; this is deliberately not a
/// dynamic-library search helper.
pub(crate) fn shared_library_filename(basename: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        format!("{basename}.dll")
    }
    #[cfg(target_os = "macos")]
    {
        format!("lib{basename}.dylib")
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        format!("lib{basename}.so")
    }
}

fn regular(path: PathBuf, id: &'static str, kind: InstalledCapabilityKind) -> InstalledCapability {
    let available = fs::metadata(&path).is_ok_and(|metadata| metadata.is_file());
    InstalledCapability {
        id,
        kind,
        expected_path: path,
        available,
        reason: (!available).then_some("expected installed regular file is missing"),
    }
}

/// Resolve one installed CIX layout without loading any provider.
///
/// The canonical executable path is passed directly to a subsequent PAQ
/// worker, without a `PATH` search. Runtime binary freezing/replacement
/// protection is a separate execution concern. With no explicit directory, the
/// package layout is `<prefix>/bin/cix` plus `<prefix>/lib/cix`; an explicit
/// absolute directory is retained verbatim for multiarch/custom prefixes.
/// Missing bridge files are capabilities, not fallback triggers.
pub fn discover(
    executable: &Path,
    library_directory: Option<&Path>,
    temporary_root: &Path,
) -> Result<InstalledProviders, InstalledProviderError> {
    if !executable.is_absolute() {
        return Err(InstalledProviderError::ExecutableNotAbsolute(
            executable.to_path_buf(),
        ));
    }
    let executable = fs::canonicalize(executable)
        .map_err(|_| InstalledProviderError::ExecutableUnavailable(executable.to_path_buf()))?;
    if !fs::metadata(&executable).is_ok_and(|metadata| metadata.is_file()) {
        return Err(InstalledProviderError::ExecutableNotRegular(executable));
    }
    if !temporary_root.is_absolute() {
        return Err(InstalledProviderError::TemporaryRootNotAbsolute(
            temporary_root.to_path_buf(),
        ));
    }
    let library_directory = match library_directory {
        Some(path) if !path.is_absolute() => {
            return Err(InstalledProviderError::LibraryDirectoryNotAbsolute(
                path.to_path_buf(),
            ))
        }
        Some(path) => path.to_path_buf(),
        None => executable
            .parent()
            .and_then(Path::parent)
            .map(|prefix| prefix.join("lib").join("cix"))
            .ok_or_else(|| InstalledProviderError::ExecutableHasNoPrefix(executable.clone()))?,
    };

    let paq = PAQ_VARIANTS.map(|descriptor| {
        regular(
            library_directory.join(shared_library_filename(descriptor.library_basename)),
            match descriptor.variant {
                PaqVariant::V215 => "paq-v215",
                PaqVariant::V216 => "paq-v216",
                PaqVariant::JointDiscount => "paq-joint-discount",
                PaqVariant::StoreState => "paq-store-state",
            },
            InstalledCapabilityKind::Paq(descriptor.variant),
        )
    });
    let jxl = regular(
        library_directory.join(shared_library_filename(JXL_BRIDGE_BASENAME)),
        "jxl-bridge",
        InstalledCapabilityKind::JxlBridge,
    );
    let spatial = regular(
        library_directory.join(shared_library_filename(SPATIAL_BRIDGE_BASENAME)),
        "spatial-bridge",
        InstalledCapabilityKind::SpatialBridge,
    );
    let capabilities = [
        paq[0].clone(),
        paq[1].clone(),
        paq[2].clone(),
        paq[3].clone(),
        jxl.clone(),
        spatial.clone(),
    ];
    Ok(InstalledProviders {
        config: ProviderConfig {
            backends: TrustedBackendPaths {
                cix: executable,
                paq_libraries: library_directory,
                temporary_root: temporary_root.to_path_buf(),
            },
            jxl_bridge: jxl.available.then_some(jxl.expected_path),
            spatial_bridge: spatial.available.then_some(spatial.expected_path),
        },
        capabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "cix-installed-discovery-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("fixture root");
            Self { root }
        }
        fn file(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture parent");
            fs::write(&path, b"").expect("fixture file");
            path
        }
        fn directory(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(&path).expect("fixture directory");
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn staged_library_files(fixture: &Fixture, directory: &str) {
        for descriptor in PAQ_VARIANTS {
            fixture.file(&format!(
                "{directory}/{}",
                shared_library_filename(descriptor.library_basename)
            ));
        }
        fixture.file(&format!(
            "{directory}/{}",
            shared_library_filename(JXL_BRIDGE_BASENAME)
        ));
        fixture.file(&format!(
            "{directory}/{}",
            shared_library_filename(SPATIAL_BRIDGE_BASENAME)
        ));
    }

    #[test]
    fn names_are_platform_specific_without_searching() {
        let name = shared_library_filename("cix_paq_v215_bridge");
        assert!(name.contains("cix_paq_v215_bridge"));
        assert!(name.ends_with(".so") || name.ends_with(".dylib") || name.ends_with(".dll"));
    }

    #[test]
    fn default_layout_records_each_exact_file_and_never_creates_temp_root() {
        let fixture = Fixture::new();
        let executable = fixture.file("prefix/bin/cix");
        staged_library_files(&fixture, "prefix/lib/cix");
        let temporary_root = fixture.root.join("not-created-by-discovery");

        let providers = discover(&executable, None, &temporary_root).expect("installed layout");
        assert_eq!(
            providers.config.backends.cix,
            fs::canonicalize(&executable).unwrap()
        );
        assert_eq!(
            providers.config.backends.paq_libraries,
            fixture.root.join("prefix/lib/cix")
        );
        assert_eq!(providers.capabilities.len(), 6);
        assert!(providers
            .capabilities
            .iter()
            .all(|capability| capability.available));
        assert!(providers.config.jxl_bridge.is_some());
        assert!(providers.config.spatial_bridge.is_some());
        assert!(!temporary_root.exists());

        fs::remove_file(
            fixture
                .root
                .join("prefix/lib/cix")
                .join(shared_library_filename(SPATIAL_BRIDGE_BASENAME)),
        )
        .unwrap();
        let missing =
            discover(&executable, None, &temporary_root).expect("partial installed layout");
        let spatial = missing
            .capabilities
            .iter()
            .find(|capability| capability.id == "spatial-bridge")
            .unwrap();
        assert!(!spatial.available);
        assert_eq!(
            spatial.reason,
            Some("expected installed regular file is missing")
        );
        assert!(missing.config.spatial_bridge.is_none());
        assert!(missing.config.jxl_bridge.is_some());
        assert!(missing
            .capabilities
            .iter()
            .filter(|capability| capability.id != "spatial-bridge")
            .all(|capability| capability.available));
    }

    #[test]
    fn explicit_library_directory_never_falls_back_to_default_layout() {
        let fixture = Fixture::new();
        let executable = fixture.file("prefix/bin/cix");
        staged_library_files(&fixture, "prefix/lib/cix");
        let explicit = fixture.directory("custom/lib/x86_64-linux-gnu/cix");
        let temporary_root = fixture.root.join("private-temp");

        let providers =
            discover(&executable, Some(&explicit), &temporary_root).expect("explicit layout");
        assert_eq!(providers.config.backends.paq_libraries, explicit);
        assert!(providers
            .capabilities
            .iter()
            .all(|capability| !capability.available));
        assert!(providers.config.jxl_bridge.is_none());
        assert!(providers.config.spatial_bridge.is_none());
        assert!(!temporary_root.exists());
    }

    #[test]
    fn executable_and_relative_path_failures_are_explicit() {
        let fixture = Fixture::new();
        let executable = fixture.file("prefix/bin/cix");
        let temporary_root = fixture.root.join("private-temp");
        let missing = fixture.root.join("prefix/bin/missing-cix");
        assert!(matches!(
            discover(&missing, None, &temporary_root),
            Err(InstalledProviderError::ExecutableUnavailable(_))
        ));
        let directory = fixture.directory("prefix/bin/not-a-file");
        assert!(matches!(
            discover(&directory, None, &temporary_root),
            Err(InstalledProviderError::ExecutableNotRegular(_))
        ));
        assert!(matches!(
            discover(
                &executable,
                Some(Path::new("relative-lib")),
                &temporary_root
            ),
            Err(InstalledProviderError::LibraryDirectoryNotAbsolute(_))
        ));
        assert!(matches!(
            discover(&executable, None, Path::new("relative-temp")),
            Err(InstalledProviderError::TemporaryRootNotAbsolute(_))
        ));
        assert!(matches!(
            discover(Path::new("cix"), None, &temporary_root),
            Err(InstalledProviderError::ExecutableNotAbsolute(_))
        ));
    }
}
