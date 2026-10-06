//! The checks the hub hands this device (D56, angle 21): HTTP(S) and TCP, from
//! the device's network, where private addresses are. Each runs at its
//! interval while the link is up, and its results go back over the link; the
//! hub keeps their state and alerts. A run is not a request, so it isn't
//! audited: the set is, once, when it changes.
//!
//! An HTTP check opens a fresh connection each time, as a probe does, so a
//! run covers the lookup, the connection and the handshake. It trusts the
//! system's roots besides Mozilla's, goes around any proxy, and reads the
//! server's certificate: its expiry, subject and issuer.

use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::time::{Duration, Instant};

use cntrl_protocol::checks::{
    CHECKS_MAX, CertInfo, CheckKind, CheckResult, DeviceCheck, INTERVAL_MIN_S, REDIRECTS,
    TIMEOUT_MS,
};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::{MissedTickBehavior, sleep, timeout};
use x509_cert::Certificate;
use x509_cert::der::Decode;

use super::uplink::now_ms;

const TIMEOUT: Duration = Duration::from_millis(TIMEOUT_MS as u64);
const USER_AGENT: &str = concat!(
    "cntrl-check/",
    env!("CARGO_PKG_VERSION"),
    " (+https://cntrl.pw)"
);
/// The longest error kept, in characters.
const ERROR_MAX: usize = 200;
/// First runs spread over this long, so a set doesn't run all at once.
const SPREAD_MS: u64 = 5_000;

/// The checks running for this session. Dropping it stops them.
pub struct Runner {
    running: HashMap<String, (DeviceCheck, AbortHandle)>,
    results: mpsc::Sender<CheckResult>,
    clients: Option<Clients>,
}

impl Runner {
    pub fn new(results: mpsc::Sender<CheckResult>) -> Self {
        Self {
            running: HashMap::new(),
            results,
            clients: None,
        }
    }

    /// Runs `checks` from now on, replacing the set before: a check that's
    /// gone or changed stops, and a new or changed one starts. Says whether
    /// the set changed. Kinds this agent doesn't know are skipped, and only
    /// the first [`CHECKS_MAX`] run.
    pub fn set(&mut self, checks: Vec<DeviceCheck>) -> Result<bool, String> {
        let wanted: HashMap<String, DeviceCheck> = checks
            .into_iter()
            .filter(|check| check.kind != CheckKind::Unknown)
            .take(CHECKS_MAX)
            .map(|check| (check.id.clone(), check))
            .collect();
        let before = self.running.len();
        self.running.retain(|id, (check, task)| {
            let keep = wanted.get(id) == Some(check);
            if !keep {
                task.abort();
            }
            keep
        });
        let mut changed = self.running.len() != before;
        let fresh: Vec<DeviceCheck> = wanted
            .into_values()
            .filter(|check| !self.running.contains_key(&check.id))
            .collect();
        if fresh.is_empty() {
            return Ok(changed);
        }
        if self.clients.is_none() {
            self.clients = Some(Clients::new()?);
        }
        let clients = self.clients.clone().ok_or("no HTTP client")?;
        for check in fresh {
            let task = tokio::spawn(run_every(
                check.clone(),
                clients.clone(),
                self.results.clone(),
            ));
            self.running
                .insert(check.id.clone(), (check, task.abort_handle()));
            changed = true;
        }
        Ok(changed)
    }

    /// The checks running, for the audit log.
    pub fn summary(&self) -> serde_json::Value {
        let mut checks: Vec<&DeviceCheck> = self.running.values().map(|(check, _)| check).collect();
        checks.sort_by(|a, b| a.id.cmp(&b.id));
        serde_json::json!(
            checks
                .iter()
                .map(|check| serde_json::json!({
                    "id": check.id,
                    "kind": check.kind,
                    "target": check.target,
                    "interval_s": check.interval_s,
                }))
                .collect::<Vec<_>>()
        )
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        for (_, task) in self.running.values() {
            task.abort();
        }
    }
}

/// One HTTP client that verifies certificates and one that doesn't, both
/// built once: loading the system's roots reads its certificate store.
#[derive(Clone)]
struct Clients {
    verifying: reqwest::Client,
    trusting: reqwest::Client,
}

impl Clients {
    fn new() -> Result<Self, String> {
        let build = |ignore_tls: bool| {
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .redirect(reqwest::redirect::Policy::limited(REDIRECTS))
                .timeout(TIMEOUT)
                .connect_timeout(TIMEOUT)
                .pool_max_idle_per_host(0)
                .no_proxy()
                .tls_info(true)
                .danger_accept_invalid_certs(ignore_tls)
                .build()
                .map_err(|e| format!("can't make an HTTP client: {}", reason(&e)))
        };
        Ok(Self {
            verifying: build(false)?,
            trusting: build(true)?,
        })
    }
}

/// Runs a check at its interval until it's stopped or nobody takes results.
async fn run_every(check: DeviceCheck, clients: Clients, results: mpsc::Sender<CheckResult>) {
    sleep(Duration::from_millis(spread(&check.id))).await;
    let every = Duration::from_secs(u64::from(check.interval_s.max(INTERVAL_MIN_S)));
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let result = run_once(&check, &clients).await;
        if results.send(result).await.is_err() {
            return;
        }
    }
}

/// Where in the first [`SPREAD_MS`] a check first runs, the same for the
/// same ID.
fn spread(id: &str) -> u64 {
    id.bytes().fold(0u64, |sum, byte| {
        sum.wrapping_mul(31).wrapping_add(u64::from(byte))
    }) % SPREAD_MS
}

/// Runs a check once.
async fn run_once(check: &DeviceCheck, clients: &Clients) -> CheckResult {
    let at_ms = now_ms();
    let found = match check.kind {
        CheckKind::Http => {
            let client = if check.ignore_tls {
                &clients.trusting
            } else {
                &clients.verifying
            };
            http(client, &check.target).await
        }
        CheckKind::Tcp => tcp(&check.target).await,
        CheckKind::Unknown => Found::down("a kind of check this agent doesn't know".to_owned()),
    };
    CheckResult {
        id: check.id.clone(),
        rev: check.rev,
        at_ms,
        ok: found.ok,
        ms: found.ms,
        code: found.code,
        error: found.error,
        cert: found.cert,
    }
}

/// What a run found.
#[derive(Debug)]
struct Found {
    ok: bool,
    ms: Option<u32>,
    code: Option<u16>,
    error: Option<String>,
    cert: Option<CertInfo>,
}

impl Found {
    fn down(error: String) -> Self {
        Self {
            ok: false,
            ms: None,
            code: None,
            error: Some(short(error)),
            cert: None,
        }
    }
}

/// Up when the final answer is 2xx or 3xx within the timeout; how long the
/// headers took, from the start.
async fn http(client: &reqwest::Client, url: &str) -> Found {
    let started = Instant::now();
    match client.get(url).send().await {
        Ok(response) => {
            let ms = elapsed_ms(started);
            let code = response.status().as_u16();
            let ok = (200..400).contains(&code);
            let cert = response
                .extensions()
                .get::<reqwest::tls::TlsInfo>()
                .and_then(|info| info.peer_certificate())
                .and_then(cert_info);
            Found {
                ok,
                ms: Some(ms),
                code: Some(code),
                error: (!ok).then(|| format!("HTTP {code}")),
                cert,
            }
        }
        Err(e) if e.is_timeout() => Found::down(no_answer()),
        Err(e) if e.is_redirect() => Found::down(format!("more than {REDIRECTS} redirects")),
        Err(e) => Found::down(reason(&e)),
    }
}

/// Up when the port takes a connection within the timeout.
async fn tcp(target: &str) -> Found {
    let Some((host, port)) = host_port(target) else {
        return Found::down("not a host and port".to_owned());
    };
    let started = Instant::now();
    match timeout(TIMEOUT, TcpStream::connect((host.as_str(), port))).await {
        Ok(Ok(_)) => Found {
            ok: true,
            ms: Some(elapsed_ms(started)),
            code: None,
            error: None,
            cert: None,
        },
        Ok(Err(e)) => Found::down(io_reason(&e)),
        Err(_) => Found::down(no_answer()),
    }
}

/// A TCP check's target, `host:port` or `[v6]:port`.
fn host_port(target: &str) -> Option<(String, u16)> {
    let (host, port) = target.trim().rsplit_once(':')?;
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() || (host.contains(':') && !target.trim().starts_with('[')) {
        return None;
    }
    let port: u16 = port.parse().ok().filter(|port| *port > 0)?;
    Some((host.to_owned(), port))
}

/// The server's leaf certificate: when it expires, its subject's common name,
/// and its issuer's organization or common name. None if it doesn't parse.
fn cert_info(der: &[u8]) -> Option<CertInfo> {
    let cert = Certificate::from_der(der).ok()?;
    let tbs = cert.tbs_certificate();
    let not_after = tbs.validity().not_after.to_unix_duration();
    let text = |value: Option<x509_cert::ext::pkix::name::DirectoryString>| {
        value.map(|value| value.value().into_owned())
    };
    let issuer = tbs.issuer();
    Some(CertInfo {
        not_after_ms: u64::try_from(not_after.as_millis()).unwrap_or(u64::MAX),
        subject: text(tbs.subject().common_name().ok().flatten()),
        issuer: text(issuer.organization().ok().flatten())
            .or_else(|| text(issuer.common_name().ok().flatten())),
    })
}

fn no_answer() -> String {
    format!("no answer within {} s", TIMEOUT.as_secs())
}

/// Why something failed, in words: the deepest cause, which says the most,
/// with a certificate's failure said plainly.
fn reason(error: &(dyn Error + 'static)) -> String {
    let mut deepest = error;
    while let Some(source) = deepest.source() {
        deepest = source;
    }
    let text = match deepest.downcast_ref::<io::Error>() {
        Some(e) => io_reason(e),
        None => deepest.to_string(),
    };
    tls_reason(&text).unwrap_or(text)
}

/// A certificate that didn't verify, in words, from what rustls says of it:
/// its variants by name, or the messages of those with context.
fn tls_reason(text: &str) -> Option<String> {
    let (_, detail) = text.split_once("invalid peer certificate: ")?;
    let has = |needles: &[&str]| needles.iter().any(|needle| detail.contains(needle));
    let plain = if has(&["UnknownIssuer", "CaUsedAsEndEntity"]) {
        "the certificate is self-signed or from an authority this device doesn't trust"
    } else if has(&["certificate expired", "Expired"]) {
        "the certificate has expired"
    } else if has(&["not valid yet", "NotValidYet"]) {
        "the certificate isn't valid yet"
    } else if has(&["not valid for name", "NotValidForName"]) {
        "the certificate isn't for this name"
    } else if has(&["Revoked"]) {
        "the certificate has been revoked"
    } else if has(&["BadSignature", "UnsupportedSignatureAlgorithm"]) {
        "the certificate's signature doesn't verify"
    } else {
        return Some(format!("the certificate doesn't verify: {detail}"));
    };
    Some(plain.to_owned())
}

fn io_reason(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => "connection refused".to_owned(),
        io::ErrorKind::ConnectionReset => "connection reset".to_owned(),
        io::ErrorKind::HostUnreachable => "host unreachable".to_owned(),
        io::ErrorKind::NetworkUnreachable => "network unreachable".to_owned(),
        io::ErrorKind::TimedOut => no_answer(),
        _ => error.to_string(),
    }
}

/// An error short enough to keep.
fn short(text: String) -> String {
    if text.chars().count() <= ERROR_MAX {
        return text;
    }
    let mut cut: String = text.chars().take(ERROR_MAX - 1).collect();
    cut.push('…');
    cut
}

fn elapsed_ms(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn check(kind: CheckKind, target: String) -> DeviceCheck {
        DeviceCheck {
            id: "chk_test".to_owned(),
            rev: 1,
            kind,
            target,
            interval_s: 30,
            ignore_tls: false,
        }
    }

    #[test]
    fn reads_a_host_and_port() {
        assert_eq!(
            host_port("db.internal:5432"),
            Some(("db.internal".to_owned(), 5432))
        );
        assert_eq!(host_port("[::1]:8080"), Some(("::1".to_owned(), 8080)));
        assert_eq!(host_port("10.0.0.5:22"), Some(("10.0.0.5".to_owned(), 22)));
        assert_eq!(host_port("db.internal"), None);
        assert_eq!(host_port("::1:8080"), None);
        assert_eq!(host_port("db:0"), None);
        assert_eq!(host_port("db:70000"), None);
    }

    #[tokio::test]
    async fn a_port_that_takes_a_connection_is_up_and_a_closed_one_down() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let open = listener.local_addr().expect("address");
        let clients = Clients::new().expect("clients");
        let up = run_once(&check(CheckKind::Tcp, open.to_string()), &clients).await;
        assert!(up.ok, "{up:?}");
        assert!(up.ms.is_some());
        drop(listener);
        let down = run_once(&check(CheckKind::Tcp, open.to_string()), &clients).await;
        assert!(!down.ok);
        assert_eq!(down.error.as_deref(), Some("connection refused"));
    }

    /// Serves one canned HTTP answer per connection.
    async fn serve(status: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let answer = format!(
                    "HTTP/1.1 {status}\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"
                );
                let _ = socket.write_all(answer.as_bytes()).await;
            }
        });
        format!("http://{address}/health")
    }

    #[tokio::test]
    async fn http_is_up_on_2xx_and_down_with_the_status_otherwise() {
        let clients = Clients::new().expect("clients");
        let up = run_once(
            &check(CheckKind::Http, serve("204 No Content").await),
            &clients,
        )
        .await;
        assert!(up.ok, "{up:?}");
        assert_eq!(up.code, Some(204));
        assert!(up.cert.is_none());
        let down = run_once(
            &check(CheckKind::Http, serve("503 Service Unavailable").await),
            &clients,
        )
        .await;
        assert!(!down.ok);
        assert_eq!(
            (down.code, down.error.as_deref()),
            (Some(503), Some("HTTP 503"))
        );
    }

    #[tokio::test]
    async fn a_new_set_replaces_the_old_and_says_whether_it_changed() {
        let (sender, _results) = mpsc::channel(8);
        let mut runner = Runner::new(sender);
        let one = check(CheckKind::Tcp, "127.0.0.1:9".to_owned());
        assert_eq!(runner.set(vec![one.clone()]), Ok(true));
        assert_eq!(runner.set(vec![one.clone()]), Ok(false));
        let changed = DeviceCheck { rev: 2, ..one };
        assert_eq!(runner.set(vec![changed]), Ok(true));
        assert_eq!(runner.set(Vec::new()), Ok(true));
        assert_eq!(runner.running.len(), 0);
    }

    #[test]
    fn reads_a_certificates_expiry_subject_and_issuer() {
        // A self-signed P-256 certificate made with openssl for this test.
        let der = include_bytes!("../../testdata/check-cert.der");
        let cert = cert_info(der).expect("parses");
        assert_eq!(cert.not_after_ms, 2_106_647_846_000);
        assert_eq!(cert.subject.as_deref(), Some("checks.test"));
        assert_eq!(cert.issuer.as_deref(), Some("cntrl test CA"));
        assert!(cert_info(b"not a certificate").is_none());
    }

    #[test]
    fn says_why_a_certificate_failed() {
        let said = |detail: &str| tls_reason(&format!("invalid peer certificate: {detail}"));
        assert_eq!(
            said("Other(OtherError(CaUsedAsEndEntity))").as_deref(),
            Some("the certificate is self-signed or from an authority this device doesn't trust"),
        );
        assert_eq!(
            said("UnknownIssuer"),
            said("Other(OtherError(CaUsedAsEndEntity))")
        );
        assert_eq!(
            said("certificate expired: verification time 2 (UNIX), but certificate is not valid after 1 (1 seconds ago)").as_deref(),
            Some("the certificate has expired"),
        );
        assert_eq!(
            said("certificate not valid for name \"a.lan\"; certificate is only valid for DnsName(\"b.lan\")").as_deref(),
            Some("the certificate isn't for this name"),
        );
        assert_eq!(
            said("Strange").as_deref(),
            Some("the certificate doesn't verify: Strange")
        );
        assert_eq!(tls_reason("connection refused"), None);
    }

    #[test]
    fn keeps_errors_short() {
        let long = "x".repeat(500);
        assert_eq!(short(long).chars().count(), ERROR_MAX);
        assert_eq!(short("refused".to_owned()), "refused");
    }
}
