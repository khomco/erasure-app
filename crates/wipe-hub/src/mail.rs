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

use crate::leads::LeadKind;

/// Upper bound on one SMTP exchange.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);

pub const VERIFICATION_SUBJECT: &str = "Verify your Wipestation email address";
pub const MAGIC_LINK_SUBJECT: &str = "Your Wipestation sign-in link";
pub const PASSWORD_RESET_SUBJECT: &str = "Reset your Wipestation password";
pub const INVITATION_SUBJECT: &str = "You're invited to join a Wipestation organization";
pub const LEAD_EARLY_ACCESS_SUBJECT: &str = "You're on the Wipestation early-access list";
pub const LEAD_CONTACT_SUBJECT: &str = "We received your message to Wipestation";

/// The confirmation subject for a sales-lead form.
pub fn lead_subject(kind: LeadKind) -> &'static str {
    match kind {
        LeadKind::EarlyAccess => LEAD_EARLY_ACCESS_SUBJECT,
        LeadKind::Contact => LEAD_CONTACT_SUBJECT,
    }
}

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

    /// The magic (sign-in) link message: plain text containing `link`.
    pub async fn send_magic_link(&self, to: &str, link: &Url) -> Result<(), MailError> {
        let body = format!(
            "Sign in to the Wipestation portal by opening the link below. \
             It works once and expires in 15 minutes.\n\n\
             {link}\n\n\
             If you did not ask for a sign-in link, ignore this email. \
             Your account is unchanged.\n"
        );
        self.send(to, MAGIC_LINK_SUBJECT, body).await
    }

    /// The password reset message: plain text containing `link`.
    pub async fn send_password_reset(&self, to: &str, link: &Url) -> Result<(), MailError> {
        let body = format!(
            "Choose a new password for your Wipestation account by opening the link below. \
             It works once and expires in 1 hour. \
             Resetting your password signs you out everywhere.\n\n\
             {link}\n\n\
             If you did not ask to reset your password, ignore this email. \
             Your password is unchanged.\n"
        );
        self.send(to, PASSWORD_RESET_SUBJECT, body).await
    }

    /// The team invitation message: plain text naming the organization and
    /// the inviter, containing `link`. The names are user input (validated:
    /// no control characters), so they appear only in the body, never in a
    /// header.
    pub async fn send_invitation(
        &self,
        to: &str,
        organization_name: &str,
        inviter_name: &str,
        link: &Url,
    ) -> Result<(), MailError> {
        self.send(
            to,
            INVITATION_SUBJECT,
            invitation_body(organization_name, inviter_name, link),
        )
        .await
    }

    /// The sales-lead confirmation: plain text naming the form, containing
    /// the unsubscribe `link`. It never echoes anything the visitor typed,
    /// so the form cannot be used to send chosen text to any address.
    pub async fn send_lead_confirmation(
        &self,
        to: &str,
        kind: LeadKind,
        link: &Url,
    ) -> Result<(), MailError> {
        self.send(to, lead_subject(kind), lead_confirmation_body(kind, link))
            .await
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

fn invitation_body(organization_name: &str, inviter_name: &str, link: &Url) -> String {
    format!(
        "{inviter_name} has invited you to join the organization \"{organization_name}\" \
         in the Wipestation portal.\n\n\
         To accept, open the link below and choose your name and password. \
         It works once and expires in 7 days.\n\n\
         {link}\n\n\
         If you did not expect this invitation, ignore this email. \
         No account is created unless you accept.\n"
    )
}

fn lead_confirmation_body(kind: LeadKind, link: &Url) -> String {
    let what = match kind {
        LeadKind::EarlyAccess => {
            "Thank you for joining the Wipestation early-access list on our website."
        }
        LeadKind::Contact => {
            "Thank you for sending a message through the Wipestation contact form."
        }
    };
    format!(
        "{what} We will reply by email to this address.\n\n\
         To stop hearing from us, open the link below and press Unsubscribe:\n\n\
         {link}\n\n\
         Not you? Someone entered this address in our form. \
         Use the link above and we will not contact you.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_confirmation_names_the_form_links_and_echoes_nothing() {
        let link = Url::parse("http://127.0.0.1:7900/unsubscribe?token=abc").unwrap();
        for (kind, names) in [
            (LeadKind::EarlyAccess, "early-access list"),
            (LeadKind::Contact, "contact form"),
        ] {
            let body = lead_confirmation_body(kind, &link);
            assert!(body.contains(names), "{body}");
            assert!(body.contains("reply by email"), "{body}");
            assert!(
                body.contains("http://127.0.0.1:7900/unsubscribe?token=abc"),
                "{body}"
            );
            assert!(body.contains("Not you?"), "{body}");
            // The body is a fixed template around the link: no parameter
            // carries submitted text.
            let without_link = body.replace(link.as_str(), "");
            assert!(!without_link.contains("token"), "{body}");
        }
        assert_ne!(
            lead_subject(LeadKind::EarlyAccess),
            lead_subject(LeadKind::Contact)
        );
    }

    #[test]
    fn invitation_body_names_both_and_links() {
        let link = Url::parse("http://127.0.0.1:7900/accept-invite?token=abc").unwrap();
        let body = invitation_body("Engines Ltd", "Ada Lovelace", &link);
        assert!(body.contains("Ada Lovelace has invited you"), "{body}");
        assert!(body.contains("\"Engines Ltd\""), "{body}");
        assert!(
            body.contains("http://127.0.0.1:7900/accept-invite?token=abc"),
            "{body}"
        );
        assert!(body.contains("expires in 7 days"), "{body}");
        assert!(body.contains("ignore this email"), "{body}");
    }
}
