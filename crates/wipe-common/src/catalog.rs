//! Model-aware enclosure catalog (ADR-0004).
//!
//! ADR-0002's form-factor abstraction is what lets a bench be described at all
//! when the hardware is something we have never seen. This adds a layer *on
//! top*: when we happen to recognise the model, say so — and draw it, and
//! pre-fill its layout.
//!
//! The catalog is a **source of defaults and artwork, never the source of
//! truth for a live layout**. Picking a model expands it into ordinary banks
//! and bays, which are then the operator's to edit. The moment rendering
//! depended on a lookup succeeding, an unlisted chassis would become a broken
//! screen instead of a plain one.

use serde::{Deserialize, Serialize};

use crate::{
    grid_bank, Bank, BayFormFactor, BayOrder, BayOrigin, Enclosure, EnclosureKind, NumberingRun,
    TrayOrientation,
};

/// Bumped when the catalog document shape changes incompatibly.
pub const CATALOG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(pub String);

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModelId {
    fn from(s: &str) -> Self {
        ModelId(s.to_string())
    }
}

// ---------------------------------------------------------------------------
// Identity — how we recognise hardware in the wild
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbId {
    pub vid: String,
    pub pid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PciId {
    pub vendor: String,
    pub device: String,
}

/// SCSI/SES INQUIRY strings. Vendors pad these to fixed widths, so matching
/// trims and case-folds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inquiry {
    pub vendor: String,
    pub product: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

fn norm(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

impl Inquiry {
    fn matches(&self, other: &Inquiry) -> bool {
        norm(&self.vendor) == norm(&other.vendor) && norm(&self.product) == norm(&other.product)
    }
    fn revision_matches(&self, other: &Inquiry) -> bool {
        match (&self.revision, &other.revision) {
            (Some(a), Some(b)) => norm(a) == norm(b),
            // A catalog entry with no revision is revision-agnostic.
            (None, _) => true,
            (Some(_), None) => false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIdentity {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usb: Vec<UsbId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pci: Vec<PciId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scsi_inquiry: Vec<Inquiry>,
}

/// What a backend can report about an enclosure it can see.
///
/// Nothing populates this today — the mock has no enclosures and
/// `wipe-engine-linux` does not exist. Defined now so landing SES/USB probing
/// later is a backend change, not a catalog-model change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnclosureIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb: Option<UsbId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pci: Option<PciId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scsi_inquiry: Option<Inquiry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ses_enclosure_id: Option<String>,
}

/// How much we trust a match. Ranked, not boolean, because a fuzzy hit should
/// suggest rather than decide (ADR-0004 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchConfidence {
    /// Product-string similarity only.
    Low,
    /// Inquiry vendor+product agree but the revision differs.
    Medium,
    /// Exact USB/PCI id, or full inquiry including revision.
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMatch {
    pub model_id: ModelId,
    pub confidence: MatchConfidence,
    /// Why we matched, shown to the operator: "USB 174c:55aa".
    pub evidence: String,
}

// ---------------------------------------------------------------------------
// Spec + capabilities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BankSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub rows: u16,
    pub cols: u16,
    pub form_factor: BayFormFactor,
    pub orientation: TrayOrientation,
    pub order: BayOrder,
    pub origin: BayOrigin,
    #[serde(default = "one")]
    pub label_start: u16,
}

fn one() -> u16 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub banks: Vec<BankSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connectors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// What a *known* model is capable of.
///
/// Absent by default and never inferred: a missing block means "we do not
/// know", which must not render the same as "no". Inferring from `kind`
/// ("it's a rackmount, so probably locate LEDs") produces buttons that do
/// nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    #[serde(default)]
    pub locate_led: bool,
    #[serde(default)]
    pub per_bay_power: bool,
    #[serde(default)]
    pub hotswap_notify: bool,
    #[serde(default)]
    pub ses_slot_addressing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnclosureModel {
    pub id: ModelId,
    pub vendor: String,
    pub product: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub kind: EnclosureKind,
    #[serde(default)]
    pub identity: ModelIdentity,
    pub spec: ModelSpec,
    /// Key into the frontend's shell registry. `None` means "no bespoke
    /// artwork" — the generic per-form-factor shell is used and labelled as
    /// generic, which is a supported outcome, not a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    /// Present only for models whose capabilities we have actually verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    /// Who verified this entry against real hardware, and when. Wrong catalog
    /// data is worse than absent catalog data, so provenance is a field, not
    /// a convention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_by: Option<String>,
}

impl EnclosureModel {
    pub fn display_name(&self) -> String {
        format!("{} {}", self.vendor, self.product)
    }

    pub fn bay_count(&self) -> usize {
        self.spec
            .banks
            .iter()
            .map(|b| b.rows as usize * b.cols as usize)
            .sum()
    }

    /// Expand into an ordinary [`Enclosure`] — the same shape a preset or a
    /// hand-written config produces. From here on the operator owns it.
    pub fn expand(&self, enclosure_id: &str) -> Enclosure {
        let banks: Vec<Bank> = self
            .spec
            .banks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let bank_id = format!("b{}", i + 1);
                grid_bank(
                    enclosure_id,
                    &bank_id,
                    b.label.as_deref(),
                    b.rows,
                    b.cols,
                    b.form_factor,
                    b.orientation,
                    b.order,
                    b.origin,
                    b.label_start,
                )
            })
            .collect();

        Enclosure {
            id: enclosure_id.to_string(),
            label: self.display_name(),
            kind: self.kind,
            model_ref: Some(self.id.clone()),
            banks,
            note: self.spec.notes.clone(),
        }
    }

    /// The numbering run each bank was generated with, for the editor.
    pub fn numbering_runs(&self) -> Vec<NumberingRun> {
        self.spec
            .banks
            .iter()
            .map(|b| NumberingRun {
                order: b.order,
                origin: b.origin,
                label_start: b.label_start,
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The catalog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub schema_version: u32,
    pub models: Vec<EnclosureModel>,
}

/// Models bundled with the binary, so an air-gapped station has the catalog
/// it shipped with (ADR-0004 §7).
const BUNDLED: &str = include_str!("../data/catalog.json");

impl Catalog {
    /// The bundled starter set.
    ///
    /// Panics only if the compiled-in JSON is malformed, which is a build-time
    /// bug caught by the catalog tests rather than something a station can hit.
    pub fn bundled() -> Self {
        serde_json::from_str(BUNDLED).expect("bundled catalog is valid")
    }

    pub fn empty() -> Self {
        Self {
            schema_version: CATALOG_SCHEMA_VERSION,
            models: Vec::new(),
        }
    }

    pub fn get(&self, id: &ModelId) -> Option<&EnclosureModel> {
        self.models.iter().find(|m| &m.id == id)
    }

    /// Overlay `other` on top of self: same-id entries in `other` win.
    ///
    /// This is how a station-local or control-plane-distributed catalog
    /// corrects our data without waiting for a release.
    pub fn overlay(&self, other: &Catalog) -> Catalog {
        let mut models = self.models.clone();
        for m in &other.models {
            match models.iter_mut().find(|x| x.id == m.id) {
                Some(existing) => *existing = m.clone(),
                None => models.push(m.clone()),
            }
        }
        Catalog {
            schema_version: CATALOG_SCHEMA_VERSION,
            models,
        }
    }

    /// Free-text search over vendor, product and aliases, for the picker.
    pub fn search(&self, query: &str) -> Vec<&EnclosureModel> {
        let q = norm(query);
        if q.is_empty() {
            return self.models.iter().collect();
        }
        self.models
            .iter()
            .filter(|m| {
                norm(&m.vendor).contains(&q)
                    || norm(&m.product).contains(&q)
                    || norm(&m.display_name()).contains(&q)
                    || m.aliases.iter().any(|a| norm(a).contains(&q))
            })
            .collect()
    }

    /// Duplicate ids, which would make `get` ambiguous.
    pub fn duplicate_ids(&self) -> Vec<ModelId> {
        let mut seen = std::collections::BTreeSet::new();
        let mut dupes = std::collections::BTreeSet::new();
        for m in &self.models {
            if !seen.insert(m.id.clone()) {
                dupes.insert(m.id.clone());
            }
        }
        dupes.into_iter().collect()
    }

    /// Rank catalog entries against hardware identity (ADR-0004 §5).
    ///
    /// Conservative by construction: exact ids and full inquiries rank High,
    /// a revision mismatch drops to Medium, and product-string similarity is
    /// only ever Low — a suggestion for the operator to confirm, never an
    /// automatic selection.
    pub fn match_identity(&self, identity: &EnclosureIdentity) -> Vec<ModelMatch> {
        let mut out: Vec<ModelMatch> = Vec::new();

        for m in &self.models {
            if let Some(found) = &identity.usb {
                if let Some(hit) =
                    m.identity.usb.iter().find(|u| {
                        norm(&u.vid) == norm(&found.vid) && norm(&u.pid) == norm(&found.pid)
                    })
                {
                    out.push(ModelMatch {
                        model_id: m.id.clone(),
                        confidence: MatchConfidence::High,
                        evidence: format!("USB {}:{}", hit.vid, hit.pid),
                    });
                    continue;
                }
            }
            if let Some(found) = &identity.pci {
                if let Some(hit) = m.identity.pci.iter().find(|p| {
                    norm(&p.vendor) == norm(&found.vendor) && norm(&p.device) == norm(&found.device)
                }) {
                    out.push(ModelMatch {
                        model_id: m.id.clone(),
                        confidence: MatchConfidence::High,
                        evidence: format!("PCI {}:{}", hit.vendor, hit.device),
                    });
                    continue;
                }
            }
            if let Some(found) = &identity.scsi_inquiry {
                if let Some(hit) = m.identity.scsi_inquiry.iter().find(|i| i.matches(found)) {
                    let full = hit.revision_matches(found);
                    out.push(ModelMatch {
                        model_id: m.id.clone(),
                        confidence: if full {
                            MatchConfidence::High
                        } else {
                            MatchConfidence::Medium
                        },
                        evidence: format!(
                            "SCSI INQUIRY {} {}{}",
                            hit.vendor.trim(),
                            hit.product.trim(),
                            if full { "" } else { " (revision differs)" }
                        ),
                    });
                    continue;
                }

                // Last resort: the product string looks like one of our
                // aliases. Suggest only.
                let p = norm(&found.product);
                if !p.is_empty()
                    && (norm(&m.product) == p || m.aliases.iter().any(|a| norm(a) == p))
                {
                    out.push(ModelMatch {
                        model_id: m.id.clone(),
                        confidence: MatchConfidence::Low,
                        evidence: format!("product string \"{}\"", found.product.trim()),
                    });
                }
            }
        }

        // Best first, then by id so the order is stable for a given input.
        out.sort_by(|a, b| {
            b.confidence
                .cmp(&a.confidence)
                .then_with(|| a.model_id.cmp(&b.model_id))
        });
        out
    }

    /// The single model to pre-select, if any.
    ///
    /// Only a High-confidence match pre-selects; anything less is a
    /// suggestion the operator confirms, and an ambiguous High (two entries
    /// claiming the same hardware) selects nothing — a catalog bug must not
    /// silently pick a winner.
    pub fn best_match(&self, identity: &EnclosureIdentity) -> Option<ModelMatch> {
        let matches = self.match_identity(identity);
        let high: Vec<_> = matches
            .iter()
            .filter(|m| m.confidence == MatchConfidence::High)
            .collect();
        match high.len() {
            1 => Some(high[0].clone()),
            _ => None,
        }
    }
}
