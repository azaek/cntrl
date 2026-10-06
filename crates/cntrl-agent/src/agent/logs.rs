//! The `logs` topic's readers (angle 11): one per subscription, running only
//! while it's open, its lines gathered into batches. On Linux the agent runs
//! `journalctl` itself. On a Mac only an admin reads the unified log, so privd
//! runs `log` as root and sends the batches back over its socket (angle 11
//! part 3). A session ends its readers with its subscriptions, and stopping a
//! reader ends what it runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cntrl_protocol::logs::{LogEntry, LogsBatch, LogsParams};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

/// How many `logs` subscriptions a session holds open at once. A new one past
/// it closes the oldest, whose viewer may have gone without the gateway
/// noticing yet (D36).
pub const MAX_OPEN: usize = 8;

/// Where readers send their batches: the subscription's ID with each.
pub type Batches = mpsc::Sender<(String, LogsBatch)>;

/// How often a batch goes out while lines come.
const FLUSH: Duration = Duration::from_millis(250);
/// A batch goes out early once it holds this many lines or bytes.
const BATCH_LINES: usize = 200;
const BATCH_BYTES: usize = 64 * 1024;
/// New lines per second a subscription passes on; past it, an even sample of
/// about this many goes (angle 11 part 4). The earlier lines a stream starts
/// with aren't counted.
const RATE: u32 = 500;
/// How many sources a batch counts lines for: the busiest.
const COUNTED_SOURCES: usize = 20;

/// A subscription's running reader.
pub struct Reader {
    task: AbortHandle,
    /// When it started, so the oldest can give way.
    pub started: Instant,
}

impl Reader {
    /// Stops the reader and what it runs.
    pub fn stop(&self) {
        self.task.abort();
    }

    /// A reader that reads nothing, for tests.
    #[cfg(test)]
    pub fn idle(started: Instant) -> Self {
        Self {
            task: tokio::spawn(std::future::pending::<()>()).abort_handle(),
            started,
        }
    }
}

/// Starts reading for subscription `id`; on a Mac through privd at `privd`.
/// When the reader stops on its own, its last batch says why.
pub fn start(id: String, params: LogsParams, privd: PathBuf, out: Batches) -> Reader {
    let task = tokio::spawn(async move {
        let reason = read(&id, &params, &privd, &out).await;
        let last = LogsBatch {
            ended: Some(reason),
            ..LogsBatch::default()
        };
        let _ = out.send((id, last)).await;
    })
    .abort_handle();
    Reader {
        task,
        started: Instant::now(),
    }
}

/// Gathers a reader's lines into batches: one goes out when it's full, the
/// rest on each [`FLUSH`] tick. A search and the sources asked for are matched
/// here, as plain text. New lines are counted by source, and past [`RATE`] a
/// second they're sampled.
pub struct Batcher {
    batch: LogsBatch,
    bytes: usize,
    needle: Option<String>,
    only: Vec<String>,
    hide: Vec<String>,
    sampler: Sampler,
    counts: HashMap<String, u64>,
}

impl Batcher {
    pub fn new(params: &LogsParams) -> Self {
        Self {
            batch: LogsBatch::default(),
            bytes: 0,
            needle: params
                .grep
                .as_deref()
                .map(|needle| needle.trim().to_lowercase())
                .filter(|needle| !needle.is_empty()),
            only: params.only.clone(),
            hide: params.hide.clone(),
            sampler: Sampler::new(Instant::now()),
            counts: HashMap::new(),
        }
    }

    /// Adds a line, `live` when it's new rather than one of the earlier lines
    /// a stream starts with; returns a batch that's full and should go now.
    pub fn push(&mut self, entry: LogEntry, live: bool) -> Option<LogsBatch> {
        self.push_at(entry, live, Instant::now())
    }

    fn push_at(&mut self, entry: LogEntry, live: bool, now: Instant) -> Option<LogsBatch> {
        if !self.wanted(&entry) {
            return None;
        }
        if live {
            *self
                .counts
                .entry(entry.source.clone().unwrap_or_default())
                .or_default() += 1;
            if !self.sampler.admit(now) {
                self.batch.skipped += 1;
                return None;
            }
        }
        self.bytes += entry.message.len() + 128;
        self.batch.entries.push(entry);
        (self.batch.entries.len() >= BATCH_LINES || self.bytes >= BATCH_BYTES).then(|| self.drain())
    }

    /// What's gathered, if there's anything.
    pub fn take(&mut self) -> Option<LogsBatch> {
        (!self.batch.entries.is_empty() || self.batch.skipped > 0 || !self.counts.is_empty())
            .then(|| self.drain())
    }

    fn wanted(&self, entry: &LogEntry) -> bool {
        if self
            .needle
            .as_ref()
            .is_some_and(|needle| !entry.message.to_lowercase().contains(needle))
        {
            return false;
        }
        let source = entry.source.as_deref().unwrap_or_default();
        (self.only.is_empty() || self.only.iter().any(|only| only == source))
            && !self.hide.iter().any(|hidden| hidden == source)
    }

    /// The batch so far, with the busiest sources' counts and the sampling.
    fn drain(&mut self) -> LogsBatch {
        self.bytes = 0;
        let mut busiest: Vec<_> = self.counts.drain().collect();
        busiest.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        busiest.truncate(COUNTED_SOURCES);
        LogsBatch {
            counts: busiest.into_iter().collect(),
            one_in: self.sampler.one_in(),
            ..std::mem::take(&mut self.batch)
        }
    }
}

/// Chooses which new lines go once more than [`RATE`] come a second: each
/// with the chance that would have let about [`RATE`] of the last second's
/// through, so what goes is an even sample of what came, as Datadog's and
/// Cloudflare's live tails do (angle 11 part 4). A sudden flood within one
/// second still stops at twice the rate.
struct Sampler {
    window: Instant,
    seen: u32,
    sent: u32,
    chance: f64,
    random: u64,
}

impl Sampler {
    fn new(now: Instant) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.subsec_nanos())
            .unwrap_or(1);
        Self {
            window: now,
            seen: 0,
            sent: 0,
            chance: 1.0,
            random: u64::from(seed) | 1,
        }
    }

    fn admit(&mut self, now: Instant) -> bool {
        if now.duration_since(self.window) >= Duration::from_secs(1) {
            self.chance = if self.seen > RATE {
                f64::from(RATE) / f64::from(self.seen)
            } else {
                1.0
            };
            self.window = now;
            self.seen = 0;
            self.sent = 0;
        }
        self.seen += 1;
        if self.sent >= 2 * RATE || (self.chance < 1.0 && self.next() >= self.chance) {
            return false;
        }
        self.sent += 1;
        true
    }

    /// About one in how many lines go, while sampling.
    fn one_in(&self) -> Option<u32> {
        // The chance is at least RATE over a second's lines, so this fits.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        (self.chance < 1.0).then(|| (1.0 / self.chance).round() as u32)
    }

    /// A number from 0 to 1 (xorshift64*), plenty for picking lines.
    fn next(&mut self) -> f64 {
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        let value = self.random.wrapping_mul(0x2545_F491_4F6C_DD1D);
        #[allow(clippy::cast_precision_loss)]
        let fraction = (value >> 11) as f64 / (1u64 << 53) as f64;
        fraction
    }
}

/// A ticker for [`FLUSH`] that doesn't catch up on ticks it missed.
pub(super) fn flush_timer() -> tokio::time::Interval {
    let mut flush = tokio::time::interval(FLUSH);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    flush
}

#[cfg(target_os = "linux")]
async fn read(id: &str, params: &LogsParams, privd: &Path, out: &Batches) -> String {
    use cntrl_host::journal;
    use tokio::io::{AsyncBufReadExt, BufReader};

    use super::uplink::now_ms;

    // The Docker socket is root's, so privd reads a container's log (D54).
    if params.container.is_some() {
        return through_privd(id, params, privd, out).await;
    }

    let started = now_ms();
    let mut child = match journal::command(params).spawn() {
        Ok(child) => child,
        Err(e) => return format!("can't run journalctl: {e}"),
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return "journalctl gave nothing to read".to_owned();
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut errors = BufReader::new(stderr).lines();
    let mut batcher = Batcher::new(params);
    let mut flush = flush_timer();
    let mut stderr_open = true;
    let closed = || "the subscription closed".to_owned();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    let Some(entry) = journal::parse(&line) else { continue };
                    let live = entry.ts >= started;
                    if let Some(full) = batcher.push(entry, live)
                        && out.send((id.to_owned(), full)).await.is_err()
                    {
                        return closed();
                    }
                }
                Ok(None) => break,
                Err(e) => return format!("can't read journalctl: {e}"),
            },
            line = errors.next_line(), if stderr_open => match line {
                // Without access to the system journal, journalctl warns
                // and goes on showing nothing, so stop with a reason.
                Ok(Some(line)) if line.contains("insufficient permissions") => {
                    return "the agent can't read the system journal; run the install command again, which gives it access".to_owned();
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => stderr_open = false,
            },
            _ = flush.tick() => {
                if let Some(batch) = batcher.take()
                    && out.send((id.to_owned(), batch)).await.is_err()
                {
                    return closed();
                }
            }
        }
    }
    if let Some(batch) = batcher.take() {
        let _ = out.send((id.to_owned(), batch)).await;
    }
    match child.wait().await {
        Ok(status) => format!("journalctl stopped ({status})"),
        Err(e) => format!("journalctl stopped: {e}"),
    }
}

/// On a Mac every log, and on Linux a container's (D54), comes through privd.
#[cfg(target_os = "macos")]
async fn read(id: &str, params: &LogsParams, privd: &Path, out: &Batches) -> String {
    through_privd(id, params, privd, out).await
}

/// privd reads the log as root and answers the one request with batches until
/// the agent hangs up, which is how the subscription closing stops it.
async fn through_privd(id: &str, params: &LogsParams, privd: &Path, out: &Batches) -> String {
    use super::ipc::{self, Call, Request, Response};

    let stream = match tokio::net::UnixStream::connect(privd).await {
        Ok(stream) => stream,
        Err(e) => return format!("can't reach the agent's root helper: {e}"),
    };
    let mut channel = ipc::channel(stream);
    let request = Request {
        id: 1,
        call: Call::LogStream {
            params: params.clone(),
        },
    };
    if let Err(e) = ipc::send(&mut channel, &request).await {
        return format!("can't ask the agent's root helper: {e}");
    }
    loop {
        let batch = match ipc::receive::<Response>(&mut channel).await {
            Ok(Some(Response {
                ok: Some(value), ..
            })) => match serde_json::from_value::<LogsBatch>(value) {
                Ok(batch) => batch,
                Err(e) => return format!("the root helper sent something unexpected: {e}"),
            },
            Ok(Some(Response { err: Some(e), .. })) => return e,
            Ok(Some(_)) => return "the root helper sent an empty answer".to_owned(),
            Ok(None) => return "the root helper stopped".to_owned(),
            Err(e) => return format!("lost the root helper: {e}"),
        };
        let ended = batch.ended.clone();
        if (!batch.entries.is_empty() || batch.skipped > 0)
            && out
                .send((
                    id.to_owned(),
                    LogsBatch {
                        ended: None,
                        ..batch
                    },
                ))
                .await
                .is_err()
        {
            return "the subscription closed".to_owned();
        }
        if let Some(reason) = ended {
            return reason;
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn read(_id: &str, _params: &LogsParams, _privd: &Path, _out: &Batches) -> String {
    "this agent can't read logs on this OS yet".to_owned()
}

/// privd's half on a Mac (angle 11 part 3): the unified log through `log`, and
/// a service's output files through `tail`, as one stream of batches.
#[cfg(target_os = "macos")]
pub mod mac {
    use std::collections::VecDeque;
    use std::path::Path;
    use std::process::Stdio;

    use cntrl_host::journal::{DEFAULT_LINES, MAX_LINES};
    use cntrl_host::oslog::{self, Job, SERVICE_WINDOW, SYSTEM_WINDOW};
    use cntrl_protocol::logs::{LogEntry, LogsBatch, LogsParams};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    use tokio::process::{Child, ChildStdout, Command};
    use tokio::sync::mpsc;
    use tokio::task::JoinSet;

    use super::super::uplink::now_ms;
    use super::{Batcher, flush_timer};

    /// What to read for a subscription.
    #[derive(Debug, Default)]
    pub struct Source {
        /// One launchd job's processes; the whole system's when absent.
        pub job: Option<Job>,
        /// The job's output files.
        pub files: Vec<String>,
        /// Who reads them: a user's own job's files are read as that user, so
        /// its plist can't point at a file only root may read. Root otherwise.
        pub files_as: Option<(u32, u32)>,
    }

    /// Reads until `log stream` stops or the caller drops this, which ends
    /// `log` and `tail`; returns why it stopped. The earlier lines come first:
    /// the unified log's from `log show`, then each file's, which carry no
    /// time.
    pub async fn read(
        params: &LogsParams,
        source: &Source,
        out: &mpsc::Sender<LogsBatch>,
    ) -> String {
        let closed = || "the subscription closed".to_owned();
        let wanted =
            usize::try_from(params.lines.unwrap_or(DEFAULT_LINES).min(MAX_LINES)).unwrap_or(0);
        let predicate = match &source.job {
            None => Some(oslog::predicate(params, None)),
            Some(job) if job.program.is_some() || job.pid.is_some() => {
                Some(oslog::predicate(params, Some(job)))
            }
            // Nothing to match its unified log lines by: its files only.
            Some(_) => None,
        };
        // A file's lines have no level, so a filter by priority leaves them out.
        let files: &[String] = if params.priority.is_some() {
            &[]
        } else {
            &source.files
        };
        if predicate.is_none() && files.is_empty() {
            return "the service has no log the agent can find".to_owned();
        }
        let mut batcher = Batcher::new(params);
        let send = |batch: Option<LogsBatch>| deliver(out, batch);

        // The stream starts first, so nothing logged while `log show` runs is
        // missed; its lines wait in the pipe meanwhile.
        let mut stream: Option<(Child, tokio::io::Lines<BufReader<ChildStdout>>)> = None;
        let mut seam = 0;
        if let Some(predicate) = &predicate {
            let mut child = match log(&oslog::stream_args(predicate), true).spawn() {
                Ok(child) => child,
                Err(e) => return format!("can't run log: {e}"),
            };
            let Some(stdout) = child.stdout.take() else {
                return "log gave nothing to read".to_owned();
            };
            let window = if source.job.is_some() {
                SERVICE_WINDOW
            } else {
                SYSTEM_WINDOW
            };
            let (earlier, newest) = match shown(predicate, window, wanted).await {
                Ok(shown) => shown,
                Err(e) => return e,
            };
            seam = newest;
            for entry in earlier {
                if !send(batcher.push(entry, false)).await {
                    return closed();
                }
            }
            stream = Some((child, BufReader::new(stdout).lines()));
        }

        let mut tails = JoinSet::new();
        let (file_out, mut file_lines) = mpsc::channel::<LogEntry>(256);
        for file in files {
            for entry in last_lines(file, wanted, source.files_as).await {
                if !send(batcher.push(entry, false)).await {
                    return closed();
                }
            }
            tails.spawn(follow(file.clone(), source.files_as, file_out.clone()));
        }
        drop(file_out);
        if !send(batcher.take()).await {
            return closed();
        }

        let mut flush = flush_timer();
        let mut files_open = !files.is_empty();
        loop {
            tokio::select! {
                line = next_line(&mut stream), if stream.is_some() => match line {
                    Ok(Some(line)) => {
                        let Some((micros, entry)) = oslog::parse(&line) else { continue };
                        // Lines `log show` already gave.
                        if micros <= seam {
                            continue;
                        }
                        seam = 0;
                        if !send(batcher.push(entry, true)).await {
                            return closed();
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return format!("can't read log: {e}"),
                },
                entry = file_lines.recv(), if files_open => match entry {
                    Some(entry) => {
                        if !send(batcher.push(entry, true)).await {
                            return closed();
                        }
                    }
                    None if stream.is_none() => return "the service's output files stopped being read".to_owned(),
                    None => files_open = false,
                },
                _ = flush.tick() => {
                    if !send(batcher.take()).await {
                        return closed();
                    }
                }
            }
        }
        send(batcher.take()).await;
        let Some((mut child, _)) = stream else {
            return "the log stopped".to_owned();
        };
        let mut problem = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = (&mut stderr).take(4_096).read_to_string(&mut problem).await;
        }
        let status = child
            .wait()
            .await
            .map(|status| status.to_string())
            .unwrap_or_default();
        match problem.lines().rfind(|line| !line.trim().is_empty()) {
            Some(line) => format!("log stopped: {}", line.trim()),
            None => format!("log stopped ({status})"),
        }
    }

    /// Sends a batch when there is one; false once nobody takes them.
    async fn deliver(out: &mpsc::Sender<LogsBatch>, batch: Option<LogsBatch>) -> bool {
        match batch {
            Some(batch) => out.send(batch).await.is_ok(),
            None => true,
        }
    }

    async fn next_line(
        stream: &mut Option<(Child, tokio::io::Lines<BufReader<ChildStdout>>)>,
    ) -> std::io::Result<Option<String>> {
        match stream {
            Some((_, lines)) => lines.next_line().await,
            None => std::future::pending().await,
        }
    }

    /// The newest `wanted` entries `log show` gives over `window`, oldest
    /// first, and the newest one's time in microseconds; read a line at a
    /// time, since the whole system's last minutes come to tens of megabytes.
    async fn shown(
        predicate: &str,
        window: &str,
        wanted: usize,
    ) -> Result<(VecDeque<LogEntry>, u64), String> {
        let mut earlier = VecDeque::with_capacity(wanted.min(1_024));
        let mut newest = 0;
        if wanted == 0 {
            return Ok((earlier, newest));
        }
        let mut child = log(&oslog::show_args(predicate, window), false)
            .spawn()
            .map_err(|e| format!("can't run log: {e}"))?;
        let stdout = child.stdout.take().ok_or("log gave nothing to read")?;
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| format!("can't read log: {e}"))?
        {
            let Some((micros, entry)) = oslog::parse(&line) else {
                continue;
            };
            newest = newest.max(micros);
            if earlier.len() == wanted {
                earlier.pop_front();
            }
            earlier.push_back(entry);
        }
        let _ = child.wait().await;
        Ok((earlier, newest))
    }

    /// A file's newest `wanted` lines, which carry no time. A line written
    /// between this and [`follow`] starting is missed.
    async fn last_lines(file: &str, wanted: usize, user: Option<(u32, u32)>) -> Vec<LogEntry> {
        if wanted == 0 {
            return Vec::new();
        }
        let mut command = tail(user);
        command.args(["-n", &wanted.to_string(), "--", file]);
        let Ok(output) = command.output().await else {
            return Vec::new();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line_of(file, line, 0))
            .collect()
    }

    /// Follows a file, even as it's rotated or replaced, sending its new lines
    /// with the time they're read.
    async fn follow(file: String, user: Option<(u32, u32)>, out: mpsc::Sender<LogEntry>) {
        let mut command = tail(user);
        command.args(["-n", "0", "-F", "--", &file]);
        let Ok(mut child) = command.spawn() else {
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            return;
        };
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if out.send(line_of(&file, &line, now_ms())).await.is_err() {
                return;
            }
        }
    }

    fn line_of(file: &str, line: &str, ts: u64) -> LogEntry {
        let name = Path::new(file)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        LogEntry {
            ts,
            priority: None,
            source: name,
            pid: None,
            message: cntrl_host::journal::cut(line.to_owned()),
        }
    }

    /// `log`, keeping what it says on stderr when `errors`, to say why it
    /// stopped.
    fn log(args: &[String], errors: bool) -> Command {
        let mut command = Command::new("/usr/bin/log");
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(if errors {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .kill_on_drop(true);
        command
    }

    /// `tail`, as `user` when given, which also drops root's other groups.
    fn tail(user: Option<(u32, u32)>) -> Command {
        let mut command = Command::new("/usr/bin/tail");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some((uid, gid)) = user {
            command.uid(uid).gid(gid);
        }
        command
    }
}

#[cfg(test)]
mod batching {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use cntrl_protocol::logs::{LogEntry, LogsBatch, LogsParams};

    use super::{Batcher, RATE};

    fn line(source: &str, i: usize) -> LogEntry {
        LogEntry {
            ts: 1,
            priority: None,
            source: Some(source.to_owned()),
            pid: None,
            message: format!("line {i}"),
        }
    }

    /// Everything a second's worth of lines at `at` sends, with the last batch.
    fn second(batcher: &mut Batcher, at: Instant, lines: usize) -> (usize, Vec<LogsBatch>) {
        let mut batches: Vec<LogsBatch> = (0..lines)
            .filter_map(|i| {
                batcher.push_at(line(if i % 5 == 0 { "quiet" } else { "loud" }, i), true, at)
            })
            .collect();
        batches.extend(batcher.take());
        (
            batches.iter().map(|batch| batch.entries.len()).sum(),
            batches,
        )
    }

    #[test]
    fn past_the_rate_an_even_sample_goes() {
        let mut batcher = Batcher::new(&LogsParams::default());
        let start = Instant::now();
        // A flood within one second stops at twice the rate.
        let (sent, batches) = second(&mut batcher, start, 5_000);
        assert_eq!(sent, 2 * RATE as usize);
        let counted: u64 = batches.iter().flat_map(|batch| batch.counts.values()).sum();
        assert_eq!(counted, 5_000, "every line is counted, sent or not");
        // The next second sends about RATE of the 5,000 it gets: one in ten.
        let (sent, batches) = second(&mut batcher, start + Duration::from_secs(1), 5_000);
        assert!((400..=600).contains(&sent), "sent {sent}");
        assert_eq!(batches.last().and_then(|batch| batch.one_in), Some(10));
        // A quiet second goes back to sending everything.
        let (sent, _) = second(&mut batcher, start + Duration::from_secs(2), 100);
        let (quiet, batches) = second(&mut batcher, start + Duration::from_secs(3), 100);
        assert!(sent <= 100 && quiet == 100);
        assert_eq!(batches.last().and_then(|batch| batch.one_in), None);
    }

    #[test]
    fn sources_can_be_hidden_or_picked_and_the_busiest_are_counted() {
        let hide = LogsParams {
            hide: vec!["loud".to_owned()],
            ..LogsParams::default()
        };
        let mut batcher = Batcher::new(&hide);
        for source in ["loud", "quiet", "loud"] {
            batcher.push(line(source, 0), true);
        }
        let batch = batcher.take().expect("a batch");
        assert_eq!(batch.entries.len(), 1);
        assert_eq!(batch.counts, BTreeMap::from([("quiet".to_owned(), 1)]));

        let only = LogsParams {
            only: vec!["loud".to_owned()],
            ..LogsParams::default()
        };
        let mut batcher = Batcher::new(&only);
        for source in ["loud", "quiet"] {
            batcher.push(line(source, 0), true);
        }
        assert_eq!(batcher.take().expect("a batch").entries.len(), 1);

        // 25 sources, the last the busiest: 20 are counted, busiest first.
        let mut batcher = Batcher::new(&LogsParams::default());
        for i in 0..25 {
            let lines = if i == 24 { 5 } else { 1 };
            for _ in 0..lines {
                batcher.push(line(&format!("source{i:02}"), i), true);
            }
        }
        let batch = batcher.take().expect("a batch");
        assert_eq!(batch.counts.len(), 20);
        assert_eq!(batch.counts.get("source24"), Some(&5));
        assert!(!batch.counts.contains_key("source23"));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::io::Write;
    use std::time::Duration;

    use cntrl_host::oslog::Job;
    use cntrl_protocol::logs::{LogsBatch, LogsParams};
    use tokio::sync::mpsc;

    use super::mac::{Source, read};

    /// Collects what the reader sends until `done` holds or time's up.
    async fn until(
        batches: &mut mpsc::Receiver<LogsBatch>,
        done: impl Fn(&[String]) -> bool,
    ) -> Vec<String> {
        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while !done(&seen) {
            match tokio::time::timeout_at(deadline, batches.recv()).await {
                Ok(Some(batch)) => {
                    seen.extend(
                        batch
                            .entries
                            .iter()
                            .map(|e| format!("{}|{}", e.ts == 0, e.message)),
                    );
                }
                _ => break,
            }
        }
        seen
    }

    #[tokio::test]
    async fn a_services_output_file_comes_with_its_newest_lines_first() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("service.log");
        std::fs::write(&path, "one\ntwo\nthree\n").expect("the file");
        let source = Source {
            // Nothing to match the unified log by: the file only.
            job: Some(Job::default()),
            files: vec![path.to_string_lossy().into_owned()],
            files_as: None,
        };
        let params = LogsParams {
            lines: Some(2),
            ..LogsParams::default()
        };
        let (out, mut batches) = mpsc::channel(16);
        let reader = tokio::spawn(async move { read(&params, &source, &out).await });
        // The newest two, with no time.
        let earlier = until(&mut batches, |seen| seen.len() >= 2).await;
        assert_eq!(earlier, ["true|two", "true|three"]);
        // A new line comes with the time it was read.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the file");
        writeln!(file, "four").expect("a line");
        let live = until(&mut batches, |seen| !seen.is_empty()).await;
        assert_eq!(live, ["false|four"]);
        reader.abort();
    }

    #[tokio::test]
    #[ignore = "reads this Mac's unified log, which takes an admin"]
    async fn the_system_log_brings_what_was_just_logged() {
        let token = format!("cntrl-test-{}", std::process::id());
        let params = LogsParams {
            grep: Some(token.clone()),
            lines: Some(10),
            ..LogsParams::default()
        };
        let (out, mut batches) = mpsc::channel(16);
        let reader = tokio::spawn(async move { read(&params, &Source::default(), &out).await });
        tokio::time::sleep(Duration::from_secs(3)).await;
        let status = std::process::Command::new("/usr/bin/logger")
            .arg(format!("hello from {token}"))
            .status()
            .expect("logger");
        assert!(status.success());
        let seen = until(&mut batches, |seen| {
            seen.iter().any(|line| line.contains("hello from"))
        })
        .await;
        assert!(
            seen.iter()
                .any(|line| line == &format!("false|hello from {token}")),
            "{seen:?}"
        );
        reader.abort();
    }
}
