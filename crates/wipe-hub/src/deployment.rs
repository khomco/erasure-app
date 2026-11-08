//! `GET /api/deployment`: what kind of hub this is (commerce-end-to-end
//! story 2, CAP-3).
//!
//! The portal shows a development banner whenever [`DeploymentReport::development`]
//! is true, so a local or staging stack cannot be mistaken for production.
//! The rule lives here alone; the portal renders `development` and `reasons`
//! and never re-derives them. The report is configuration only: no account
//! data, no paths, no key ids. It needs no session.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::config::{Config, PaymentProvider};
use crate::AppState;

/// How the vendor root that signs licences is held (ADR-0007, "Not decided
/// here"). A future custody kind (HSM or KMS) adds a value here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VendorRootCustody {
    /// Read from a seed file: not production custody.
    File,
    /// No vendor root: licence issuance is unavailable.
    None,
}

/// Why a hub counts as a development hub, in the order they are reported:
/// payment provider first, then the vendor root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The Fake payment provider: no money moves.
    FakePaymentProvider,
    /// The vendor root is held in a file.
    FileVendorRoot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeploymentReport {
    /// True when there is at least one reason.
    pub development: bool,
    /// `fake` or `stripe`.
    pub payment_provider: &'static str,
    pub vendor_root: VendorRootCustody,
    pub reasons: Vec<Reason>,
}

/// The report for `config`: development when the payment provider is Fake
/// or the vendor root is file-held.
pub fn report(config: &Config) -> DeploymentReport {
    let vendor_root = match config.vendor_root {
        Some(_) => VendorRootCustody::File,
        None => VendorRootCustody::None,
    };
    let mut reasons = Vec::new();
    if config.payment_provider == PaymentProvider::Fake {
        reasons.push(Reason::FakePaymentProvider);
    }
    if vendor_root == VendorRootCustody::File {
        reasons.push(Reason::FileVendorRoot);
    }
    DeploymentReport {
        development: !reasons.is_empty(),
        payment_provider: config.payment_provider.as_str(),
        vendor_root,
        reasons,
    }
}

pub async fn deployment(State(state): State<AppState>) -> Json<DeploymentReport> {
    Json(report(&state.config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The unit-test defaults for `provider`, with a file-held root when
    /// `seed`. Built without reading the environment.
    fn config(provider: PaymentProvider, seed: bool) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let seed_path = dir.path().join("root.seed");
        std::fs::write(&seed_path, [7u8; 32]).unwrap();
        let stripe = provider == PaymentProvider::Stripe;
        let mut args = crate::config::tests::args();
        args.payment_provider = provider;
        args.stripe_webhook_secret = stripe.then(|| "whsec_test".into());
        args.stripe_secret_key = stripe.then(|| "sk_test_x".into());
        args.vendor_root_seed = seed.then_some(seed_path);
        // The seed is read here, so the directory may go afterwards.
        Config::from_args(args).unwrap()
    }

    fn body(provider: PaymentProvider, seed: bool) -> serde_json::Value {
        serde_json::to_value(report(&config(provider, seed))).unwrap()
    }

    #[test]
    fn default_local_is_development_for_the_fake_provider() {
        assert_eq!(
            body(PaymentProvider::Fake, false),
            json!({"development": true, "payment_provider": "fake", "vendor_root": "none",
                   "reasons": ["fake_payment_provider"]})
        );
    }

    #[test]
    fn demo_reports_both_reasons_provider_first() {
        assert_eq!(
            body(PaymentProvider::Fake, true),
            json!({"development": true, "payment_provider": "fake", "vendor_root": "file",
                   "reasons": ["fake_payment_provider", "file_vendor_root"]})
        );
    }

    #[test]
    fn stripe_with_a_file_root_is_development() {
        assert_eq!(
            body(PaymentProvider::Stripe, true),
            json!({"development": true, "payment_provider": "stripe", "vendor_root": "file",
                   "reasons": ["file_vendor_root"]})
        );
    }

    #[test]
    fn stripe_without_a_root_is_not_development() {
        assert_eq!(
            body(PaymentProvider::Stripe, false),
            json!({"development": false, "payment_provider": "stripe", "vendor_root": "none",
                   "reasons": []})
        );
    }
}
