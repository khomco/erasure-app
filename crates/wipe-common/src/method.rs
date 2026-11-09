use serde::{Deserialize, Serialize};

use crate::{Capabilities, MediaType};

/// NIST SP 800-88 Rev. 2 sanitization category.
///
/// Rev. 2 defers the technical "how" to IEEE 2883-2022 but retains
/// Clear / Purge / Destroy as the canonical decision boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Clear,
    Purge,
    Destroy,
}

/// FIPS 199 data confidentiality classification, fed into the R2 decision flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Low,
    Moderate,
    High,
}

/// Operator's intent for the media post-sanitization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Reuse,
    Recycle,
    Destroy,
}

/// Concrete sanitization technique. Each variant maps to a Category
/// per the R2 / IEEE 2883 tables and the device's capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Method {
    /// NVMe Admin Sanitize, action 0x02 (Block Erase). NIST Purge.
    NvmeSanitizeBlockErase {
        /// Allow Unrestricted Sanitize Exit (AUSE) — false is preferred per Rev. 2.
        ause: bool,
        no_deallocate: bool,
    },
    /// NVMe Admin Sanitize, action 0x04 (Crypto Erase). NIST Purge when
    /// SED is properly provisioned and key destruction is verifiable.
    NvmeSanitizeCryptoErase {
        ause: bool,
        no_deallocate: bool,
    },
    /// NVMe Admin Sanitize, action 0x03 (Overwrite). NIST Purge (single pass).
    NvmeSanitizeOverwrite {
        ause: bool,
        no_deallocate: bool,
        pattern_u32: u32,
    },
    /// ATA SECURITY ERASE UNIT. Enhanced=true is required to reach reallocated sectors.
    AtaSecureErase { enhanced: bool },
    /// Generic block-level overwrite. Maps to Clear (single pass) on modern flash.
    BlockOverwrite { pattern: Pattern, passes: u8 },
    /// SED Revert / SID-revoke style crypto-erase outside of NVMe Sanitize.
    OpalRevert,
    /// Operator declared the device will be physically destroyed.
    Destroy { method: DestructMethod },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    Zeros,
    Ones,
    Random,
    Fixed(u8),
}

/// Destruction technique: what the bench physically did to the drive
/// (ADR-0011 §5).
///
/// The first five are the NIST SP 800-88 Rev. 2 §3.1.3 Destroy techniques.
/// `Crush`, `Punch`, `Degauss` and `Drill` are recorded honestly as physical
/// damage that is **not** NIST Destroy; see [`DestructMethod::standard_label`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructMethod {
    Shred,
    Disintegrate,
    Incinerate,
    Pulverize,
    Melt,
    Crush,
    Punch,
    Degauss,
    Drill,
}

/// Label for a technique that NIST SP 800-88 Rev. 2 §3.1.3 counts as Destroy.
pub const NIST_DESTROY_LABEL: &str = "NIST SP 800-88 Destroy";
/// Label for a technique that only damages the drive physically.
pub const NOT_NIST_DESTROY_LABEL: &str = "physical damage — not NIST Destroy";
/// NIST SP 800-88 Rev. 2 §3.1.3's caution on shred and pulverize, shown when
/// the Job's classification is above Low (ADR-0011 §5).
pub const SHRED_PULVERIZE_CAUTION: &str = "NIST SP 800-88 Rev. 2: shred and pulverize \
     should be avoided for anything but the lowest security categories.";

impl DestructMethod {
    /// Every technique, NIST Destroy techniques first, in declaration order.
    pub const ALL: [DestructMethod; 9] = [
        Self::Shred,
        Self::Disintegrate,
        Self::Incinerate,
        Self::Pulverize,
        Self::Melt,
        Self::Crush,
        Self::Punch,
        Self::Degauss,
        Self::Drill,
    ];

    /// Whether NIST SP 800-88 Rev. 2 §3.1.3 counts this technique as Destroy.
    pub fn is_nist_destroy(self) -> bool {
        match self {
            Self::Shred | Self::Disintegrate | Self::Incinerate | Self::Pulverize | Self::Melt => {
                true
            }
            Self::Crush | Self::Punch | Self::Degauss | Self::Drill => false,
        }
    }

    /// The technique's name as an operator reads it, e.g. "Shred".
    pub fn human_name(self) -> &'static str {
        match self {
            Self::Shred => "Shred",
            Self::Disintegrate => "Disintegrate",
            Self::Incinerate => "Incinerate",
            Self::Pulverize => "Pulverize",
            Self::Melt => "Melt",
            Self::Crush => "Crush",
            Self::Punch => "Punch",
            Self::Degauss => "Degauss",
            Self::Drill => "Drill",
        }
    }

    /// The standard the technique meets, stated honestly.
    pub fn standard_label(self) -> &'static str {
        if self.is_nist_destroy() {
            NIST_DESTROY_LABEL
        } else {
            NOT_NIST_DESTROY_LABEL
        }
    }

    /// The shred/pulverize caution, for those two techniques when the Job's
    /// classification is above Low; `None` otherwise.
    pub fn caution(self, classification: Classification) -> Option<&'static str> {
        match (self, classification) {
            (_, Classification::Low) => None,
            (Self::Shred | Self::Pulverize, _) => Some(SHRED_PULVERIZE_CAUTION),
            _ => None,
        }
    }
}

impl Method {
    pub fn category(&self) -> Category {
        match self {
            Self::NvmeSanitizeBlockErase { .. }
            | Self::NvmeSanitizeCryptoErase { .. }
            | Self::NvmeSanitizeOverwrite { .. }
            | Self::AtaSecureErase { enhanced: true }
            | Self::OpalRevert => Category::Purge,
            Self::AtaSecureErase { enhanced: false } | Self::BlockOverwrite { .. } => {
                Category::Clear
            }
            Self::Destroy { .. } => Category::Destroy,
        }
    }

    pub fn human_name(&self) -> &'static str {
        match self {
            Self::NvmeSanitizeBlockErase { .. } => "NVMe Sanitize — Block Erase",
            Self::NvmeSanitizeCryptoErase { .. } => "NVMe Sanitize — Crypto Erase",
            Self::NvmeSanitizeOverwrite { .. } => "NVMe Sanitize — Overwrite",
            Self::AtaSecureErase { enhanced: true } => "ATA Secure Erase (Enhanced)",
            Self::AtaSecureErase { enhanced: false } => "ATA Secure Erase",
            Self::BlockOverwrite { .. } => "Block Overwrite",
            Self::OpalRevert => "TCG Opal Revert",
            Self::Destroy { .. } => "Physical Destruction",
        }
    }
}

/// Pick a method automatically given a device's capabilities, the operator's
/// data classification, and the operator's reuse intent. Mirrors the
/// NIST 800-88 Rev. 2 decision flow at a high level.
pub fn select_method(
    caps: &Capabilities,
    media: MediaType,
    classification: Classification,
    intent: Intent,
) -> Option<Method> {
    if intent == Intent::Destroy {
        return Some(Method::Destroy {
            method: DestructMethod::Disintegrate,
        });
    }

    let want_purge = matches!(
        classification,
        Classification::Moderate | Classification::High
    );

    // Prefer NVMe Sanitize on NVMe flash.
    if media == MediaType::SsdNvme {
        if let Some(nvme) = &caps.nvme_sanitize {
            if want_purge {
                if nvme.crypto_erase && caps.sed != crate::SedStatus::None {
                    return Some(Method::NvmeSanitizeCryptoErase {
                        ause: false,
                        no_deallocate: false,
                    });
                }
                if nvme.block_erase {
                    return Some(Method::NvmeSanitizeBlockErase {
                        ause: false,
                        no_deallocate: false,
                    });
                }
                if nvme.overwrite {
                    return Some(Method::NvmeSanitizeOverwrite {
                        ause: false,
                        no_deallocate: false,
                        pattern_u32: 0,
                    });
                }
            } else if nvme.block_erase {
                return Some(Method::NvmeSanitizeBlockErase {
                    ause: false,
                    no_deallocate: false,
                });
            }
        }
    }

    // SATA SSD or HDD: prefer ATA Security Erase Enhanced for Purge,
    // basic for Clear.
    if let Some(ata) = &caps.ata_security {
        if ata.supported && !ata.frozen {
            return Some(Method::AtaSecureErase {
                enhanced: want_purge && ata.enhanced_supported,
            });
        }
    }

    // Fallback for Clear: block overwrite (single pass — R2 says one is enough on flash).
    if !want_purge {
        return Some(Method::BlockOverwrite {
            pattern: Pattern::Zeros,
            passes: 1,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_nine_techniques_nist_destroy_first() {
        let names: Vec<String> = DestructMethod::ALL
            .iter()
            .map(|m| {
                serde_json::to_value(m)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "shred",
                "disintegrate",
                "incinerate",
                "pulverize",
                "melt",
                "crush",
                "punch",
                "degauss",
                "drill"
            ]
        );
    }

    #[test]
    fn standard_labels_are_pinned() {
        for m in [
            DestructMethod::Shred,
            DestructMethod::Disintegrate,
            DestructMethod::Incinerate,
            DestructMethod::Pulverize,
            DestructMethod::Melt,
        ] {
            assert!(m.is_nist_destroy());
            assert_eq!(m.standard_label(), "NIST SP 800-88 Destroy");
        }
        for m in [
            DestructMethod::Crush,
            DestructMethod::Punch,
            DestructMethod::Degauss,
            DestructMethod::Drill,
        ] {
            assert!(!m.is_nist_destroy());
            assert_eq!(m.standard_label(), "physical damage — not NIST Destroy");
        }
    }

    #[test]
    fn human_names() {
        assert_eq!(DestructMethod::Shred.human_name(), "Shred");
        assert_eq!(DestructMethod::Degauss.human_name(), "Degauss");
    }

    #[test]
    fn caution_only_for_shred_and_pulverize_above_low() {
        let caution = "NIST SP 800-88 Rev. 2: shred and pulverize should be avoided for \
                       anything but the lowest security categories.";
        for m in DestructMethod::ALL {
            assert_eq!(m.caution(Classification::Low), None);
            for c in [Classification::Moderate, Classification::High] {
                let expected = matches!(m, DestructMethod::Shred | DestructMethod::Pulverize)
                    .then_some(caution);
                assert_eq!(m.caution(c), expected, "{m:?} at {c:?}");
            }
        }
    }

    #[test]
    fn destroy_selection_still_disintegrates() {
        let m = select_method(
            &Capabilities::default(),
            MediaType::HddMagnetic,
            Classification::High,
            Intent::Destroy,
        );
        assert_eq!(
            m,
            Some(Method::Destroy {
                method: DestructMethod::Disintegrate
            })
        );
    }
}
