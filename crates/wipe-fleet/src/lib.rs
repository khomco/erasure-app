//! Fleet discovery + lead election over mDNS.
//!
//! Each station advertises `_wipestation._tcp.local.` with a TXT record
//! describing its role, version, port, station id, and start time. Browsing
//! the same service type populates a peer registry. The current "lead"
//! station is computed deterministically: smallest `(started_at, id)` wins,
//! which lets us elect a lead without a coordination round.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use parking_lot::RwLock;
use serde::Serialize;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use wipe_common::{StationId, StationInfo, StationRole};

pub const SERVICE_TYPE: &str = "_wipestation._tcp.local.";

#[derive(Debug, Error)]
pub enum FleetError {
    #[error("mdns daemon error: {0}")]
    Mdns(String),
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

pub type FleetResult<T> = Result<T, FleetError>;

#[derive(Debug, Clone, Serialize)]
pub enum FleetEvent {
    PeerDiscovered(StationInfo),
    PeerUpdated(StationInfo),
    PeerLost(StationId),
    LeadChanged(Option<StationId>),
}

#[derive(Clone)]
pub struct FleetService {
    inner: Arc<FleetInner>,
}

struct FleetInner {
    self_info: RwLock<StationInfo>,
    peers: RwLock<HashMap<StationId, StationInfo>>,
    daemon: ServiceDaemon,
    service_fullname: RwLock<Option<String>>,
    events: broadcast::Sender<FleetEvent>,
    last_lead: RwLock<Option<StationId>>,
}

impl FleetService {
    pub fn start(mut self_info: StationInfo) -> FleetResult<Self> {
        let daemon = ServiceDaemon::new().map_err(|e| FleetError::Mdns(e.to_string()))?;

        let hostname = ensure_dot(&self_info.hostname);
        self_info.hostname = self_info.hostname.trim_end_matches('.').to_string();
        let txt = build_txt(&self_info);
        let service_name = format!("wipestation-{}", self_info.id);

        // Build a ServiceInfo with all required fields. We let mdns-sd
        // auto-resolve our IP addresses.
        let mut svc = ServiceInfo::new(
            SERVICE_TYPE,
            &service_name,
            &hostname,
            "", // IP empty -> auto-resolve
            self_info.api_port,
            txt,
        )
        .map_err(|e| FleetError::Mdns(e.to_string()))?;
        svc = svc.enable_addr_auto();

        daemon
            .register(svc.clone())
            .map_err(|e| FleetError::Mdns(e.to_string()))?;
        let fullname = svc.get_fullname().to_string();

        let (events, _) = broadcast::channel(256);
        let inner = Arc::new(FleetInner {
            self_info: RwLock::new(self_info),
            peers: RwLock::new(HashMap::new()),
            daemon: daemon.clone(),
            service_fullname: RwLock::new(Some(fullname)),
            events,
            last_lead: RwLock::new(None),
        });

        let svc = Self {
            inner: inner.clone(),
        };
        svc.spawn_browser(daemon)?;
        Ok(svc)
    }

    fn spawn_browser(&self, daemon: ServiceDaemon) -> FleetResult<()> {
        let receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| FleetError::Mdns(e.to_string()))?;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            loop {
                match receiver.recv_async().await {
                    Ok(event) => handle_event(event, &inner),
                    Err(e) => {
                        warn!(?e, "mdns browse channel closed");
                        break;
                    }
                }
            }
        });
        Ok(())
    }

    pub fn self_id(&self) -> StationId {
        self.inner.self_info.read().id.clone()
    }

    pub fn self_info(&self) -> StationInfo {
        self.inner.self_info.read().clone()
    }

    pub fn peers(&self) -> Vec<StationInfo> {
        self.inner.peers.read().values().cloned().collect()
    }

    pub fn all_known(&self) -> Vec<StationInfo> {
        let mut out: Vec<_> = self.peers();
        out.push(self.self_info());
        out
    }

    pub fn subscribe(&self) -> broadcast::Receiver<FleetEvent> {
        self.inner.events.subscribe()
    }

    /// The currently-elected lead station id. Computed from the union of
    /// `peers + self` using the `(started_at, id)` ordering.
    pub fn current_lead(&self) -> Option<StationId> {
        let all = self.all_known();
        all.iter().min_by_key(|s| s.election_key()).map(|s| s.id.clone())
    }

    pub fn is_lead(&self) -> bool {
        self.current_lead() == Some(self.self_id())
    }

    /// Update our own active job count. Re-registers the mDNS record so
    /// peers see the change.
    pub fn update_active_jobs(&self, n: u32) -> FleetResult<()> {
        let new_info = {
            let mut info = self.inner.self_info.write();
            info.active_jobs = n;
            info.clone()
        };
        self.refresh_advertisement(new_info)
    }

    fn refresh_advertisement(&self, new_info: StationInfo) -> FleetResult<()> {
        // Unregister the old service.
        let prev = self
            .inner
            .service_fullname
            .write()
            .take()
            .unwrap_or_default();
        if !prev.is_empty() {
            let _ = self.inner.daemon.unregister(&prev);
        }
        let hostname = ensure_dot(&new_info.hostname);
        let txt = build_txt(&new_info);
        let service_name = format!("wipestation-{}", new_info.id);
        let mut svc = ServiceInfo::new(
            SERVICE_TYPE,
            &service_name,
            &hostname,
            "",
            new_info.api_port,
            txt,
        )
        .map_err(|e| FleetError::Mdns(e.to_string()))?;
        svc = svc.enable_addr_auto();
        self.inner
            .daemon
            .register(svc.clone())
            .map_err(|e| FleetError::Mdns(e.to_string()))?;
        *self.inner.service_fullname.write() = Some(svc.get_fullname().to_string());
        Ok(())
    }

    pub fn shutdown(&self) {
        let prev = self
            .inner
            .service_fullname
            .write()
            .take()
            .unwrap_or_default();
        if !prev.is_empty() {
            let _ = self.inner.daemon.unregister(&prev);
        }
        let _ = self.inner.daemon.shutdown();
    }
}

fn ensure_dot(host: &str) -> String {
    // mDNS requires the hostname to live under `.local.`.
    let trimmed = host.trim_end_matches('.');
    let base = trimmed.trim_end_matches(".local");
    format!("{base}.local.")
}

fn build_txt(info: &StationInfo) -> HashMap<String, String> {
    let mut t = HashMap::new();
    t.insert("id".into(), info.id.0.clone());
    t.insert("role".into(), role_to_str(info.role).into());
    t.insert("version".into(), info.version.clone());
    t.insert("port".into(), info.api_port.to_string());
    t.insert(
        "started".into(),
        info.started_at.unix_timestamp().to_string(),
    );
    t.insert("active".into(), info.active_jobs.to_string());
    t
}

fn role_to_str(r: StationRole) -> &'static str {
    match r {
        StationRole::Lead => "lead",
        StationRole::Member => "member",
        StationRole::Console => "console",
        StationRole::Hub => "hub",
    }
}

fn role_from_str(s: &str) -> StationRole {
    match s {
        "lead" => StationRole::Lead,
        "console" => StationRole::Console,
        "hub" => StationRole::Hub,
        _ => StationRole::Member,
    }
}

fn handle_event(event: ServiceEvent, inner: &Arc<FleetInner>) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            if let Some(station) = station_from_service(&info) {
                if station.id == inner.self_info.read().id {
                    return;
                }
                let is_new;
                {
                    let mut peers = inner.peers.write();
                    is_new = !peers.contains_key(&station.id);
                    peers.insert(station.id.clone(), station.clone());
                }
                if is_new {
                    info!(peer = %station.id, "peer discovered");
                    let _ = inner.events.send(FleetEvent::PeerDiscovered(station));
                } else {
                    let _ = inner.events.send(FleetEvent::PeerUpdated(station));
                }
                recompute_lead(inner);
            }
        }
        ServiceEvent::ServiceRemoved(_, fullname) => {
            if let Some(station_id) = id_from_fullname(&fullname) {
                let mut peers = inner.peers.write();
                if peers.remove(&station_id).is_some() {
                    drop(peers);
                    debug!(peer = %station_id, "peer lost");
                    let _ = inner.events.send(FleetEvent::PeerLost(station_id));
                    recompute_lead(inner);
                }
            }
        }
        _ => {}
    }
}

fn recompute_lead(inner: &Arc<FleetInner>) {
    let all: Vec<StationInfo> = inner
        .peers
        .read()
        .values()
        .cloned()
        .chain(std::iter::once(inner.self_info.read().clone()))
        .collect();
    let lead = all
        .iter()
        .min_by_key(|s| s.election_key())
        .map(|s| s.id.clone());
    let mut last = inner.last_lead.write();
    if *last != lead {
        *last = lead.clone();
        let _ = inner.events.send(FleetEvent::LeadChanged(lead));
    }
}

fn id_from_fullname(fullname: &str) -> Option<StationId> {
    // e.g. "wipestation-<id>._wipestation._tcp.local."
    let name = fullname.split('.').next()?;
    let id = name.strip_prefix("wipestation-")?;
    Some(StationId(id.to_string()))
}

fn station_from_service(info: &ServiceInfo) -> Option<StationInfo> {
    let txt = info.get_properties();
    let id = txt.get_property_val_str("id").map(|s| StationId(s.to_string()))?;
    let role = txt
        .get_property_val_str("role")
        .map(role_from_str)
        .unwrap_or(StationRole::Member);
    let version = txt
        .get_property_val_str("version")
        .unwrap_or("0.0.0")
        .to_string();
    let port: u16 = txt
        .get_property_val_str("port")
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| info.get_port());
    let started_unix: i64 = txt
        .get_property_val_str("started")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let active: u32 = txt
        .get_property_val_str("active")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Some(StationInfo {
        id,
        hostname: info.get_hostname().trim_end_matches('.').to_string(),
        role,
        version,
        api_port: port,
        started_at: OffsetDateTime::from_unix_timestamp(started_unix).ok()?,
        active_jobs: active,
        last_seen: Some(OffsetDateTime::now_utc()),
    })
}

/// Convenience: wait until at least `min_peers` peers have been discovered,
/// or `timeout` elapses. Used in tests and demo scripts.
pub async fn wait_for_peers(svc: &FleetService, min_peers: usize, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if svc.peers().len() >= min_peers {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
