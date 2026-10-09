//! Secrets in log lines, redacted on the device before a line leaves it (D102,
//! angle 24), so a secret logged by mistake never reaches the gateway, Console
//! or anyone who reads the device's logs there; the raw line stays in the
//! machine's own log. Log lines are the only free text a device sends.
//!
//! Three kinds of rule, kept few so a busy log stays cheap and plain lines
//! read as they are: a value after a sensitive key (Datadog's process
//! scrubbing words, joined by `=` or `:`, or by a space after a `-` or `--`
//! flag, never a bare space, so sshd's "Failed password for root" is left
//! alone); credentials in URLs and `Authorization` values; and token shapes
//! with literal prefixes, as gitleaks matches them. A private key spread over
//! several lines is hidden line by line until its end ([`KeyBlocks`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use cntrl_protocol::logs::LogEntry;
use regex::{Captures, Regex};

/// What a secret becomes, the owner's word (D102).
pub const REDACTED: &str = "<redacted>";

/// A sensitive key's words. Password-like ones hide any value; the broader
/// ones skip a value that's a count or a flag, such as `max_tokens=4096`.
const WORDS: &str = r"password|passwd|mysql_pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?|stripetoken";

/// The rules, compiled once.
struct Rules {
    /// Fragments one of which a line holds before any rule can match, so most
    /// lines skip the rules entirely, as gitleaks' keywords do.
    prefilter: Regex,
    /// A private key on one line, as JSON escapes one: its body goes.
    key_inline: Regex,
    /// A password in a URL, with or without a user: `postgres://app:pw@db`, `redis://:pw@cache`.
    url: Regex,
    /// An `Authorization` header's value, and a bearer token anywhere.
    authorization: Regex,
    /// `key=value` and `key: value`, in env, query, JSON and YAML styles. The
    /// key may carry more around its word (`DB_PASSWORD`, `x-api-key`), but
    /// never runs on into another word, so `tokenizer: bert` is left alone.
    key_value: Regex,
    /// A flag and its value: `--password hunter2`, `-token abc`.
    flag: Regex,
    /// Token shapes with literal prefixes: AWS key IDs, GitHub, Slack, Stripe,
    /// Google API, OpenAI and Anthropic keys, and JWTs.
    tokens: Regex,
    /// A heartbeat check's URL (D55), which a cron job's command line, logged
    /// by cron, carries: anyone with it could ping the check.
    ping: Regex,
}

impl Rules {
    fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            prefilter: Regex::new(
                r"(?i)pass|pwd|secret|token|key|credential|auth|bearer|://|/ping/|akia|asia|gh[pousr]_|github_pat_|xox|[rs]k_|sk-|aiza|eyj",
            )?,
            key_inline: Regex::new(
                r"(?s)(?P<begin>-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----)(?P<body>.*?)(?P<end>-----END [A-Z0-9 ]*PRIVATE KEY-----|$)",
            )?,
            url: Regex::new(r"(?i)(?P<head>\b[a-z][a-z0-9+.-]*://[^\s:/@]*:)[^\s/@]+@")?,
            authorization: Regex::new(
                r#"(?i)(?P<head>\b(?:proxy-)?authorization["']?\s*[:=]\s*["']?(?:(?:bearer|basic|token|digest)\s+)?)[^\s"',;]+|(?P<bearer>\bbearer\s+)[a-z0-9._~+/-]{8,}=*"#,
            )?,
            key_value: Regex::new(&format!(
                r#"(?i)(?P<key>\b[a-z0-9_.-]*?(?:{WORDS})(?:[_.-][a-z0-9_.-]*)?)(?P<sep>["']?\s*[:=]\s*)(?P<value>"(?:[^"\\]|\\.)*"|'[^']*'|[^\s,;&"'<>(){{}}\[\]]+)"#
            ))?,
            flag: Regex::new(&format!(
                r"(?i)(?P<key>(?:^|\s)--?[a-z0-9_.-]*?(?:{WORDS})(?:[_.-][a-z0-9_.-]*)?)(?P<sep>\s+)(?P<value>[^\s-]\S*)"
            ))?,
            tokens: Regex::new(concat!(
                r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
                r"|\bgh[pousr]_[A-Za-z0-9]{36,}\b",
                r"|\bgithub_pat_[A-Za-z0-9_]{22,}\b",
                r"|\bxox[abposr]-[A-Za-z0-9-]{10,}",
                r"|\b[rs]k_(?:live|test)_[A-Za-z0-9]{16,}\b",
                r"|\bAIza[0-9A-Za-z_-]{35}\b",
                r"|\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{20,}",
                r"|\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            ))?,
            ping: Regex::new(r"(?P<head>/ping/)[A-Za-z0-9_-]{22}\b")?,
        })
    }
}

/// The rules; tests prove they compile, and a line goes whole if they didn't.
static RULES: LazyLock<Result<Rules, regex::Error>> = LazyLock::new(Rules::new);

/// A value that's a count or a flag, not a secret, after a broad key.
fn plain(value: &str) -> bool {
    let bare = value.trim_matches(|c| c == '"' || c == '\'');
    bare.is_empty()
        || bare.bytes().all(|b| b.is_ascii_digit())
        || [
            "true",
            "false",
            "null",
            "none",
            "nil",
            "undefined",
            "<redacted>",
        ]
        .iter()
        .any(|word| bare.eq_ignore_ascii_case(word))
}

/// A key whose value is hidden whatever it is.
fn strict(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("pass") || key.contains("pwd")
}

/// Applies one rule, borrowing the line while nothing changes.
fn apply<'a>(
    line: Cow<'a, str>,
    rule: &Regex,
    replace: impl Fn(&Captures) -> String,
) -> Cow<'a, str> {
    if !rule.is_match(&line) {
        return line;
    }
    let out = rule
        .replace_all(&line, |captures: &Captures| replace(captures))
        .into_owned();
    // A match left as it was, such as a count after a broad key, changes nothing.
    if out == *line { line } else { Cow::Owned(out) }
}

/// A key and its value, or the key with the value redacted, in the value's quotes.
fn keyed(captures: &Captures) -> String {
    let key = &captures["key"];
    let value = &captures["value"];
    let sep = &captures["sep"];
    if !strict(key) && plain(value) {
        return format!("{key}{sep}{value}");
    }
    match value.chars().next() {
        Some(quote @ ('"' | '\'')) => format!("{key}{sep}{quote}{REDACTED}{quote}"),
        _ => format!("{key}{sep}{REDACTED}"),
    }
}

/// The line with every secret the rules find redacted, borrowed when there's
/// none. Were the rules ever not to compile, it fails closed: the whole line.
pub fn redact(line: &str) -> Cow<'_, str> {
    let Ok(rules) = &*RULES else {
        return Cow::Borrowed(REDACTED);
    };
    if !rules.prefilter.is_match(line) {
        return Cow::Borrowed(line);
    }
    let mut line = Cow::Borrowed(line);
    // A line that only begins a key has nothing to hide yet; [`KeyBlocks`] hides what follows.
    line = apply(line, &rules.key_inline, |c| {
        if c["body"].trim().is_empty() {
            c[0].to_owned()
        } else {
            format!("{}{REDACTED}{}", &c["begin"], &c["end"])
        }
    });
    line = apply(line, &rules.url, |c| format!("{}{REDACTED}@", &c["head"]));
    line = apply(line, &rules.authorization, |c| match c.name("head") {
        Some(head) => format!("{}{REDACTED}", head.as_str()),
        None => format!("{}{REDACTED}", &c["bearer"]),
    });
    line = apply(line, &rules.key_value, keyed);
    line = apply(line, &rules.flag, keyed);
    line = apply(line, &rules.tokens, |_| REDACTED.to_owned());
    apply(line, &rules.ping, |c| format!("{}{REDACTED}", &c["head"]))
}

/// Redacts a log entry's message in place.
pub fn redact_entry(entry: &mut LogEntry) {
    if let Cow::Owned(message) = redact(&entry.message) {
        entry.message = message;
    }
}

/// Private keys spread over several lines, as a program that prints one has
/// each line logged on its own: from a line that begins one until the line
/// that ends it, each line from the same source and process is hidden. It
/// sees every line, filtered out or not, so a dropped line can't leave a key
/// open, and gives up on a key that hasn't ended after [`KEY_LINES`].
#[derive(Default)]
pub struct KeyBlocks {
    open: HashMap<(Option<String>, Option<u32>), u16>,
}

/// More lines than any private key's PEM has.
const KEY_LINES: u16 = 200;

impl KeyBlocks {
    pub fn hide(&mut self, entry: &mut LogEntry) {
        let id = (entry.source.clone(), entry.pid);
        if let Some(lines) = self.open.get_mut(&id) {
            if entry.message.contains("PRIVATE KEY-----") && entry.message.contains("-----END") {
                self.open.remove(&id);
            } else {
                *lines += 1;
                if *lines > KEY_LINES {
                    self.open.remove(&id);
                }
                entry.message = REDACTED.to_owned();
            }
            return;
        }
        if entry.message.contains("-----BEGIN")
            && let Some(at) = entry.message.find("PRIVATE KEY-----")
            && !entry.message[at..].contains("-----END")
        {
            self.open.insert(id, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(line: &str) -> String {
        redact(line).into_owned()
    }

    #[test]
    fn rules_compile() {
        assert!(RULES.is_ok(), "{:?}", RULES.as_ref().err());
    }

    #[test]
    fn hides_values_after_sensitive_keys() {
        assert_eq!(
            r("DB_PASSWORD=hunter2 started"),
            "DB_PASSWORD=<redacted> started"
        );
        assert_eq!(
            r(r#"{"password": "hunter2", "user": "ann"}"#),
            r#"{"password": "<redacted>", "user": "ann"}"#
        );
        assert_eq!(r("api_key: abc123def"), "api_key: <redacted>");
        assert_eq!(r("x-api-key: abc123def"), "x-api-key: <redacted>");
        assert_eq!(
            r("GET /cb?code=1&access_token=ya29.abc&state=x"),
            "GET /cb?code=1&access_token=<redacted>&state=x"
        );
        assert_eq!(
            r("client_secret='s3cr3t' given"),
            "client_secret='<redacted>' given"
        );
        assert_eq!(
            r("mysql --password hunter2 -u root"),
            "mysql --password <redacted> -u root"
        );
        assert_eq!(
            r("cntrl-agent install --token abcdefgh"),
            "cntrl-agent install --token <redacted>"
        );
    }

    #[test]
    fn leaves_plain_lines_alone() {
        for line in [
            "Failed password for root from 203.0.113.9 port 51234 ssh2",
            "Failed password for invalid user admin from 203.0.113.9 port 22 ssh2",
            "pam_unix(sshd:auth): authentication failure; logname= uid=0 euid=0 tty=ssh ruser= rhost=203.0.113.9  user=root",
            "Accepted publickey for deploy from 198.51.100.4 port 50022 ssh2: ED25519 SHA256:abc",
            "password reset requested for user ann",
            "max_tokens=4096 temperature=0.2",
            "tokens: 1532 used",
            "tokenizer: bert-base",
            "secretary: Jane",
            "cache key: user:42",
            "secret_enabled=true",
            "Started Session 12 of User root.",
            "GET https://example.com:8443/health 200",
            "connecting to postgres://db.internal:5432/app",
        ] {
            assert_eq!(r(line), line, "{line}");
        }
    }

    #[test]
    fn hides_credentials_in_urls_and_headers() {
        assert_eq!(
            r("dial postgres://app:pa55@db:5432/app"),
            "dial postgres://app:<redacted>@db:5432/app"
        );
        assert_eq!(
            r("redis://:s3cret@cache:6379"),
            "redis://:<redacted>@cache:6379"
        );
        assert_eq!(
            r("Authorization: Bearer abcdefghijk.lmn"),
            "Authorization: Bearer <redacted>"
        );
        assert_eq!(
            r(r#""authorization":"Basic dXNlcjpwYXNz""#),
            r#""authorization":"Basic <redacted>""#
        );
        assert_eq!(
            r("curl -H bearer abcdefghijklmnop"),
            "curl -H bearer <redacted>"
        );
    }

    #[test]
    fn hides_known_token_shapes() {
        assert_eq!(
            r("key id AKIAIOSFODNN7EXAMPLE in use"),
            "key id <redacted> in use"
        );
        assert_eq!(
            r(&format!("token ghp_{}", "a".repeat(36))),
            "token <redacted>"
        );
        assert_eq!(
            r("xoxb-123456789012-abcdefghij posted"),
            "<redacted> posted"
        );
        assert_eq!(
            r(&format!("using sk_live_{}", "b".repeat(24))),
            "using <redacted>"
        );
        assert_eq!(
            r(&format!("model call with sk-ant-{}", "c".repeat(30))),
            "model call with <redacted>"
        );
        assert_eq!(
            r("jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJlc2ln"),
            "jwt <redacted>"
        );
        assert_eq!(
            r("curl -fsS https://gw.cntrl.pw/ping/AbCdEfGhIjKlMnOpQrStUv"),
            "curl -fsS https://gw.cntrl.pw/ping/<redacted>"
        );
    }

    #[test]
    fn hides_private_keys_on_one_line_and_across_lines() {
        assert_eq!(
            r(r#"{"key":"-----BEGIN PRIVATE KEY-----\nMIIEv\n-----END PRIVATE KEY-----"}"#),
            r#"{"key":"-----BEGIN PRIVATE KEY-----<redacted>-----END PRIVATE KEY-----"}"#
        );
        let line = |message: &str| LogEntry {
            ts: 0,
            priority: None,
            source: Some("app".to_owned()),
            pid: Some(7),
            message: message.to_owned(),
        };
        assert_eq!(
            r("-----BEGIN RSA PRIVATE KEY-----"),
            "-----BEGIN RSA PRIVATE KEY-----"
        );
        let mut blocks = KeyBlocks::default();
        let mut out = Vec::new();
        for message in [
            "here it is:",
            "-----BEGIN RSA PRIVATE KEY-----",
            "MIIEowIBAAKCAQEA",
            "q1w2e3r4",
            "-----END RSA PRIVATE KEY-----",
            "done",
        ] {
            let mut entry = line(message);
            blocks.hide(&mut entry);
            out.push(entry.message);
        }
        assert_eq!(
            out,
            [
                "here it is:",
                "-----BEGIN RSA PRIVATE KEY-----",
                "<redacted>",
                "<redacted>",
                "-----END RSA PRIVATE KEY-----",
                "done"
            ]
        );
    }

    /// Reads a file of real log lines and prints each line the rules change,
    /// redacted, to check for false positives; never the original, which may
    /// hold a real secret: `CNTRL_REDACT_SAMPLE=lines.txt cargo test -p
    /// cntrl-agent redact::tests::sample -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads a sample of real logs"]
    fn sample() {
        let path = std::env::var("CNTRL_REDACT_SAMPLE").expect("CNTRL_REDACT_SAMPLE names a file");
        let text = std::fs::read_to_string(path).expect("the sample reads");
        let mut changed = 0;
        let total = text.lines().count();
        for line in text.lines() {
            if let Cow::Owned(after) = redact(line) {
                changed += 1;
                println!("{after}");
            }
        }
        println!("{changed} of {total} lines changed");
    }
}
