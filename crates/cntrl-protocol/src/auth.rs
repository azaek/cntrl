//! What the device key signs to open a session: the gateway's challenge, the
//! gateway it came from, and who the device says it is. Binding the host stops a
//! signature captured by one gateway from opening a session at another.

/// The string the device key signs in a [`crate::frame::Hello`].
pub fn hello_signing_string(
    sid: &str,
    nonce: &str,
    gateway_host: &str,
    device_id: &str,
    key_id: &str,
    generation: u64,
) -> String {
    format!("cntrl-hello-v1\n{sid}\n{nonce}\n{gateway_host}\n{device_id}\n{key_id}\n{generation}")
}

/// The gateway host a hello signs, from the URL the agent dialed or the gateway
/// was reached at: lowercase, without user info, and with the port only when it
/// isn't the scheme's default. This is what a WHATWG URL's `host` gives, which is
/// what the gateway compares. `None` when `url` isn't a ws, wss, http or https URL.
pub fn gateway_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "ws" | "http" => 80,
        "wss" | "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .to_ascii_lowercase();
    // A port follows the last colon, unless that colon is inside an IPv6 literal.
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, port),
        _ => (authority.as_str(), ""),
    };
    if host.is_empty() {
        return None;
    }
    if port.is_empty() {
        return Some(host.to_owned());
    }
    let port: u16 = port.parse().ok()?;
    Some(if port == default_port {
        host.to_owned()
    } else {
        format!("{host}:{port}")
    })
}
