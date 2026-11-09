//! Destruction records (ADR-0011): the operator's "recorded for destruction"
//! attestation, the run's "destruction completed" confirmation, an optional
//! concurrence, and the identity assurance of whoever signed each one.
//!
//! The statement an operator signs is rendered only here, by the
//! constructors, from the wording in
//! `.scratch/destruction-attestation/research.md`. The constructors refuse
//! blank required fields and a completion whose verification did not happen,
//! so a record never claims something that was not attested. Build records
//! only through them.

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

use crate::{DestructMethod, OperatorRef};

/// Why a destruction record was not created.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DestructionRecordError {
    /// A required text field was empty or only whitespace. Names the field.
    #[error("`{0}` is required")]
    Blank(&'static str),
    /// Neither an asset tag nor a serial was recorded for the device.
    #[error("an asset tag or a serial is required")]
    NoIdentifier,
    /// The completion claims inspected remnants, so they must have been.
    #[error("the remnants must be inspected before destruction is recorded as completed")]
    RemnantsNotInspected,
    /// The completion claims the equipment finished its cycle, so it must have.
    #[error("the equipment cycle must be completed before destruction is recorded as completed")]
    EquipmentCycleNotCompleted,
}

/// How sure the record is of who signed it (ADR-0011 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
pub enum IdentityAssurance {
    /// The typed full name and title from the station session, plus an
    /// explicit "I attest" confirmation. The typed name is the signature.
    SelfAsserted,
    /// The identity came from the customer's identity provider. Needs the
    /// `OperatorSso` entitlement and the unbuilt SSO sign-in; nothing
    /// constructs it yet.
    Verified { issuer: String, subject: String },
}

impl IdentityAssurance {
    /// Operator-facing label: "Self-asserted" or "Verified by {issuer}".
    pub fn label(&self) -> String {
        match self {
            Self::SelfAsserted => "Self-asserted".to_owned(),
            Self::Verified { issuer, .. } => format!("Verified by {issuer}"),
        }
    }
}

/// Who signed a destruction record, as the statement names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationSigner {
    /// The station session's operator.
    pub operator: OperatorRef,
    /// Full name as typed for this signature.
    pub full_name: String,
    /// Position or title, as NIST SP 800-88 Rev. 2 §4.6 asks.
    pub title: String,
    pub identity_assurance: IdentityAssurance,
}

impl AttestationSigner {
    /// A signer whose typed name and title are the signature.
    pub fn self_asserted(
        operator: OperatorRef,
        full_name: &str,
        title: &str,
    ) -> Result<Self, DestructionRecordError> {
        Ok(Self {
            operator,
            full_name: required("full_name", full_name)?,
            title: required("title", title)?,
            identity_assurance: IdentityAssurance::SelfAsserted,
        })
    }

    /// Re-checks a signer built outside [`AttestationSigner::self_asserted`].
    fn checked(mut self) -> Result<Self, DestructionRecordError> {
        self.full_name = required("full_name", &self.full_name)?;
        self.title = required("title", &self.title)?;
        Ok(self)
    }
}

/// Where a device's recorded identifiers came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierSource {
    /// Scanned from the drive's labels (bar code or typed).
    Scanned,
    /// Read from the enumerated device snapshot.
    Device,
}

/// The identifiers captured for a device recorded for destruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedIdentifiers {
    pub asset_tag: Option<String>,
    pub serial: Option<String>,
    pub source: IdentifierSource,
}

/// "Recorded for destruction": the operator's attestation that a device was
/// placed into the destruction process (ADR-0011 §1, first step). It sits on
/// the `DestructionEvent`, whose `method` is the technique.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestructionAttestation {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub equipment_id: String,
    pub location: String,
    pub identifiers: RecordedIdentifiers,
    pub signer: AttestationSigner,
    /// The statement the signer confirmed, verbatim.
    pub statement: String,
}

/// The "recorded for destruction" statement for these inputs, exactly as
/// [`DestructionAttestation::new`] stores it. Trims and refuses blank fields
/// the same way, so a preview shown to the operator before "I attest" is the
/// text a record with the same inputs signs.
pub fn attestation_statement(
    technique: DestructMethod,
    full_name: &str,
    title: &str,
    equipment_id: &str,
    location: &str,
) -> Result<String, DestructionRecordError> {
    let full_name = required("full_name", full_name)?;
    let title = required("title", title)?;
    let equipment_id = required("equipment_id", equipment_id)?;
    let location = required("location", location)?;
    Ok(format!(
        "I, {full_name}, {title}, recorded the storage device(s) listed above for destruction \
         by {} using {equipment_id} at {location}. I attest that the identifiers shown were \
         captured from the devices themselves and that these devices are being placed into \
         the destruction process now. I attest that the information provided on this \
         statement is accurate to the best of my knowledge.",
        technique.human_name(),
    ))
}

impl DestructionAttestation {
    /// `technique` is used only to render the statement; it is recorded once,
    /// as `DestructionEvent::method`, so the two cannot disagree.
    pub fn new(
        technique: DestructMethod,
        at: OffsetDateTime,
        equipment_id: &str,
        location: &str,
        identifiers: RecordedIdentifiers,
        signer: AttestationSigner,
    ) -> Result<Self, DestructionRecordError> {
        let signer = signer.checked()?;
        let equipment_id = required("equipment_id", equipment_id)?;
        let location = required("location", location)?;
        let identifiers = RecordedIdentifiers {
            asset_tag: optional(identifiers.asset_tag.as_deref()),
            serial: optional(identifiers.serial.as_deref()),
            source: identifiers.source,
        };
        if identifiers.asset_tag.is_none() && identifiers.serial.is_none() {
            return Err(DestructionRecordError::NoIdentifier);
        }
        let statement = attestation_statement(
            technique,
            &signer.full_name,
            &signer.title,
            &equipment_id,
            &location,
        )?;
        Ok(Self {
            at,
            equipment_id,
            location,
            identifiers,
            signer,
            statement,
        })
    }
}

/// How a destruction run was verified (NIST SP 800-88 Rev. 2 §4.5.1:
/// inspect the remnants, identify the equipment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestructionVerification {
    pub remnants_inspected: bool,
    pub equipment_cycle_completed: bool,
    /// The equipment that ran, as the completion statement names it.
    pub equipment_id: String,
    /// What the remnants were checked against, e.g. "platters will not engage".
    pub criterion: String,
}

/// "Destruction completed": the confirmation that a destruction run (one
/// `DestructionManifest`) finished and was verified (ADR-0011 §1, second step).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestructionCompletion {
    #[serde(with = "time::serde::rfc3339")]
    pub completed_at: OffsetDateTime,
    pub verification: DestructionVerification,
    /// Video/CCTV reference for the run, when there is one.
    pub video_ref: Option<String>,
    pub signer: AttestationSigner,
    /// The statement the signer confirmed, verbatim.
    pub statement: String,
}

/// The "destruction completed" statement for these inputs, exactly as
/// [`DestructionCompletion::new`] stores it. Trims and refuses blank fields
/// the same way (a blank `video_ref` is no video), so a preview shown before
/// "I attest" is the text a completion with the same inputs signs. It does
/// not check the verification flags; the constructor refuses those.
pub fn completion_statement(
    completed_at: OffsetDateTime,
    equipment_id: &str,
    criterion: &str,
    video_ref: Option<&str>,
) -> Result<String, DestructionRecordError> {
    let equipment_id = required("equipment_id", equipment_id)?;
    let criterion = required("criterion", criterion)?;
    let mut statement = format!(
        "Destruction of the device(s) listed above was completed on {}. Verification: \
         the remnants were inspected and the equipment {equipment_id} completed its cycle; \
         the result meets {criterion}.",
        rfc3339_utc(completed_at),
    );
    if let Some(video) = optional(video_ref) {
        statement.push_str(&format!(" Video reference: {video}."));
    }
    Ok(statement)
}

impl DestructionCompletion {
    pub fn new(
        completed_at: OffsetDateTime,
        verification: DestructionVerification,
        video_ref: Option<&str>,
        signer: AttestationSigner,
    ) -> Result<Self, DestructionRecordError> {
        let signer = signer.checked()?;
        if !verification.remnants_inspected {
            return Err(DestructionRecordError::RemnantsNotInspected);
        }
        if !verification.equipment_cycle_completed {
            return Err(DestructionRecordError::EquipmentCycleNotCompleted);
        }
        let verification = DestructionVerification {
            equipment_id: required("equipment_id", &verification.equipment_id)?,
            criterion: required("criterion", &verification.criterion)?,
            ..verification
        };
        let video_ref = optional(video_ref);
        let statement = completion_statement(
            completed_at,
            &verification.equipment_id,
            &verification.criterion,
            video_ref.as_deref(),
        )?;
        Ok(Self {
            completed_at,
            verification,
            video_ref,
            signer,
            statement,
        })
    }
}

/// What a concurring signer did. The kind fixes the statement, so a reviewer
/// is never recorded as a witness (ADR-0011 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrenceKind {
    Review,
    Witness,
}

/// A site's concurrence policy (ADR-0011 §3). `None`, the default, matches
/// the standards: no second signer is required.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencePolicy {
    #[default]
    None,
    Review,
    Witness,
}

impl ConcurrencePolicy {
    /// The concurrence a manifest needs before its Jobs reach `Destroyed`.
    pub fn required_kind(self) -> Option<ConcurrenceKind> {
        match self {
            Self::None => None,
            Self::Review => Some(ConcurrenceKind::Review),
            Self::Witness => Some(ConcurrenceKind::Witness),
        }
    }
}

/// A second signer's concurrence with a destruction run, given per manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concurrence {
    pub kind: ConcurrenceKind,
    pub manifest_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub signer: AttestationSigner,
    /// The statement the signer confirmed, verbatim.
    pub statement: String,
}

/// The concurrence statement for these inputs, exactly as
/// [`Concurrence::new`] stores it. Trims and refuses a blank name or title
/// the same way, so a preview is the text a concurrence signs.
pub fn concurrence_statement(
    kind: ConcurrenceKind,
    manifest_id: Uuid,
    full_name: &str,
    title: &str,
) -> Result<String, DestructionRecordError> {
    let full_name = required("full_name", full_name)?;
    let title = required("title", title)?;
    let act = match kind {
        ConcurrenceKind::Review => "reviewed the destruction records and verification evidence",
        ConcurrenceKind::Witness => "witnessed the destruction",
    };
    Ok(format!(
        "I, {full_name}, {title}, {act} for manifest {manifest_id} and concur that the devices \
         listed were destroyed as stated."
    ))
}

impl Concurrence {
    pub fn new(
        kind: ConcurrenceKind,
        manifest_id: Uuid,
        at: OffsetDateTime,
        signer: AttestationSigner,
    ) -> Result<Self, DestructionRecordError> {
        let signer = signer.checked()?;
        let statement = concurrence_statement(kind, manifest_id, &signer.full_name, &signer.title)?;
        Ok(Self {
            kind,
            manifest_id,
            at,
            signer,
            statement,
        })
    }
}

/// Whether two destruction records were signed by the same person: the same
/// operator id, or the same full name ignoring case and whitespace. A
/// concurrence must come from someone other than the completion's signer.
pub fn same_signer(a: &AttestationSigner, b: &AttestationSigner) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ")
    };
    a.operator.id == b.operator.id || norm(&a.full_name) == norm(&b.full_name)
}

fn required(field: &'static str, value: &str) -> Result<String, DestructionRecordError> {
    optional(Some(value)).ok_or(DestructionRecordError::Blank(field))
}

fn optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn rfc3339_utc(at: OffsetDateTime) -> String {
    at.to_offset(UtcOffset::UTC)
        .format(&Rfc3339)
        .expect("a UTC OffsetDateTime always formats as RFC 3339")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    fn operator() -> OperatorRef {
        OperatorRef {
            id: "op-1".into(),
            display_name: "Ana".into(),
            email: "ana@example.com".into(),
        }
    }

    fn signer() -> AttestationSigner {
        AttestationSigner::self_asserted(operator(), " Ana Ruiz ", "Bench lead").unwrap()
    }

    fn scanned(asset_tag: Option<&str>, serial: Option<&str>) -> RecordedIdentifiers {
        RecordedIdentifiers {
            asset_tag: asset_tag.map(Into::into),
            serial: serial.map(Into::into),
            source: IdentifierSource::Scanned,
        }
    }

    fn attest(
        equipment: &str,
        location: &str,
        ids: RecordedIdentifiers,
    ) -> Result<DestructionAttestation, DestructionRecordError> {
        DestructionAttestation::new(
            DestructMethod::Shred,
            datetime!(2026-11-08 10:00 UTC),
            equipment,
            location,
            ids,
            signer(),
        )
    }

    fn verification(inspected: bool, cycle: bool, criterion: &str) -> DestructionVerification {
        DestructionVerification {
            remnants_inspected: inspected,
            equipment_cycle_completed: cycle,
            equipment_id: "SHR-2".into(),
            criterion: criterion.into(),
        }
    }

    #[test]
    fn attestation_renders_the_research_statement() {
        let a = attest("SHR-2", "Bay 3", scanned(Some("AT-1"), None)).unwrap();
        assert_eq!(
            a.statement,
            "I, Ana Ruiz, Bench lead, recorded the storage device(s) listed above for \
             destruction by Shred using SHR-2 at Bay 3. I attest that the identifiers shown \
             were captured from the devices themselves and that these devices are being \
             placed into the destruction process now. I attest that the information provided \
             on this statement is accurate to the best of my knowledge."
        );
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(
            v["signer"]["identity_assurance"],
            json!({"level": "self_asserted"})
        );
        assert_eq!(v["signer"]["full_name"], "Ana Ruiz");
        assert_eq!(v["at"], "2026-11-08T10:00:00Z");
        assert_eq!(
            v["identifiers"],
            json!({"asset_tag": "AT-1", "serial": null, "source": "scanned"})
        );
        assert_eq!(v["equipment_id"], "SHR-2");
        assert_eq!(v["location"], "Bay 3");
        let back: DestructionAttestation = serde_json::from_value(v).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn the_statement_function_matches_what_new_stores() {
        for technique in [
            DestructMethod::Shred,
            DestructMethod::Crush,
            DestructMethod::Degauss,
        ] {
            let a = DestructionAttestation::new(
                technique,
                datetime!(2026-11-08 10:00 UTC),
                " SHR-2 ",
                "Bay 3 ",
                scanned(Some("AT-1"), None),
                signer(),
            )
            .unwrap();
            assert_eq!(
                attestation_statement(technique, " Ana Ruiz ", "Bench lead", " SHR-2 ", "Bay 3 "),
                Ok(a.statement)
            );
        }
        assert_eq!(
            attestation_statement(DestructMethod::Shred, "Ana", "Lead", "SHR-2", " "),
            Err(DestructionRecordError::Blank("location"))
        );
        assert_eq!(
            attestation_statement(DestructMethod::Shred, "", "Lead", "SHR-2", "Bay 3"),
            Err(DestructionRecordError::Blank("full_name"))
        );
    }

    #[test]
    fn attestation_refuses_blank_fields_and_missing_identifiers() {
        use DestructionRecordError::*;
        let ids = || scanned(Some("AT-1"), None);
        assert_eq!(attest(" ", "Bay 3", ids()), Err(Blank("equipment_id")));
        assert_eq!(attest("SHR-2", "", ids()), Err(Blank("location")));
        assert_eq!(
            attest("SHR-2", "Bay 3", scanned(None, None)),
            Err(NoIdentifier)
        );
        assert_eq!(
            attest("SHR-2", "Bay 3", scanned(Some("  "), Some(""))),
            Err(NoIdentifier)
        );
        assert!(attest("SHR-2", "Bay 3", scanned(None, Some("SN-9"))).is_ok());
        assert_eq!(
            AttestationSigner::self_asserted(operator(), "  ", "Bench lead"),
            Err(Blank("full_name"))
        );
        assert_eq!(
            AttestationSigner::self_asserted(operator(), "Ana Ruiz", ""),
            Err(Blank("title"))
        );
        let mut s = signer();
        s.title = " ".into();
        let r = DestructionAttestation::new(
            DestructMethod::Crush,
            datetime!(2026-11-08 10:00 UTC),
            "CR-1",
            "Bay 3",
            ids(),
            s,
        );
        assert_eq!(r, Err(Blank("title")));
    }

    #[test]
    fn completion_renders_with_and_without_video() {
        let at = datetime!(2026-11-08 12:30:00 +2);
        let c = DestructionCompletion::new(
            at,
            verification(true, true, "platters will not engage"),
            Some("CAM-4/2026-11-08T10:30"),
            signer(),
        )
        .unwrap();
        assert_eq!(
            c.statement,
            "Destruction of the device(s) listed above was completed on \
             2026-11-08T10:30:00Z. Verification: the remnants were inspected and the \
             equipment SHR-2 completed its cycle; the result meets platters will not engage. \
             Video reference: CAM-4/2026-11-08T10:30."
        );
        let none = DestructionCompletion::new(
            at,
            verification(true, true, "particle size ≤ 2 mm"),
            Some("   "),
            signer(),
        )
        .unwrap();
        assert_eq!(none.video_ref, None);
        assert!(!none.statement.contains("Video reference"));
        assert!(none
            .statement
            .ends_with("the result meets particle size ≤ 2 mm."));
        let v = serde_json::to_value(&none).unwrap();
        assert_eq!(v["video_ref"], json!(null));
        assert_eq!(v["verification"]["remnants_inspected"], true);
        assert_eq!(v["verification"]["equipment_cycle_completed"], true);
    }

    #[test]
    fn completion_refuses_unverified_runs() {
        use DestructionRecordError::*;
        let at = datetime!(2026-11-08 10:00 UTC);
        let complete = |v| DestructionCompletion::new(at, v, None, signer());
        assert_eq!(
            complete(verification(false, true, "c")),
            Err(RemnantsNotInspected)
        );
        assert_eq!(
            complete(verification(true, false, "c")),
            Err(EquipmentCycleNotCompleted)
        );
        assert_eq!(
            complete(verification(true, true, " ")),
            Err(Blank("criterion"))
        );
        let mut v = verification(true, true, "c");
        v.equipment_id = "".into();
        assert_eq!(complete(v), Err(Blank("equipment_id")));
    }

    #[test]
    fn concurrence_kind_fixes_the_statement() {
        let id = Uuid::parse_str("6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f").unwrap();
        let at = datetime!(2026-11-08 10:00 UTC);
        let review = Concurrence::new(ConcurrenceKind::Review, id, at, signer()).unwrap();
        assert_eq!(
            review.statement,
            "I, Ana Ruiz, Bench lead, reviewed the destruction records and verification \
             evidence for manifest 6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f and concur that the \
             devices listed were destroyed as stated."
        );
        assert!(!review.statement.contains("witnessed"));
        let witness = Concurrence::new(ConcurrenceKind::Witness, id, at, signer()).unwrap();
        assert_eq!(
            witness.statement,
            "I, Ana Ruiz, Bench lead, witnessed the destruction for manifest \
             6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f and concur that the devices listed were \
             destroyed as stated."
        );
        assert_eq!(serde_json::to_value(&witness).unwrap()["kind"], "witness");
    }

    #[test]
    fn the_completion_statement_function_matches_what_new_stores() {
        let at = datetime!(2026-11-08 12:30:00 +2);
        for video in [Some(" CAM-3 14:02 "), Some("  "), None] {
            let mut v = verification(true, true, " particles ≤ 2 mm ");
            v.equipment_id = " SHR-2 ".into();
            let c = DestructionCompletion::new(at, v, video, signer()).unwrap();
            assert_eq!(
                completion_statement(at, " SHR-2 ", " particles ≤ 2 mm ", video),
                Ok(c.statement)
            );
        }
        assert_eq!(
            completion_statement(at, " ", "c", None),
            Err(DestructionRecordError::Blank("equipment_id"))
        );
        assert_eq!(
            completion_statement(at, "SHR-2", "", None),
            Err(DestructionRecordError::Blank("criterion"))
        );
    }

    #[test]
    fn the_concurrence_statement_function_matches_what_new_stores() {
        let id = Uuid::new_v4();
        let at = datetime!(2026-11-08 10:00 UTC);
        for kind in [ConcurrenceKind::Review, ConcurrenceKind::Witness] {
            let c = Concurrence::new(kind, id, at, signer()).unwrap();
            assert_eq!(
                concurrence_statement(kind, id, " Ana Ruiz ", "Bench lead "),
                Ok(c.statement)
            );
        }
        assert_eq!(
            concurrence_statement(ConcurrenceKind::Review, id, "", "Lead"),
            Err(DestructionRecordError::Blank("full_name"))
        );
        assert_eq!(
            concurrence_statement(ConcurrenceKind::Witness, id, "Ana", " "),
            Err(DestructionRecordError::Blank("title"))
        );
    }

    #[test]
    fn attestation_trims_identifiers_and_records_their_source() {
        let ids = RecordedIdentifiers {
            asset_tag: Some(" AT-1 ".into()),
            serial: Some("   ".into()),
            source: IdentifierSource::Device,
        };
        let a = attest("SHR-2", "Bay 3", ids).unwrap();
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(
            v["identifiers"],
            json!({"asset_tag": "AT-1", "serial": null, "source": "device"})
        );
    }

    #[test]
    fn concurrence_refuses_a_blank_signer_name() {
        let mut s = signer();
        s.full_name = "  ".into();
        let r = Concurrence::new(
            ConcurrenceKind::Review,
            Uuid::new_v4(),
            datetime!(2026-11-08 10:00 UTC),
            s,
        );
        assert_eq!(r, Err(DestructionRecordError::Blank("full_name")));
    }

    #[test]
    fn same_signer_matches_operator_id_or_normalized_name() {
        let other_op = OperatorRef {
            id: "op-2".into(),
            display_name: "Bo".into(),
            email: "bo@example.com".into(),
        };
        let a = signer();
        let same_id = AttestationSigner::self_asserted(operator(), "Someone Else", "QA").unwrap();
        let same_name =
            AttestationSigner::self_asserted(other_op.clone(), "  ana   RUIZ ", "QA").unwrap();
        let different = AttestationSigner::self_asserted(other_op, "Bo Chen", "QA").unwrap();
        assert!(same_signer(&a, &same_id));
        assert!(same_signer(&a, &same_name));
        assert!(!same_signer(&a, &different));
    }

    #[test]
    fn concurrence_policy_defaults_to_none() {
        assert_eq!(ConcurrencePolicy::default(), ConcurrencePolicy::None);
        assert_eq!(ConcurrencePolicy::None.required_kind(), None);
        assert_eq!(
            ConcurrencePolicy::Review.required_kind(),
            Some(ConcurrenceKind::Review)
        );
        assert_eq!(
            ConcurrencePolicy::Witness.required_kind(),
            Some(ConcurrenceKind::Witness)
        );
        assert_eq!(
            serde_json::to_value(ConcurrencePolicy::None).unwrap(),
            json!("none")
        );
    }

    #[test]
    fn identity_assurance_shapes_and_labels() {
        let v = IdentityAssurance::Verified {
            issuer: "https://idp.example.com".into(),
            subject: "u-42".into(),
        };
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            json!({"level": "verified", "issuer": "https://idp.example.com", "subject": "u-42"})
        );
        assert_eq!(v.label(), "Verified by https://idp.example.com");
        assert_eq!(IdentityAssurance::SelfAsserted.label(), "Self-asserted");
    }
}
