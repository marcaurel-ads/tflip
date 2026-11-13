use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::uri::Authority;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;

use crate::ADDRESS;

#[derive(Clone)]
pub struct Token(pub Arc<str>);

impl Token {
    pub fn random() -> anyhow::Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)
            .map_err(|error| anyhow::anyhow!("no random bytes from the OS: {error}"))?;
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Token(hex.into()))
    }
}

/// A DNS-rebound page reaches the studio under its own name, so any other
/// Host is a foreign page.
pub async fn check_host(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if host.is_some_and(|host| is_own_host(host, ADDRESS)) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// A rebound page always arrives under its own domain name, so only
/// `localhost` and loopback addresses can be the studio's own pages. Takes the
/// address rather than reading `ADDRESS`, so the tests cover IPv6 and port 80.
fn is_own_host(host: &str, address: SocketAddr) -> bool {
    let Ok(authority) = host.parse::<Authority>() else {
        return false;
    };
    let name = authority.host();
    // IPv6 addresses keep their brackets in a Host header.
    let ip = name
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(name);
    let host_matches = name.eq_ignore_ascii_case("localhost")
        || ip.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    // Browsers leave out the default port.
    let port = authority.port_u16().unwrap_or(80);
    let port_matches = port == address.port();
    host_matches && port_matches
}

/// Constant time, so how fast a wrong guess fails reveals nothing.
fn is_token(sent: &str, token: &Token) -> bool {
    bool::from(sent.as_bytes().ct_eq(token.0.as_bytes()))
}

pub async fn check_token(State(token): State<Token>, request: Request, next: Next) -> Response {
    let sent = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match sent {
        Some(sent) if is_token(sent, &token) => next.run(request).await,
        // An empty 401 rather than an ApiError: no ADS code means "open the
        // link the studio printed", and the frontend must handle this apart
        // from every ADS error.
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// A browser cannot put a header on a WebSocket, so the token comes in the
/// query string. A URL ends up in more places than a header does (history,
/// proxies), so the Origin check guards the socket even if the token leaks.
pub async fn check_socket(State(token): State<Token>, request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if !origin.is_some_and(|origin| is_own_origin(origin, ADDRESS)) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // The token is hex, so it needs no percent-decoding.
    let sent = request
        .uri()
        .query()
        .into_iter()
        .flat_map(|query| query.split('&'))
        .find_map(|pair| pair.strip_prefix("token="));
    match sent {
        Some(sent) if is_token(sent, &token) => next.run(request).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// The studio serves plain HTTP, so its own pages' Origin is `http://` plus
/// a Host it accepts.
fn is_own_origin(origin: &str, address: SocketAddr) -> bool {
    origin
        .strip_prefix("http://")
        .is_some_and(|host| is_own_host(host, address))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_loopback_names_on_the_studios_port() {
        let v4: SocketAddr = "127.0.0.1:6263".parse().unwrap();
        let v6_80: SocketAddr = "[::1]:80".parse().unwrap();
        // (Host, accepted for v4, accepted for v6_80)
        let cases = [
            ("127.0.0.1:6263", true, false),
            ("localhost:6263", true, false),
            ("[::1]:6263", true, false),
            ("localhost:5173", false, false),
            ("evil.example:6263", false, false),
            ("[::1]", false, true),
            ("localhost", false, true),
            ("127.0.0.1", false, true),
            ("evil.example", false, false),
            ("", false, false),
        ];
        for (host, for_v4, for_v6_80) in cases {
            assert_eq!(is_own_host(host, v4), for_v4, "{host:?} for {v4}");
            assert_eq!(is_own_host(host, v6_80), for_v6_80, "{host:?} for {v6_80}");
        }
    }

    #[test]
    fn accepts_only_the_studios_own_origin() {
        let v4: SocketAddr = "127.0.0.1:6263".parse().unwrap();
        let cases = [
            ("http://127.0.0.1:6263", true),
            ("http://localhost:6263", true),
            ("https://localhost:6263", false),
            ("http://localhost:5173", false),
            ("http://evil.example:6263", false),
            ("null", false),
        ];
        for (origin, accepted) in cases {
            assert_eq!(is_own_origin(origin, v4), accepted, "{origin:?}");
        }
    }
}
