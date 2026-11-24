//! A faithful port of CPython's `urllib.parse.urlsplit` (3.12) and the
//! `hostname`/`port` accessors, so URL sanitising and domain blocking behave exactly
//! like 0.1 (no WHATWG path/host normalisation, no IDNA).

use std::net::Ipv6Addr;

/// The five `urlsplit` components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitUrl {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidUrl;

const SCHEME_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-.";

/// `urllib.parse.urlsplit`.
pub fn urlsplit(raw: &str) -> Result<SplitUrl, InvalidUrl> {
    // WHATWG: strip leading C0 control-or-space, drop ASCII tab/newline anywhere.
    let stripped = raw.trim_start_matches(|c: char| c <= ' ');
    let mut url: String = stripped.chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut scheme = String::new();
    if let Some(index) = url.find(':')
        && index > 0
        && url.as_bytes()[0].is_ascii_alphabetic()
        && url[..index].chars().all(|c| SCHEME_CHARS.contains(c))
    {
        scheme = url[..index].to_ascii_lowercase();
        url = url[index + 1..].to_string();
    }
    let mut netloc = String::new();
    if let Some(after) = url.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_string();
        url = after[end..].to_string();
        let open = netloc.contains('[');
        let close = netloc.contains(']');
        if open != close {
            return Err(InvalidUrl);
        }
        if open {
            let bracketed = netloc.split_once('[').map(|(_, rest)| rest).unwrap_or("");
            let host = bracketed.split_once(']').map(|(host, _)| host).unwrap_or(bracketed);
            check_bracketed_host(host)?;
        }
    }
    let mut fragment = String::new();
    if let Some((before, after)) = url.split_once('#') {
        fragment = after.to_string();
        url = before.to_string();
    }
    let mut query = String::new();
    if let Some((before, after)) = url.split_once('?') {
        query = after.to_string();
        url = before.to_string();
    }
    Ok(SplitUrl { scheme, netloc, path: url, query, fragment })
}

fn check_bracketed_host(host: &str) -> Result<(), InvalidUrl> {
    if let Some(rest) = host.strip_prefix('v') {
        // IPvFuture: v<hex>.<anything>
        let (hex, tail) = rest.split_once('.').ok_or(InvalidUrl)?;
        if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) && !tail.is_empty() {
            return Ok(());
        }
        return Err(InvalidUrl);
    }
    let address = host.split_once('%').map(|(addr, _)| addr).unwrap_or(host);
    address.parse::<Ipv6Addr>().map(|_| ()).map_err(|_| InvalidUrl)
}

impl SplitUrl {
    fn hostinfo(&self) -> (&str, Option<&str>) {
        let hostinfo = self.netloc.rsplit_once('@').map(|(_, host)| host).unwrap_or(&self.netloc);
        let (host, port) = match hostinfo.split_once('[') {
            Some((_, bracketed)) => {
                let (host, rest) = bracketed.split_once(']').unwrap_or((bracketed, ""));
                (host, rest.split_once(':').map(|(_, port)| port).unwrap_or(""))
            }
            None => hostinfo.split_once(':').unwrap_or((hostinfo, "")),
        };
        (host, if port.is_empty() { None } else { Some(port) })
    }

    /// Lower-cased host (brackets removed for IPv6), `None` when empty.
    pub fn hostname(&self) -> Option<String> {
        let (host, _) = self.hostinfo();
        if host.is_empty() { None } else { Some(host.to_lowercase()) }
    }

    /// Port, `Ok(None)` when absent, `Err` when not a decimal in `0..=65535`.
    pub fn port(&self) -> Result<Option<u16>, InvalidUrl> {
        match self.hostinfo().1 {
            None => Ok(None),
            Some(port) if port.bytes().all(|b| b.is_ascii_digit()) => {
                port.parse::<u16>().map(Some).map_err(|_| InvalidUrl)
            }
            Some(_) => Err(InvalidUrl),
        }
    }
}

/// Retain only an HTTP(S) scheme, host, port and path — never credentials, query or
/// fragment. Returns `None` for anything else (0.1 `sanitize_url`).
pub fn sanitize_url(raw: &str) -> Option<String> {
    let parsed = urlsplit(raw).ok()?;
    if parsed.scheme != "http" && parsed.scheme != "https" {
        return None;
    }
    let hostname = parsed.hostname()?;
    let port = parsed.port().ok()?;
    let mut netloc = if hostname.contains(':') { format!("[{hostname}]") } else { hostname };
    if let Some(port) = port.filter(|port| *port != 0) {
        netloc.push_str(&format!(":{port}"));
    }
    let path = if parsed.path.is_empty() {
        "/".to_string()
    } else if parsed.path.starts_with('/') {
        parsed.path
    } else {
        format!("/{}", parsed.path)
    };
    Some(format!("{}://{netloc}{path}", parsed.scheme))
}

/// Host used for domain matching: lower-cased, trailing dot removed.
pub fn match_host(raw: &str) -> Option<String> {
    let host = urlsplit(raw).ok()?.hostname()?;
    let host = host.trim_end_matches('.').to_string();
    if host.is_empty() { None } else { Some(host) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_matches_cpython() {
        let s = urlsplit("HTTPS://User:Pw@Example.COM:8443/a/b;p?q=1#frag").unwrap();
        assert_eq!(s.scheme, "https");
        assert_eq!(s.netloc, "User:Pw@Example.COM:8443");
        assert_eq!(s.path, "/a/b;p");
        assert_eq!(s.query, "q=1");
        assert_eq!(s.fragment, "frag");
        assert_eq!(s.hostname().as_deref(), Some("example.com"));
        assert_eq!(s.port(), Ok(Some(8443)));
    }

    #[test]
    fn split_without_scheme_or_netloc() {
        let s = urlsplit("example.com/path").unwrap();
        assert_eq!(s.scheme, "");
        assert_eq!(s.netloc, "");
        assert_eq!(s.path, "example.com/path");
        assert_eq!(s.hostname(), None);
        let s = urlsplit("1http://x").unwrap();
        assert_eq!(s.scheme, "");
    }

    #[test]
    fn whitespace_and_controls_are_stripped() {
        let s = urlsplit("  \thttps://ex\nample.com/p\r").unwrap();
        assert_eq!(s.hostname().as_deref(), Some("example.com"));
        assert_eq!(s.path, "/p");
    }

    #[test]
    fn ipv6_hosts() {
        let s = urlsplit("http://[::1]:8080/x").unwrap();
        assert_eq!(s.hostname().as_deref(), Some("::1"));
        assert_eq!(s.port(), Ok(Some(8080)));
        assert!(urlsplit("http://[::1/x").is_err());
        assert!(urlsplit("http://::1]/x").is_err());
        assert!(urlsplit("http://[127.0.0.1]/").is_err());
        assert!(urlsplit("http://[v1.fe]/").is_ok());
        assert_eq!(sanitize_url("http://[::1]:8080/x?y").as_deref(), Some("http://[::1]:8080/x"));
    }

    #[test]
    fn invalid_ports() {
        assert!(urlsplit("http://h:99999/").unwrap().port().is_err());
        assert!(urlsplit("http://h:ab/").unwrap().port().is_err());
        assert_eq!(urlsplit("http://h:/").unwrap().port(), Ok(None));
    }

    #[test]
    fn sanitize_keeps_origin_and_path_only() {
        assert_eq!(
            sanitize_url("https://user:pass@Example.com:8443/a/b?token=1#x").as_deref(),
            Some("https://example.com:8443/a/b")
        );
        assert_eq!(sanitize_url("http://example.com").as_deref(), Some("http://example.com/"));
        assert_eq!(sanitize_url("https://example.com:443/").as_deref(), Some("https://example.com:443/"));
        assert_eq!(sanitize_url("https://example.com:0/x").as_deref(), Some("https://example.com/x"));
        assert_eq!(sanitize_url("https://example.com?q").as_deref(), Some("https://example.com/"));
    }

    #[test]
    fn sanitize_rejects_non_http_and_hostless() {
        for url in [
            "ftp://example.com/",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "about:blank",
            "moz-extension://abc/page.html",
            "https://",
            "https://user@/x",
            "https://h:70000/",
            "example.com",
            "",
        ] {
            assert_eq!(sanitize_url(url), None, "{url}");
        }
    }

    #[test]
    fn match_host_trims_trailing_dot() {
        assert_eq!(match_host("https://WWW.Chase.com./x").as_deref(), Some("www.chase.com"));
        assert_eq!(match_host("[blocked]"), None);
    }
}
