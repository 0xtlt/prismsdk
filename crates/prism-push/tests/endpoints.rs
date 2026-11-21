//! S5 (security review): a subscriber chooses its Web Push endpoint, so the
//! sender must not post wherever it says. Loopback HTTP servers stand in
//! for private services.
#![cfg(feature = "send")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use prism_push::webpush::ReceiverKeys;
use prism_push::{Push, PushError, Sender, Subscription};

/// A server on a loopback port answering every request with `reply`;
/// returns its origin and how many requests reached it.
fn server(reply: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let me = origin.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let _ = reader.get_mut().write_all(&reply(&me));
        }
    });
    (origin, hits)
}

fn subscription(endpoint: &str) -> Subscription {
    let keys = ReceiverKeys::generate();
    Subscription::from_json(
        &serde_json::json!({
            "service": "webpush",
            "endpoint": endpoint,
            "keys": {"p256dh": keys.p256dh(), "auth": keys.auth()},
        })
        .to_string(),
    )
    .unwrap()
}

fn send(sender: &Sender, endpoint: &str) -> Result<(), PushError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(sender.send(&subscription(endpoint), &Push::new().title("hi")))
}

const CREATED: &[u8] = b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

#[test]
fn a_private_endpoint_gets_no_request() {
    let (origin, hits) = server(|_| CREATED.to_vec());
    let err = send(&Sender::new(), &format!("{origin}/push")).unwrap_err();
    assert!(matches!(err, PushError::Config(_)), "{err}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[test]
fn a_trusted_distributor_gets_one_post_and_no_redirect_is_followed() {
    let (target, redirected) = server(|_| CREATED.to_vec());
    let location = format!("{target}/elsewhere");
    let (origin, hits) = server(move |_| {
        format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
    });
    let sender = Sender::new().trust_origin(&origin).unwrap();
    let err = send(&sender, &format!("{origin}/push")).unwrap_err();
    assert!(
        matches!(err, PushError::Rejected { status: 307, .. }),
        "{err}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(redirected.load(Ordering::SeqCst), 0);

    // An approved distributor that accepts: one POST, delivered.
    let (origin, hits) = server(|_| CREATED.to_vec());
    let sender = Sender::new().trust_origin(&origin).unwrap();
    send(&sender, &format!("{origin}/push")).unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_long_reply_is_cut_short() {
    let (origin, _) = server(|_| {
        let mut reply =
            b"HTTP/1.1 400 Bad Request\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                .to_vec();
        let chunk = vec![b'x'; 1 << 16];
        for _ in 0..256 {
            reply.extend(format!("{:x}\r\n", chunk.len()).bytes());
            reply.extend(&chunk);
            reply.extend(b"\r\n");
        }
        reply.extend(b"0\r\n\r\n");
        reply
    });
    let sender = Sender::new().trust_origin(&origin).unwrap();
    let err = send(&sender, &format!("{origin}/push")).unwrap_err();
    assert!(
        err.to_string().len() < 4096,
        "{} bytes of error",
        err.to_string().len()
    );
}
