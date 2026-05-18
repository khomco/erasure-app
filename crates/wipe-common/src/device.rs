use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DeviceId {
    fn from(s: &str) -> Self {
        DeviceId(s.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    HddMagnetic,
    SsdSata,
    SsdNvme,
    Emmc,
    Ufs,
    UsbFlash,
    Optical,
    Tape,
    Unknown,
}

impl MediaType {
    pub fn is_flash(self) -> bool {
        matches!(
            self,
            Self::SsdSata | Self::SsdNvme | Self::Emmc | Self::Ufs | Self::UsbFlash
        )
    }

    pub fn class_label(self) -> &'static str {
        match self {
            Self::HddMagnetic => "magnetic-hdd",
            Self::SsdSata => "ssd-sata",
            Self::SsdNvme => "ssd-nvme",
            Self::Emmc => "emmc",
            Self::Ufs => "ufs",
            Self::UsbFlash => "usb-flash",
            Self::Optical => "optical",
            Self::Tape => "tape",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusType {
    Sata,
    Nvme,
    Scsi,
    Sas,
    Usb,
    Mmc,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub vendor: String,
    pub model: String,
    pub serial: String,
    pub wwn: Option<String>,
    pub capacity_bytes: u64,
    pub media_type: MediaType,
    pub bus: BusType,
    pub firmware: Option<String>,
    pub removable: bool,
    pub block_size: u32,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub ata_security: Option<AtaSecurityCaps>,
    pub nvme_sanitize: Option<NvmeSanitizeCaps>,
    pub trim: bool,
    pub crypto_erase_supported: bool,
    pub sed: SedStatus,
    pub hpa_present: bool,
    pub dco_present: bool,
    pub frozen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtaSecurityCaps {
    pub supported: bool,
    pub enhanced_supported: bool,
    pub estimated_minutes: Option<u32>,
    pub enhanced_estimated_minutes: Option<u32>,
    pub frozen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NvmeSanitizeCaps {
    pub block_erase: bool,
    pub overwrite: bool,
    pub crypto_erase: bool,
    /// No-deallocate Inhibited support (NDI)
    pub ndi_inhibited: bool,
    /// No-deallocate modifies media after sanitize (NODMMAS field, 2-bit)
    pub nodmmas: u8,
    /// Sanitize estimated time in seconds (per applicable action)
    pub estimated_block_erase_secs: Option<u32>,
    pub estimated_crypto_erase_secs: Option<u32>,
    pub estimated_overwrite_secs: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SedStatus {
    #[default]
    None,
    SupportedNotProvisioned,
    Provisioned,
    Locked,
}
