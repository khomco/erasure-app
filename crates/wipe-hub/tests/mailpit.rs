//! The Mailpit client in `tests/support`. Needs `docker compose up -d`; fails,
//! never skips, when Mailpit is down.

mod support;

use std::time::Duration;

use support::{unique_address, Mailpit};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use wipe_hub::config::{DEFAULT_SMTP_HOST, DEFAULT_SMTP_PORT};

/// Read one (possibly multi-line) SMTP reply and check its code.
async fn expect_reply<R: AsyncBufReadExt + Unpin>(r: &mut R, code: &str) {
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).await.unwrap();
        assert!(n > 0, "SMTP connection closed waiting for {code}");
        assert!(line.starts_with(code), "expected {code}, got {line:?}");
        if line.as_bytes().get(3) != Some(&b'-') {
            return;
        }
    }
}

/// Send one plain-text message over raw SMTP to Mailpit.
async fn send(to: &str, subject: &str, body: &str) {
    let addr = format!("{DEFAULT_SMTP_HOST}:{DEFAULT_SMTP_PORT}");
    let stream = TcpStream::connect(&addr).await.unwrap_or_else(|e| {
        panic!(
            "Mailpit SMTP is unreachable at {addr}: {e}; run docker compose up -d at the repo root"
        )
    });
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::new(r);
    expect_reply(&mut r, "220").await;
    let steps: [(String, &str); 4] = [
        ("EHLO wipehub.test\r\n".into(), "250"),
        ("MAIL FROM:<hub@wipehub.test>\r\n".into(), "250"),
        (format!("RCPT TO:<{to}>\r\n"), "250"),
        ("DATA\r\n".into(), "354"),
    ];
    for (cmd, code) in steps {
        w.write_all(cmd.as_bytes()).await.unwrap();
        expect_reply(&mut r, code).await;
    }
    let msg = format!(
        "From: hub@wipehub.test\r\nTo: {to}\r\nSubject: {subject}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n.\r\n"
    );
    w.write_all(msg.as_bytes()).await.unwrap();
    expect_reply(&mut r, "250").await;
    w.write_all(b"QUIT\r\n").await.unwrap();
    expect_reply(&mut r, "221").await;
}

#[tokio::test]
async fn captured_mail_is_readable_by_recipient() {
    let mailpit = Mailpit::new();
    let to = unique_address();
    let other = unique_address();
    send(
        &to,
        "Harness check",
        "Hello from the wipe-hub test harness.",
    )
    .await;

    let msg = mailpit
        .wait_for_message_to(&to, Duration::from_secs(10))
        .await;
    assert_eq!(msg.subject, "Harness check");
    assert!(
        msg.text.contains("Hello from the wipe-hub test harness."),
        "{:?}",
        msg.text
    );
    assert_eq!(mailpit.messages_to(&to).await.len(), 1);
    assert!(mailpit.messages_to(&other).await.is_empty());
}

#[tokio::test]
#[should_panic(expected = "no message to")]
async fn waiting_for_mail_that_never_comes_panics_naming_the_address() {
    Mailpit::new()
        .wait_for_message_to(&unique_address(), Duration::from_millis(300))
        .await;
}

#[tokio::test]
#[should_panic(expected = "run docker compose up -d")]
async fn unreachable_mailpit_panics_with_the_compose_hint() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    Mailpit::with_base(&format!("http://127.0.0.1:{port}"))
        .messages_to(&unique_address())
        .await;
}
