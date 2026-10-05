//! A Mac's unified log through `log stream` and `log show` with `--style
//! ndjson` (angle 11 part 3). Reading it takes an admin, so privd runs `log`
//! as root; this module builds its arguments and reads its lines, in plain
//! Rust, so it builds and its tests run on any OS.

use cntrl_protocol::logs::{LogEntry, LogsParams};
use serde_json::Value;

use crate::journal::cut;

/// How far back `log show` looks for a service's earlier lines, and for the
/// whole system's, which comes to tens of thousands of lines a minute.
pub const SERVICE_WINDOW: &str = "1h";
pub const SYSTEM_WINDOW: &str = "2m";

/// Which processes a launchd job's lines come from: its program, and while it
/// runs its process, which still matches once a wrapper has handed over to
/// another program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Job {
    pub program: Option<String>,
    pub pid: Option<u32>,
}

/// The predicate for a subscription: log events, not `log`'s own, at the
/// priority asked for, containing the search, and from the job when there is
/// one. Values are quoted, so nothing typed can change its shape.
pub fn predicate(params: &LogsParams, job: Option<&Job>) -> String {
    let mut clauses = vec![
        r#"eventType == "logEvent""#.to_owned(),
        r#"process != "log""#.to_owned(),
    ];
    // There's no warning level: Error and Fault are the ones above Default.
    match params.priority {
        Some(priority) if priority <= 2 => clauses.push(r#"messageType == "fault""#.to_owned()),
        Some(priority) if priority <= 4 => {
            clauses.push(r#"(messageType == "error" OR messageType == "fault")"#.to_owned());
        }
        _ => {}
    }
    if let Some(grep) = params
        .grep
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        clauses.push(format!("eventMessage CONTAINS[c] {}", quote(grep)));
    }
    if let Some(job) = job {
        let mut from = Vec::new();
        if let Some(program) = &job.program {
            from.push(format!("processImagePath == {}", quote(program)));
        }
        if let Some(pid) = job.pid {
            from.push(format!("processID == {pid}"));
        }
        clauses.push(format!("({})", from.join(" OR ")));
    }
    clauses.join(" AND ")
}

/// `log stream`'s arguments, for new lines from now on.
pub fn stream_args(predicate: &str) -> Vec<String> {
    ["stream", "--style", "ndjson", "--predicate", predicate]
        .map(str::to_owned)
        .into()
}

/// `log show`'s arguments, for the lines logged over the last `window`.
pub fn show_args(predicate: &str, window: &str) -> Vec<String> {
    [
        "show",
        "--style",
        "ndjson",
        "--last",
        window,
        "--predicate",
        predicate,
    ]
    .map(str::to_owned)
    .into()
}

/// A string for a predicate: in double quotes, with `\` and `"` escaped and
/// control characters as spaces, so it stays one string whatever it holds.
pub fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str(r"\\"),
            '"' => quoted.push_str(r#"\""#),
            c if c.is_control() => quoted.push(' '),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// One line of `log`'s output as an entry, with its time in microseconds for
/// ordering; `None` for anything else, such as the plain line `log stream`
/// starts with when it filters, `log show`'s closing count, or an activity.
pub fn parse(line: &str) -> Option<(u64, LogEntry)> {
    if !line.starts_with('{') {
        return None;
    }
    let event: Value = serde_json::from_str(line).ok()?;
    let text = |key: &str| event.get(key).and_then(Value::as_str);
    if text("eventType")? != "logEvent" {
        return None;
    }
    let micros = timestamp(text("timestamp")?)?;
    let source = text("processImagePath")
        .and_then(|path| path.rsplit('/').next())
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    let entry = LogEntry {
        ts: micros / 1_000,
        priority: text("messageType").and_then(priority),
        source,
        pid: event
            .get("processID")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()),
        message: cut(text("eventMessage").unwrap_or_default().to_owned()),
    };
    Some((micros, entry))
}

/// A message type as a syslog priority. Default has no syslog match and reads
/// as notice, between Error and Info.
fn priority(kind: &str) -> Option<u8> {
    Some(match kind {
        "Fault" => 2,
        "Error" => 3,
        "Default" => 5,
        "Info" => 6,
        "Debug" => 7,
        _ => return None,
    })
}

/// `2026-10-05 12:49:50.069609+0530` as microseconds since the Unix epoch.
fn timestamp(text: &str) -> Option<u64> {
    let (date, rest) = text.split_once(' ')?;
    let mut date = date.splitn(3, '-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    let sign_at = rest.rfind(['+', '-'])?;
    let (time, offset) = rest.split_at(sign_at);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock = clock.splitn(3, ':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Microseconds from however many digits there are.
    let digits: String = fraction.chars().take(6).collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let micros: i64 = format!("{digits:0<6}").parse().ok()?;
    let zone: String = offset[1..].chars().filter(|c| *c != ':').collect();
    if zone.len() != 4 || !zone.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let zone_minutes = zone[..2].parse::<i64>().ok()? * 60 + zone[2..].parse::<i64>().ok()?;
    let zone_seconds = if offset.starts_with('-') {
        -zone_minutes
    } else {
        zone_minutes
    } * 60;
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - zone_seconds;
    u64::try_from(seconds * 1_000_000 + micros).ok()
}

/// Days from 1970-01-01 to a Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_times_with_their_offsets() {
        assert_eq!(
            timestamp("2026-10-05 12:49:50.069609+0530"),
            Some(1_791_184_790_069_609)
        );
        assert_eq!(
            timestamp("1999-12-31 23:59:59.999999-0800"),
            Some(946_713_599_999_999)
        );
        assert_eq!(
            timestamp("2024-02-29 00:00:00+0000"),
            Some(1_709_164_800_000_000)
        );
        assert_eq!(timestamp("2026-13-05 12:00:00.0+0000"), None);
        assert_eq!(timestamp("yesterday"), None);
    }

    #[test]
    fn reads_log_events_and_skips_the_rest() {
        let line = r#"{"timestamp":"2026-10-05 12:49:50.069609+0530","messageType":"Error","eventType":"logEvent","eventMessage":"bind() failed","processImagePath":"/usr/libexec/rapportd","processID":444,"subsystem":"com.apple.amp","category":"XPC"}"#;
        let (micros, entry) = parse(line).expect("an entry");
        assert_eq!(micros, 1_791_184_790_069_609);
        assert_eq!(
            entry,
            LogEntry {
                ts: 1_791_184_790_069,
                priority: Some(3),
                source: Some("rapportd".to_owned()),
                pid: Some(444),
                message: "bind() failed".to_owned(),
            }
        );
        assert_eq!(
            parse(r#"Filtering the log data using "type == 1024""#),
            None
        );
        assert_eq!(parse(r#"{"count":11,"finished":1}"#), None);
        let activity = r#"{"timestamp":"2026-10-05 12:49:50.069609+0530","eventType":"activityCreateEvent","eventMessage":""}"#;
        assert_eq!(parse(activity), None);
    }

    #[test]
    fn a_search_stays_one_string() {
        let params = LogsParams {
            grep: Some(r#"close") OR (TRUEPREDICATE \ "#.to_owned()),
            priority: Some(3),
            ..LogsParams::default()
        };
        let job = Job {
            program: Some("/opt/homebrew/opt/nginx/bin/nginx".to_owned()),
            pid: Some(812),
        };
        assert_eq!(
            predicate(&params, Some(&job)),
            r#"eventType == "logEvent" AND process != "log" AND (messageType == "error" OR messageType == "fault") AND eventMessage CONTAINS[c] "close\") OR (TRUEPREDICATE \\" AND (processImagePath == "/opt/homebrew/opt/nginx/bin/nginx" OR processID == 812)"#
        );
        assert_eq!(quote("a\nb"), r#""a b""#);
        // The whole system at every level.
        assert_eq!(
            predicate(&LogsParams::default(), None),
            r#"eventType == "logEvent" AND process != "log""#
        );
    }
}
