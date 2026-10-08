//! Alert rules this device decides itself (D43; plans/alerts.md phase 2). The
//! hub sends the rules that cover it in an `alerts` frame. Once a minute the
//! agent judges each metric rule by the minute's average of its reading, so a
//! spike never counts; every 15 seconds it checks the services and containers
//! (D54) its rules name. A rule fires once its condition has held for its `minutes`, as
//! Prometheus's `for` does: a metric's for that many minutes in a row, a
//! service's at every check across them. It resolves after a short run back,
//! as Grafana's "keep firing for" does. Firing and resolving go out as `alert`
//! records through the outbox, which keeps them until the hub has them. The
//! rules and what's firing are saved in the state directory, so a restart
//! neither loses the rules nor leaves an incident open for good. While the
//! device is disabled in Console the rules rest (D87): nothing is judged,
//! what was firing is dropped without a word, since Console resolves its side,
//! and once it's back the rules judge afresh.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cntrl_protocol::alerts::{AlertMetric, AlertOp, AlertRule, AlertRuleKind};
use cntrl_protocol::containers::{Container, ContainerState};
use cntrl_protocol::frame::RecordKind;
use cntrl_protocol::records::{AlertRecord, AlertState};
use cntrl_protocol::service::{ServiceState, ServiceStatus};
use cntrl_protocol::stats::{SensorKind, StatsSample};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, Notify, watch};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::docker::Listing;
use super::ipc::{self, Call};
use super::outbox::Outbox;
use super::stats::Latest;
use super::uplink::now_ms;

pub const ALERTS_FILE: &str = "alerts.json";
/// How often services are checked; every fourth check closes a minute of
/// readings.
const CHECK: Duration = Duration::from_secs(15);
const CHECKS_PER_MINUTE: u32 = 4;
/// How long a check waits for the service list; a check that gets none
/// changes nothing, and the minute goes on.
const LIST_LIMIT: Duration = Duration::from_secs(10);
/// Minutes back before a rule resolves: a reading's under the line, a
/// service's running at every check.
const METRIC_CLEAR_MINUTES: u32 = 2;
const SERVICE_CLEAR_MINUTES: u32 = 1;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// What's saved: the rules, what each firing rule reported, and whether
/// they rest.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    rules: Vec<AlertRule>,
    firing: BTreeMap<String, Firing>,
    /// The device is disabled in Console (D87). Saved, so a restart doesn't
    /// judge before the gateway says otherwise.
    #[serde(default, skip_serializing_if = "is_false")]
    resting: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// The rules to judge: none while they rest.
fn awake(saved: &Saved) -> Vec<AlertRule> {
    if saved.resting {
        Vec::new()
    } else {
        saved.rules.clone()
    }
}

/// A rule that fired and hasn't resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Firing {
    rev: u64,
    since: u64,
    /// The furthest past the line so far; none for a service.
    peak: Option<f64>,
}

/// The rules, shared with the uplink, which replaces them when the hub sends
/// new ones.
pub struct Alerts {
    path: PathBuf,
    saved: Mutex<Saved>,
    changed: Notify,
}

impl Alerts {
    /// Opens what was saved in `state_dir`; a file that doesn't read starts
    /// empty until the hub sends the rules again.
    pub async fn open(state_dir: &Path) -> Self {
        let path = state_dir.join(ALERTS_FILE);
        let saved = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!(
                    "{} is unreadable, starting without alert rules: {e}",
                    path.display()
                );
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        Self {
            path,
            saved: Mutex::new(saved),
            changed: Notify::new(),
        }
    }

    /// Rests the rules while the device is disabled in Console, or wakes
    /// them (D87). Resting drops what was firing without a word: Console
    /// resolves its side.
    pub async fn rest(&self, resting: bool) {
        let mut saved = self.saved.lock().await;
        if saved.resting == resting {
            return;
        }
        saved.resting = resting;
        if resting {
            saved.firing.clear();
        }
        save(&self.path, &saved).await;
        drop(saved);
        self.changed.notify_one();
    }

    /// Whether the rules rest.
    #[cfg(test)]
    pub async fn resting(&self) -> bool {
        self.saved.lock().await.resting
    }

    /// The hub's rules for this device, replacing those before.
    pub async fn set(&self, rules: Vec<AlertRule>) {
        let mut saved = self.saved.lock().await;
        if saved.rules == rules {
            return;
        }
        saved.rules = rules;
        save(&self.path, &saved).await;
        drop(saved);
        self.changed.notify_one();
    }
}

/// Judges the rules until shutdown.
pub async fn run(
    alerts: Arc<Alerts>,
    latest: Arc<Latest>,
    outbox: Arc<Outbox>,
    privd: PathBuf,
    token: CancellationToken,
) -> Result<(), String> {
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + CHECK, CHECK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut checks: u32 = 0;
    let mut judges: HashMap<String, Judge> = HashMap::new();
    let mut minute = Minute::default();
    let mut minute_start = now_ms();
    let mut samples: Option<watch::Receiver<Option<Arc<StatsSample>>>> = None;
    // The rules as last judged, so a change can resolve what it ends.
    let (mut known, resting) = {
        let saved = alerts.saved.lock().await;
        (awake(&saved), saved.resting)
    };
    if resting {
        outbox.discard(RecordKind::Alert).await;
    }
    loop {
        // A receiver keeps the sampler reading while a rule needs readings.
        let wants_readings = known.iter().any(|rule| rule.kind == AlertRuleKind::Metric);
        if wants_readings && samples.is_none() {
            samples = Some(latest.subscribe());
        } else if !wants_readings {
            samples = None;
        }
        tokio::select! {
            _ = ticker.tick() => {
                checks = checks.wrapping_add(1);
                let minute_ends = checks.is_multiple_of(CHECKS_PER_MINUTE);
                let services = if known.iter().any(|rule| rule.kind == AlertRuleKind::Service) {
                    tokio::time::timeout(LIST_LIMIT, super::uplink::list_services()).await.ok().and_then(Result::ok)
                } else {
                    None
                };
                let containers = if known.iter().any(|rule| rule.kind == AlertRuleKind::Container) {
                    list_containers(&privd).await
                } else {
                    None
                };
                let now = now_ms();
                let records = {
                    let mut saved = alerts.saved.lock().await;
                    let mut records = Vec::new();
                    if let Some(services) = &services {
                        records.extend(check_services(&known, &mut judges, &mut saved.firing, services, now));
                    }
                    if let Some(containers) = &containers {
                        records.extend(check_containers(&known, &mut judges, &mut saved.firing, containers, now));
                    }
                    if minute_ends {
                        records.extend(judge_minute(&known, &mut judges, &mut saved.firing, &minute, minute_start, now));
                    }
                    if !records.is_empty() {
                        save(&alerts.path, &saved).await;
                    }
                    records
                };
                for record in records {
                    match serde_json::to_value(&record) {
                        Ok(data) => outbox.push(RecordKind::Alert, data).await,
                        Err(e) => warn!("an alert record didn't serialize: {e}"),
                    }
                }
                if minute_ends {
                    minute = Minute::default();
                    minute_start = now;
                }
            }
            Some(()) = next_sample(&mut samples) => {
                if let Some(sample) = samples.as_ref().and_then(|receiver| receiver.borrow().clone()) {
                    minute.add(&known, &sample, now_ms());
                }
            }
            () = alerts.changed.notified() => {
                let mut saved = alerts.saved.lock().await;
                let state = &mut *saved;
                // Resting, nothing resolves: what fired was dropped as the rules
                // went to rest.
                let records = if state.resting {
                    Vec::new()
                } else {
                    ended(&state.rules, &mut state.firing, now_ms())
                };
                if !records.is_empty() {
                    save(&alerts.path, &saved).await;
                }
                known = awake(&saved);
                let resting = saved.resting;
                drop(saved);
                // Alerts from before the rest don't go out once the device is back.
                if resting {
                    outbox.discard(RecordKind::Alert).await;
                }
                // A changed rule starts over, its minute too; a removed one is
                // forgotten.
                let unchanged = |id: &String, rev: u64| known.iter().any(|rule| &rule.id == id && rule.rev == rev);
                judges.retain(|id, judge| unchanged(id, judge.rev));
                minute.sums.retain(|id, sum| unchanged(id, sum.rev));
                for record in records {
                    if let Ok(data) = serde_json::to_value(&record) {
                        outbox.push(RecordKind::Alert, data).await;
                    }
                }
            }
            () = token.cancelled() => return Ok(()),
        }
    }
}

/// The next sample, once there's a receiver; never with none.
async fn next_sample(
    samples: &mut Option<watch::Receiver<Option<Arc<StatsSample>>>>,
) -> Option<()> {
    match samples {
        Some(receiver) => receiver.changed().await.ok(),
        None => std::future::pending().await,
    }
}

/// Where one rule stands between verdicts: a metric's come each minute, a
/// service's each check.
#[derive(Debug, Default)]
struct Judge {
    rev: u64,
    /// Verdicts in a row past the line, and when the first began.
    past: u32,
    since: Option<u64>,
    /// Verdicts in a row back, while firing.
    back: u32,
}

/// The readings one minute gathered, per rule, to average.
#[derive(Debug, Default)]
struct Minute {
    sums: HashMap<String, Sum>,
}

#[derive(Debug)]
struct Sum {
    /// The rule's revision the readings were for.
    rev: u64,
    total: f64,
    count: u32,
    /// When the first reading came, which a rule that arrived mid-minute
    /// starts at.
    first: u64,
}

impl Minute {
    fn add(&mut self, rules: &[AlertRule], sample: &StatsSample, now: u64) {
        for rule in rules {
            let Some(metric) = rule.metric else { continue };
            if let Some(value) = reading(metric, rule.target.as_deref(), sample) {
                let sum = self.sums.entry(rule.id.clone()).or_insert(Sum {
                    rev: rule.rev,
                    total: 0.0,
                    count: 0,
                    first: now,
                });
                sum.total += value;
                sum.count += 1;
            }
        }
    }

    /// The rule's average this minute, and when its readings began.
    fn average(&self, rule: &str) -> Option<(f64, u64)> {
        self.sums
            .get(rule)
            .filter(|sum| sum.count > 0)
            .map(|sum| (sum.total / f64::from(sum.count), sum.first))
    }
}

/// Judges every metric rule on the minute that just ended, and says what
/// fired or resolved.
fn judge_minute(
    rules: &[AlertRule],
    judges: &mut HashMap<String, Judge>,
    firing: &mut BTreeMap<String, Firing>,
    minute: &Minute,
    minute_start: u64,
    now: u64,
) -> Vec<AlertRecord> {
    let mut records = Vec::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.kind == AlertRuleKind::Metric)
    {
        let (Some(op), Some(threshold)) = (rule.op, rule.threshold) else {
            continue;
        };
        let average = minute.average(&rule.id);
        let past = average.map(|(value, _)| beyond(op, value, threshold));
        let began = average.map_or(minute_start, |(_, first)| first);
        let judge = judge_of(judges, rule);
        if let Some(record) = step(
            rule,
            judge,
            firing,
            past,
            average.map(|(value, _)| value),
            began,
            now,
        ) {
            records.push(record);
        }
    }
    records
}

/// Checks every service rule against the services as they are now, and says
/// what fired or resolved.
fn check_services(
    rules: &[AlertRule],
    judges: &mut HashMap<String, Judge>,
    firing: &mut BTreeMap<String, Firing>,
    services: &[ServiceStatus],
    now: u64,
) -> Vec<AlertRecord> {
    let mut records = Vec::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.kind == AlertRuleKind::Service)
    {
        let Some(target) = rule.target.as_deref() else {
            continue;
        };
        let past = !running(services, target);
        let judge = judge_of(judges, rule);
        if let Some(record) = step(rule, judge, firing, Some(past), None, now, now) {
            records.push(record);
        }
    }
    records
}

/// Checks every container rule against the containers as they are now, and
/// says what fired or resolved (D54).
fn check_containers(
    rules: &[AlertRule],
    judges: &mut HashMap<String, Judge>,
    firing: &mut BTreeMap<String, Firing>,
    containers: &[Container],
    now: u64,
) -> Vec<AlertRecord> {
    let mut records = Vec::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.kind == AlertRuleKind::Container)
    {
        let Some(target) = rule.target.as_deref() else {
            continue;
        };
        let past = !container_running(containers, target);
        let judge = judge_of(judges, rule);
        if let Some(record) = step(rule, judge, firing, Some(past), None, now, now) {
            records.push(record);
        }
    }
    records
}

/// The containers, as privd lists them; none when there's no engine to ask or
/// it didn't answer, which judges nothing.
async fn list_containers(privd: &Path) -> Option<Vec<Container>> {
    let value = ipc::call_within(privd, Call::Containers { counters: false }, LIST_LIMIT)
        .await
        .ok()?;
    let listing: Listing = serde_json::from_value(value).ok()?;
    if listing.engine.is_none() || listing.note.is_some() {
        return None;
    }
    Some(
        listing
            .containers
            .into_iter()
            .map(|listed| listed.container)
            .collect(),
    )
}

/// Whether a container runs: one named so, or any of a Compose service's
/// replicas. One gone counts as down.
fn container_running(containers: &[Container], target: &str) -> bool {
    containers.iter().any(|container| {
        (container.name == target || container.service.as_deref() == Some(target))
            && container.state == ContainerState::Running
    })
}

fn judge_of<'a>(judges: &'a mut HashMap<String, Judge>, rule: &AlertRule) -> &'a mut Judge {
    judges.entry(rule.id.clone()).or_insert_with(|| Judge {
        rev: rule.rev,
        ..Judge::default()
    })
}

/// Verdicts in a row a rule needs past its line to fire, and back to resolve.
/// A metric's verdicts are minutes. A service's are checks, and a run of them
/// spans a minute only from its first to its fifth.
fn needs(rule: &AlertRule) -> (u32, u32) {
    match rule.kind {
        AlertRuleKind::Service | AlertRuleKind::Container => (
            rule.minutes
                .max(1)
                .saturating_mul(CHECKS_PER_MINUTE)
                .saturating_add(1),
            SERVICE_CLEAR_MINUTES * CHECKS_PER_MINUTE + 1,
        ),
        _ => (rule.minutes.max(1), METRIC_CLEAR_MINUTES),
    }
}

/// One verdict on one rule: whether it was past the line, its reading, and
/// when what it judged began. A verdict with nothing to judge by changes
/// nothing.
fn step(
    rule: &AlertRule,
    judge: &mut Judge,
    firing: &mut BTreeMap<String, Firing>,
    past: Option<bool>,
    value: Option<f64>,
    began: u64,
    now: u64,
) -> Option<AlertRecord> {
    let past = past?;
    let (fire_after, clear_after) = needs(rule);
    if past {
        if judge.past == 0 {
            judge.since = Some(began);
        }
        judge.past += 1;
        judge.back = 0;
        if let Some(fired) = firing.get_mut(&rule.id) {
            fired.peak = worse(rule.op, fired.peak, value);
            return None;
        }
        if judge.past < fire_after {
            return None;
        }
        let since = judge.since.unwrap_or(began);
        firing.insert(
            rule.id.clone(),
            Firing {
                rev: rule.rev,
                since,
                peak: value,
            },
        );
        return Some(AlertRecord {
            rule: rule.id.clone(),
            rev: rule.rev,
            state: AlertState::Firing,
            since,
            at: now,
            value,
            peak: None,
        });
    }
    judge.past = 0;
    judge.since = None;
    let fired = firing.get(&rule.id)?;
    judge.back += 1;
    if judge.back < clear_after {
        return None;
    }
    let record = AlertRecord {
        rule: rule.id.clone(),
        rev: fired.rev,
        state: AlertState::Resolved,
        since: fired.since,
        at: now,
        value,
        peak: fired.peak,
    };
    firing.remove(&rule.id);
    judge.back = 0;
    Some(record)
}

/// Resolves what fired under a rule the hub no longer sends, or sends changed.
fn ended(rules: &[AlertRule], firing: &mut BTreeMap<String, Firing>, now: u64) -> Vec<AlertRecord> {
    let gone: Vec<String> = firing
        .iter()
        .filter(|(id, fired)| {
            !rules
                .iter()
                .any(|rule| &rule.id == *id && rule.rev == fired.rev)
        })
        .map(|(id, _)| id.clone())
        .collect();
    gone.into_iter()
        .filter_map(|id| {
            let fired = firing.remove(&id)?;
            Some(AlertRecord {
                rule: id,
                rev: fired.rev,
                state: AlertState::Resolved,
                since: fired.since,
                at: now,
                value: None,
                peak: fired.peak,
            })
        })
        .collect()
}

fn beyond(op: AlertOp, value: f64, threshold: f64) -> bool {
    match op {
        AlertOp::Above => value > threshold,
        AlertOp::Below => value < threshold,
    }
}

/// The further past the line of two readings: the higher above it, the lower
/// below it.
fn worse(op: Option<AlertOp>, peak: Option<f64>, value: Option<f64>) -> Option<f64> {
    match (peak, value) {
        (Some(peak), Some(value)) => Some(match op {
            Some(AlertOp::Below) => peak.min(value),
            _ => peak.max(value),
        }),
        (peak, value) => peak.or(value),
    }
}

/// Whether a service runs: found by its name, with or without systemd's
/// `.service`, and running, or finished cleanly as a oneshot unit or an
/// on-demand job does. One still starting counts as down, so a start that
/// hangs, or a crash loop waiting to restart, alerts once it lasts the rule's
/// minutes; an ordinary start is over long before.
fn running(services: &[ServiceStatus], target: &str) -> bool {
    services.iter().any(|service| {
        (service.unit == target || service.unit.strip_suffix(".service") == Some(target))
            && matches!(service.state, ServiceState::Running | ServiceState::Exited)
    })
}

/// A sample's reading for a metric, in the unit its rules use.
#[allow(clippy::cast_precision_loss)]
fn reading(metric: AlertMetric, target: Option<&str>, sample: &StatsSample) -> Option<f64> {
    let percent = |part: u64, whole: u64| (whole > 0).then(|| part as f64 / whole as f64 * 100.0);
    let mounts = || {
        sample
            .filesystems
            .iter()
            .filter(move |fs| fs.total > 0 && target.is_none_or(|mount| fs.mount == mount))
    };
    let max = |values: &mut dyn Iterator<Item = f64>| {
        values.fold(None, |most: Option<f64>, value| {
            Some(most.map_or(value, |most| most.max(value)))
        })
    };
    match metric {
        AlertMetric::Cpu => Some(sample.cpu.busy * 100.0),
        AlertMetric::Memory => percent(
            sample.memory.total.saturating_sub(sample.memory.available),
            sample.memory.total,
        ),
        AlertMetric::Swap => sample
            .swap
            .as_ref()
            .and_then(|swap| percent(swap.used, swap.total)),
        AlertMetric::DiskUsed => max(&mut mounts().filter_map(|fs| percent(fs.used, fs.total))),
        // The fullest disk is the one to worry about.
        AlertMetric::DiskFree => mounts()
            .map(|fs| fs.available as f64 / GIB)
            .fold(None, |least: Option<f64>, value| {
                Some(least.map_or(value, |least| least.min(value)))
            }),
        AlertMetric::Load => Some(sample.cpu.load.one),
        AlertMetric::Temperature => {
            let kind = match target {
                Some("cpu") => Some(SensorKind::Cpu),
                Some("gpu") => Some(SensorKind::Gpu),
                Some("disk") => Some(SensorKind::Disk),
                _ => None,
            };
            max(&mut sample
                .temperatures
                .iter()
                .filter(|sensor| kind.is_none_or(|kind| sensor.sensor == kind))
                .map(|sensor| sensor.celsius))
        }
        AlertMetric::GpuBusy => {
            max(&mut gpus(sample, target).filter_map(|gpu| gpu.busy.map(|busy| busy * 100.0)))
        }
        AlertMetric::GpuMemory => max(&mut gpus(sample, target)
            .filter_map(|gpu| percent(gpu.memory_used?, gpu.memory_total?))),
        AlertMetric::GpuTemperature => {
            max(&mut gpus(sample, target).filter_map(|gpu| gpu.temperature))
        }
        AlertMetric::Unknown => None,
    }
}

fn gpus<'a>(
    sample: &'a StatsSample,
    target: Option<&'a str>,
) -> impl Iterator<Item = &'a cntrl_protocol::stats::GpuStats> + 'a {
    sample
        .gpus
        .iter()
        .filter(move |gpu| target.is_none_or(|name| gpu.name.contains(name)))
}

/// Writes the saved state: a temporary file renamed into place.
async fn save(path: &Path, saved: &Saved) {
    let result = async {
        let bytes = serde_json::to_vec(saved).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        let mut file = tokio::fs::File::create(&temporary).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        tokio::fs::rename(&temporary, path).await
    };
    if let Err(e) = result.await {
        warn!("can't save the alert rules to {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use cntrl_protocol::service::{ServiceKind, ServiceScope};
    use cntrl_protocol::stats::{CpuStats, Filesystem, LoadAverage, MemoryStats, Temperature};

    use super::*;

    fn service(unit: &str, state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            unit: unit.to_owned(),
            description: None,
            state,
            detail: None,
            pid: None,
            protected: false,
            scope: ServiceScope::System,
            user: None,
            kind: ServiceKind::Service,
            enabled: None,
            vendor: false,
        }
    }

    #[tokio::test]
    async fn resting_drops_what_fired_and_holds_after_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let alerts = Alerts::open(dir.path()).await;
        alerts.set(vec![cpu_rule(1)]).await;
        alerts.saved.lock().await.firing.insert(
            "alr_cpu".to_owned(),
            Firing {
                rev: 1,
                since: 1,
                peak: Some(0.9),
            },
        );
        alerts.rest(true).await;
        {
            let saved = alerts.saved.lock().await;
            assert!(saved.firing.is_empty(), "resting drops what fired");
            assert!(awake(&saved).is_empty(), "resting judges nothing");
            assert_eq!(saved.rules.len(), 1, "the rules stay for when it's back");
        }
        drop(alerts);
        let reopened = Alerts::open(dir.path()).await;
        assert!(reopened.saved.lock().await.resting, "a restart still rests");
        reopened.rest(false).await;
        assert_eq!(awake(&*reopened.saved.lock().await).len(), 1);
        drop(reopened);
        let woken = Alerts::open(dir.path()).await;
        assert!(!woken.saved.lock().await.resting);
    }

    #[tokio::test]
    async fn resting_rules_keep_old_alerts_from_going_out() {
        let dir = tempfile::tempdir().expect("temp dir");
        let outbox = Arc::new(Outbox::open(dir.path()).await);
        outbox
            .push(RecordKind::Alert, serde_json::json!({ "rule": "alr_cpu" }))
            .await;
        let alerts = Arc::new(Alerts::open(dir.path()).await);
        alerts.set(vec![cpu_rule(1)]).await;
        let latest = Arc::new(Latest::new(None));
        let token = CancellationToken::new();
        let judging = tokio::spawn(run(
            Arc::clone(&alerts),
            latest,
            Arc::clone(&outbox),
            dir.path().join("privd.sock"),
            token.clone(),
        ));
        alerts.rest(true).await;
        for _ in 0..50 {
            if outbox.pending(10).await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            outbox.pending(10).await.is_empty(),
            "the alert from before the rest is dropped"
        );
        token.cancel();
        judging.await.expect("joined").expect("ran");
    }

    fn cpu_rule(minutes: u32) -> AlertRule {
        AlertRule {
            id: "alr_cpu".to_owned(),
            rev: 1,
            kind: AlertRuleKind::Metric,
            minutes,
            metric: Some(AlertMetric::Cpu),
            target: None,
            op: Some(AlertOp::Above),
            threshold: Some(90.0),
        }
    }

    /// Runs a rule through minutes of readings, returning what each said.
    fn minutes(
        rule: &AlertRule,
        values: &[Option<f64>],
    ) -> (Vec<Option<AlertState>>, BTreeMap<String, Firing>) {
        let mut judge = Judge::default();
        let mut firing = BTreeMap::new();
        let states = values
            .iter()
            .enumerate()
            .map(|(i, value)| {
                let past = value.map(|value| beyond(AlertOp::Above, value, 90.0));
                let at = (i as u64) * 60_000;
                step(rule, &mut judge, &mut firing, past, *value, at, at + 60_000)
                    .map(|record| record.state)
            })
            .collect();
        (states, firing)
    }

    #[test]
    fn fires_on_the_last_minute_in_a_row_and_resolves_after_two_back() {
        let rule = cpu_rule(3);
        let (states, firing) = minutes(
            &rule,
            &[
                Some(95.0),
                Some(97.0),
                Some(93.0),
                Some(99.0),
                Some(50.0),
                Some(40.0),
            ],
        );
        assert_eq!(
            states,
            [
                None,
                None,
                Some(AlertState::Firing),
                None,
                None,
                Some(AlertState::Resolved)
            ]
        );
        assert!(firing.is_empty());
    }

    #[test]
    fn a_dip_starts_over_and_a_gap_changes_nothing() {
        let rule = cpu_rule(3);
        // Two minutes past, a dip, then a minute without readings and two past:
        // only three in a row of judged minutes fire it.
        let (states, _) = minutes(
            &rule,
            &[
                Some(95.0),
                Some(95.0),
                Some(80.0),
                None,
                Some(95.0),
                Some(95.0),
                Some(95.0),
            ],
        );
        assert_eq!(
            states,
            [None, None, None, None, None, None, Some(AlertState::Firing)]
        );
    }

    #[test]
    fn resolving_reports_the_peak_and_when_it_began() {
        let rule = cpu_rule(1);
        let mut judge = Judge::default();
        let mut firing = BTreeMap::new();
        let mut run = |value: f64, minute: u64| {
            step(
                &rule,
                &mut judge,
                &mut firing,
                Some(value > 90.0),
                Some(value),
                minute * 60_000,
                (minute + 1) * 60_000,
            )
        };
        let fired = run(94.0, 10).expect("fired");
        assert_eq!((fired.since, fired.value), (600_000, Some(94.0)));
        assert!(run(99.5, 11).is_none());
        assert!(run(60.0, 12).is_none());
        let resolved = run(55.0, 13).expect("resolved");
        assert_eq!(resolved.state, AlertState::Resolved);
        assert_eq!((resolved.since, resolved.peak), (600_000, Some(99.5)));
    }

    #[test]
    fn a_changed_or_removed_rule_resolves_what_it_had_firing() {
        let mut firing = BTreeMap::new();
        firing.insert(
            "alr_cpu".to_owned(),
            Firing {
                rev: 1,
                since: 5,
                peak: Some(97.0),
            },
        );
        // Unchanged: nothing ends.
        assert!(ended(&[cpu_rule(5)], &mut firing, 9).is_empty());
        let changed = AlertRule {
            rev: 2,
            ..cpu_rule(5)
        };
        let records = ended(&[changed], &mut firing, 9);
        assert_eq!(records.len(), 1);
        assert_eq!(
            (records[0].state, records[0].rev, records[0].peak),
            (AlertState::Resolved, 1, Some(97.0))
        );
        assert!(firing.is_empty());
    }

    #[test]
    fn a_service_runs_by_its_name_with_or_without_the_suffix() {
        let services = [
            service("nginx.service", ServiceState::Running),
            service("cron.service", ServiceState::Failed),
            service("ufw.service", ServiceState::Exited),
            service("api.service", ServiceState::Starting),
        ];
        assert!(running(&services, "nginx"));
        assert!(running(&services, "nginx.service"));
        assert!(!running(&services, "cron"));
        assert!(!running(&services, "postgresql"));
        // A oneshot that finished runs; a start that hasn't finished doesn't.
        assert!(running(&services, "ufw"));
        assert!(!running(&services, "api"));
    }

    #[test]
    fn a_service_fires_once_down_at_every_check_across_its_minutes() {
        let rule = AlertRule {
            id: "alr_web".to_owned(),
            rev: 1,
            kind: AlertRuleKind::Service,
            minutes: 1,
            metric: None,
            target: Some("nginx".to_owned()),
            op: None,
            threshold: None,
        };
        let up = [service("nginx.service", ServiceState::Running)];
        let down = [service("nginx.service", ServiceState::Failed)];
        let mut judges = HashMap::new();
        let mut firing = BTreeMap::new();
        let mut check = |services: &[ServiceStatus], second: u64| {
            check_services(
                std::slice::from_ref(&rule),
                &mut judges,
                &mut firing,
                services,
                second * 1000,
            )
            .pop()
            .map(|record| (record.state, record.since))
        };
        // Down from 0 s to 45 s isn't a minute yet, and running at 60 s
        // starts it over.
        for second in [0, 15, 30, 45] {
            assert_eq!(check(&down, second), None);
        }
        assert_eq!(check(&up, 60), None);
        // Down from 75 s: the check at 135 s makes a minute, from the first.
        for second in [75, 90, 105, 120] {
            assert_eq!(check(&down, second), None);
        }
        assert_eq!(check(&down, 135), Some((AlertState::Firing, 75_000)));
        // Back, then down again within the minute: still firing.
        for second in [150, 165, 180] {
            assert_eq!(check(&up, second), None);
        }
        assert_eq!(check(&down, 195), None);
        // A whole minute running resolves it.
        for second in [210, 225, 240, 255] {
            assert_eq!(check(&up, second), None);
        }
        assert_eq!(check(&up, 270), Some((AlertState::Resolved, 75_000)));
    }

    fn replica(name: &str, service: Option<&str>, state: ContainerState) -> Container {
        Container {
            id: name.to_owned(),
            name: name.to_owned(),
            image: "nginx".to_owned(),
            project: service.map(|_| "app".to_owned()),
            service: service.map(str::to_owned),
            state,
            health: None,
            status: String::new(),
            created: 0,
            ports: Vec::new(),
            cpu: None,
            memory: None,
            memory_limit: None,
            network: None,
        }
    }

    #[test]
    fn a_container_rule_goes_by_name_or_compose_service_and_fires_like_a_service() {
        let running = [
            replica("app-web-1", Some("web"), ContainerState::Running),
            replica("app-web-2", Some("web"), ContainerState::Exited),
            replica("cache", None, ContainerState::Running),
        ];
        assert!(container_running(&running, "web"));
        assert!(container_running(&running, "cache"));
        assert!(container_running(&running, "app-web-1"));
        assert!(!container_running(&running, "app-web-2"));
        // Gone counts as down.
        assert!(!container_running(&running, "db"));

        let rule = AlertRule {
            id: "alr_web".to_owned(),
            rev: 1,
            kind: AlertRuleKind::Container,
            minutes: 1,
            metric: None,
            target: Some("web".to_owned()),
            op: None,
            threshold: None,
        };
        let down = [replica(
            "app-web-1",
            Some("web"),
            ContainerState::Restarting,
        )];
        let mut judges = HashMap::new();
        let mut firing = BTreeMap::new();
        let mut check = |containers: &[Container], second: u64| {
            check_containers(
                std::slice::from_ref(&rule),
                &mut judges,
                &mut firing,
                containers,
                second * 1000,
            )
            .pop()
            .map(|record| (record.state, record.since))
        };
        for second in [0, 15, 30, 45] {
            assert_eq!(check(&down, second), None);
        }
        assert_eq!(check(&down, 60), Some((AlertState::Firing, 0)));
        for second in [75, 90, 105, 120] {
            assert_eq!(check(&running, second), None);
        }
        assert_eq!(check(&running, 135), Some((AlertState::Resolved, 0)));
    }

    #[test]
    fn reads_each_metric_in_its_unit() {
        let sample = StatsSample {
            ts: 0,
            cpu: CpuStats {
                busy: 0.42,
                load: LoadAverage {
                    one: 1.5,
                    five: 1.0,
                    fifteen: 0.5,
                },
            },
            memory: MemoryStats {
                total: 1000,
                available: 250,
            },
            swap: None,
            disk_io: None,
            network: None,
            filesystems: vec![
                Filesystem {
                    mount: "/".to_owned(),
                    name: None,
                    kind: "ext4".to_owned(),
                    total: 100 * GIB as u64,
                    used: 90 * GIB as u64,
                    available: 10 * GIB as u64,
                },
                Filesystem {
                    mount: "/data".to_owned(),
                    name: None,
                    kind: "ext4".to_owned(),
                    total: 100 * GIB as u64,
                    used: 20 * GIB as u64,
                    available: 80 * GIB as u64,
                },
            ],
            temperatures: vec![
                Temperature {
                    sensor: SensorKind::Cpu,
                    label: "Package".to_owned(),
                    celsius: 71.0,
                },
                Temperature {
                    sensor: SensorKind::Disk,
                    label: "nvme".to_owned(),
                    celsius: 45.0,
                },
            ],
            gpus: vec![],
        };
        assert_eq!(reading(AlertMetric::Cpu, None, &sample), Some(42.0));
        assert_eq!(reading(AlertMetric::Memory, None, &sample), Some(75.0));
        assert_eq!(reading(AlertMetric::Swap, None, &sample), None);
        assert_eq!(reading(AlertMetric::DiskUsed, None, &sample), Some(90.0));
        assert_eq!(
            reading(AlertMetric::DiskUsed, Some("/data"), &sample),
            Some(20.0)
        );
        assert_eq!(reading(AlertMetric::DiskFree, None, &sample), Some(10.0));
        assert_eq!(reading(AlertMetric::Load, None, &sample), Some(1.5));
        assert_eq!(reading(AlertMetric::Temperature, None, &sample), Some(71.0));
        assert_eq!(
            reading(AlertMetric::Temperature, Some("disk"), &sample),
            Some(45.0)
        );
        assert_eq!(reading(AlertMetric::GpuBusy, None, &sample), None);
    }
}
