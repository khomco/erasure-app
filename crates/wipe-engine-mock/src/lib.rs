//! Mock device backend for tests, demos, and headless CI.
//!
//! The mock simulates a small fleet of realistic-looking devices and times
//! out erasures over wall-clock seconds, emitting plausible command evidence.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use parking_lot::Mutex;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use wipe_common::{
    AtaSecurityCaps, BusType, Capabilities, CommandEvidence, Device, DeviceId, MediaType, Method,
    NvmeSanitizeCaps, SedStatus, VerificationMethod, VerificationReport, WipeError, WipeResult,
    SampleResult,
};
use wipe_engine::{BackendHandle, BackendProgress, DeviceBackend};

/// Configuration knob — speed up simulations in tests.
#[derive(Debug, Clone, Copy)]
pub struct MockTiming {
    pub nvme_sanitize_secs: f32,
    pub ata_secure_erase_secs: f32,
    pub block_overwrite_secs: f32,
    /// If true, all backends share a deterministic random seed so verification
    /// reports compare equal across runs.
    pub deterministic: bool,
}

impl Default for MockTiming {
    fn default() -> Self {
        Self {
            nvme_sanitize_secs: 2.0,
            ata_secure_erase_secs: 3.0,
            block_overwrite_secs: 4.0,
            deterministic: true,
        }
    }
}

impl MockTiming {
    pub fn fast() -> Self {
        Self {
            nvme_sanitize_secs: 0.3,
            ata_secure_erase_secs: 0.5,
            block_overwrite_secs: 0.6,
            deterministic: true,
        }
    }
}

pub struct MockBackend {
    devices: Vec<Device>,
    caps: HashMap<DeviceId, Capabilities>,
    state: Arc<Mutex<MockState>>,
    timing: MockTiming,
}

#[derive(Default)]
struct MockState {
    ops: HashMap<Uuid, OperationState>,
}

#[derive(Clone)]
struct OperationState {
    device: DeviceId,
    method: Method,
    started_at: OffsetDateTime,
    duration_secs: f32,
    /// If set, the operation will fail at this fraction.
    fail_at: Option<f32>,
    /// If true, this is an aborted operation.
    aborted: bool,
}

impl MockBackend {
    /// Construct with the default catalog of simulated devices.
    pub fn default_catalog() -> Self {
        Self::with_catalog(default_devices(), MockTiming::default())
    }

    pub fn fast_catalog() -> Self {
        Self::with_catalog(default_devices(), MockTiming::fast())
    }

    pub fn with_catalog(devices: Vec<Device>, timing: MockTiming) -> Self {
        let mut caps = HashMap::new();
        for d in &devices {
            caps.insert(d.id.clone(), default_caps_for(d));
        }
        Self {
            devices,
            caps,
            state: Arc::new(Mutex::new(MockState::default())),
            timing,
        }
    }

    /// Inject capability overrides for a specific device — used in tests
    /// to simulate frozen drives, missing Sanitize, etc.
    pub fn override_caps(&mut self, id: &DeviceId, caps: Capabilities) {
        self.caps.insert(id.clone(), caps);
    }

    fn duration_for(&self, method: &Method) -> f32 {
        match method {
            Method::NvmeSanitizeBlockErase { .. }
            | Method::NvmeSanitizeCryptoErase { .. }
            | Method::NvmeSanitizeOverwrite { .. } => self.timing.nvme_sanitize_secs,
            Method::AtaSecureErase { .. } | Method::OpalRevert => {
                self.timing.ata_secure_erase_secs
            }
            Method::BlockOverwrite { passes, .. } => {
                self.timing.block_overwrite_secs * (*passes as f32).max(1.0)
            }
            Method::Destroy { .. } => 0.1,
        }
    }
}

#[async_trait]
impl DeviceBackend for MockBackend {
    async fn enumerate(&self) -> WipeResult<Vec<Device>> {
        Ok(self.devices.clone())
    }

    async fn capabilities(&self, id: &DeviceId) -> WipeResult<Capabilities> {
        self.caps
            .get(id)
            .cloned()
            .ok_or_else(|| WipeError::DeviceNotFound(id.to_string()))
    }

    async fn unfreeze(&self, _id: &DeviceId) -> WipeResult<()> {
        // Simulate the standard suspend/resume cycle.
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(())
    }

    async fn issue(&self, id: &DeviceId, method: &Method) -> WipeResult<BackendHandle> {
        if !self.caps.contains_key(id) {
            return Err(WipeError::DeviceNotFound(id.to_string()));
        }
        let op_id = Uuid::new_v4();
        let started_at = OffsetDateTime::now_utc();
        let duration_secs = self.duration_for(method);

        // Built-in failure injection: a device whose serial contains "FAIL"
        // fails at 70%.
        let fail_at = self
            .devices
            .iter()
            .find(|d| d.id == *id)
            .filter(|d| d.serial.contains("FAIL"))
            .map(|_| 0.7_f32);

        self.state.lock().ops.insert(
            op_id,
            OperationState {
                device: id.clone(),
                method: method.clone(),
                started_at,
                duration_secs,
                fail_at,
                aborted: false,
            },
        );

        let evidence = issue_evidence(method);
        Ok(BackendHandle {
            id: op_id,
            device: id.clone(),
            method: method.clone(),
            issued_evidence: evidence,
        })
    }

    async fn poll(&self, handle: &BackendHandle) -> WipeResult<BackendProgress> {
        let op = self
            .state
            .lock()
            .ops
            .get(&handle.id)
            .cloned()
            .ok_or_else(|| WipeError::InvalidState(format!("op {} not found", handle.id)))?;

        if op.aborted {
            return Ok(BackendProgress::Failed {
                evidence: complete_evidence(&op.method, "aborted"),
                reason: "aborted by operator".into(),
            });
        }

        let elapsed = (OffsetDateTime::now_utc() - op.started_at).as_seconds_f32();
        let fraction = (elapsed / op.duration_secs).clamp(0.0, 1.0);

        if let Some(fail_at) = op.fail_at {
            if fraction >= fail_at {
                self.state.lock().ops.remove(&handle.id);
                return Ok(BackendProgress::Failed {
                    evidence: complete_evidence(&op.method, "device returned error"),
                    reason: format!(
                        "simulated firmware error at {:.0}% of operation",
                        fail_at * 100.0
                    ),
                });
            }
        }

        if fraction >= 1.0 {
            self.state.lock().ops.remove(&handle.id);
            Ok(BackendProgress::Completed {
                final_evidence: complete_evidence(&op.method, "ok"),
            })
        } else {
            let bytes_total: u64 = self
                .devices
                .iter()
                .find(|d| d.id == op.device)
                .map(|d| d.capacity_bytes)
                .unwrap_or(0);
            Ok(BackendProgress::InProgress {
                fraction,
                eta_seconds: Some(((1.0 - fraction) * op.duration_secs).max(0.0) as u64),
                bytes_processed: Some((bytes_total as f32 * fraction) as u64),
                latest_evidence: Some(progress_evidence(&op.method, fraction)),
            })
        }
    }

    async fn cancel(&self, handle: &BackendHandle) -> WipeResult<()> {
        if let Some(op) = self.state.lock().ops.get_mut(&handle.id) {
            op.aborted = true;
        }
        Ok(())
    }

    async fn verify(
        &self,
        id: &DeviceId,
        method: VerificationMethod,
        samples: u32,
    ) -> WipeResult<VerificationReport> {
        let device = self
            .devices
            .iter()
            .find(|d| d.id == *id)
            .ok_or_else(|| WipeError::DeviceNotFound(id.to_string()))?;

        let seed = if self.timing.deterministic {
            // Deterministic per-device seed so tests are stable.
            let mut hasher = Sha256::new();
            hasher.update(device.serial.as_bytes());
            let digest = hasher.finalize();
            u64::from_le_bytes(digest[..8].try_into().unwrap())
        } else {
            rand::random()
        };
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

        let sample_size: u32 = 1024 * 4; // 4 KiB
        let mut samples_out = Vec::with_capacity(samples as usize);
        for _ in 0..samples {
            let offset: u64 = if device.capacity_bytes > sample_size as u64 {
                rng.gen_range(0..(device.capacity_bytes - sample_size as u64))
            } else {
                0
            };
            // Simulate sampled bytes. For a successful Purge/Clear, pattern is all-zero.
            let bytes = match method {
                VerificationMethod::SampledPattern => vec![0u8; sample_size as usize],
                VerificationMethod::SampledEntropy => {
                    (0..sample_size).map(|_| rng.gen::<u8>()).collect()
                }
                VerificationMethod::FullReadback => vec![0u8; sample_size as usize],
            };
            let sha = Sha256::digest(&bytes);
            let entropy = shannon_entropy_bits_per_byte(&bytes);
            let passed = match method {
                VerificationMethod::SampledPattern => bytes.iter().all(|b| *b == 0),
                VerificationMethod::SampledEntropy => entropy > 7.0,
                VerificationMethod::FullReadback => bytes.iter().all(|b| *b == 0),
            };
            samples_out.push(SampleResult {
                offset_bytes: offset,
                size_bytes: sample_size,
                sha256_hex: hex::encode(sha),
                entropy_bits_per_byte: entropy,
                passed,
            });
        }
        let all_passed = samples_out.iter().all(|s| s.passed);
        Ok(VerificationReport {
            method,
            sample_count: samples,
            bytes_sampled: (samples as u64) * (sample_size as u64),
            samples: samples_out,
            all_passed,
        })
    }
}

fn shannon_entropy_bits_per_byte(data: &[u8]) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let total = data.len() as f32;
    let mut entropy = 0.0_f32;
    for c in counts.iter().filter(|&&c| c > 0) {
        let p = *c as f32 / total;
        entropy -= p * p.log2();
    }
    entropy
}

fn issue_evidence(method: &Method) -> CommandEvidence {
    match method {
        Method::NvmeSanitizeBlockErase { ause, no_deallocate } => CommandEvidence {
            interface: "nvme-admin".into(),
            opcode: Some(0x84),
            action: Some(0x02),
            raw_cdb: Some(vec![
                0x84,
                if *ause { 0x01 } else { 0x00 },
                0x02,
                if *no_deallocate { 0x01 } else { 0x00 },
            ]),
            status: Some(0),
            sense: None,
            log_page: None,
            duration_ms: 0,
            note: Some("issued (mock)".into()),
        },
        Method::NvmeSanitizeCryptoErase { ause, no_deallocate } => CommandEvidence {
            interface: "nvme-admin".into(),
            opcode: Some(0x84),
            action: Some(0x04),
            raw_cdb: Some(vec![
                0x84,
                if *ause { 0x01 } else { 0x00 },
                0x04,
                if *no_deallocate { 0x01 } else { 0x00 },
            ]),
            status: Some(0),
            sense: None,
            log_page: None,
            duration_ms: 0,
            note: Some("issued (mock)".into()),
        },
        Method::NvmeSanitizeOverwrite { pattern_u32, .. } => CommandEvidence {
            interface: "nvme-admin".into(),
            opcode: Some(0x84),
            action: Some(0x03),
            raw_cdb: Some(pattern_u32.to_le_bytes().to_vec()),
            status: Some(0),
            sense: None,
            log_page: None,
            duration_ms: 0,
            note: Some("issued (mock)".into()),
        },
        Method::AtaSecureErase { enhanced } => CommandEvidence {
            interface: "ata-passthrough".into(),
            opcode: Some(if *enhanced { 0xF4 } else { 0xF1 }),
            action: None,
            raw_cdb: Some(vec![if *enhanced { 0xF4 } else { 0xF1 }, 0x00]),
            status: Some(0),
            sense: None,
            log_page: None,
            duration_ms: 0,
            note: Some("issued (mock)".into()),
        },
        Method::BlockOverwrite { passes, .. } => CommandEvidence {
            interface: "block-write".into(),
            opcode: None,
            action: None,
            raw_cdb: None,
            status: Some(0),
            sense: None,
            log_page: None,
            duration_ms: 0,
            note: Some(format!("block overwrite, {passes} pass(es) (mock)")),
        },
        Method::OpalRevert => CommandEvidence {
            interface: "tcg-opal".into(),
            opcode: None,
            action: None,
            raw_cdb: None,
            status: Some(0),
            sense: None,
            log_page: None,
            duration_ms: 0,
            note: Some("opal revert (mock)".into()),
        },
        Method::Destroy { method } => CommandEvidence {
            interface: "manual".into(),
            opcode: None,
            action: None,
            raw_cdb: None,
            status: Some(0),
            sense: None,
            log_page: None,
            duration_ms: 0,
            note: Some(format!("destroy intent: {:?} (mock)", method)),
        },
    }
}

fn progress_evidence(method: &Method, fraction: f32) -> CommandEvidence {
    let mut ev = issue_evidence(method);
    ev.note = Some(format!("progress {:.0}% (mock)", fraction * 100.0));
    // Simulate Get Log Page 0x81 (Sanitize Status) — bytes 0-1 progress, byte 2 status.
    ev.log_page = Some({
        let p = (fraction * 65535.0) as u16;
        let mut buf = vec![0u8; 8];
        buf[0..2].copy_from_slice(&p.to_le_bytes());
        buf[2] = 0x02; // in-progress
        buf
    });
    ev
}

fn complete_evidence(method: &Method, note: &str) -> CommandEvidence {
    let mut ev = issue_evidence(method);
    ev.note = Some(format!("{note} (mock)"));
    ev.log_page = Some({
        let mut buf = vec![0u8; 8];
        buf[0..2].copy_from_slice(&65535_u16.to_le_bytes());
        buf[2] = 0x01; // completed successfully
        buf
    });
    ev.duration_ms = 0;
    ev
}

/// Public alias for test/demo code that wants the canonical mock catalog.
pub fn default_devices_public() -> Vec<Device> {
    default_devices()
}

fn default_devices() -> Vec<Device> {
    vec![
        Device {
            id: DeviceId("dev-nvme-0".into()),
            vendor: "Samsung".into(),
            model: "MZ-V9P2T0".into(),
            serial: "S6ABNX0W123456".into(),
            wwn: Some("eui.0025385811b1d567".into()),
            capacity_bytes: 2_000_000_000_000,
            media_type: MediaType::SsdNvme,
            bus: BusType::Nvme,
            firmware: Some("4B2QGXA7".into()),
            removable: false,
            block_size: 512,
            path: "/dev/nvme0n1".into(),
        },
        Device {
            id: DeviceId("dev-nvme-1".into()),
            vendor: "Western Digital".into(),
            model: "WDS500G3X0E".into(),
            serial: "21030L800123".into(),
            wwn: Some("eui.0014ee21ab12cd34".into()),
            capacity_bytes: 500_000_000_000,
            media_type: MediaType::SsdNvme,
            bus: BusType::Nvme,
            firmware: Some("613300WD".into()),
            removable: false,
            block_size: 512,
            path: "/dev/nvme1n1".into(),
        },
        Device {
            id: DeviceId("dev-sata-0".into()),
            vendor: "Crucial".into(),
            model: "MX500 1TB".into(),
            serial: "203440FAILSIM".into(), // marked for simulated failure
            wwn: None,
            capacity_bytes: 1_000_000_000_000,
            media_type: MediaType::SsdSata,
            bus: BusType::Sata,
            firmware: Some("M3CR046".into()),
            removable: false,
            block_size: 512,
            path: "/dev/sda".into(),
        },
        Device {
            id: DeviceId("dev-hdd-0".into()),
            vendor: "Seagate".into(),
            model: "ST8000NM0055".into(),
            serial: "ZA1FH123".into(),
            wwn: None,
            capacity_bytes: 8_000_000_000_000,
            media_type: MediaType::HddMagnetic,
            bus: BusType::Sata,
            firmware: Some("SN03".into()),
            removable: false,
            block_size: 4096,
            path: "/dev/sdb".into(),
        },
    ]
}

fn default_caps_for(device: &Device) -> Capabilities {
    match device.media_type {
        MediaType::SsdNvme => Capabilities {
            ata_security: None,
            nvme_sanitize: Some(NvmeSanitizeCaps {
                block_erase: true,
                overwrite: true,
                crypto_erase: true,
                ndi_inhibited: false,
                nodmmas: 0,
                estimated_block_erase_secs: Some(2),
                estimated_crypto_erase_secs: Some(1),
                estimated_overwrite_secs: Some(900),
            }),
            trim: true,
            crypto_erase_supported: true,
            sed: SedStatus::Provisioned,
            hpa_present: false,
            dco_present: false,
            frozen: false,
        },
        MediaType::SsdSata => Capabilities {
            ata_security: Some(AtaSecurityCaps {
                supported: true,
                enhanced_supported: true,
                estimated_minutes: Some(2),
                enhanced_estimated_minutes: Some(2),
                frozen: false,
            }),
            nvme_sanitize: None,
            trim: true,
            crypto_erase_supported: false,
            sed: SedStatus::None,
            hpa_present: false,
            dco_present: false,
            frozen: false,
        },
        MediaType::HddMagnetic => Capabilities {
            ata_security: Some(AtaSecurityCaps {
                supported: true,
                enhanced_supported: true,
                estimated_minutes: Some(420),
                enhanced_estimated_minutes: Some(450),
                frozen: false,
            }),
            nvme_sanitize: None,
            trim: false,
            crypto_erase_supported: false,
            sed: SedStatus::None,
            hpa_present: false,
            dco_present: false,
            frozen: false,
        },
        _ => Capabilities::default(),
    }
}
