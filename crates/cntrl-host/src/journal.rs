//! The systemd journal through `journalctl --output=json`, as Cockpit reads
//! it (angle 06 §2, angle 11). A subscription's latest lines are read to
//! their end, so the agent knows when it has them all; new ones are then
//! followed from the last of them, by its cursor. The agent's user reads the system journal as a member of
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

/// Where following picks up once a subscription's earlier lines are read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// After the last earlier line, by its cursor: nothing again, nothing missed.
    After(String),
    /// With no earlier lines, from this second (since the Unix epoch): the
    /// journal had nothing to read then, so all that comes after is new.
    Since(u64),
    /// Only new lines, for a subscription that asked for no earlier ones.
    New,
}

/// journalctl's arguments for a subscription's earlier lines, which it prints
/// and then exits, so the agent knows when it has them all. Each is its own
/// argument, so a unit name can't add others; the caller checks the name
/// anyway. `--all` keeps fields over 4,096 bytes, which would otherwise come as
/// null, and `parse` cuts them instead.
pub fn earlier_args(params: &LogsParams) -> Vec<String> {
    let mut args = base(params);
    args.push(format!(
        "--lines={}",
        params.lines.unwrap_or(DEFAULT_LINES).min(MAX_LINES)
    ));
    args
}

/// journalctl's arguments for following a subscription's new lines from
/// `resume` [Documented: journalctl(1), `--after-cursor`, `--since`].
pub fn follow_args(params: &LogsParams, resume: &Resume) -> Vec<String> {
    let mut args = base(params);
    args.push("--follow".to_owned());
    args.push(match resume {
        Resume::After(cursor) => format!("--after-cursor={cursor}"),
        Resume::Since(seconds) => format!("--since=@{seconds}"),
        Resume::New => "--lines=0".to_owned(),
    });
    args
}

fn base(params: &LogsParams) -> Vec<String> {
    let mut args: Vec<String> = ["--output=json", "--all", "--no-pager", "--quiet"]
        .map(str::to_owned)
        .into();
    if let Some(unit) = &params.unit {
        args.push(format!("--unit={unit}"));
    }
    if let Some(priority) = params.priority {
        args.push(format!("--priority={}", priority.min(7)));
    }
    args
}

/// A record's cursor, which names its place in the journal.
pub fn cursor(line: &str) -> Option<String> {
    let record: Value = serde_json::from_str(line).ok()?;
    record.get("__CURSOR")?.as_str().map(str::to_owned)
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
        // Named as journalctl names it, by its identifier; the unit only when
        // there's nothing else, since a program outside a service, such as
        // systemd itself, runs in a scope like `init.scope` (angle 11 part 4).
        source: text("SYSLOG_IDENTIFIER")
            .or_else(|| text("_COMM"))
            .or_else(|| text("_SYSTEMD_UNIT")),
        pid: text("_PID").and_then(|pid| pid.parse().ok()),
        message: cut(text("MESSAGE").unwrap_or_default()),
    })
}

/// A message cut at [`MAX_MESSAGE`] bytes, on a character, with an ellipsis.
pub fn cut(mut message: String) -> String {
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

/// A journalctl process with these arguments, killed when its handle is
/// dropped.
#[cfg(target_os = "linux")]
pub fn command(args: Vec<String>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("journalctl");
    command
        .args(args)
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
            ..LogsParams::default()
        };
        let asked = earlier_args(&params);
        assert!(asked.contains(&"--lines=1000".to_owned()));
        assert!(asked.contains(&"--unit=nginx.service".to_owned()));
        assert!(asked.contains(&"--priority=7".to_owned()));
        // The earlier lines end, so the agent knows when it has them all.
        assert!(!asked.contains(&"--follow".to_owned()));
        // Matching is the agent's, so it works without journalctl's pcre2.
        assert!(!asked.iter().any(|a| a.starts_with("--grep")));
        assert!(earlier_args(&LogsParams::default()).contains(&"--lines=100".to_owned()));
    }

    #[test]
    fn follows_from_where_the_earlier_lines_ended() {
        let params = LogsParams {
            unit: Some("nginx.service".to_owned()),
            ..LogsParams::default()
        };
        let after = follow_args(&params, &Resume::After("s=1;i=2".to_owned()));
        assert!(after.contains(&"--follow".to_owned()));
        assert!(after.contains(&"--after-cursor=s=1;i=2".to_owned()));
        assert!(after.contains(&"--unit=nginx.service".to_owned()));
        // No --lines with a cursor: it would add earlier lines again.
        assert!(!after.iter().any(|a| a.starts_with("--lines")));
        assert!(
            follow_args(&params, &Resume::Since(1_791_000_000))
                .contains(&"--since=@1791000000".to_owned())
        );
        assert!(follow_args(&params, &Resume::New).contains(&"--lines=0".to_owned()));
        assert_eq!(
            cursor(r#"{"__CURSOR":"s=abc;i=1f","MESSAGE":"hi"}"#).as_deref(),
            Some("s=abc;i=1f")
        );
        assert_eq!(cursor(r#"{"MESSAGE":"hi"}"#), None);
    }

    #[test]
    fn reads_journal_records() {
        let line = r#"{"__REALTIME_TIMESTAMP":"1791000000123456","PRIORITY":"3","_SYSTEMD_UNIT":"nginx.service","SYSLOG_IDENTIFIER":"nginx","_PID":"812","MESSAGE":"bind() failed"}"#;
        assert_eq!(
            parse(line),
            Some(LogEntry {
                ts: 1_791_000_000_123,
                priority: Some(3),
                source: Some("nginx".to_owned()),
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
