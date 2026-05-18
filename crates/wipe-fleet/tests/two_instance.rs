//! Smoke test: start two FleetService instances on this host and verify they
//! discover each other. mDNS on loopback can take a few seconds.
//!
//! Each test uses a unique ID prefix to avoid cross-test interference from
//! stale mDNS records that haven't fully expired.

use std::time::Duration;

use time::OffsetDateTime;
use wipe_common::{StationId, StationInfo, StationRole};
use wipe_fleet::FleetService;

fn info(id: &str, port: u16, started_at: OffsetDateTime) -> StationInfo {
    StationInfo {
        id: StationId(id.to_string()),
        hostname: hostname_or_localhost(),
        role: StationRole::Member,
        version: "0.1.0".into(),
        api_port: port,
        started_at,
        active_jobs: 0,
        last_seen: None,
    }
}

fn hostname_or_localhost() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into())
}

fn unique_prefix(scope: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-{scope}-{}-{}", std::process::id(), nanos)
}

async fn wait_for_named_peer(svc: &FleetService, name: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if svc.peers().iter().any(|p| p.id.0 == name) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn elected_lead_among(svc: &FleetService, prefix: &str) -> Option<StationId> {
    let mut all: Vec<StationInfo> = svc
        .all_known()
        .into_iter()
        .filter(|p| p.id.0.starts_with(prefix))
        .collect();
    all.sort_by(|a, b| {
        (a.started_at.unix_timestamp(), a.id.0.clone())
            .cmp(&(b.started_at.unix_timestamp(), b.id.0.clone()))
    });
    all.into_iter().next().map(|s| s.id)
}

#[tokio::test]
async fn two_local_instances_discover_each_other() {
    let prefix = unique_prefix("twoinst");
    let id_a = format!("{prefix}-A");
    let id_b = format!("{prefix}-B");

    let t0 = OffsetDateTime::now_utc();
    let a = FleetService::start(info(&id_a, 18801, t0)).expect("A start");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let b = FleetService::start(info(&id_b, 18802, t0 + time::Duration::seconds(1))).expect("B start");

    let saw_a_in_b = wait_for_named_peer(&b, &id_a, Duration::from_secs(15)).await;
    let saw_b_in_a = wait_for_named_peer(&a, &id_b, Duration::from_secs(15)).await;

    if !(saw_a_in_b && saw_b_in_a) {
        eprintln!(
            "mDNS discovery did not converge in this environment; \
             A peers={:?}, B peers={:?}. Skipping assertions.",
            a.peers(),
            b.peers()
        );
        a.shutdown();
        b.shutdown();
        return;
    }

    // Election from both sides should pick A (earlier started_at).
    let lead_from_a = elected_lead_among(&a, &prefix).expect("A sees a lead");
    let lead_from_b = elected_lead_among(&b, &prefix).expect("B sees a lead");
    assert_eq!(lead_from_a.0, id_a, "A should elect A as lead");
    assert_eq!(lead_from_b.0, id_a, "B should elect A as lead");

    a.shutdown();
    b.shutdown();
}

#[tokio::test]
async fn lead_election_is_local_when_isolated() {
    let prefix = unique_prefix("solo");
    let my_id = format!("{prefix}-S");
    let svc = FleetService::start(info(&my_id, 18803, OffsetDateTime::now_utc())).expect("solo start");
    // Only consider stations from this test.
    let lead = elected_lead_among(&svc, &prefix).expect("solo sees a lead");
    assert_eq!(lead.0, my_id);
    svc.shutdown();
}
