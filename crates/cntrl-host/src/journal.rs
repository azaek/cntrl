//! The systemd journal through `journalctl --output=json --follow`, as Cockpit
//! reads it (angle 06 §2, angle 11): the latest lines, then new ones as they
//! come. The agent's user reads the system journal as a member of
//! `systemd-journal`, which the installer adds it to. Parsing is plain Rust,
//! so it builds and its tests run on any OS.

use cntrl_protocol::logs::{LogEntry, LogsParams};
use serde_json::Value;

/// How many earlier lines a subscription gets when it doesn't say, and the most
/// it may ask for.
pub const DEFAULT_LINES: u32 = 100;
pub const MAX_LINES: u32 = 1_000;
/// A message longer than this is cut, with an ellipsis.
pub const MAX_MESSAGE: usize = 4_096;

/// journalctl's arguments for a subscription. Each is its own argument, so a
/// unit name can't add others; the caller checks the name anyway. `--all`
/// keeps fields over 4,096 bytes, which would otherwise come as null, and
/// `parse` cuts them instead.
pub fn args(params: &LogsParams) -> Vec<String> {
    let mut args: Vec<String> = [
        "--output=json",
        "--all",
        "--no-pager",
        "--quiet",
        "--follow",
    ]
    .map(str::to_owned)
    .into();
    args.push(format!(
        "--lines={}",
        params.lines.unwrap_or(DEFAULT_LINES).min(MAX_LINES)
    ));
    if let Some(unit) = &params.unit {
        args.push(format!("--unit={unit}"));
    }
    if let Some(priority) = params.priority {
        args.push(format!("--priority={}", priority.min(7)));
    }
    args
}

/// One line of journalctl's JSON as an entry; `None` for anything else. Every
/// field is a string, except values that aren't UTF-8, which come as arrays of
/// bytes [Documented: journalctl(1), `--output=json`].
pub fn parse(line: &str) -> Option<LogEntry> {
    let record: Value = serde_json::from_str(line).ok()?;
    let text = |name: &str| -> Option<String> {
        match record.get(name)? {
            Value::String(text) => Some(text.clone()),
            Value::Array(bytes) => {
                let bytes: Vec<u8> = bytes
                    .iter()
                    .filter_map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
                    .collect();
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
            _ => None,
        }
    };
    let micros: u64 = text("__REALTIME_TIMESTAMP")?.parse().ok()?;
    Some(LogEntry {
        ts: micros / 1_000,
        priority: text("PRIORITY").and_then(|p| p.parse().ok()),
        source: text("_SYSTEMD_UNIT")
            .or_else(|| text("SYSLOG_IDENTIFIER"))
            .or_else(|| text("_COMM")),
        pid: text("_PID").and_then(|pid| pid.parse().ok()),
        message: cut(text("MESSAGE").unwrap_or_default()),
    })
}

fn cut(mut message: String) -> String {
    if message.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push('…');
    }
    message
}

/// The journalctl process for a subscription, killed when its handle is
/// dropped.
#[cfg(target_os = "linux")]
pub fn command(params: &LogsParams) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("journalctl");
    command
        .args(args(params))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_journalctl_for_what_was_subscribed() {
        let params = LogsParams {
            unit: Some("nginx.service".to_owned()),
            priority: Some(9),
            grep: Some("error".to_owned()),
            lines: Some(5_000),
        };
        let asked = args(&params);
        assert!(asked.contains(&"--lines=1000".to_owned()));
        assert!(asked.contains(&"--unit=nginx.service".to_owned()));
        assert!(asked.contains(&"--priority=7".to_owned()));
        // Matching is the agent's, so it works without journalctl's pcre2.
        assert!(!asked.iter().any(|a| a.starts_with("--grep")));
        assert!(args(&LogsParams::default()).contains(&"--lines=100".to_owned()));
    }

    #[test]
    fn reads_journal_records() {
        let line = r#"{"__REALTIME_TIMESTAMP":"1791000000123456","PRIORITY":"3","_SYSTEMD_UNIT":"nginx.service","SYSLOG_IDENTIFIER":"nginx","_PID":"812","MESSAGE":"bind() failed"}"#;
        assert_eq!(
            parse(line),
            Some(LogEntry {
                ts: 1_791_000_000_123,
                priority: Some(3),
                source: Some("nginx.service".to_owned()),
                pid: Some(812),
                message: "bind() failed".to_owned(),
            })
        );
        // A kernel line has no unit, and a binary message comes as bytes.
        let kernel = r#"{"__REALTIME_TIMESTAMP":"1791000000000000","SYSLOG_IDENTIFIER":"kernel","MESSAGE":[104,105,255]}"#;
        let entry = parse(kernel).expect("an entry");
        assert_eq!(entry.source.as_deref(), Some("kernel"));
        assert_eq!(entry.message, "hi\u{fffd}");
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(r#"{"MESSAGE":"no time"}"#), None);
    }

    #[test]
    fn long_messages_are_cut_on_a_character() {
        let long = format!("{}é{}", "a".repeat(MAX_MESSAGE - 1), "b".repeat(10));
        let cut = cut(long);
        assert!(cut.len() <= MAX_MESSAGE + '…'.len_utf8());
        assert!(cut.ends_with('…'));
    }
}
