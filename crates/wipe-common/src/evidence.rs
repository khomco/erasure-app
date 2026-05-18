use serde::{Deserialize, Serialize};

/// A single command-level evidence record.
///
/// The differentiator: we capture the actual issued command bytes and the
/// device's response. Auditors can read the underlying proof rather than
/// trusting a marketing-summary claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEvidence {
    /// Logical interface: "nvme-admin", "ata-passthrough", "scsi", "block-write", "sysfs-read".
    pub interface: String,
    /// NVMe admin opcode (e.g. 0x84 for Sanitize) or ATA command byte.
    pub opcode: Option<u8>,
    /// NVMe Sanitize action (0x01..0x04) or similar sub-action.
    pub action: Option<u8>,
    /// Raw command descriptor block where applicable, hex-encoded by serializer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "crate::serde_hex_opt")]
    pub raw_cdb: Option<Vec<u8>>,
    /// Device-returned status field.
    pub status: Option<u16>,
    /// SCSI sense / NVMe completion error bits, raw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "crate::serde_hex_opt")]
    pub sense: Option<Vec<u8>>,
    /// Captured log page bytes (e.g. NVMe Get Log Page 0x81 Sanitize Status).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "crate::serde_hex_opt")]
    pub log_page: Option<Vec<u8>>,
    pub duration_ms: u64,
    pub note: Option<String>,
}

/// Verification: post-erasure sampled reads + entropy check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub method: VerificationMethod,
    pub sample_count: u32,
    pub bytes_sampled: u64,
    pub samples: Vec<SampleResult>,
    pub all_passed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationMethod {
    /// Read N random regions, hash each, check against the expected pattern.
    SampledPattern,
    /// Read N random regions, compute Shannon entropy; for crypto-erased media we
    /// expect ~8 bits/byte randomness.
    SampledEntropy,
    /// Full surface read-back.
    FullReadback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleResult {
    pub offset_bytes: u64,
    pub size_bytes: u32,
    pub sha256_hex: String,
    pub entropy_bits_per_byte: f32,
    pub passed: bool,
}

