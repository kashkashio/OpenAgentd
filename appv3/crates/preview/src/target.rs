//! Preview targets: a dev server (local or external) or a workspace
//! directory.
//!
//! Loopback dev servers were the original scope. External http/https sites
//! are proxied too, so the agent can drive staging sites; link-local
//! (cloud metadata), multicast and broadcast addresses are never proxied.
//! The listener only serves loopback callers and machines granted access
//! through the authenticated API (see `manager::Entry::peer_allowed`), so
//! it is not an open relay.

use reqwest::Url;
use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

/// An origin the proxy forwards to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UrlTarget {
    pub scheme: String,
    /// Host as written in a URL: `localhost`, `127.0.0.1`, `[::1]`, or (for
    /// an external target) a domain or IP literal.
    pub host: String,
    pub port: u16,
    /// Not on this machine's loopback.
    pub external: bool,
}

impl UrlTarget {
    fn default_port(&self) -> u16 {
        if self.scheme == "https" {
            443
        } else {
            80
        }
    }

    fn origin_with(&self, host: &str, explicit_port: bool) -> String {
        if !explicit_port && self.port == self.default_port() {
            format!("{}://{}", self.scheme, host)
        } else {
            format!("{}://{}:{}", self.scheme, host, self.port)
        }
    }

    /// As browsers write it: the scheme's default port is left out.
    pub fn origin(&self) -> String {
        self.origin_with(&self.host, false)
    }

    /// `Host` header value: the default port is left out.
    pub fn authority(&self) -> String {
        if self.port == self.default_port() {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Origins that name the same server: with and without an explicit
    /// default port, and for a loopback target `localhost` vs `127.0.0.1`.
    pub fn origin_aliases(&self) -> Vec<String> {
        let mut hosts = vec![self.host.clone()];
        if !self.external {
            for h in ["localhost", "127.0.0.1", "[::1]"] {
                if !hosts.iter().any(|x| x == h) {
                    hosts.push(h.to_string());
                }
            }
        }
        let mut out = Vec::new();
        for h in &hosts {
            for explicit in [true, false] {
                let o = self.origin_with(h, explicit);
                if !out.contains(&o) {
                    out.push(o);
                }
            }
        }
        out
    }
}

/// What a preview listener serves.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    Upstream(UrlTarget),
    /// A workspace root served as static files.
    Static(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    Invalid(String),
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetError::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for TargetError {}

fn invalid(msg: impl Into<String>) -> TargetError {
    TargetError::Invalid(msg.into())
}

fn refused_ip(ip: IpAddr) -> bool {
    match ip {
        // 169.254/16 holds cloud metadata services.
        IpAddr::V4(v4) => v4.is_link_local() || v4.is_multicast() || v4.is_broadcast(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80 || v6.is_multicast(),
    }
}

/// Parse a user- or agent-supplied URL into a target plus the path (with
/// query and fragment) to open first. A bare `localhost:3000` or `:3000`
/// gets `http://` added.
pub fn parse_url_target(raw: &str) -> Result<(UrlTarget, String), TargetError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(invalid("URL is empty."));
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else if let Some(port) = raw.strip_prefix(':') {
        format!("http://localhost:{port}")
    } else {
        format!("http://{raw}")
    };
    let url = Url::parse(&with_scheme).map_err(|e| invalid(format!("Invalid URL '{raw}': {e}.")))?;
    let scheme = url.scheme().to_string();
    if scheme != "http" && scheme != "https" {
        return Err(invalid(format!("Only http and https URLs can be previewed, not '{scheme}'.")));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("Preview URLs cannot contain credentials."));
    }
    let refused = |h: &str| invalid(format!("'{h}' cannot be previewed: link-local, multicast and broadcast addresses are refused."));
    let (host, external) = match url.host() {
        Some(url::Host::Domain(d)) if d.eq_ignore_ascii_case("localhost") => ("localhost".to_string(), false),
        Some(url::Host::Domain(d)) => (d.to_ascii_lowercase(), true),
        Some(url::Host::Ipv4(ip)) if ip.is_unspecified() => ("127.0.0.1".to_string(), false),
        Some(url::Host::Ipv4(ip)) if IpAddr::V4(ip).is_loopback() => (ip.to_string(), false),
        Some(url::Host::Ipv4(ip)) if refused_ip(IpAddr::V4(ip)) => return Err(refused(&ip.to_string())),
        Some(url::Host::Ipv4(ip)) => (ip.to_string(), true),
        Some(url::Host::Ipv6(ip)) if ip.is_loopback() || ip.is_unspecified() => ("[::1]".to_string(), false),
        Some(url::Host::Ipv6(ip)) if refused_ip(IpAddr::V6(ip)) => return Err(refused(&ip.to_string())),
        Some(url::Host::Ipv6(ip)) => (format!("[{ip}]"), true),
        None => return Err(invalid(format!("URL '{raw}' has no host."))),
    };
    let port = url.port_or_known_default().ok_or_else(|| invalid("URL has no port."))?;
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    if let Some(f) = url.fragment() {
        path.push('#');
        path.push_str(f);
    }
    Ok((UrlTarget { scheme, host, port, external }, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(raw: &str) -> (String, String) {
        let (t, p) = parse_url_target(raw).unwrap();
        (t.origin(), p)
    }

    #[test]
    fn accepts_loopback_urls() {
        assert_eq!(ok("http://localhost:5173/pricing?x=1#top"), ("http://localhost:5173".into(), "/pricing?x=1#top".into()));
        assert_eq!(ok("http://127.0.0.1:3000"), ("http://127.0.0.1:3000".into(), "/".into()));
        assert_eq!(ok("http://127.1.2.3:3000/"), ("http://127.1.2.3:3000".into(), "/".into()));
        assert_eq!(ok("https://localhost:8443/"), ("https://localhost:8443".into(), "/".into()));
        assert_eq!(ok("http://[::1]:4000/a"), ("http://[::1]:4000".into(), "/a".into()));
        assert_eq!(ok("http://LOCALHOST/"), ("http://localhost".into(), "/".into()));
    }

    #[test]
    fn normalizes_shorthand_and_unspecified() {
        assert_eq!(ok("localhost:3000"), ("http://localhost:3000".into(), "/".into()));
        assert_eq!(ok(":5173"), ("http://localhost:5173".into(), "/".into()));
        assert_eq!(ok("http://0.0.0.0:8080/x"), ("http://127.0.0.1:8080".into(), "/x".into()));
    }

    #[test]
    fn accepts_external_sites() {
        let (t, p) = parse_url_target("https://Develop.Example.com/login?next=/").unwrap();
        assert!(t.external);
        assert_eq!((t.origin(), t.authority(), p), ("https://develop.example.com".into(), "develop.example.com".into(), "/login?next=/".into()));
        let (t, _) = parse_url_target("http://192.168.1.2:3000").unwrap();
        assert!(t.external);
        assert_eq!(t.origin(), "http://192.168.1.2:3000");
        assert!(!parse_url_target("http://localhost:3000").unwrap().0.external);
    }

    #[test]
    fn rejects_odd_urls_and_metadata_addresses() {
        for raw in [
            "",
            "ftp://localhost/",
            "file:///etc/passwd",
            "http://user:pw@localhost:3000/",
            "javascript:alert(1)",
            "http://169.254.169.254/latest/meta-data",
            "http://[fe80::1]/",
            "http://224.0.0.1/",
            "http://255.255.255.255/",
        ] {
            assert!(parse_url_target(raw).is_err(), "{raw} should be rejected");
        }
    }

    #[test]
    fn aliases_cover_loopback_names_and_default_ports() {
        let (t, _) = parse_url_target("http://localhost:5173").unwrap();
        assert_eq!(t.origin_aliases(), vec!["http://localhost:5173", "http://127.0.0.1:5173", "http://[::1]:5173"]);
        let (t, _) = parse_url_target("https://develop.example.com").unwrap();
        assert_eq!(t.origin_aliases(), vec!["https://develop.example.com:443", "https://develop.example.com"]);
    }
}
