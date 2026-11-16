//! Renewal grace on auto-renewing licences (controlled-licensing story 5,
//! ADR-0013 §4): every hub row of the story's I/O matrix through real HTTP,
//! the Fake provider standing in for Stripe. A station licence for an
//! active, renewing annual subscription runs 30 days past its paid term and
//! records that term as `paid_through`; cancelling or the provider ending
//! the subscription withdraws the grace in the same transaction; a station's
//! own fetch withdraws it when that could not happen; a failed renewal is
//! recorded, shown and emailed to the owners once. Needs `docker compose up
//! -d`; fails, never skips, when it is down.

mod support;

use std::time::Duration;

use reqwest::{Response, StatusCode};
use serde_json::{json, Value};
use support::hub::{mail_texts, spawn_hub_with_root, Hub, Who, BASE};
use support::Mailpit;
use time::OffsetDateTime;
use uuid::Uuid;
use wipe_cert::SigningKey;
use wipe_hub::mail::RENEWAL_FAILED_SUBJECT;
use wipe_hub::order_routes::PAYMENT_WEBHOOK_PATH;
use wipe_hub::payment_events::{self, Receipt};
use wipe_hub::payments::{
    Fake, PaymentProvider, FAKE_SUBSCRIPTION_CANCELLED, FAKE_SUBSCRIPTION_ENDED,
    FAKE_SUBSCRIPTION_RENEWAL_FAILED,
};
use wipe_license::station_request::{sign, HEADER_CHALLENGE, HEADER_KEY_ID, HEADER_SIGNATURE};
use wipe_license::{
    check_installable, parse_license, parse_license_file, Entitlements, VendorRoot,
};

const CHALLENGE: &str = "/api/station/challenge";
const LICENCE: &str = "/api/station/licence";
const GRACE: time::Duration = time::Duration::days(30);

async fn ok_json(resp: Response, want: StatusCode) -> Value {
    let code = resp.status();
    let v: Value = resp.json().await.unwrap();
    assert_eq!(code, want, "{v}");
    v
}

/// An annual offering with two machine seats, inserted directly: its id.
async fn two_seat_annual(hub: &Hub) -> String {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO offerings (name, description, price_minor, currency, billing_interval, \
                                status, scope, seats, quota_erasures, features, term_months, \
                                published_at) \
         VALUES ('Station — annual, 2 seats', 'Two stations.', 240000, 'USD', 'annual', \
                 'published', 'machine', 2, NULL, '{hub_sync}', 12, now()) RETURNING id",
    )
    .fetch_one(&hub.db.pool)
    .await
    .unwrap();
    id.to_string()
}

/// `who` buys `offering` and pays on the Fake checkout page: the new
/// entitlement's id.
async fn buy(hub: &Hub, who: &Who, offering: &str) -> String {
    let v = ok_json(
        hub.post(
            "/api/checkout",
            json!({ "offering_id": offering }),
            Some(&who.cookie),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    let order = v["order"]["id"].as_str().unwrap().to_string();
    let url = url::Url::parse(v["checkout_url"].as_str().unwrap()).unwrap();
    let session = url
        .query_pairs()
        .find(|(k, _)| k == "session")
        .unwrap()
        .1
        .into_owned();
    ok_json(
        hub.post_empty(
            &format!("/api/payments/fake/sessions/{session}/pay"),
            Some(&who.cookie),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM entitlements WHERE order_id = $1::uuid")
        .bind(&order)
        .fetch_one(&hub.db.pool)
        .await
        .unwrap();
    id.to_string()
}

struct Fixture {
    hub: Hub,
    root: VendorRoot,
    owner: Who,
    /// Annual, two machine seats, renewing.
    annual: String,
}

async fn fixture() -> Fixture {
    let (hub, root) = spawn_hub_with_root().await;
    let owner = hub.verified_owner("Acme Recyclers").await;
    let offering = two_seat_annual(&hub).await;
    let annual = buy(&hub, &owner, &offering).await;
    Fixture {
        hub,
        root,
        owner,
        annual,
    }
}

/// A registered station: its key, `stations` row id and first licence id.
struct Station {
    key: SigningKey,
    id: String,
    licence: String,
}

impl Fixture {
    /// The owner registers a new key on `entitlement` as `station_id`, with
    /// no credits in the file.
    async fn register_on(&self, entitlement: &str, station_id: &str) -> Station {
        let key = SigningKey::generate();
        let v = ok_json(
            self.hub
                .post(
                    "/api/stations",
                    json!({
                        "entitlement_id": entitlement,
                        "instance_key_id": key.public_key_id(),
                        "station_id": station_id,
                    }),
                    Some(&self.owner.cookie),
                )
                .await,
            StatusCode::CREATED,
        )
        .await;
        Station {
            key,
            id: v["station"]["id"].as_str().unwrap().to_string(),
            licence: v["licence"]["id"].as_str().unwrap().to_string(),
        }
    }

    async fn register(&self, station_id: &str) -> Station {
        self.register_on(&self.annual, station_id).await
    }

    /// The licence file the owner downloads, checked to install on `key`'s
    /// station: its signed entitlements.
    async fn downloaded(&self, licence: &str, key: &SigningKey) -> Entitlements {
        let resp = self
            .hub
            .get(
                &format!("/api/licences/{licence}/download"),
                Some(&self.owner.cookie),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let file = parse_license_file(&resp.text().await.unwrap()).unwrap();
        check_installable(
            &file.license,
            &key.public_key_id(),
            &[self.root.verifying_key()],
        )
        .unwrap();
        file.license.body.entitlements
    }

    async fn term_end(&self, entitlement: &str) -> OffsetDateTime {
        let micros: i64 = sqlx::query_scalar(
            "SELECT (extract(epoch FROM term_end) * 1000000)::bigint FROM entitlements \
             WHERE id = $1::uuid",
        )
        .bind(entitlement)
        .fetch_one(&self.hub.db.pool)
        .await
        .unwrap();
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(micros) * 1000).unwrap()
    }

    async fn subscription(&self) -> String {
        sqlx::query_scalar("SELECT subscription_id FROM entitlements WHERE id = $1::uuid")
            .bind(&self.annual)
            .fetch_one(&self.hub.db.pool)
            .await
            .unwrap()
    }

    /// A Fake subscription route (`renew` or `cancel`) on the annual
    /// entitlement, as the owner's Billing buttons call it.
    async fn fake_route(&self, action: &str) -> Value {
        ok_json(
            self.hub
                .post_empty(
                    &format!("/api/payments/fake/entitlements/{}/{action}", self.annual),
                    Some(&self.owner.cookie),
                )
                .await,
            StatusCode::OK,
        )
        .await
    }

    /// A signed Fake `event_type` event for the annual subscription, posted
    /// to the webhook as the provider would.
    async fn webhook(&self, event_type: &str, invoice: Option<&str>) {
        let sub = self.subscription().await;
        let (body, signature) = Fake::signed_event_with(event_type, None, Some(&sub), invoice);
        let resp = self
            .hub
            .http
            .post(self.hub.url(PAYMENT_WEBHOOK_PATH))
            .headers(Fake::signature_headers(&signature))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// The organization's annual entitlement as the portal shows it.
    async fn entitlement(&self) -> Value {
        let v = ok_json(
            self.hub
                .get("/api/entitlements", Some(&self.owner.cookie))
                .await,
            StatusCode::OK,
        )
        .await;
        v["entitlements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == json!(self.annual))
            .unwrap()
            .clone()
    }

    async fn station(&self, id: &str) -> Value {
        let v = ok_json(
            self.hub
                .get("/api/stations", Some(&self.owner.cookie))
                .await,
            StatusCode::OK,
        )
        .await;
        v["stations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == json!(id))
            .unwrap()
            .clone()
    }

    /// Run a SQL statement on the hub's database as a test fixture would;
    /// `$1`, if used, is the annual entitlement's id.
    async fn sql(&self, statement: &str) {
        let mut q = sqlx::query(statement);
        if statement.contains("$1") {
            q = q.bind(Uuid::parse_str(&self.annual).unwrap());
        }
        q.execute(&self.hub.db.pool).await.unwrap();
    }
}

/// A fresh challenge, then the signed `POST /api/station/licence` for `s`
/// holding `held`, exactly as the station sends it.
async fn fetch(hub: &Hub, s: &Station, held: &str) -> Response {
    let v = ok_json(
        hub.http.get(hub.url(CHALLENGE)).send().await.unwrap(),
        StatusCode::OK,
    )
    .await;
    let challenge = v["challenge"].as_str().unwrap();
    let body = json!({
        "instance_public_key": s.key.verifying_key().to_base64(),
        "licence_id": held,
    })
    .to_string();
    let signed = sign(&s.key, challenge, BASE, "POST", LICENCE, body.as_bytes());
    hub.http
        .post(hub.url(LICENCE))
        .header("content-type", "application/json")
        .header(HEADER_KEY_ID, signed.instance_key_id)
        .header(HEADER_CHALLENGE, signed.challenge)
        .header(HEADER_SIGNATURE, signed.signature_b64)
        .body(body)
        .send()
        .await
        .unwrap()
}

/// The licence a 200 fetch answered, checked to install on `s`: its id and
/// signed entitlements.
async fn fetched(resp: Response, s: &Station, root: &VendorRoot) -> (String, Entitlements) {
    let v = ok_json(resp, StatusCode::OK).await;
    let licence = parse_license(&serde_json::to_string(&v["licence"]).unwrap()).unwrap();
    check_installable(&licence, &s.key.public_key_id(), &[root.verifying_key()]).unwrap();
    (licence.body.license_id, licence.body.entitlements)
}

async fn assert_current(resp: Response) {
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

// ---------------------------------------------------------------------------
// Issue

/// AC 1: an annual, renewing purchase's licence carries 30 days' grace and
/// its paid-through date; one-off and trial purchases' licences end with
/// their term and carry none.
#[tokio::test]
async fn an_annual_subscription_s_licence_carries_grace_and_one_offs_and_trials_do_not() {
    let f = fixture().await;
    let s = f.register("bench-1").await;
    let term_end = f.term_end(&f.annual).await;
    let ent = f.downloaded(&s.licence, &s.key).await;
    assert_eq!(ent.not_after, term_end + GRACE);
    assert_eq!(ent.paid_through, Some(term_end));
    // The portal shows when the grace ends.
    let e = f.entitlement().await;
    let grace_ends = OffsetDateTime::parse(
        e["grace_ends"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert_eq!(
        grace_ends.unix_timestamp(),
        (term_end + GRACE).unix_timestamp()
    );
    // The audit row records the paid-through date.
    let issued = f.hub.audited("licence.issued").await;
    assert!(issued[0].2["paid_through"].as_str().unwrap().ends_with('Z'));

    // A one-off purchase: no grace.
    let one_off = f.hub.published_one_off_offering("Site — 2 years", 24).await;
    let one_off = buy(&f.hub, &f.owner, &one_off).await;
    let o = f.register_on(&one_off, "bench-2").await;
    let ent = f.downloaded(&o.licence, &o.key).await;
    assert_eq!(ent.not_after, f.term_end(&one_off).await);
    assert_eq!(ent.paid_through, None);
    let issued = f.hub.audited("licence.issued").await;
    assert_eq!(issued.last().unwrap().2["paid_through"], Value::Null);

    // A trial: no grace.
    let trial = f.hub.published_trial_offering("Free trial").await;
    let taken = ok_json(
        f.hub
            .post(
                "/api/trials",
                json!({ "offering_id": trial }),
                Some(&f.owner.cookie),
            )
            .await,
        StatusCode::CREATED,
    )
    .await;
    let trial = taken["entitlement"]["id"].as_str().unwrap().to_string();
    assert_eq!(taken["entitlement"]["grace_ends"], Value::Null);
    let t = f.register_on(&trial, "bench-3").await;
    let ent = f.downloaded(&t.licence, &t.key).await;
    assert_eq!(ent.not_after, f.term_end(&trial).await);
    assert_eq!(ent.paid_through, None);
}

/// A paid renewal keeps the anniversary: the term grows 12 months from the
/// old end, and the fetch reissues to the new end plus grace.
#[tokio::test]
async fn a_renewal_keeps_the_anniversary_and_the_fetch_renews_with_grace() {
    let f = fixture().await;
    let s = f.register("bench-1").await;
    let old_end = f.term_end(&f.annual).await;
    f.fake_route("renew").await;
    let new_end = f.term_end(&f.annual).await;
    assert_eq!(
        new_end.date(),
        old_end.date().replace_year(old_end.year() + 1).unwrap()
    );
    assert_eq!(new_end.time(), old_end.time());
    assert_eq!(f.station(&s.id).await["renewal_available"], true);
    let (_, ent) = fetched(fetch(&f.hub, &s, &s.licence).await, &s, &f.root).await;
    assert_eq!(ent.not_after, new_end + GRACE);
    assert_eq!(ent.paid_through, Some(new_end));
    assert_eq!(f.station(&s.id).await["renewal_available"], false);
}

// ---------------------------------------------------------------------------
// Withdrawal

/// AC 2: cancelling withdraws the grace from both stations' licences in the
/// cancellation's own transaction; each station's next fetch answers the
/// trimmed licence.
#[tokio::test]
async fn cancelling_withdraws_grace_from_every_station_in_the_same_transaction() {
    let f = fixture().await;
    let a = f.register("bench-1").await;
    let b = f.register("bench-2").await;
    let term_end = f.term_end(&f.annual).await;

    let v = f.fake_route("cancel").await;
    assert_eq!(v["entitlement"]["renews"], false);
    assert_eq!(v["entitlement"]["grace_ends"], Value::Null);

    for s in [&a, &b] {
        let (id, ent) = fetched(fetch(&f.hub, s, &s.licence).await, s, &f.root).await;
        assert_ne!(id, s.licence);
        assert_eq!(ent.not_after, term_end);
        assert_eq!(ent.paid_through, None);
        // Fetching again with the trimmed licence: nothing newer.
        assert_current(fetch(&f.hub, s, &id).await).await;
        // Still registered, still not renewable.
        let st = f.station(&s.id).await;
        assert_eq!(st["released_at"], Value::Null);
        assert_eq!(st["renewal_available"], false);
        assert_eq!(st["licence"]["id"], json!(id));
    }

    let stopped = f.hub.audited("entitlement.renewal_stopped").await;
    assert_eq!(stopped.len(), 1);
    let superseded = f.hub.audited("licence.superseded").await;
    assert_eq!(superseded.len(), 2);
    for (actor, subject, detail) in &superseded {
        assert_eq!(actor, "provider:fake");
        assert_eq!(detail["reason"], "grace_withdrawn");
        assert!(
            [&a.licence, &b.licence]
                .iter()
                .any(|l| *subject == format!("licence:{l}")),
            "{subject}"
        );
    }
    // Two at registration, two withdrawals.
    let issued = f.hub.audited("licence.issued").await;
    assert_eq!(issued.len(), 4);
    for (actor, _, detail) in &issued[2..] {
        assert_eq!(actor, "provider:fake");
        assert_eq!(detail["paid_through"], Value::Null);
        assert!(detail["reissue_of"].is_string(), "{detail}");
    }
    // One transaction: the stop and every withdrawal share its `now()`.
    let times: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT occurred_at) FROM audit_log \
         WHERE action IN ('entitlement.renewal_stopped', 'licence.superseded') \
            OR (action = 'licence.issued' AND actor = 'provider:fake')",
    )
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    assert_eq!(
        times, 1,
        "the withdrawals were not in the cancellation's transaction"
    );
}

/// The provider ends the subscription after its term (failed retries): the
/// licences are reissued to end at the term, already past.
#[tokio::test]
async fn a_subscription_ended_after_its_term_is_trimmed_to_the_past_term() {
    let f = fixture().await;
    let s = f.register("bench-1").await;
    // Move the paid term into the past, as a year going by would.
    f.sql("ALTER TABLE entitlements DISABLE TRIGGER USER").await;
    f.sql(
        "UPDATE entitlements SET term_start = now() - interval '370 days', \
                                 term_end = now() - interval '5 days' \
         WHERE id = $1",
    )
    .await;
    f.sql("ALTER TABLE entitlements ENABLE TRIGGER USER").await;
    let term_end = f.term_end(&f.annual).await;
    assert!(term_end < OffsetDateTime::now_utc());

    f.webhook(FAKE_SUBSCRIPTION_ENDED, None).await;
    assert_eq!(f.entitlement().await["status"], "cancelled");
    let (_, ent) = fetched(fetch(&f.hub, &s, &s.licence).await, &s, &f.root).await;
    assert_eq!(ent.not_after, term_end);
    assert_eq!(ent.paid_through, None);
    assert!(ent.not_after < ent.not_before, "signed after its own end");
    let superseded = f.hub.audited("licence.superseded").await;
    assert_eq!(superseded.len(), 1);
    assert_eq!(superseded[0].2["reason"], "grace_withdrawn");
}

/// A cancellation applied while the hub had no vendor root still applies;
/// the station's next fetch, on a hub with one, withdraws the grace itself.
#[tokio::test]
async fn a_cancel_without_a_root_applies_and_the_station_s_fetch_withdraws_the_grace() {
    let f = fixture().await;
    let s = f.register("bench-1").await;
    let term_end = f.term_end(&f.annual).await;

    // The event applied with no vendor root, through the one store-and-apply
    // path.
    let sub = f.subscription().await;
    let (body, signature) =
        Fake::signed_event_with(FAKE_SUBSCRIPTION_CANCELLED, None, Some(&sub), None);
    let event = Fake::new(url::Url::parse(BASE).unwrap())
        .verify_event(&Fake::signature_headers(&signature), &body)
        .unwrap();
    let mut tx = f.hub.db.pool.begin().await.unwrap();
    let receipt = payment_events::receive(&mut tx, &event, &body, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(receipt, Receipt::Recorded(_)), "{receipt:?}");
    assert_eq!(f.entitlement().await["renews"], false);
    assert!(f.hub.audited("licence.superseded").await.is_empty());

    // Self-heal: the station's fetch withdraws it, audited as the station.
    let (id, ent) = fetched(fetch(&f.hub, &s, &s.licence).await, &s, &f.root).await;
    assert_eq!(ent.not_after, term_end);
    assert_eq!(ent.paid_through, None);
    let superseded = f.hub.audited("licence.superseded").await;
    assert_eq!(superseded.len(), 1);
    assert_eq!(superseded[0].0, format!("station:{}", s.id));
    assert_eq!(superseded[0].2["reason"], "grace_withdrawn");
    assert_eq!(superseded[0].2["superseded_by"], json!(id));
}

/// A station that never fetches after the cancellation keeps the grace it
/// holds; nothing at the hub can take it back offline.
#[tokio::test]
async fn an_offline_station_keeps_its_grace_licence() {
    let f = fixture().await;
    let s = f.register("bench-1").await;
    let term_end = f.term_end(&f.annual).await;
    let before = f.downloaded(&s.licence, &s.key).await;
    f.fake_route("cancel").await;
    // The file it holds still verifies and still runs to the grace end.
    assert_eq!(before.not_after, term_end + GRACE);
}

/// Resuming the renewal after a cancellation makes the grace due again:
/// `renewal_available`, and the fetch reissues with grace.
#[tokio::test]
async fn resuming_after_a_cancel_restores_the_grace() {
    let f = fixture().await;
    let s = f.register("bench-1").await;
    let term_end = f.term_end(&f.annual).await;
    f.fake_route("cancel").await;
    let (trimmed, _) = fetched(fetch(&f.hub, &s, &s.licence).await, &s, &f.root).await;
    // The owner resumes at the provider (`customer.subscription.updated`,
    // applied by `entitlements::change_renewal`, covered in `stripe.rs`).
    f.sql("UPDATE entitlements SET renews = true WHERE id = $1")
        .await;
    assert_eq!(f.station(&s.id).await["renewal_available"], true);
    let (_, ent) = fetched(fetch(&f.hub, &s, &trimmed).await, &s, &f.root).await;
    assert_eq!(ent.not_after, term_end + GRACE);
    assert_eq!(ent.paid_through, Some(term_end));
}

/// A licence issued before renewal grace existed (ending at the term) on a
/// renewing subscription is behind: the fetch reissues it with grace.
#[tokio::test]
async fn a_licence_from_before_renewal_grace_is_reissued_with_it() {
    let f = fixture().await;
    // Issued while the entitlement did not renew: exactly a pre-story
    // licence's shape.
    f.sql("UPDATE entitlements SET renews = false WHERE id = $1")
        .await;
    let s = f.register("bench-1").await;
    let term_end = f.term_end(&f.annual).await;
    let old = f.downloaded(&s.licence, &s.key).await;
    assert_eq!((old.not_after, old.paid_through), (term_end, None));
    f.sql("UPDATE entitlements SET renews = true WHERE id = $1")
        .await;
    assert_eq!(f.station(&s.id).await["renewal_available"], true);
    let (_, ent) = fetched(fetch(&f.hub, &s, &s.licence).await, &s, &f.root).await;
    assert_eq!(ent.not_after, term_end + GRACE);
    assert_eq!(ent.paid_through, Some(term_end));
    let superseded = f.hub.audited("licence.superseded").await;
    assert_eq!(superseded[0].2["reason"], "renewed");
}

// ---------------------------------------------------------------------------
// Failed renewals

/// A failed renewal is recorded, audited, shown with the grace end and
/// emailed to each verified owner once; a retry that fails again changes
/// nothing; the paid renewal clears it. No licence is touched.
#[tokio::test]
async fn a_failed_renewal_is_recorded_and_emailed_to_owners_once() {
    let f = fixture().await;
    let s = f.register("bench-1").await;
    let member = f.hub.add(&f.owner, "member").await;
    let second_owner = f.hub.add(&f.owner, "owner").await;
    f.hub.verify(&second_owner.email).await;
    let unverified_owner = f.hub.add(&f.owner, "owner").await;
    sqlx::query("UPDATE users SET email_verified_at = NULL WHERE lower(email) = lower($1)")
        .bind(&unverified_owner.email)
        .execute(&f.hub.db.pool)
        .await
        .unwrap();
    let licences = f.hub.count("licences").await;

    f.webhook(FAKE_SUBSCRIPTION_RENEWAL_FAILED, Some("fake_in_1"))
        .await;
    let e = f.entitlement().await;
    let failed_at = e["renewal_failed_at"].as_str().unwrap().to_string();
    assert!(failed_at.ends_with('Z'));
    assert!(e["grace_ends"].is_string());
    let audited = f.hub.audited("entitlement.renewal_failed").await;
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].0, "provider:fake");
    assert_eq!(audited[0].2["invoice_id"], "fake_in_1");
    assert!(audited[0].2["event_id"].is_string());

    let term_end = f.term_end(&f.annual).await;
    let grace_day = wipe_hub::entitlements::format_date((term_end + GRACE).date());
    let texts = mail_texts(&f.owner.email, RENEWAL_FAILED_SUBJECT, 1).await;
    let text = &texts[0];
    assert!(text.contains("Station — annual, 2 seats"), "{text}");
    assert!(
        text.contains(&format!(
            "Your stations keep working until {grace_day}, when new jobs start to be refused \
             unless the renewal is paid."
        )),
        "{text}"
    );
    assert!(text.contains(&format!("{BASE}/seats")), "{text}");

    // Stripe retries, and the charge fails again: unchanged, no second mail.
    f.webhook(FAKE_SUBSCRIPTION_RENEWAL_FAILED, Some("fake_in_1"))
        .await;
    assert_eq!(f.entitlement().await["renewal_failed_at"], json!(failed_at));
    assert_eq!(f.hub.audited("entitlement.renewal_failed").await.len(), 1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mailpit = Mailpit::new();
    let notices = |msgs: Vec<support::MessageSummary>| {
        msgs.into_iter()
            .filter(|m| m.subject == RENEWAL_FAILED_SUBJECT)
            .count()
    };
    assert_eq!(notices(mailpit.messages_to(&f.owner.email).await), 1);
    // Every verified owner gets exactly one; an unverified owner none.
    mail_texts(&second_owner.email, RENEWAL_FAILED_SUBJECT, 1).await;
    assert_eq!(notices(mailpit.messages_to(&second_owner.email).await), 1);
    assert_eq!(
        notices(mailpit.messages_to(&unverified_owner.email).await),
        0
    );
    // Members do not hold billing: no notice.
    assert_eq!(notices(mailpit.messages_to(&member.email).await), 0);
    // A failed renewal touches no licence.
    assert_eq!(f.hub.count("licences").await, licences);
    assert_current(fetch(&f.hub, &s, &s.licence).await).await;

    // The renewal is paid: cleared.
    f.fake_route("renew").await;
    assert_eq!(f.entitlement().await["renewal_failed_at"], Value::Null);
}

/// A failed renewal on a subscription that no longer renews is unchanged:
/// nothing recorded, nobody emailed.
#[tokio::test]
async fn a_failed_renewal_after_a_cancel_changes_nothing() {
    let f = fixture().await;
    f.fake_route("cancel").await;
    f.webhook(FAKE_SUBSCRIPTION_RENEWAL_FAILED, Some("fake_in_1"))
        .await;
    assert_eq!(f.entitlement().await["renewal_failed_at"], Value::Null);
    assert!(f.hub.audited("entitlement.renewal_failed").await.is_empty());
    let outcome: Option<String> =
        sqlx::query_scalar("SELECT outcome FROM payment_events WHERE event_type = $1")
            .bind(FAKE_SUBSCRIPTION_RENEWAL_FAILED)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(outcome.as_deref(), Some("unchanged"));
}

/// Stripe does not order events: a failure delivered after the same
/// invoice's payment was applied is stale and changes nothing.
#[tokio::test]
async fn a_failure_delivered_after_its_invoice_was_paid_changes_nothing() {
    let f = fixture().await;
    f.fake_route("renew").await;
    let invoice: String = sqlx::query_scalar(
        "SELECT invoice_id FROM entitlement_renewals WHERE entitlement_id = $1::uuid",
    )
    .bind(&f.annual)
    .fetch_one(&f.hub.db.pool)
    .await
    .unwrap();
    f.webhook(FAKE_SUBSCRIPTION_RENEWAL_FAILED, Some(&invoice))
        .await;
    assert_eq!(f.entitlement().await["renewal_failed_at"], Value::Null);
    assert!(f.hub.audited("entitlement.renewal_failed").await.is_empty());
    let outcome: Option<String> =
        sqlx::query_scalar("SELECT outcome FROM payment_events WHERE event_type = $1")
            .bind(FAKE_SUBSCRIPTION_RENEWAL_FAILED)
            .fetch_one(&f.hub.db.pool)
            .await
            .unwrap();
    assert_eq!(outcome.as_deref(), Some("unchanged"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let sent = Mailpit::new()
        .messages_to(&f.owner.email)
        .await
        .into_iter()
        .filter(|m| m.subject == RENEWAL_FAILED_SUBJECT)
        .count();
    assert_eq!(sent, 0);
}

/// A refund touches no licence: a station holding a grace licence on a
/// refunded subscription keeps it, and its fetch withdraws nothing.
#[tokio::test]
async fn a_refund_never_withdraws_the_grace() {
    let f = fixture().await;
    let s = f.register("bench-1").await;
    let licences = f.hub.count("licences").await;
    f.sql(
        "UPDATE entitlements SET status = 'refunded', refunded_at = now(), renews = false \
         WHERE id = $1",
    )
    .await;
    assert_current(fetch(&f.hub, &s, &s.licence).await).await;
    assert!(f
        .hub
        .audited("licence.superseded")
        .await
        .iter()
        .all(|(_, _, d)| d["reason"] != "grace_withdrawn"));
    assert_eq!(f.hub.count("licences").await, licences);
}
