//! Logs on Windows (D58, angle 14): the Event Log's System and Application
//! channels, which Windows lets services read. The latest events come first,
//! from a query in reverse, then new ones from a subscription made before
//! it, skipping what the query already gave. A service's log is what its own
//! provider wrote, and what the Service Control Manager wrote of it.

use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::time::Duration;

use cntrl_protocol::logs::LogEntry;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::EventLog::{
    EVT_HANDLE, EVT_VARIANT, EvtClose, EvtCreateRenderContext, EvtFormatMessage,
    EvtFormatMessageEvent, EvtNext, EvtOpenPublisherMetadata, EvtQuery, EvtQueryChannelPath,
    EvtQueryReverseDirection, EvtRender, EvtRenderContextSystem, EvtRenderEventValues,
    EvtSubscribe, EvtSubscribeToFutureEvents, EvtVarTypeByte, EvtVarTypeFileTime, EvtVarTypeString,
    EvtVarTypeUInt32, EvtVarTypeUInt64,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

/// The channels read, as the journal is the system's one log.
const CHANNELS: [&str; 2] = ["System", "Application"];
/// The provider that writes when services start, stop and fail.
const SERVICE_CONTROL_MANAGER: &str = "Service Control Manager";
/// The longest message passed on, as the journal's lines are cut.
const MESSAGE_MAX: usize = 4096;
/// How often a wait for new events checks whether anyone still reads.
const CHECK_EVERY: Duration = Duration::from_millis(500);
/// Events fetched a call.
const BATCH: usize = 64;
/// System properties, in EVT_SYSTEM_PROPERTY_ID's order.
const PROVIDER: usize = 0;
const LEVEL: usize = 4;
const CREATED: usize = 8;
const RECORD: usize = 9;
const PROCESS: usize = 12;
const CHANNEL: usize = 14;

/// The structured query for the whole system's log, or one service's: its
/// name, and its display name when it has one, as providers and the Service
/// Control Manager name it. `priority` is the least important to keep, as a
/// syslog priority.
pub fn query(service: Option<(&str, Option<&str>)>, priority: Option<u8>) -> String {
    let levels = priority.and_then(levels);
    // What `System` must hold, with the levels: `*` alone for everything.
    let inner = |condition: Option<String>| -> Option<String> {
        let all: Vec<String> = condition.into_iter().chain(levels.clone()).collect();
        (!all.is_empty()).then(|| all.join(" and "))
    };
    let system = |condition: Option<String>| -> String {
        inner(condition).map_or_else(|| "*".to_owned(), |inner| format!("*[System[{inner}]]"))
    };
    let selects: Vec<(&str, String)> = match service {
        None => CHANNELS
            .iter()
            .map(|channel| (*channel, system(None)))
            .collect(),
        Some((name, display)) => {
            let names: Vec<String> = Some(name)
                .into_iter()
                .chain(display)
                .filter_map(literal)
                .map(|name| format!("@Name={name}"))
                .collect();
            let mut selects: Vec<(&str, String)> = CHANNELS
                .iter()
                .map(|channel| {
                    (
                        *channel,
                        system(Some(format!("Provider[{}]", names.join(" or ")))),
                    )
                })
                .collect();
            // The manager names the service by its display name, in param1.
            if let Some(display) = display.or(Some(name)).and_then(literal) {
                let manager = format!("Provider[@Name='{SERVICE_CONTROL_MANAGER}']");
                let inner = inner(Some(manager)).unwrap_or_default();
                selects.push((
                    "System",
                    format!("*[System[{inner}] and EventData[Data[@Name='param1']={display}]]"),
                ));
            }
            selects
        }
    };
    let selects: String = selects
        .iter()
        .map(|(channel, path)| format!(r#"<Select Path="{channel}">{}</Select>"#, xml(path)))
        .collect();
    format!(r#"<QueryList><Query Id="0">{selects}</Query></QueryList>"#)
}

/// Event levels as important as a syslog priority, or more: 1 critical,
/// 2 error, 3 warning, 4 information; 0, logged always, counts as information.
fn levels(priority: u8) -> Option<String> {
    let most = match priority {
        0..=2 => 1,
        3 => 2,
        4 | 5 => 3,
        6 => 4,
        _ => return None,
    };
    Some(if most >= 4 {
        format!("(Level=0 or (Level>=1 and Level<={most}))")
    } else {
        format!("(Level>=1 and Level<={most})")
    })
}

/// An XPath string literal; `None` when `text` holds both kinds of quote.
fn literal(text: &str) -> Option<String> {
    if !text.contains('\'') {
        Some(format!("'{text}'"))
    } else if !text.contains('"') {
        Some(format!("\"{text}\""))
    } else {
        None
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The syslog priority of an event level.
pub fn priority(level: u8) -> Option<u8> {
    match level {
        1 => Some(2),
        2 => Some(3),
        3 => Some(4),
        0 | 4 => Some(6),
        5 => Some(7),
        _ => None,
    }
}

/// Reads `query`: the latest `earlier` events, oldest first, then new ones as
/// they come, handing each to `each` with whether it's new, until `each`
/// says to stop or `closed` says nobody reads. `caught_up` is called once,
/// when the earlier events are all handed over, even if there were none.
/// Returns why it stopped.
pub fn follow(
    query: &str,
    earlier: u32,
    closed: &dyn Fn() -> bool,
    each: &mut dyn FnMut(LogEntry, bool) -> bool,
    caught_up: &mut dyn FnMut() -> bool,
) -> String {
    match read(query, earlier, closed, each, caught_up) {
        Ok(()) => "the subscription closed".to_owned(),
        Err(e) => e,
    }
}

fn read(
    query: &str,
    earlier: u32,
    closed: &dyn Fn() -> bool,
    each: &mut dyn FnMut(LogEntry, bool) -> bool,
    caught_up: &mut dyn FnMut() -> bool,
) -> Result<(), String> {
    let query: Vec<u16> = query.encode_utf16().chain(Some(0)).collect();
    let context = Handle::evt(
        // SAFETY: no value paths: the system's properties.
        unsafe { EvtCreateRenderContext(0, null(), EvtRenderContextSystem) },
        "can't read events",
    )?;
    let mut reader = Reader {
        context,
        publishers: HashMap::new(),
    };
    // Subscribe first, so nothing falls between the query and it.
    // SAFETY: an auto-reset event, closed on drop.
    let signal = unsafe { CreateEventW(null(), 0, 1, null()) };
    if signal.is_null() {
        return Err(format!(
            "can't wait for events: {}",
            std::io::Error::last_os_error()
        ));
    }
    let signal = Kernel(signal);
    let subscription = Handle::evt(
        // SAFETY: the query is NUL-terminated; the signal event outlives the
        // subscription, which is closed first.
        unsafe {
            EvtSubscribe(
                0,
                signal.0,
                null(),
                query.as_ptr(),
                0,
                null(),
                None,
                EvtSubscribeToFutureEvents,
            )
        },
        "can't follow the Event Log",
    )?;
    // The latest records seen, by channel, which the subscription may bring
    // again.
    let mut seen: HashMap<String, u64> = HashMap::new();
    if earlier > 0 {
        let result = Handle::evt(
            // SAFETY: the query is NUL-terminated.
            unsafe {
                EvtQuery(
                    0,
                    null(),
                    query.as_ptr(),
                    EvtQueryChannelPath | EvtQueryReverseDirection,
                )
            },
            "can't read the Event Log",
        )?;
        let mut latest = Vec::new();
        while latest.len() < earlier as usize {
            let events = next(&result, u32::MAX)?;
            if events.is_empty() {
                break;
            }
            for event in events {
                if latest.len() < earlier as usize
                    && let Some((entry, channel, record)) = reader.entry(&event)
                {
                    let last = seen.entry(channel).or_default();
                    *last = (*last).max(record);
                    latest.push(entry);
                }
            }
        }
        for entry in latest.into_iter().rev() {
            if !each(entry, false) {
                return Ok(());
            }
        }
    }
    if !caught_up() {
        return Ok(());
    }
    loop {
        if closed() {
            return Ok(());
        }
        // SAFETY: an open event handle.
        let signalled = unsafe {
            WaitForSingleObject(
                signal.0,
                u32::try_from(CHECK_EVERY.as_millis()).unwrap_or(500),
            )
        };
        if signalled != WAIT_OBJECT_0 {
            continue;
        }
        loop {
            let events = next(&subscription, 0)?;
            if events.is_empty() {
                break;
            }
            for event in events {
                let Some((entry, channel, record)) = reader.entry(&event) else {
                    continue;
                };
                if seen.get(&channel).is_some_and(|last| record <= *last) {
                    continue;
                }
                if !each(entry, true) {
                    return Ok(());
                }
            }
        }
    }
}

/// Up to [`BATCH`] events from a query or a subscription; none when it has
/// no more for now.
fn next(set: &Handle, timeout: u32) -> Result<Vec<Handle>, String> {
    let mut events = [0 as EVT_HANDLE; BATCH];
    let mut returned = 0u32;
    // SAFETY: room for BATCH handles, each closed when its Handle drops.
    let read = unsafe {
        EvtNext(
            set.0,
            u32::try_from(BATCH).unwrap_or(1),
            events.as_mut_ptr(),
            timeout,
            0,
            &mut returned,
        )
    };
    if read == 0 {
        let error = std::io::Error::last_os_error();
        // ERROR_NO_MORE_ITEMS, and ERROR_TIMEOUT when waiting.
        return match error.raw_os_error() {
            Some(259 | 1460) => Ok(Vec::new()),
            _ => Err(format!("can't read the Event Log: {error}")),
        };
    }
    Ok(events[..returned as usize]
        .iter()
        .map(|&event| Handle(event))
        .collect())
}

struct Reader {
    context: Handle,
    /// Each provider's messages, opened once; `None` for one without.
    publishers: HashMap<String, Option<Handle>>,
}

impl Reader {
    /// An event as a log line, with its channel and record number.
    fn entry(&mut self, event: &Handle) -> Option<(LogEntry, String, u64)> {
        let values = self.values(event)?;
        let provider = values.string(PROVIDER).unwrap_or_default();
        let channel = values.string(CHANNEL).unwrap_or_default();
        let record = values.number(RECORD).unwrap_or(0);
        // FILETIME, in 100 ns since 1601.
        let ts = values.number(CREATED).map_or(0, |ticks| {
            (ticks / 10_000).saturating_sub(11_644_473_600_000)
        });
        let level = values
            .number(LEVEL)
            .and_then(|level| u8::try_from(level).ok());
        let message = self.message(&provider, event).unwrap_or_else(|| {
            format!("(an event from {provider} without a message Windows can show)")
        });
        let mut message = message.trim().replace("\r\n", "\n");
        if message.len() > MESSAGE_MAX {
            let mut end = MESSAGE_MAX;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        Some((
            LogEntry {
                ts,
                priority: level.and_then(priority),
                source: (!provider.is_empty()).then_some(provider),
                pid: values
                    .number(PROCESS)
                    .and_then(|pid| u32::try_from(pid).ok())
                    .filter(|pid| *pid > 0),
                message,
            },
            channel,
            record,
        ))
    }

    fn values(&self, event: &Handle) -> Option<Values> {
        let mut used = 0u32;
        let mut count = 0u32;
        // SAFETY: a size query.
        unsafe {
            EvtRender(
                self.context.0,
                event.0,
                EvtRenderEventValues,
                0,
                null_mut(),
                &mut used,
                &mut count,
            )
        };
        if std::io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        {
            return None;
        }
        // Aligned for the variants, whose strings point into the buffer too.
        let mut buffer = vec![0u64; (used as usize).div_ceil(8)];
        // SAFETY: the buffer holds `used` bytes.
        let rendered = unsafe {
            EvtRender(
                self.context.0,
                event.0,
                EvtRenderEventValues,
                used,
                buffer.as_mut_ptr().cast(),
                &mut used,
                &mut count,
            )
        };
        (rendered != 0).then_some(Values {
            buffer,
            count: count as usize,
        })
    }

    /// The event's message in the system's language, from its provider.
    fn message(&mut self, provider: &str, event: &Handle) -> Option<String> {
        let publisher = self
            .publishers
            .entry(provider.to_owned())
            .or_insert_with(|| {
                let name: Vec<u16> = provider.encode_utf16().chain(Some(0)).collect();
                // SAFETY: a NUL-terminated provider name.
                let handle = unsafe { EvtOpenPublisherMetadata(0, name.as_ptr(), null(), 0, 0) };
                (handle != 0).then_some(Handle(handle))
            })
            .as_ref()?;
        let mut used = 0u32;
        // SAFETY: a size query.
        unsafe {
            EvtFormatMessage(
                publisher.0,
                event.0,
                0,
                0,
                null(),
                EvtFormatMessageEvent,
                0,
                null_mut(),
                &mut used,
            )
        };
        if used == 0 {
            return None;
        }
        let mut text = vec![0u16; used as usize];
        // SAFETY: the buffer holds `used` characters.
        let formatted = unsafe {
            EvtFormatMessage(
                publisher.0,
                event.0,
                0,
                0,
                null(),
                EvtFormatMessageEvent,
                used,
                text.as_mut_ptr(),
                &mut used,
            )
        };
        if formatted == 0 {
            return None;
        }
        let length = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        Some(String::from_utf16_lossy(&text[..length])).filter(|text| !text.trim().is_empty())
    }
}

/// An event's system properties, as EvtRender gives them.
struct Values {
    buffer: Vec<u64>,
    count: usize,
}

impl Values {
    fn variant(&self, index: usize) -> Option<&EVT_VARIANT> {
        if index >= self.count {
            return None;
        }
        // SAFETY: the buffer begins with `count` variants.
        Some(unsafe { &*self.buffer.as_ptr().cast::<EVT_VARIANT>().add(index) })
    }

    fn string(&self, index: usize) -> Option<String> {
        let variant = self.variant(index)?;
        if variant.Type != EvtVarTypeString as u32 {
            return None;
        }
        // SAFETY: a string variant's value is a NUL-terminated string in the
        // buffer.
        unsafe {
            let text = variant.Anonymous.StringVal;
            if text.is_null() {
                return None;
            }
            let length = (0..).take_while(|&i| *text.add(i) != 0).count();
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                text, length,
            )))
        }
    }

    fn number(&self, index: usize) -> Option<u64> {
        let variant = self.variant(index)?;
        // SAFETY: the type says which of the union's fields is set.
        unsafe {
            match variant.Type {
                t if t == EvtVarTypeByte as u32 => Some(u64::from(variant.Anonymous.ByteVal)),
                t if t == EvtVarTypeUInt32 as u32 => Some(u64::from(variant.Anonymous.UInt32Val)),
                t if t == EvtVarTypeUInt64 as u32 => Some(variant.Anonymous.UInt64Val),
                t if t == EvtVarTypeFileTime as u32 => Some(variant.Anonymous.FileTimeVal),
                _ => None,
            }
        }
    }
}

/// An Event Log handle, closed when dropped.
struct Handle(EVT_HANDLE);

impl Handle {
    fn evt(handle: EVT_HANDLE, what: &str) -> Result<Self, String> {
        if handle == 0 {
            return Err(format!("{what}: {}", std::io::Error::last_os_error()));
        }
        Ok(Self(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: an open handle that this owns.
        unsafe { EvtClose(self.0) };
    }
}

/// A kernel handle, closed when dropped.
struct Kernel(HANDLE);

impl Drop for Kernel {
    fn drop(&mut self) {
        // SAFETY: an open handle that this owns.
        unsafe { CloseHandle(self.0) };
    }
}

/// Now, in milliseconds since the Unix epoch, for tests.
#[cfg(test)]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_the_system_or_a_service() {
        assert_eq!(
            query(None, None),
            r#"<QueryList><Query Id="0"><Select Path="System">*</Select><Select Path="Application">*</Select></Query></QueryList>"#
        );
        let warnings = query(None, Some(4));
        assert!(
            warnings.contains("*[System[(Level&gt;=1 and Level&lt;=3)]]"),
            "{warnings}"
        );
        let service = query(Some(("Spooler", Some("Print Spooler"))), None);
        assert!(
            service.contains("Provider[@Name='Spooler' or @Name='Print Spooler']"),
            "{service}"
        );
        assert!(
            service.contains("Data[@Name='param1']='Print Spooler'"),
            "{service}"
        );
    }

    #[test]
    fn maps_levels_to_syslog_priorities() {
        assert_eq!(priority(1), Some(2));
        assert_eq!(priority(2), Some(3));
        assert_eq!(priority(3), Some(4));
        assert_eq!(priority(4), Some(6));
        assert_eq!(priority(0), Some(6));
        assert_eq!(priority(5), Some(7));
    }

    #[test]
    fn reads_the_latest_events() {
        let mut got = Vec::new();
        let reason = follow(&query(None, None), 20, &|| false, &mut |entry, live| {
            got.push((entry, live));
            got.len() < 20
        });
        assert!(!got.is_empty(), "{reason}");
        assert!(got.iter().all(|(_, live)| !live));
        assert!(
            got.iter()
                .all(|(entry, _)| entry.ts > 0 && entry.ts <= now_ms())
        );
        assert!(got.iter().any(|(entry, _)| entry.source.is_some()));
    }
}
