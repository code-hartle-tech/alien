//! Acer model plug-ins found in the locally extracted PredatorSense packages.
//!
//! This is a deliberately small, reviewable projection of locally generated
//! matrices from PredatorSense 3.00.3152 and 3.00.3198. Extraction inputs and
//! generated decompiler artifacts stay outside the public snapshot. Keep
//! package revisions separate: PH517-52 demonstrates that Acer can change a
//! model's declared machine/per-key profile over time.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackageEvidence {
    pub version: &'static str,
    pub machine_type: u8,
    pub lighting_type: u8,
    pub fan_type: u8,
    pub per_key: bool,
    pub gpu_overclock: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelEvidence {
    pub model: &'static str,
    /// Acer model-code series, used only to organise the catalog.
    pub series: &'static str,
    pub packages: &'static [PackageEvidence],
    /// The one machine on which Alien's current paths were exercised live.
    pub live_reference: bool,
}

impl PackageEvidence {
    pub const fn family_name(self) -> &'static str {
        match self.machine_type {
            1 => "Covini",
            5 => "Defender",
            6 => "Spyder",
            8 => "Clubman",
            9 => "XC90",
            10 => "Evoque",
            _ => "unmapped",
        }
    }
}

/// A product/model group named on Acer's public Predator/Nitro GPU table.
/// These entries have no extracted plug-in or live Alien protocol mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcosystemCandidate {
    pub product: &'static str,
    pub models: &'static [&'static str],
}

const V3152_M1_NON_PER_KEY: PackageEvidence = package("3.00.3152", 1, 1, 1, false);
const V3198_M8_NON_PER_KEY: PackageEvidence = package("3.00.3198", 8, 1, 1, false);
const V3198_M10_PER_KEY: PackageEvidence = package("3.00.3198", 10, 1, 1, true);

const fn package(
    version: &'static str,
    machine_type: u8,
    lighting_type: u8,
    fan_type: u8,
    per_key: bool,
) -> PackageEvidence {
    PackageEvidence {
        version,
        machine_type,
        lighting_type,
        fan_type,
        per_key,
        // Every plug-in in these two extracted matrices advertises GPU OC.
        gpu_overclock: true,
    }
}

pub const MODELS: &[ModelEvidence] = &[
    ModelEvidence {
        model: "Predator PH315-53",
        series: "PH315",
        packages: &[V3152_M1_NON_PER_KEY],
        live_reference: true,
    },
    ModelEvidence {
        model: "Predator PH315-54",
        series: "PH315",
        packages: &[V3198_M8_NON_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PH315-55",
        series: "PH315",
        packages: &[V3198_M10_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PH315-55s",
        series: "PH315",
        packages: &[V3198_M10_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PH317-54",
        series: "PH317",
        packages: &[V3152_M1_NON_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PH317-55",
        series: "PH317",
        packages: &[V3198_M8_NON_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PH317-56",
        series: "PH317",
        packages: &[V3198_M10_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PH517-52",
        series: "PH517",
        packages: &[V3152_M1_NON_PER_KEY, package("3.00.3198", 9, 1, 1, true)],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PH517-53",
        series: "PH517",
        packages: &[V3198_M10_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PH717-72",
        series: "PH717",
        packages: &[package("3.00.3152", 5, 1, 1, true)],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PT314-51s",
        series: "PT314",
        packages: &[V3198_M8_NON_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PT314-52s",
        series: "PT314",
        packages: &[V3198_M8_NON_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PT315-52",
        series: "PT315",
        packages: &[V3152_M1_NON_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PT315-53",
        series: "PT315",
        packages: &[V3198_M8_NON_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PT316-51s",
        series: "PT316",
        packages: &[V3198_M8_NON_PER_KEY],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PT515-52",
        series: "PT515",
        packages: &[package("3.00.3152", 6, 1, 1, true)],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PT516-51s",
        series: "PT516",
        packages: &[package("3.00.3198", 8, 1, 2, false)],
        live_reference: false,
    },
    ModelEvidence {
        model: "Predator PT516-52s",
        series: "PT516",
        packages: &[V3198_M8_NON_PER_KEY],
        live_reference: false,
    },
];

pub const MODEL_COUNT: usize = MODELS.len();

/// Predator laptop candidates named by official Acer GPU, mobile-compatibility
/// and product-specification sources as of 2026-08-12. These are ecosystem
/// associations only; no Alien protocol mapping is inferred from them.
pub const ECOSYSTEM_CANDIDATES: &[EcosystemCandidate] = &[
    EcosystemCandidate {
        product: "Predator Helios 16 AI",
        models: &["PH16-73"],
    },
    EcosystemCandidate {
        product: "Predator Helios 18 AI",
        models: &["PH18-I71", "PH18-73"],
    },
    EcosystemCandidate {
        product: "Predator Helios 16",
        models: &["PH16-71", "PH16-72"],
    },
    EcosystemCandidate {
        product: "Predator Helios 18",
        models: &["PH18-71", "PH18-72"],
    },
    EcosystemCandidate {
        product: "Predator Helios Neo 16S AI",
        models: &["PHN16S-I71", "PHN16S-I51", "PHN16S-71"],
    },
    EcosystemCandidate {
        product: "Predator Helios Neo 16 AI",
        models: &["PHN16-I71", "PHN16-73"],
    },
    EcosystemCandidate {
        product: "Predator Helios Neo 18 AI",
        models: &["PHN18-I71", "PHN18-72"],
    },
    EcosystemCandidate {
        product: "Predator Helios Neo 16",
        models: &["PHN16-I31", "PHN16-71", "PHN16-72"],
    },
    EcosystemCandidate {
        product: "Predator Helios Neo 18",
        models: &["PHN18-71"],
    },
    EcosystemCandidate {
        product: "Predator Helios Neo 14 AI",
        models: &["PHN14-71"],
    },
    EcosystemCandidate {
        product: "Predator Helios Neo 14",
        models: &["PHN14-51"],
    },
    EcosystemCandidate {
        product: "Predator Helios 3D 15 SpatialLabs Edition",
        models: &["PH3D15-71"],
    },
    EcosystemCandidate {
        product: "Predator Triton 14 AI",
        models: &["PT14-52T"],
    },
    EcosystemCandidate {
        product: "Predator Triton 14",
        models: &["PT14-51"],
    },
    EcosystemCandidate {
        product: "Predator Triton Neo 16",
        models: &["PTN16-51"],
    },
    EcosystemCandidate {
        product: "Predator Triton 16",
        models: &["PT16-51"],
    },
    EcosystemCandidate {
        product: "Predator Triton 17 X",
        models: &["PTX17-71"],
    },
    EcosystemCandidate {
        product: "Predator Helios 300",
        models: &["PH315-52", "PH317-53"],
    },
    EcosystemCandidate {
        product: "Predator Helios 700",
        models: &["PH717-71"],
    },
    EcosystemCandidate {
        product: "Predator Triton 300",
        models: &["PT315-51"],
    },
    EcosystemCandidate {
        product: "Predator Triton 500",
        models: &["PT515-51"],
    },
    EcosystemCandidate {
        product: "Predator Triton 900",
        models: &["PT917-71"],
    },
    EcosystemCandidate {
        product: "Predator Helios 18P AI",
        models: &["PH18P-73"],
    },
    EcosystemCandidate {
        product: "Predator Orion desktop PCs",
        models: &["PO3-620", "PO5-615s", "PO9-920"],
    },
];

pub const ECOSYSTEM_CANDIDATE_COUNT: usize = 36;
pub const ACER_GPU_SPEC_URL: &str =
    "https://www.acer.com/us-en/predator/laptops/predator-and-nitro-gaming-laptop-gpu-specs";
pub const ACER_MOBILE_COMPAT_URL: &str =
    "https://community.acer.com/en/kb/articles/12700-predatorsense-mobile-application-compatibility";
pub const ACER_HELIOS_18P_URL: &str = "https://news.acer.com/acer-unleashes-predator-helios-18p-ai-hybrid-gaming-laptop-for-work-and-play";

pub fn matches(model: &ModelEvidence, query: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }

    model.model.to_ascii_lowercase().contains(&query)
        || model.series.to_ascii_lowercase().contains(&query)
        || (model.live_reference && "live verified reference".contains(&query))
        || model.packages.iter().any(|package| {
            package.version.contains(&query)
                || package.family_name().to_ascii_lowercase().contains(&query)
                || format!(
                    "m{} l{} f{} {} gpu oc",
                    package.machine_type,
                    package.lighting_type,
                    package.fan_type,
                    if package.per_key {
                        "per-key 1"
                    } else {
                        "per-key 0"
                    }
                )
                .contains(&query)
        })
}

pub fn candidate_matches(candidate: &EcosystemCandidate, query: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    query.is_empty()
        || candidate.product.to_ascii_lowercase().contains(&query)
        || candidate
            .models
            .iter()
            .any(|model| model.to_ascii_lowercase().contains(&query))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_preserves_all_unique_models_and_package_plugins() {
        assert_eq!(MODELS.len(), 18);
        assert_eq!(MODELS.iter().map(|m| m.packages.len()).sum::<usize>(), 19);
        assert_eq!(
            MODELS
                .iter()
                .map(|m| m.series)
                .collect::<HashSet<_>>()
                .len(),
            9
        );
        assert_eq!(
            MODELS.iter().map(|m| m.model).collect::<HashSet<_>>().len(),
            18
        );
        assert_eq!(
            MODELS
                .iter()
                .flat_map(|model| model.packages)
                .map(|package| package.machine_type)
                .collect::<HashSet<_>>()
                .len(),
            6
        );
    }

    #[test]
    fn ph517_52_package_drift_is_not_collapsed() {
        let model = MODELS
            .iter()
            .find(|model| model.model == "Predator PH517-52")
            .unwrap();
        assert_eq!(model.packages.len(), 2);
        assert_eq!(model.packages[0].version, "3.00.3152");
        assert_eq!(model.packages[0].machine_type, 1);
        assert!(!model.packages[0].per_key);
        assert_eq!(model.packages[1].version, "3.00.3198");
        assert_eq!(model.packages[1].machine_type, 9);
        assert!(model.packages[1].per_key);
    }

    #[test]
    fn only_ph315_53_is_marked_as_the_live_reference() {
        let references: Vec<_> = MODELS
            .iter()
            .filter(|model| model.live_reference)
            .map(|model| model.model)
            .collect();
        assert_eq!(references, ["Predator PH315-53"]);
    }

    #[test]
    fn search_covers_model_series_package_and_profile_fields() {
        assert_eq!(MODELS.iter().filter(|m| matches(m, "PH315")).count(), 4);
        assert_eq!(MODELS.iter().filter(|m| matches(m, "3.00.3152")).count(), 6);
        assert_eq!(MODELS.iter().filter(|m| matches(m, "F2")).count(), 1);
        assert_eq!(
            MODELS
                .iter()
                .filter(|m| matches(m, "live verified"))
                .count(),
            1
        );
        assert_eq!(MODELS.iter().filter(|m| matches(m, "evoque")).count(), 4);
    }

    #[test]
    fn official_candidate_snapshot_count_is_explicit_and_searchable() {
        assert_eq!(
            ECOSYSTEM_CANDIDATES
                .iter()
                .map(|candidate| candidate.models.len())
                .sum::<usize>(),
            ECOSYSTEM_CANDIDATE_COUNT
        );
        assert_eq!(
            ECOSYSTEM_CANDIDATES
                .iter()
                .filter(|candidate| candidate_matches(candidate, "PHN16"))
                .flat_map(|candidate| candidate.models)
                .filter(|model| model.contains("PHN16"))
                .count(),
            8
        );
    }
}
