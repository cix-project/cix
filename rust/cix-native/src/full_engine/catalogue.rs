//! Pure, deterministic specialist candidate discovery.
//!
//! The catalogue only describes content-derived recipes.  It does not start a
//! backend, select a winner, read a pathname, or rely on a corpus identity.
//! Dispatch owns the separate questions of codec availability, resource
//! admission, archive construction, and qualified selection.

use super::{
    address_relations::{self, ExecutableKind, HistoricalFrame},
    record_ordering,
    spatial_provider::SpatialCodec,
    strided_values,
    volume_lifting::{self, DicomRegion},
};

const MIB: usize = 1024 * 1024;
const BROTLI_MEMORY: usize = 256 * MIB;
const SPATIAL_MEMORY: usize = 1024 * MIB;
const VOLUME_MEMORY: usize = 1536 * MIB;
const FILTER_MEMORY_BASE: usize = 1_000_000_000;
const PAQ_MEMORY_BASE: usize = 4_500_000_000;

/// A CIX-owned transform recipe.  Backends and archive dispatch are deliberately
/// absent: this value contains only the information needed to make selection
/// reproducible from the input bytes and fixed release policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Recipe {
    Geometry {
        representation: u8,
    },
    Hydrogen,
    RecordConstraints {
        mode: u8,
        paq: bool,
    },
    FixedColumns,
    GroupedGrid,
    /// Retained bounded-grid route.  Dispatch decides whether its native
    /// encoder/backend is available for a particular build and resource cap.
    BoundedGrid {
        mode: u8,
    },
    Address {
        family: HistoricalFrame,
        paq: bool,
        mode: u8,
    },
    Spatial {
        region: DicomRegion,
        effort: u8,
    },
    SpatialAlternative {
        region: DicomRegion,
        codec: SpatialCodec,
    },
    Volume {
        region: DicomRegion,
        mode: u8,
        effort: u8,
    },
    VolumeSharedIdentity {
        region: DicomRegion,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub family: &'static str,
    pub recipe: Recipe,
    pub memory_bytes: usize,
}

/// A factual admission record.  `available` concerns whether a native recipe
/// is present in this build; it is not a compression qualification claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub family: &'static str,
    pub admitted: bool,
    pub available: bool,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalogue {
    pub candidates: Vec<Candidate>,
    pub diagnostics: Vec<Diagnostic>,
}

fn scaled_memory(base: usize, input_len: usize) -> Result<usize, String> {
    base.checked_add(
        input_len
            .checked_mul(8)
            .ok_or("specialist memory estimate overflow")?,
    )
    .ok_or_else(|| "specialist memory estimate overflow".into())
}

fn add(
    candidates: &mut Vec<Candidate>,
    id: &str,
    family: &'static str,
    recipe: Recipe,
    memory: usize,
) {
    candidates.push(Candidate {
        id: id.to_owned(),
        family,
        recipe,
        memory_bytes: memory,
    });
}

fn geometry_candidates(candidates: &mut Vec<Candidate>, input_len: usize) -> Result<(), String> {
    // This helper is only called after the caller's complete structured scan.
    // Its `input_len` is deliberately the original stream length because PAQ
    // reserves source copies as well as transformed material.
    let memory = scaled_memory(PAQ_MEMORY_BASE, input_len)?;
    add(
        candidates,
        "bonded-coordinate-palette-paq-v215-8",
        "chemical-geometry",
        Recipe::Geometry { representation: 2 },
        memory,
    );
    add(
        candidates,
        "bonded-coordinate-text-paq-v215-8",
        "chemical-geometry",
        Recipe::Geometry { representation: 1 },
        memory,
    );
    add(
        candidates,
        "chemical-hydrogen-paq-v215-8",
        "chemical-hydrogen",
        Recipe::Hydrogen,
        memory,
    );
    Ok(())
}

fn add_database_candidates(
    candidates: &mut Vec<Candidate>,
    input_len: usize,
) -> Result<(), String> {
    for mode in 0..=4 {
        add(
            candidates,
            &format!("database-constraints-{mode}"),
            "database-constraints",
            Recipe::RecordConstraints { mode, paq: false },
            BROTLI_MEMORY,
        );
    }
    add(
        candidates,
        "database-remaining-count-paq-v215-8",
        "database-remaining-count",
        Recipe::RecordConstraints { mode: 3, paq: true },
        scaled_memory(PAQ_MEMORY_BASE, input_len)?,
    );
    add(
        candidates,
        "database-implied-width",
        "database-fixed-columns",
        Recipe::FixedColumns,
        BROTLI_MEMORY,
    );
    Ok(())
}

fn add_grid_candidates(candidates: &mut Vec<Candidate>, input_len: usize) -> Result<(), String> {
    let memory = scaled_memory(PAQ_MEMORY_BASE, input_len)?;
    add(
        candidates,
        "grouped-prediction-context3-paq-v215-8",
        "grouped-grid",
        Recipe::GroupedGrid,
        memory,
    );
    for mode in 1..=4 {
        add(
            candidates,
            &format!("bounded-grid-mode{mode}-paq-v215-8"),
            "bounded-grid",
            Recipe::BoundedGrid { mode },
            memory,
        );
    }
    Ok(())
}

fn add_dicom_candidates(candidates: &mut Vec<Candidate>, regions: &[DicomRegion]) {
    for (index, region) in regions.iter().cloned().enumerate() {
        add(
            candidates,
            &format!("image-jxl-effort7-{index}"),
            "spatial-image",
            Recipe::Spatial {
                region: region.clone(),
                effort: 7,
            },
            SPATIAL_MEMORY,
        );
        add(
            candidates,
            &format!("image-jxl-effort10-{index}"),
            "spatial-image",
            Recipe::Spatial {
                region: region.clone(),
                effort: 10,
            },
            SPATIAL_MEMORY,
        );
        if region.height > region.slice_height {
            for mode in 0..=5 {
                add(
                    candidates,
                    &format!("volume-mode{mode}-effort9-{index}"),
                    "volume",
                    Recipe::Volume {
                        region: region.clone(),
                        mode,
                        effort: 9,
                    },
                    VOLUME_MEMORY,
                );
            }
            add(
                candidates,
                &format!("volume-mode3-effort10-shared-cixi2-{index}"),
                "volume-multiscale-identity",
                Recipe::VolumeSharedIdentity { region },
                VOLUME_MEMORY,
            );
        }
    }
    // Append newly restored alternatives after the established image/volume
    // recipes so earlier equal-size choices retain their original priority.
    for (index, region) in regions.iter().cloned().enumerate() {
        for (name, codec) in [
            ("jls", SpatialCodec::JpegLs),
            ("j2k", SpatialCodec::Jpeg2000),
        ] {
            add(
                candidates,
                &format!("image-{name}-{index}"),
                "spatial-image",
                Recipe::SpatialAlternative {
                    region: region.clone(),
                    codec,
                },
                SPATIAL_MEMORY,
            );
        }
    }
}

fn add_address_candidates(
    candidates: &mut Vec<Candidate>,
    input_len: usize,
    got: bool,
    pointer: bool,
    section: bool,
    filtered: bool,
    elf: bool,
) -> Result<(), String> {
    let paq = scaled_memory(PAQ_MEMORY_BASE, input_len)?;
    let filter = scaled_memory(FILTER_MEMORY_BASE, input_len)?;

    // Keep each family’s PAQ/Brotli pair adjacent: this is the retained Python
    // catalogue order, not a backend-grouped reordering.
    if got {
        add(
            candidates,
            "ecoff-got-mode3-paq",
            "executable-got",
            Recipe::Address {
                family: HistoricalFrame::Got,
                paq: true,
                mode: 3,
            },
            paq,
        );
        add(
            candidates,
            "ecoff-got-mode3-brotli",
            "executable-got",
            Recipe::Address {
                family: HistoricalFrame::Got,
                paq: false,
                mode: 3,
            },
            filter,
        );
    }
    if pointer {
        add(
            candidates,
            "ecoff-pointer-mode5-paq",
            "executable-pointer",
            Recipe::Address {
                family: HistoricalFrame::Pointer,
                paq: true,
                mode: 5,
            },
            paq,
        );
        add(
            candidates,
            "ecoff-pointer-mode5-brotli",
            "executable-pointer",
            Recipe::Address {
                family: HistoricalFrame::Pointer,
                paq: false,
                mode: 5,
            },
            filter,
        );
    }
    if section {
        add(
            candidates,
            "ecoff-section-mode4-paq",
            "executable-section",
            Recipe::Address {
                family: HistoricalFrame::Section,
                paq: true,
                mode: 4,
            },
            paq,
        );
        add(
            candidates,
            "ecoff-section-mode4-brotli",
            "executable-section",
            Recipe::Address {
                family: HistoricalFrame::Section,
                paq: false,
                mode: 4,
            },
            filter,
        );
    }
    if filtered {
        add(
            candidates,
            "ecoff-filter-mode0",
            "executable-filter",
            Recipe::Address {
                family: HistoricalFrame::Filtered,
                paq: false,
                mode: 0,
            },
            filter,
        );
        for mode in 1..=2 {
            add(
                candidates,
                &format!("ecoff-filter-mode{mode}"),
                "executable-filter",
                Recipe::Address {
                    family: HistoricalFrame::Filtered,
                    paq: true,
                    mode,
                },
                paq,
            );
        }
    }
    if elf || section {
        add(
            candidates,
            "executable-joint-discount",
            "executable-joint-discount",
            Recipe::Address {
                family: HistoricalFrame::JointDiscount,
                paq: true,
                mode: 2,
            },
            paq,
        );
    }
    // Append the restored ECRP1 pair without changing existing relative order.
    if pointer {
        for (id, use_paq, memory) in [
            ("relocated-pointer-mode5-paq", true, paq),
            ("relocated-pointer-mode5-brotli", false, filter),
        ] {
            add(
                candidates,
                id,
                "executable-relocated-pointer",
                Recipe::Address {
                    family: HistoricalFrame::Relocated,
                    paq: use_paq,
                    mode: 5,
                },
                memory,
            );
        }
    }
    Ok(())
}
/// Build the fixed-order, content-only specialist registry for `data`.
///
/// Candidate presence means the fixed content-only admission rule matched.  The
/// dispatch layer still performs exact transform construction and may reject an
/// inapplicable recipe without affecting catalogue determinism.
pub fn catalog(data: &[u8]) -> Result<Catalogue, String> {
    let mut candidates = Vec::new();
    let mut diagnostics = Vec::new();
    // The geometry encoder preserves non-SDF members literally, so retain the
    // historical marker admission rather than excluding a valid final record
    // which has no `$$$$\n` terminator.  This is content-only and its exact
    // record parser still validates a member when dispatch constructs it.
    let has_sdf = data.windows(5).any(|window| window == b"V2000")
        && data.windows(6).any(|window| window == b"M  END");
    if has_sdf {
        geometry_candidates(&mut candidates, data.len())?;
    }
    diagnostics.push(Diagnostic {
        family: "chemical",
        admitted: has_sdf,
        available: true,
        reason: if has_sdf {
            "sdf-v2000-marker; transform-validates-members"
        } else {
            "no-sdf-v2000-marker"
        },
    });

    let has_records = record_ordering::admit_mysql_records(data);
    if has_records {
        add_database_candidates(&mut candidates, data.len())?;
    }
    diagnostics.push(Diagnostic {
        family: "database",
        admitted: has_records,
        available: true,
        reason: if has_records {
            "myisam-header-admission"
        } else {
            "no-myisam-header-admission"
        },
    });

    let has_grid = strided_values::recognize_wcs_grid(data).is_some();
    if has_grid {
        add_grid_candidates(&mut candidates, data.len())?;
    }
    diagnostics.push(Diagnostic {
        family: "grid",
        admitted: has_grid,
        available: true,
        reason: if has_grid {
            "validated-wcstools-grid; bounded-grid-qualification-pending"
        } else {
            "no-wcstools-grid-admission"
        },
    });

    let dicom = volume_lifting::dicom_regions(data);
    if !dicom.is_empty() {
        add_dicom_candidates(&mut candidates, &dicom);
    }
    diagnostics.push(Diagnostic {
        family: "spatial-image",
        admitted: !dicom.is_empty(),
        available: true,
        reason: if dicom.is_empty() {
            "no-implicit-vr-dicom-admission"
        } else {
            "native-jxl-recipe-unqualified"
        },
    });
    diagnostics.push(Diagnostic {
        family: "spatial-jls",
        admitted: !dicom.is_empty(),
        available: true,
        reason: "native-jls-recipe-requires-installed-spatial-bridge",
    });
    diagnostics.push(Diagnostic {
        family: "spatial-j2k",
        admitted: !dicom.is_empty(),
        available: true,
        reason: "native-j2k-recipe-requires-installed-spatial-bridge",
    });

    // One bounded discovery pass handles both standalone and later TAR-member
    // objects.  It validates an ELF table before returning `Elf`; ordinary
    // bytes which merely begin with an ELF magic are not admitted.
    let executable = address_relations::discover_executable_regions(data);
    let has_elf = executable
        .iter()
        .any(|region| region.kind == ExecutableKind::Elf);
    let got = address_relations::eligible_got(data);
    let pointer = address_relations::eligible_pointer_deltas(data);
    let section = address_relations::eligible_section_pointers(data);
    let filtered = address_relations::eligible_filtered_pdata(data);
    add_address_candidates(
        &mut candidates,
        data.len(),
        got,
        pointer,
        section,
        filtered,
        has_elf,
    )?;
    for (family, admitted) in [
        ("executable-got", got),
        ("executable-pointer", pointer),
        ("executable-section", section),
        ("executable-filter", filtered),
        ("executable-elf", has_elf),
    ] {
        diagnostics.push(Diagnostic {
            family,
            admitted,
            available: true,
            reason: if admitted {
                "exact-native-transform-admission"
            } else {
                "no-exact-native-transform-admission"
            },
        });
    }

    Ok(Catalogue {
        candidates,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_member(name: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..name.len().min(100)].copy_from_slice(&name[..name.len().min(100)]);
        header[100..108].copy_from_slice(b"0000644\0");
        let size = format!("{:011o}\0", payload.len());
        header[124..136].copy_from_slice(size.as_bytes());
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        let encoded = format!("{:06o}\0 ", checksum);
        header[148..156].copy_from_slice(encoded.as_bytes());
        let mut tar = header.to_vec();
        tar.extend_from_slice(payload);
        tar.resize((tar.len() + 511) & !511, 0);
        tar.extend_from_slice(&[0; 1024]);
        tar
    }

    #[test]
    fn catalogue_is_deterministic_and_content_only() {
        let source = include_bytes!("../../tests/fixtures/regions/mixed.bin");
        let owned = source.to_vec();
        assert_eq!(catalog(source).unwrap(), catalog(&owned).unwrap());
    }

    #[test]
    fn later_tar_member_ecoff_is_admitted() {
        let ecoff = include_bytes!("../../tests/fixtures/address_relations/source.bin");
        let mut archive = tar_member(b"first.txt", b"ordinary");
        let second = tar_member(b"later.ecoff", ecoff);
        archive.truncate(archive.len() - 1024);
        archive.extend_from_slice(&second);
        let result = catalog(&archive).unwrap();
        assert!(result
            .candidates
            .iter()
            .any(|candidate| candidate.id == "executable-joint-discount"));
    }

    #[test]
    fn ascii_magic_is_not_a_binary_executable_admission() {
        let text = b"ELF CIXB\x1d ECOFF executable-joint-discount";
        let result = catalog(text).unwrap();
        assert!(!result
            .candidates
            .iter()
            .any(|candidate| candidate.family.starts_with("executable")));
    }
}
