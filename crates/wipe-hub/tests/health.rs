//! `GET /api/health` over real HTTP.
//!
//! The degraded tests need no network: they point the hub at closed ports or
//! at an in-test fake SMTP server. The green test needs `docker compose up -d`
//! and fails (never skips) when the compose services are unreachable.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use wipe_hub::health::{Component, HealthReport, Status};
use wipe_hub::{AppState, Args, Config};

/// A port with nothing listening on it.
async fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

/// A fake SMTP server that greets every connection with `greeting`
/// (or never says anything, when `greeting` is `None`).
async fn fake_smtp(greeting: Option<&'static [u8]>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                if let Some(g) = greeting {
                    let _ = s.write_all(g).await;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
                drop(s);
            });
        }
    });
    port
}

fn config(database_url: &str, smtp_port: u16) -> Config {
    let args = Args {
        addr: "127.0.0.1:0".parse().unwrap(),
        database_url: database_url.to_string(),
        smtp_host: "127.0.0.1".into(),
        smtp_port,
        public_base_url: wipe_hub::config::DEFAULT_PUBLIC_BASE_URL.into(),
        payment_provider: wipe_hub::PaymentProvider::Fake,
        stripe_webhook_secret: None,
        vendor_root_seed: None,
    };
    Config::from_args(args).unwrap()
}

async fn spawn_hub(config: Config) -> SocketAddr {
    let state = AppState::new(config).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(wipe_hub::serve_on(listener, state));
    addr
}

async fn get_health(addr: SocketAddr) -> (u16, HealthReport, Duration) {
    let started = Instant::now();
    let resp = reqwest::get(format!("http://{addr}/api/health"))
        .await
        .expect("hub did not answer");
    let code = resp.status().as_u16();
    let report = resp.json::<HealthReport>().await.unwrap();
    (code, report, started.elapsed())
}

#[tokio::test]
async fn degraded_when_database_and_mail_are_down() {
    let db_port = closed_port().await;
    let smtp_port = closed_port().await;
    let addr = spawn_hub(config(
        &format!("postgres://wipehub:wipehub@127.0.0.1:{db_port}/wipehub"),
        smtp_port,
    ))
    .await;

    let (code, report, _) = get_health(addr).await;
    assert_eq!(code, 503);
    assert_eq!(
        report,
        HealthReport {
            status: Status::Degraded,
            database: Component::Down,
            mail: Component::Down,
        }
    );
}

#[tokio::test]
async fn degraded_body_is_the_documented_json() {
    let addr = spawn_hub(config(
        &format!(
            "postgres://wipehub:wipehub@127.0.0.1:{}/wipehub",
            closed_port().await
        ),
        closed_port().await,
    ))
    .await;
    let body: serde_json::Value = reqwest::get(format!("http://{addr}/api/health"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        body,
        serde_json::json!({"status": "degraded", "database": "down", "mail": "down"})
    );
}

#[tokio::test]
async fn mail_up_database_down_reports_only_the_database() {
    let smtp_port = fake_smtp(Some(b"220 fake ESMTP ready\r\n")).await;
    let addr = spawn_hub(config(
        &format!(
            "postgres://wipehub:wipehub@127.0.0.1:{}/wipehub",
            closed_port().await
        ),
        smtp_port,
    ))
    .await;
    let (code, report, _) = get_health(addr).await;
    assert_eq!(code, 503);
    assert_eq!(report.status, Status::Degraded);
    assert_eq!(report.database, Component::Down);
    assert_eq!(report.mail, Component::Up);
}

#[tokio::test]
async fn smtp_that_is_not_smtp_is_down() {
    let smtp_port = fake_smtp(Some(b"HTTP/1.1 400 Bad Request\r\n")).await;
    assert!(!wipe_hub::health::probe_smtp("127.0.0.1", smtp_port).await);
}

#[tokio::test]
async fn silent_smtp_and_database_time_out_within_bound() {
    // Both accept TCP but never speak, so only the timeout ends each probe.
    let silent_smtp = fake_smtp(None).await;
    let silent_db = fake_smtp(None).await;
    let addr = spawn_hub(config(
        &format!("postgres://wipehub:wipehub@127.0.0.1:{silent_db}/wipehub"),
        silent_smtp,
    ))
    .await;
    let (code, report, elapsed) = get_health(addr).await;
    assert_eq!(code, 503);
    assert_eq!(report.database, Component::Down);
    assert_eq!(report.mail, Component::Down);
    // Probes run concurrently, each bounded at 2 s.
    assert!(elapsed < Duration::from_millis(3500), "took {elapsed:?}");
}

/// Needs the compose services. Fails, never skips, when they are down.
#[tokio::test]
async fn green_against_compose() {
    let addr = spawn_hub(config(
        wipe_hub::config::DEFAULT_DATABASE_URL,
        wipe_hub::config::DEFAULT_SMTP_PORT,
    ))
    .await;
    let (code, report, _) = get_health(addr).await;
    let db_url = url::Url::parse(wipe_hub::config::DEFAULT_DATABASE_URL).unwrap();
    let db_addr = format!(
        "{}:{}",
        db_url.host_str().unwrap(),
        db_url.port().unwrap_or(5432)
    );
    let smtp_addr = format!(
        "{}:{}",
        wipe_hub::config::DEFAULT_SMTP_HOST,
        wipe_hub::config::DEFAULT_SMTP_PORT
    );
    assert_eq!(
        report.database,
        Component::Up,
        "Postgres is unreachable at {db_addr}: run docker compose up -d at the repo root \
         (if it is already up, credentials may be stale: docker compose down -v)"
    );
    assert_eq!(
        report.mail,
        Component::Up,
        "Mailpit SMTP is unreachable at {smtp_addr}: run docker compose up -d at the repo root"
    );
    assert_eq!(code, 200);
    assert_eq!(report.status, Status::Ok);
}
