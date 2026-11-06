//! Outgoing mail over plain SMTP (ADR-0007 §3; Mailpit locally).
//!
//! STARTTLS and SMTP auth for a production relay are a deployment decision
//! (ROADMAP §0.5), so the transport here is unencrypted and unauthenticated.
//! Health's `probe_smtp` is a separate, connection-only check.

use std::time::Duration;

use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use url::Url;

/// Upper bound on one SMTP exchange.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);

pub const VERIFICATION_SUBJECT: &str = "Verify your Wipestation email address";

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("recipient {0:?} is not a valid address")]
    BadRecipient(String),
    #[error("building the message failed: {0}")]
    Build(#[from] lettre::error::Error),
    #[error("SMTP send failed: {0}")]
    Smtp(#[from] lettre::transport::smtp::Error),
}

/// Sends the hub's emails. Cheap to clone; connects per message.
#[derive(Clone)]
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    pub fn new(host: &str, port: u16, from: Mailbox) -> Self {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .port(port)
            .timeout(Some(SEND_TIMEOUT))
            .build();
        Self { transport, from }
    }

    /// The email-verification message: plain text containing `link`.
    pub async fn send_verification(&self, to: &str, link: &Url) -> Result<(), MailError> {
        let body = format!(
            "Welcome to Wipestation.\n\n\
             Confirm this email address by opening the link below. \
             It works once and expires in 24 hours.\n\n\
             {link}\n\n\
             If you did not create a Wipestation account, ignore this email.\n"
        );
        self.send(to, VERIFICATION_SUBJECT, body).await
    }

    async fn send(&self, to: &str, subject: &str, body: String) -> Result<(), MailError> {
        let to: Mailbox = to
            .parse()
            .map_err(|_| MailError::BadRecipient(to.to_string()))?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body)?;
        self.transport.send(message).await?;
        Ok(())
    }
}
