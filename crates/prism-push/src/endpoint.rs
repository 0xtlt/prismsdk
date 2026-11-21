//! Where a Web Push or UnifiedPush subscription may send the server: its
//! endpoint is chosen by whoever subscribed, so it is parsed strictly and,
//! unless the deployment trusts its origin, must be a public HTTPS server.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::PushError;

/// A parsed endpoint: what the HTTP client will connect to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub https: bool,
    pub host: Host,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    /// A DNS name, lowercase, without a trailing dot.
    Name(String),
    Ip(IpAddr),
}

fn bad(why: &str) -> PushError {
    PushError::Config(format!("push endpoint: {why}"))
}

impl Endpoint {
    /// Parses `url` (`https://push.example.net/p/abc`) the strict way: ASCII
    /// only, no credentials, no backslash or whitespace, a plain DNS name or
    /// IP address as host. Anything a lenient URL parser could read as
    /// another host (`0x7f.1`, `%31%32%37.0.0.1`, full-width digits) is
    /// refused rather than guessed.
    pub fn parse(url: &str) -> Result<Self, PushError> {
        if url
            .chars()
            .any(|c| !c.is_ascii() || c.is_ascii_whitespace() || c.is_ascii_control() || c == '\\')
        {
            return Err(bad("unexpected character"));
        }
        let (scheme, rest) = url.split_once("://").ok_or_else(|| bad("not a URL"))?;
        let https = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            _ => return Err(bad("not HTTP")),
        };
        let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
        if authority.contains('@') {
            return Err(bad("credentials in the URL"));
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (ip, after) = v6.split_once(']').ok_or_else(|| bad("bad IPv6 address"))?;
                let ip: Ipv6Addr = ip.parse().map_err(|_| bad("bad IPv6 address"))?;
                let port = match after {
                    "" => None,
                    p => Some(p.strip_prefix(':').ok_or_else(|| bad("bad port"))?),
                };
                (Host::Ip(IpAddr::V6(ip)), port)
            }
            None => {
                let (host, port) = match authority.rsplit_once(':') {
                    Some((h, p)) => (h, Some(p)),
                    None => (authority, None),
                };
                (host_name(host)?, port)
            }
        };
        let port = match port {
            None => {
                if https {
                    443
                } else {
                    80
                }
            }
            // Digits only, without a leading zero (`:0443`, `:+443`).
            Some(p) => Some(p)
                .filter(|p| p.bytes().all(|b| b.is_ascii_digit()) && !p.starts_with('0'))
                .and_then(|p| p.parse::<u16>().ok())
                .ok_or_else(|| bad("bad port"))?,
        };
        Ok(Endpoint { https, host, port })
    }

    /// The origin, as `trust_origin` compares it: scheme, host and port.
    pub fn origin(&self) -> (bool, String, u16) {
        let host = match &self.host {
            Host::Name(n) => n.clone(),
            Host::Ip(ip) => ip.to_string(),
        };
        (self.https, host, self.port)
    }

    /// The default policy, for an origin the deployment did not trust:
    /// HTTPS on port 443, a host that is not `localhost`, and an address,
    /// if the host is one, that is publicly routable. (A DNS name's
    /// addresses are checked when the sender connects.)
    pub fn check_public(&self) -> Result<(), PushError> {
        if !self.https {
            return Err(bad("not HTTPS"));
        }
        if self.port != 443 {
            return Err(bad("not on port 443"));
        }
        match &self.host {
            Host::Name(n) if n == "localhost" || n.ends_with(".localhost") => {
                Err(bad("a local host"))
            }
            Host::Ip(ip) if !is_public(*ip) => Err(bad("not a public address")),
            _ => Ok(()),
        }
    }
}

/// A DNS name or a dotted IPv4 address. A name whose last label is a
/// number is an IPv4 address to URL parsers: it must be a plain one.
fn host_name(host: &str) -> Result<Host, PushError> {
    let host = host.to_ascii_lowercase();
    let name = host.strip_suffix('.').unwrap_or(&host);
    if name.is_empty() || name.len() > 253 {
        return Err(bad("bad host"));
    }
    let last = name.rsplit('.').next().unwrap_or(name);
    if last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x") {
        return name
            .parse::<Ipv4Addr>()
            .map(|ip| Host::Ip(IpAddr::V4(ip)))
            .map_err(|_| bad("bad IPv4 address"));
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    };
    if !name.split('.').all(label_ok) {
        return Err(bad("bad host"));
    }
    Ok(Host::Name(name.to_string()))
}

/// Whether `ip` is a publicly routable unicast address: not loopback,
/// private, shared, link-local, multicast, documentation or reserved, in
/// IPv4 or IPv6, including IPv4 written as IPv6 (mapped, NAT64, 6to4).
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..128).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    let v4 = |hi: u16, lo: u16| Ipv4Addr::from(((hi as u32) << 16) | lo as u32);
    // IPv4-mapped (::ffff:a.b.c.d) and NAT64 (64:ff9b::a.b.c.d): the IPv4
    // address is where the packets go.
    if s[..5] == [0; 5] && s[5] == 0xffff {
        return public_v4(v4(s[6], s[7]));
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return public_v4(v4(s[6], s[7]));
    }
    // 6to4 (2002:a.b.c.d::/48).
    if s[0] == 0x2002 {
        return public_v4(v4(s[1], s[2]));
    }
    // Global unicast is 2000::/3; within it, protocol assignments
    // (2001::/23, Teredo among them), documentation (2001:db8::/32,
    // 3fff::/20). Everything outside it (loopback, unspecified, ULA,
    // link-local, multicast, IPv4-compatible…) is not public.
    (s[0] & 0xe000) == 0x2000
        && !(s[0] == 0x2001 && s[1] < 0x0200)
        && !(s[0] == 0x2001 && s[1] == 0x0db8)
        && !(s[0] == 0x3fff && s[1] < 0x1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(url: &str) -> Result<Host, String> {
        Endpoint::parse(url)
            .map(|e| e.host)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn s5_endpoints_are_parsed_strictly() {
        assert_eq!(
            Endpoint::parse("https://fcm.googleapis.com/fcm/send/abc").unwrap(),
            Endpoint {
                https: true,
                host: Host::Name("fcm.googleapis.com".into()),
                port: 443
            }
        );
        assert_eq!(
            Endpoint::parse("HTTP://Ntfy.LAN:8080/up?x=1")
                .unwrap()
                .origin(),
            (false, "ntfy.lan".into(), 8080)
        );
        assert_eq!(
            host("https://[2a00:1450::1]:443/x").unwrap(),
            Host::Ip("2a00:1450::1".parse().unwrap())
        );
        assert_eq!(
            host("https://1.2.3.4/x").unwrap(),
            Host::Ip("1.2.3.4".parse().unwrap())
        );
        for url in [
            "https://user:pw@push.example/x",
            "https://push.example@127.0.0.1/x",
            "https://push.example\\@127.0.0.1/x",
            "https://0x7f.0.0.1/x",
            "https://2130706433/x",
            "https://0177.0.0.1/x",
            "https://127.1/x",
            "https://%31%32%37.0.0.1/x",
            "https://\u{ff11}\u{ff12}\u{ff17}.0.0.1/x",
            "https://127.0.0.\n1/x",
            "https://[fe80::1%25en0]/x",
            "https://push.example:0443/x",
            "https://push.example:/x",
            "https://push.example:99999/x",
            "https://push..example/x",
            "ftp://push.example/x",
            "push.example/x",
            "https:///x",
        ] {
            assert!(Endpoint::parse(url).is_err(), "{url:?} must be refused");
        }
    }

    #[test]
    fn s5_the_default_policy_wants_public_https() {
        for url in [
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            "https://web.push.apple.com/abc",
            "https://ntfy.sh/upAbC?up=1",
            "https://8.8.8.8/x",
            "https://[2606:4700::1111]/x",
        ] {
            Endpoint::parse(url).unwrap().check_public().unwrap();
        }
        for url in [
            "http://push.example/x",
            "https://push.example:8443/x",
            "https://localhost/x",
            "https://a.localhost/x",
            "https://127.0.0.1/x",
            "https://10.0.0.1/x",
            "https://172.16.0.1/x",
            "https://192.168.1.1/x",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/x",
            "https://0.0.0.0/x",
            "https://[::1]/x",
            "https://[::]/x",
            "https://[fd00::1]/x",
            "https://[fe80::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://[::ffff:a9fe:a9fe]/x",
            "https://[64:ff9b::a00:1]/x",
            "https://[2002:c0a8:101::1]/x",
            "https://[2001:db8::1]/x",
            "https://[2001::1]/x",
            "https://[ff02::1]/x",
        ] {
            let e = Endpoint::parse(url).unwrap();
            assert!(e.check_public().is_err(), "{url:?} must be refused");
        }
    }
}
