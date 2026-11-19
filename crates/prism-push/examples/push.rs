//! A small command line around prism-push:
//!
//! ```sh
//! cargo run -p prismsdk-push --example push -- vapid
//!     # a new VAPID key pair (PRISM_VAPID_PUBLIC_KEY for the web app, the private one for the server)
//! cargo run -p prismsdk-push --example push -- apns-payload '{"title":"Hi","link":"/"}' > hi.apns
//!     # an .apns file for `xcrun simctl push <device> <bundle id> hi.apns`
//! cargo run -p prismsdk-push --example push -- encrypt '<subscription json>' '{"title":"Hi"}'
//!     # the encrypted Web Push / UnifiedPush body, base64url (for tests)
//! cargo run -p prismsdk-push --example push -- send '<subscription json>' '{"title":"Hi"}'
//!     # sends it; keys from PRISM_APNS_P8 (+ _KEY_ID, _TEAM_ID), PRISM_FCM_SERVICE_ACCOUNT,
//!     # PRISM_VAPID_PRIVATE_KEY (+ PRISM_VAPID_SUBJECT)
//! ```

use prism_push::{Push, Sender, Subscription};

fn push_from(json: &str) -> Push {
    let v: serde_json::Value = serde_json::from_str(json).expect("payload JSON");
    let s = |k: &str| v[k].as_str().map(str::to_string);
    let mut p = Push::new();
    p.title = s("title");
    p.subtitle = s("subtitle");
    p.body = s("body");
    p.link = s("link");
    p.silent = v["silent"].as_bool().unwrap_or(false);
    if let Some(data) = v["data"].as_object() {
        p.data = data.clone();
    }
    p
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("vapid") => {
            let key = prism_push::webpush::VapidKey::generate();
            println!("public:  {}", key.public_key());
            println!("private: {}", key.private_base64());
        }
        Some("apns-payload") => {
            let push = push_from(args.get(1).expect("payload"));
            println!("{}", prism_push::apns::payload(&push));
        }
        Some("encrypt") => {
            let sub = Subscription::from_json(args.get(1).expect("subscription")).unwrap();
            let keys = sub.keys.expect("a Web Push / UnifiedPush subscription");
            use base64::Engine;
            let b = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let body = prism_push::webpush::encrypt(
                push_from(args.get(2).expect("payload"))
                    .payload()
                    .to_string()
                    .as_bytes(),
                &b.decode(keys.p256dh.trim_end_matches('=')).unwrap(),
                &b.decode(keys.auth.trim_end_matches('=')).unwrap(),
            )
            .unwrap();
            println!("{}", b.encode(body));
        }
        Some("send") => {
            let sub = Subscription::from_json(args.get(1).expect("subscription")).unwrap();
            let push = push_from(args.get(2).expect("payload"));
            let env = |k: &str| std::env::var(k).ok();
            let mut sender = Sender::new();
            if let (Some(p8), Some(kid), Some(team)) = (
                env("PRISM_APNS_P8"),
                env("PRISM_APNS_KEY_ID"),
                env("PRISM_APNS_TEAM_ID"),
            ) {
                let p8 = std::fs::read_to_string(p8).expect("PRISM_APNS_P8");
                sender = sender.apns(prism_push::apns::ApnsKey::from_p8(&p8, &kid, &team).unwrap());
            }
            if let Some(sa) = env("PRISM_FCM_SERVICE_ACCOUNT") {
                let sa = std::fs::read_to_string(sa).expect("PRISM_FCM_SERVICE_ACCOUNT");
                sender = sender.fcm(prism_push::fcm::ServiceAccount::from_json(&sa).unwrap());
            }
            if let Some(k) = env("PRISM_VAPID_PRIVATE_KEY") {
                let subject =
                    env("PRISM_VAPID_SUBJECT").unwrap_or_else(|| "mailto:admin@localhost".into());
                sender = sender.vapid(
                    prism_push::webpush::VapidKey::from_base64(&k).unwrap(),
                    subject,
                );
            }
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            match rt.block_on(sender.send(&sub, &push)) {
                Ok(()) => println!("sent"),
                Err(e) => {
                    eprintln!("not sent: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => eprintln!(
            "usage: push vapid | apns-payload <json> | encrypt <subscription> <json> | send <subscription> <json>"
        ),
    }
}
