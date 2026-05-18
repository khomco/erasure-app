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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructMethod {
    Shred,
    Disintegrate,
    Incinerate,
    Pulverize,
    Melt,
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
