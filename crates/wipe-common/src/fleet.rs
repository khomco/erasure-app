use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Stable per-station identity, persisted across reboots in installed mode
/// and bound to the boot session in PXE mode.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StationId(pub String);

impl StationId {
    pub fn new_random() -> Self {
        StationId(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for StationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationRole {
    /// Holds canonical config + audit log for the site.
    Lead,
    /// Regular station; pulls config from the lead.
    Member,
    /// Operator console; not a wipestation but participates in discovery.
    Console,
    /// Multi-site coordinator (self-hosted or cloud).
    Hub,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationInfo {
    pub id: StationId,
    pub hostname: String,
    pub role: StationRole,
    pub version: String,
    pub api_port: u16,
    /// Sortable stable timestamp used to break ties in lead election.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// Number of in-flight jobs.
    pub active_jobs: u32,
    /// Last heartbeat seen by the peer doing the reporting.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen: Option<OffsetDateTime>,
}

impl StationInfo {
    /// Lead-election ordering: prefer earlier-started station, then lex-min id.
    /// Lower value = higher priority for lead.
    pub fn election_key(&self) -> (i64, &str) {
        (self.started_at.unix_timestamp(), self.id.0.as_str())
    }
}
