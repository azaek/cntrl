//! Follow-ups (D119, D121): one-use plans the machine carries out when a job is
//! done. Console sets one up with `followup.start`; this task watches the job,
//! judges each minute whether it's busy, and once the job exits or has been
//! quiet long enough, carries out the steps in order. A step that acts counts
//! down, or asks first and waits for an answer; each one runs through privd,
//! which checks the policy again and audits it. Everything that happens goes
//! out as a `followup` record through the outbox, so a report made offline
//! reaches Console once the link is back, and the `followups` topic says where
//! each follow-up is. Follow-ups are saved in the state directory with the boot
//! they began in; one from an earlier boot ends without running anything.
//!
//! The judging is written as pure functions over a saved follow-up (`judge_*`,
//! `advance`), so the rules are tested without a clock or a machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cntrl_protocol::ErrorCode;
use cntrl_protocol::containers::{Container, ContainerState, ContainersSample};
use cntrl_protocol::followup::{
    self, AtDeadline, DoneWhy, EndedWhy, Event, Finish, FollowUpRecord, FollowUpState,
    FollowUpsSample, Minute, Outcome, Plan, Scope, SkippedWhy, Stage, Started, StepAction,
    StopOrRestart, Watch,
};
use cntrl_protocol::frame::{Actor, RecordKind};
use cntrl_protocol::process::{ProcessInfo, StopResult};
use cntrl_protocol::service::ServiceAction;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, Notify, watch};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::docker;
use super::host;
use super::ipc::{self, Call, CallError};
use super::outbox::Outbox;
use super::policy::PolicyState;
use super::uplink::now_ms;
use super::{containers, processes};

pub const FOLLOWUPS_FILE: &str = "followups.json";
/// How often follow-ups are looked at; every fourth look closes a minute.
const TICK: Duration = Duration::from_secs(15);
const TICKS_PER_MINUTE: u32 = 4;
/// How long a step's call to privd may take: a service's job can take minutes.
const STEP_LIMIT: Duration = Duration::from_secs(300);
/// How long a stop waits for the job's processes to close before forcing them.
const FORCE_AFTER: Duration = Duration::from_secs(30);
/// How long reading the process table or the containers may take at setup.
const READ_LIMIT: Duration = Duration::from_secs(10);
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;

/// What's saved: the follow-ups, and the boot they were saved in.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    boot_id: String,
    #[serde(default)]
    followups: Vec<Kept>,
}

/// One follow-up, as the machine keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Kept {
    plan: Plan,
    /// Who set it up: privd audits each step as theirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<Actor>,
    since: u64,
    scope: Scope,
    phase: Phase,
    #[serde(default)]
    quiet_minutes: u32,
    /// The last whole minute's readings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    minute: Option<Minute>,
}

/// Where a follow-up is in its life.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Phase {
    /// Watching the job.
    Waiting,
    /// The job is done; the steps run from `next`.
    Stepping { next: u32, pending: Pending },
    /// Over: it goes from the list once its last record is out.
    Over,
}

/// What the step at `next` is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Pending {
    /// Nothing yet: it starts its countdown, or asks.
    Fresh,
    Asking {
        until: u64,
    },
    CountingDown {
        until: u64,
    },
    /// Someone approved it, or its countdown ended: it runs now.
    Due,
    /// Someone skipped it.
    Skipped,
}

/// What a follow-up needs done, decided under the lock and done after it.
#[derive(Debug, Clone, PartialEq)]
enum Effect {
    Record(Event),
    /// Run the step; its outcome comes back through [`finish_step`].
    Run(u32),
}

/// The CPU and disk of one minute so far.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Sums {
    cpu: f64,
    disk: Option<f64>,
    samples: u32,
}

impl Sums {
    fn add(&mut self, cpu: f64, disk: Option<f64>) {
        self.cpu += cpu;
        if let Some(disk) = disk {
            *self.disk.get_or_insert(0.0) += disk;
        }
        self.samples += 1;
    }

    /// The minute's averages; none without a sample.
    fn minute(&self, finish: &Finish) -> Option<Minute> {
        if self.samples == 0 {
            return None;
        }
        let samples = f64::from(self.samples);
        let cpu = self.cpu / samples;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let disk = self.disk.map(|sum| (sum / samples).round().max(0.0) as u64);
        let busy = match finish {
            Finish::Quiet {
                cpu: line,
                disk: disk_line,
                ..
            } => cpu > *line || disk_line.zip(disk).is_some_and(|(line, disk)| disk > line),
            // Watching only for an exit, every minute counts as busy.
            Finish::Exits | Finish::Unknown => true,
        };
        Some(Minute { cpu, disk, busy })
    }
}

/// The follow-ups, shared with the uplink, which sets them up and answers them.
pub struct FollowUps {
    path: PathBuf,
    saved: Mutex<Saved>,
    changed: Notify,
    outbox: Arc<Outbox>,
    privd: PathBuf,
    latest: watch::Sender<Option<Arc<FollowUpsSample>>>,
}

impl FollowUps {
    /// Opens what was saved in `state_dir`; one that doesn't read starts empty.
    pub async fn open(state_dir: &Path, outbox: Arc<Outbox>, privd: PathBuf) -> Self {
        let path = state_dir.join(FOLLOWUPS_FILE);
        let saved = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!(
                    "{} is unreadable, starting without follow-ups: {e}",
                    path.display()
                );
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        let followups = Self {
            path,
            saved: Mutex::new(saved),
            changed: Notify::new(),
            outbox,
            privd,
            latest: watch::Sender::new(None),
        };
        followups.publish(&*followups.saved.lock().await);
        followups
    }

    /// The `followups` topic's latest sample, kept current by the task.
    pub fn subscribe(&self) -> watch::Receiver<Option<Arc<FollowUpsSample>>> {
        self.latest.subscribe()
    }

    /// Sets up a follow-up (`followup.start`). The policy must allow every
    /// step now, and still will have to when each runs; the job must be one
    /// the machine can watch. The same ID again answers with the follow-up
    /// already set up.
    pub async fn start(
        &self,
        plan: Plan,
        actor: Option<Actor>,
        policy: &PolicyState,
    ) -> Result<Started, CallError> {
        check_plan(&plan, policy, now_ms())?;
        {
            let saved = self.saved.lock().await;
            if let Some(kept) = saved.followups.iter().find(|kept| kept.plan.id == plan.id) {
                if kept.plan == plan {
                    return Ok(Started {
                        id: plan.id,
                        scope: kept.scope.clone(),
                    });
                }
                return Err(bad(
                    "a follow-up with this ID is set up already, with another plan",
                ));
            }
            let live = saved
                .followups
                .iter()
                .filter(|kept| kept.phase != Phase::Over)
                .count();
            if live >= followup::MAX_FOLLOWUPS {
                let most = followup::MAX_FOLLOWUPS;
                return Err(bad(format!(
                    "this machine keeps at most {most} follow-ups at once"
                )));
            }
        }
        let scope = match &plan.watch {
            Watch::App { .. } | Watch::Process { .. } => {
                let table =
                    tokio::time::timeout(READ_LIMIT, processes::read_once(self.privd.clone()))
                        .await
                        .map_err(|_| {
                            CallError::new(
                                ErrorCode::Timeout,
                                "reading the process table took too long",
                            )
                        })?
                        .map_err(CallError::internal)?;
                scope_of(&plan.watch, &table, std::process::id())?
            }
            Watch::Container { id } => {
                let containers = tokio::time::timeout(READ_LIMIT, list_containers(&self.privd))
                    .await
                    .ok()
                    .flatten()
                    .ok_or_else(|| {
                        CallError::new(ErrorCode::NotFound, "no container engine answered")
                    })?;
                let container = find_container(&containers, id).ok_or_else(|| {
                    CallError::new(ErrorCode::NotFound, format!("no container `{id}`"))
                })?;
                if container.state != ContainerState::Running {
                    return Err(bad(format!("`{}` isn't running", container.name)));
                }
                Scope {
                    name: container.name.clone(),
                    pid: None,
                    started: None,
                    processes: 0,
                }
            }
            Watch::Unknown => return Err(bad("this agent doesn't know what to watch")),
        };
        let now = now_ms();
        let kept = Kept {
            plan,
            actor,
            since: now,
            scope: scope.clone(),
            phase: Phase::Waiting,
            quiet_minutes: 0,
            minute: None,
        };
        let id = kept.plan.id.clone();
        {
            let mut saved = self.saved.lock().await;
            // Another start of the same ID may have won the race to the lock.
            if !saved.followups.iter().any(|other| other.plan.id == id) {
                saved.followups.push(kept);
                save(&self.path, &saved).await;
                self.publish(&saved);
            }
        }
        self.record(
            &id,
            now,
            Event::Started {
                scope: scope.clone(),
            },
        )
        .await;
        self.changed.notify_one();
        Ok(Started { id, scope })
    }

    /// Approves a step that asked first (`followup.approve`).
    pub async fn approve(&self, id: &str, step: u32) -> Result<(), CallError> {
        self.answer(id, step, |pending| match pending {
            Pending::Asking { .. } => Ok(Pending::Due),
            _ => Err(bad("that step isn't asking")),
        })
        .await
    }

    /// Skips a step that asks or counts down (`followup.skip`).
    pub async fn skip(&self, id: &str, step: u32) -> Result<(), CallError> {
        self.answer(id, step, |pending| match pending {
            Pending::Fresh | Pending::Asking { .. } | Pending::CountingDown { .. } => {
                Ok(Pending::Skipped)
            }
            Pending::Skipped => Ok(Pending::Skipped),
            Pending::Due => Err(bad("that step is running already")),
        })
        .await
    }

    async fn answer(
        &self,
        id: &str,
        step: u32,
        change: impl FnOnce(Pending) -> Result<Pending, CallError>,
    ) -> Result<(), CallError> {
        let mut saved = self.saved.lock().await;
        let kept = saved
            .followups
            .iter_mut()
            .find(|kept| kept.plan.id == id && kept.phase != Phase::Over)
            .ok_or_else(|| CallError::new(ErrorCode::NotFound, "no such follow-up"))?;
        let Phase::Stepping { next, pending } = kept.phase else {
            return Err(bad("its job isn't done yet"));
        };
        if next != step {
            return Err(bad(format!(
                "step {step} isn't the one waiting; step {next} is"
            )));
        }
        kept.phase = Phase::Stepping {
            next,
            pending: change(pending)?,
        };
        save(&self.path, &saved).await;
        drop(saved);
        self.changed.notify_one();
        Ok(())
    }

    /// Ends a follow-up (`followup.cancel`); what hasn't run won't.
    pub async fn cancel(&self, id: &str) -> Result<(), CallError> {
        let mut saved = self.saved.lock().await;
        let kept = saved
            .followups
            .iter_mut()
            .find(|kept| kept.plan.id == id && kept.phase != Phase::Over)
            .ok_or_else(|| CallError::new(ErrorCode::NotFound, "no such follow-up"))?;
        kept.phase = Phase::Over;
        saved.followups.retain(|kept| kept.phase != Phase::Over);
        save(&self.path, &saved).await;
        self.publish(&saved);
        drop(saved);
        self.record(
            id,
            now_ms(),
            Event::Ended {
                why: EndedWhy::Cancelled,
            },
        )
        .await;
        self.changed.notify_one();
        Ok(())
    }

    /// Pushes one record; the outbox keeps it until Console has it.
    async fn record(&self, id: &str, at: u64, event: Event) {
        let record = FollowUpRecord {
            id: id.to_owned(),
            at,
            event,
        };
        match serde_json::to_value(&record) {
            Ok(data) => self.outbox.push(RecordKind::Followup, data).await,
            Err(e) => warn!("a follow-up record didn't serialize: {e}"),
        }
    }

    /// Puts what the follow-ups are into the topic's latest sample.
    fn publish(&self, saved: &Saved) {
        self.latest
            .send_replace(Some(Arc::new(sample(saved, None, now_ms()))));
    }
}

/// The `followups` topic's sample: each follow-up, with the minute so far
/// where the task has one.
fn sample(saved: &Saved, sums: Option<&HashMap<String, Sums>>, now: u64) -> FollowUpsSample {
    let followups = saved
        .followups
        .iter()
        .filter(|kept| kept.phase != Phase::Over)
        .map(|kept| FollowUpState {
            id: kept.plan.id.clone(),
            scope: kept.scope.clone(),
            since: kept.since,
            stage: stage_of(&kept.phase),
            minute: sums
                .and_then(|sums| sums.get(&kept.plan.id))
                .and_then(|sums| sums.minute(&kept.plan.finish))
                .or_else(|| kept.minute.clone()),
            quiet_minutes: kept.quiet_minutes,
            keeping_awake: false,
        })
        .collect();
    FollowUpsSample { ts: now, followups }
}

fn stage_of(phase: &Phase) -> Stage {
    match phase {
        Phase::Waiting | Phase::Over => Stage::Waiting,
        Phase::Stepping { next, pending } => match pending {
            Pending::Asking { .. } => Stage::Asking { step: *next },
            Pending::CountingDown { until } => Stage::CountingDown {
                step: *next,
                until: *until,
            },
            Pending::Fresh | Pending::Due | Pending::Skipped => Stage::Running { step: *next },
        },
    }
}

fn bad(msg: impl Into<String>) -> CallError {
    CallError::new(ErrorCode::BadRequest, msg)
}

/// Checks a plan before anything is read: its ID, what it watches and how it
/// finishes are ones this agent knows, a power step comes last, and the
/// policy allows every step now.
fn check_plan(plan: &Plan, policy: &PolicyState, now: u64) -> Result<(), CallError> {
    let id_ok = !plan.id.is_empty()
        && plan.id.len() <= followup::ID_MAX
        && plan
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !id_ok {
        return Err(bad(
            "a follow-up's ID is up to 64 letters, digits, `_` and `-`",
        ));
    }
    if matches!(plan.watch, Watch::Unknown) {
        return Err(bad("this agent doesn't know what to watch"));
    }
    match &plan.finish {
        Finish::Exits => {}
        Finish::Quiet { minutes, cpu, .. } => {
            if !(1..=24 * 60).contains(minutes) || !cpu.is_finite() || *cpu < 0.0 {
                return Err(bad(
                    "quiet needs 1 to 1440 minutes and a CPU line of 0 or more",
                ));
            }
        }
        Finish::Unknown => return Err(bad("this agent doesn't know when that job is done")),
    }
    if let Some(deadline) = &plan.deadline {
        if deadline.at <= now {
            return Err(bad("the deadline has passed"));
        }
        if deadline.then == AtDeadline::Unknown {
            return Err(bad("this agent doesn't know what to do at that deadline"));
        }
    }
    let container = matches!(plan.watch, Watch::Container { .. });
    for (index, step) in plan.steps.iter().enumerate() {
        let what = &step.what;
        if matches!(what, StepAction::Unknown) {
            return Err(bad(format!("step {index} is one this agent doesn't know")));
        }
        if matches!(what, StepAction::Power { .. }) && index + 1 != plan.steps.len() {
            return Err(bad("a power step has to be the last"));
        }
        let capability = match what {
            // Stopping a container is the containers' permission.
            StepAction::Stop if container => Some("containers.manage"),
            _ => what.capability(),
        };
        if let Some(capability) = capability
            && !policy.allows(capability)
        {
            return Err(CallError::new(
                ErrorCode::PolicyDenied,
                format!("the device policy doesn't allow {capability}"),
            ));
        }
    }
    if plan.keep_awake && !policy.allows("power.inhibit") {
        return Err(CallError::new(
            ErrorCode::PolicyDenied,
            "the device policy doesn't allow power.inhibit".to_owned(),
        ));
    }
    Ok(())
}

/// What an app or process watch covers in `table`: the job's top process,
/// matched by its start time so a reused PID never counts. The operating
/// system's core and the agent itself can't be watched.
fn scope_of(watch: &Watch, table: &[ProcessInfo], agent: u32) -> Result<Scope, CallError> {
    let (Watch::App { pid, started } | Watch::Process { pid, started }) = watch else {
        return Err(bad("not a process"));
    };
    let picked = table
        .iter()
        .find(|process| process.pid == *pid && process.started == *started)
        .ok_or_else(|| {
            CallError::new(ErrorCode::NotFound, "that process isn't running any more")
        })?;
    let top = match watch {
        Watch::App { .. } => followup::job_top(table, picked.pid).unwrap_or(picked),
        _ => picked,
    };
    if top.pid <= 1 || top.kernel || top.pid == agent {
        return Err(bad(format!(
            "{} is part of the system, and never finishes",
            top.name
        )));
    }
    let processes = u32::try_from(followup::job_members(table, top.pid).len()).unwrap_or(u32::MAX);
    Ok(Scope {
        name: top.name.clone(),
        pid: Some(top.pid),
        started: Some(top.started),
        processes,
    })
}

/// The job's processes in `table`: its top and everything under it, or none
/// once the top is gone.
fn members<'a>(scope: &Scope, table: &'a [ProcessInfo]) -> Vec<&'a ProcessInfo> {
    let (Some(pid), Some(started)) = (scope.pid, scope.started) else {
        return Vec::new();
    };
    if !table
        .iter()
        .any(|process| process.pid == pid && process.started == started)
    {
        return Vec::new();
    }
    followup::job_members(table, pid)
}

fn find_container<'a>(containers: &'a [Container], id: &str) -> Option<&'a Container> {
    containers.iter().find(|container| {
        container.name == id
            || (id.len() >= 12 && (container.id.starts_with(id) || id.starts_with(&container.id)))
    })
}

async fn list_containers(privd: &Path) -> Option<Vec<Container>> {
    let value = ipc::call_within(privd, Call::Containers { counters: false }, READ_LIMIT)
        .await
        .ok()?;
    let listing: docker::Listing = serde_json::from_value(value).ok()?;
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

/// The minute closed: busy or quiet, and done after enough quiet minutes.
fn judge_minute(kept: &mut Kept, sums: Option<Sums>) -> Option<DoneWhy> {
    if kept.phase != Phase::Waiting {
        return None;
    }
    // A minute with no reading doesn't count either way.
    let minute = sums?.minute(&kept.plan.finish)?;
    kept.quiet_minutes = if minute.busy {
        0
    } else {
        kept.quiet_minutes + 1
    };
    kept.minute = Some(minute);
    match kept.plan.finish {
        Finish::Quiet { minutes, .. } if kept.quiet_minutes >= minutes => Some(DoneWhy::Quiet),
        _ => None,
    }
}

/// Whether the job is gone: for any finish, a job that exits is done.
fn judge_exit(kept: &Kept, present: bool) -> Option<DoneWhy> {
    (kept.phase == Phase::Waiting && !present).then_some(DoneWhy::Exited)
}

/// What the deadline does once it's passed: the steps run, or it gives up.
fn judge_deadline(kept: &Kept, now: u64) -> Option<AtDeadline> {
    let deadline = kept.plan.deadline.as_ref()?;
    (kept.phase == Phase::Waiting && now >= deadline.at).then_some(deadline.then)
}

/// The job is done: the record, then the steps from the first.
fn done(kept: &mut Kept, why: DoneWhy, now: u64) -> Vec<Effect> {
    kept.phase = Phase::Stepping {
        next: 0,
        pending: Pending::Fresh,
    };
    let mut effects = vec![Effect::Record(Event::Done { why })];
    effects.extend(advance(kept, now));
    effects
}

/// Moves through the steps as far as they go without waiting: messages go
/// out, skipped steps pass, an acting step starts its countdown or asks, and
/// one that's due is run. Ends the follow-up after its last step.
fn advance(kept: &mut Kept, now: u64) -> Vec<Effect> {
    let mut effects = Vec::new();
    loop {
        let Phase::Stepping { next, pending } = kept.phase else {
            return effects;
        };
        let Some(step) = kept.plan.steps.get(next as usize) else {
            kept.phase = Phase::Over;
            effects.push(Effect::Record(Event::Ended {
                why: EndedWhy::Completed,
            }));
            return effects;
        };
        let after = Phase::Stepping {
            next: next + 1,
            pending: Pending::Fresh,
        };
        if !step.what.acts() {
            effects.push(Effect::Record(Event::Step {
                step: next,
                outcome: Outcome::Ran { processes: None },
            }));
            kept.phase = after;
            continue;
        }
        match pending {
            Pending::Fresh if step.ask => {
                let until = kept
                    .plan
                    .deadline
                    .as_ref()
                    .map_or(now + u64::from(followup::ASK_HOURS) * HOUR_MS, |deadline| {
                        deadline.at.max(now + MINUTE_MS)
                    });
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::Asking { until },
                };
                effects.push(Effect::Record(Event::Asking { step: next }));
                return effects;
            }
            Pending::Fresh => {
                let until = now + u64::from(followup::COUNTDOWN_SECONDS) * 1_000;
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::CountingDown { until },
                };
                effects.push(Effect::Record(Event::CountingDown { step: next, until }));
                return effects;
            }
            Pending::Asking { until } if now >= until => {
                let outcome = Outcome::Skipped {
                    why: SkippedWhy::NoAnswer,
                };
                effects.push(Effect::Record(Event::Step {
                    step: next,
                    outcome,
                }));
                kept.phase = after;
            }
            Pending::CountingDown { until } if now >= until => {
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::Due,
                };
            }
            Pending::Asking { .. } | Pending::CountingDown { .. } => return effects,
            Pending::Skipped => {
                let outcome = Outcome::Skipped {
                    why: SkippedWhy::Skipped,
                };
                effects.push(Effect::Record(Event::Step {
                    step: next,
                    outcome,
                }));
                kept.phase = after;
            }
            Pending::Due => {
                effects.push(Effect::Run(next));
                return effects;
            }
        }
    }
}

/// A step ran: its outcome on the record, then on to the next.
fn finish_step(kept: &mut Kept, step: u32, outcome: Outcome, now: u64) -> Vec<Effect> {
    let mut effects = vec![Effect::Record(Event::Step { step, outcome })];
    if let Phase::Stepping { next, .. } = kept.phase
        && next == step
    {
        kept.phase = Phase::Stepping {
            next: next + 1,
            pending: Pending::Fresh,
        };
        effects.extend(advance(kept, now));
    }
    effects
}

/// Watches the follow-ups until shutdown.
pub async fn run(
    followups: Arc<FollowUps>,
    processes: Arc<processes::Latest>,
    containers: Arc<containers::Latest>,
    token: CancellationToken,
) -> Result<(), String> {
    end_other_boots(&followups).await;
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + TICK, TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut ticks: u32 = 0;
    let mut sums: HashMap<String, Sums> = HashMap::new();
    let mut tables: Option<watch::Receiver<Option<Arc<processes::Table>>>> = None;
    let mut engines: Option<watch::Receiver<Option<Arc<ContainersSample>>>> = None;
    // What was present at the last reading, by follow-up.
    let mut present: HashMap<String, bool> = HashMap::new();
    loop {
        // A receiver keeps a sampler reading while a follow-up waits on it.
        let (wants_processes, wants_containers) = {
            let saved = followups.saved.lock().await;
            let waiting = |container: bool| {
                saved.followups.iter().any(|kept| {
                    kept.phase == Phase::Waiting
                        && matches!(kept.plan.watch, Watch::Container { .. }) == container
                })
            };
            (waiting(false), waiting(true))
        };
        if wants_processes && tables.is_none() {
            tables = Some(processes.subscribe());
        } else if !wants_processes {
            tables = None;
        }
        if wants_containers && engines.is_none() {
            engines = Some(containers.subscribe());
        } else if !wants_containers {
            engines = None;
        }
        tokio::select! {
            _ = ticker.tick() => {
                ticks = ticks.wrapping_add(1);
                let minute_ends = ticks.is_multiple_of(TICKS_PER_MINUTE);
                let now = now_ms();
                let effects = {
                    let mut saved = followups.saved.lock().await;
                    let mut effects = Vec::new();
                    for kept in &mut saved.followups {
                        let id = kept.plan.id.clone();
                        let mut found = Vec::new();
                        if let Some(why) = judge_exit(kept, present.get(&id).copied().unwrap_or(true)) {
                            found.extend(done(kept, why, now));
                        }
                        match judge_deadline(kept, now) {
                            Some(AtDeadline::RunSteps) => found.extend(done(kept, DoneWhy::Deadline, now)),
                            Some(AtDeadline::GiveUp | AtDeadline::Unknown) => {
                                kept.phase = Phase::Over;
                                found.push(Effect::Record(Event::Ended { why: EndedWhy::GaveUp }));
                            }
                            None => {}
                        }
                        if minute_ends
                            && let Some(why) = judge_minute(kept, sums.remove(&id)) {
                                found.extend(done(kept, why, now));
                            }
                        // Countdowns and questions that ran out.
                        found.extend(advance(kept, now));
                        effects.extend(found.into_iter().map(|effect| (id.clone(), effect)));
                    }
                    if minute_ends {
                        sums.clear();
                    }
                    if !effects.is_empty() {
                        save(&followups.path, &saved).await;
                    }
                    effects
                };
                carry_out(&followups, effects).await;
                let saved = followups.saved.lock().await;
                followups.latest.send_replace(Some(Arc::new(sample(&saved, Some(&sums), now_ms()))));
            }
            Some(()) = next_change(&mut tables) => {
                if let Some(table) = tables.as_ref().and_then(|receiver| receiver.borrow().clone()) {
                    let saved = followups.saved.lock().await;
                    for kept in saved.followups.iter().filter(|kept| kept.phase == Phase::Waiting) {
                        if matches!(kept.plan.watch, Watch::Container { .. }) {
                            continue;
                        }
                        let job = members(&kept.scope, &table.processes);
                        present.insert(kept.plan.id.clone(), !job.is_empty());
                        if !job.is_empty() {
                            let cpu = job.iter().map(|process| process.cpu).sum();
                            sums.entry(kept.plan.id.clone()).or_default().add(cpu, None);
                        }
                    }
                }
            }
            Some(()) = next_change(&mut engines) => {
                if let Some(engine) = engines.as_ref().and_then(|receiver| receiver.borrow().clone()) {
                    let cores = std::thread::available_parallelism().map_or(1.0, |n| f64::from(u32::try_from(n.get()).unwrap_or(1)));
                    let saved = followups.saved.lock().await;
                    for kept in saved.followups.iter().filter(|kept| kept.phase == Phase::Waiting) {
                        let Watch::Container { id } = &kept.plan.watch else {
                            continue;
                        };
                        let running = find_container(&engine.containers, id)
                            .filter(|container| container.state == ContainerState::Running);
                        present.insert(kept.plan.id.clone(), running.is_some());
                        if let Some(container) = running {
                            // A container's share of the machine, as a percentage of one core.
                            let cpu = container.cpu.unwrap_or(0.0) * cores * 100.0;
                            sums.entry(kept.plan.id.clone()).or_default().add(cpu, None);
                        }
                    }
                }
            }
            () = followups.changed.notified() => {
                let now = now_ms();
                let effects = {
                    let mut saved = followups.saved.lock().await;
                    let effects: Vec<(String, Effect)> = saved
                        .followups
                        .iter_mut()
                        .flat_map(|kept| {
                            let id = kept.plan.id.clone();
                            advance(kept, now).into_iter().map(move |effect| (id.clone(), effect))
                        })
                        .collect();
                    if !effects.is_empty() {
                        save(&followups.path, &saved).await;
                    }
                    effects
                };
                carry_out(&followups, effects).await;
                let saved = followups.saved.lock().await;
                followups.latest.send_replace(Some(Arc::new(sample(&saved, Some(&sums), now_ms()))));
            }
            () = token.cancelled() => return Ok(()),
        }
    }
}

/// Ends, without running anything, the follow-ups saved in an earlier boot.
async fn end_other_boots(followups: &FollowUps) {
    let boot = host::boot_id();
    let ended: Vec<String> = {
        let mut saved = followups.saved.lock().await;
        if saved.boot_id == boot {
            return;
        }
        let ended = saved
            .followups
            .iter()
            .filter(|kept| kept.phase != Phase::Over)
            .map(|kept| kept.plan.id.clone())
            .collect();
        saved.boot_id = boot;
        saved.followups.clear();
        save(&followups.path, &saved).await;
        followups.publish(&saved);
        ended
    };
    for id in ended {
        followups
            .record(
                &id,
                now_ms(),
                Event::Ended {
                    why: EndedWhy::Restarted,
                },
            )
            .await;
    }
}

/// Pushes the records and runs the steps the effects call for, each step's
/// outcome moving its follow-up on, until nothing more is due.
async fn carry_out(followups: &FollowUps, mut effects: Vec<(String, Effect)>) {
    while !effects.is_empty() {
        let mut more = Vec::new();
        for (id, effect) in effects {
            match effect {
                Effect::Record(event) => followups.record(&id, now_ms(), event).await,
                Effect::Run(step) => {
                    let Some(kept) = followups
                        .saved
                        .lock()
                        .await
                        .followups
                        .iter()
                        .find(|kept| kept.plan.id == id)
                        .cloned()
                    else {
                        continue;
                    };
                    let power = matches!(
                        kept.plan.steps.get(step as usize).map(|step| &step.what),
                        Some(StepAction::Power { .. })
                    );
                    if power {
                        // Gone from the saved list before the machine goes, so
                        // a restart can't run it twice; the records follow
                        // within privd's short delay.
                        remove(followups, &id).await;
                    }
                    let outcome = run_step(followups, &kept, step).await;
                    let now = now_ms();
                    if power {
                        followups
                            .record(&id, now, Event::Step { step, outcome })
                            .await;
                        followups
                            .record(
                                &id,
                                now,
                                Event::Ended {
                                    why: EndedWhy::Completed,
                                },
                            )
                            .await;
                        continue;
                    }
                    let mut saved = followups.saved.lock().await;
                    if let Some(kept) = saved.followups.iter_mut().find(|kept| kept.plan.id == id) {
                        more.extend(
                            finish_step(kept, step, outcome, now)
                                .into_iter()
                                .map(|effect| (id.clone(), effect)),
                        );
                    }
                    save(&followups.path, &saved).await;
                }
            }
        }
        // Follow-ups that ended leave the list once their records are out.
        let mut saved = followups.saved.lock().await;
        let before = saved.followups.len();
        saved.followups.retain(|kept| kept.phase != Phase::Over);
        if saved.followups.len() != before {
            save(&followups.path, &saved).await;
        }
        followups.publish(&saved);
        drop(saved);
        effects = more;
    }
}

async fn remove(followups: &FollowUps, id: &str) {
    let mut saved = followups.saved.lock().await;
    saved.followups.retain(|kept| kept.plan.id != id);
    save(&followups.path, &saved).await;
    followups.publish(&saved);
}

/// Runs one step through privd, which checks the policy again and audits it
/// as the person who set the follow-up up.
async fn run_step(followups: &FollowUps, kept: &Kept, step: u32) -> Outcome {
    let Some(what) = kept
        .plan
        .steps
        .get(step as usize)
        .map(|step| step.what.clone())
    else {
        return Outcome::Failed {
            reason: "no such step".to_owned(),
        };
    };
    let privd = followups.privd.as_path();
    let request_id = format!("{}/{step}", kept.plan.id);
    let actor = kept.actor.clone();
    let result = match what {
        StepAction::Notify | StepAction::Unknown => return Outcome::Ran { processes: None },
        StepAction::Stop => match &kept.plan.watch {
            Watch::Container { id } => {
                let call = Call::ContainerAct {
                    request_id,
                    container: id.clone(),
                    action: docker::Action::Stop,
                    actor,
                };
                ipc::call_within(privd, call, STEP_LIMIT.max(docker::ACTION_LIMIT))
                    .await
                    .map(|_| None)
            }
            _ => return stop_job(privd, kept, &request_id).await,
        },
        StepAction::QuitApp { app, user } => {
            let call = Call::AppQuit {
                request_id,
                app,
                user,
                force: false,
                actor,
            };
            ipc::call_within(privd, call, STEP_LIMIT)
                .await
                .map(|_| None)
        }
        StepAction::Service {
            unit,
            scope,
            user,
            action,
        } => {
            let action = match action {
                StopOrRestart::Restart => ServiceAction::Restart,
                StopOrRestart::Stop | StopOrRestart::Unknown => ServiceAction::Stop,
            };
            let call = Call::ServiceAct {
                request_id,
                unit,
                action,
                scope,
                user,
                actor,
            };
            ipc::call_within(privd, call, STEP_LIMIT)
                .await
                .map(|_| None)
        }
        StepAction::Container { id, action } => {
            let action = match action {
                StopOrRestart::Restart => docker::Action::Restart,
                StopOrRestart::Stop | StopOrRestart::Unknown => docker::Action::Stop,
            };
            let call = Call::ContainerAct {
                request_id,
                container: id,
                action,
                actor,
            };
            ipc::call_within(privd, call, STEP_LIMIT.max(docker::ACTION_LIMIT))
                .await
                .map(|_| None)
        }
        StepAction::Power {
            action,
            skip_if_in_use,
        } => {
            if skip_if_in_use {
                match super::presence::in_use().await {
                    Some(false) => {}
                    Some(true) => {
                        return Outcome::Skipped {
                            why: SkippedWhy::InUse,
                        };
                    }
                    // It can't tell, and the person guarded against someone being there.
                    None => {
                        return Outcome::Skipped {
                            why: SkippedWhy::CantTell,
                        };
                    }
                }
            }
            let call = Call::Power {
                request_id,
                action,
                actor,
            };
            ipc::call_within(privd, call, STEP_LIMIT)
                .await
                .map(|_| None)
        }
    };
    match result {
        Ok(processes) => Outcome::Ran { processes },
        Err(e) => outcome_of(e),
    }
}

/// A refusal is the policy's or a protected target's; anything else failed.
fn outcome_of(error: CallError) -> Outcome {
    match error.code {
        ErrorCode::PolicyDenied | ErrorCode::SignatureRequired | ErrorCode::SignatureInvalid => {
            Outcome::Refused { reason: error.msg }
        }
        _ => Outcome::Failed { reason: error.msg },
    }
}

/// Stops the job: its top process first, which takes its children with it
/// more often than not, then whatever of the job is left, and after
/// [`FORCE_AFTER`] forces what still runs. Windows can only end processes at
/// once, so there it forces from the start.
async fn stop_job(privd: &Path, kept: &Kept, request_id: &str) -> Outcome {
    let force = cfg!(windows);
    let Ok(table) = processes::read_once(privd.to_path_buf()).await else {
        return Outcome::Failed {
            reason: "the process table couldn't be read".to_owned(),
        };
    };
    let job: Vec<(u32, u64)> = members(&kept.scope, &table)
        .iter()
        .map(|p| (p.pid, p.started))
        .collect();
    if job.is_empty() {
        return Outcome::Skipped {
            why: SkippedWhy::NothingToDo,
        };
    }
    let mut stopped: u32 = 0;
    let mut left = Vec::new();
    for (pid, started) in job {
        let call = Call::ProcessSignal {
            request_id: request_id.to_owned(),
            pid,
            started,
            force,
            actor: kept.actor.clone(),
        };
        match ipc::call_within(privd, call, STEP_LIMIT).await {
            Ok(value) => {
                match serde_json::from_value::<cntrl_protocol::process::ProcessSignalResult>(value)
                {
                    Ok(result) if result.result == StopResult::Stopped => stopped += 1,
                    Ok(result) if result.result == StopResult::StillRunning => {
                        left.push((pid, started))
                    }
                    _ => {}
                }
            }
            Err(e) if e.code == ErrorCode::PolicyDenied => return outcome_of(e),
            Err(e) => warn!("stopping {pid} for a follow-up failed: {}", e.msg),
        }
    }
    if !left.is_empty() {
        tokio::time::sleep(FORCE_AFTER).await;
        for (pid, started) in left {
            let call = Call::ProcessSignal {
                request_id: request_id.to_owned(),
                pid,
                started,
                force: true,
                actor: kept.actor.clone(),
            };
            if let Ok(value) = ipc::call_within(privd, call, STEP_LIMIT).await
                && serde_json::from_value::<cntrl_protocol::process::ProcessSignalResult>(value)
                    .is_ok_and(|result| result.result == StopResult::Stopped)
            {
                stopped += 1;
            }
        }
    }
    Outcome::Ran {
        processes: Some(stopped),
    }
}

/// The next change in a sampler's latest, once there's a receiver; never with none.
async fn next_change<T>(receiver: &mut Option<watch::Receiver<Option<Arc<T>>>>) -> Option<()> {
    match receiver {
        Some(receiver) => receiver.changed().await.ok(),
        None => std::future::pending().await,
    }
}

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
        warn!("can't save the follow-ups to {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use cntrl_protocol::followup::{Deadline, Step};
    use cntrl_protocol::power::PowerAction;

    use super::*;

    fn process(pid: u32, parent: Option<u32>, name: &str, started: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent,
            name: name.to_owned(),
            user: None,
            cpu: 0.0,
            memory: 0,
            started,
            unit: None,
            kernel: false,
            system: false,
            protected: false,
        }
    }

    fn quiet(minutes: u32) -> Finish {
        Finish::Quiet {
            minutes,
            cpu: 6.0,
            disk: None,
        }
    }

    fn kept(steps: Vec<Step>, finish: Finish) -> Kept {
        Kept {
            plan: Plan {
                id: "fu_1".to_owned(),
                watch: Watch::App {
                    pid: 100,
                    started: 10,
                },
                finish,
                deadline: None,
                steps,
                keep_awake: false,
            },
            actor: None,
            since: 0,
            scope: Scope {
                name: "T3 Code".to_owned(),
                pid: Some(100),
                started: Some(10),
                processes: 3,
            },
            phase: Phase::Waiting,
            quiet_minutes: 0,
            minute: None,
        }
    }

    fn step(what: StepAction, ask: bool) -> Step {
        Step { what, ask }
    }

    fn sums(cpu: f64) -> Option<Sums> {
        let mut sums = Sums::default();
        sums.add(cpu, None);
        Some(sums)
    }

    fn records(effects: &[Effect]) -> Vec<Event> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Record(event) => Some(event.clone()),
                Effect::Run(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_reused_pid_isnt_the_job() {
        let table = vec![
            process(1, None, "launchd", 0),
            process(100, Some(1), "T3 Code", 99),
        ];
        let watch = Watch::App {
            pid: 100,
            started: 10,
        };
        assert_eq!(
            scope_of(&watch, &table, 4242).map_err(|e| e.code),
            Err(ErrorCode::NotFound)
        );
        let scope = Scope {
            name: "T3 Code".to_owned(),
            pid: Some(100),
            started: Some(10),
            processes: 1,
        };
        assert!(members(&scope, &table).is_empty());
    }

    #[test]
    fn an_app_is_watched_from_its_top_and_the_core_never() {
        let table = vec![
            process(1, None, "launchd", 0),
            process(100, Some(1), "T3 Code", 10),
            process(101, Some(100), "T3 Code Helper", 11),
            process(102, Some(101), "git", 12),
        ];
        let scope = scope_of(
            &Watch::App {
                pid: 102,
                started: 12,
            },
            &table,
            4242,
        )
        .expect("a scope");
        assert_eq!(
            (scope.pid, scope.processes, scope.name.as_str()),
            (Some(100), 3, "T3 Code")
        );
        let only = scope_of(
            &Watch::Process {
                pid: 101,
                started: 11,
            },
            &table,
            4242,
        )
        .expect("a scope");
        assert_eq!((only.pid, only.processes), (Some(101), 2));
        let core = scope_of(&Watch::Process { pid: 1, started: 0 }, &table, 4242);
        assert_eq!(core.map_err(|e| e.code), Err(ErrorCode::BadRequest));
        let agent = scope_of(
            &Watch::Process {
                pid: 100,
                started: 10,
            },
            &table,
            100,
        );
        assert!(agent.is_err());
    }

    #[test]
    fn quiet_minutes_in_a_row_finish_it_and_busy_starts_over() {
        let mut kept = kept(vec![], quiet(3));
        assert_eq!(judge_minute(&mut kept, sums(2.0)), None);
        assert_eq!(judge_minute(&mut kept, sums(2.0)), None);
        assert_eq!(judge_minute(&mut kept, sums(40.0)), None);
        assert_eq!(kept.quiet_minutes, 0);
        // A minute with no reading counts neither way.
        assert_eq!(judge_minute(&mut kept, None), None);
        assert_eq!(judge_minute(&mut kept, sums(1.0)), None);
        assert_eq!(judge_minute(&mut kept, sums(1.0)), None);
        assert_eq!(judge_minute(&mut kept, sums(1.0)), Some(DoneWhy::Quiet));
    }

    #[test]
    fn disk_over_its_line_keeps_a_quiet_cpu_busy() {
        let finish = Finish::Quiet {
            minutes: 1,
            cpu: 6.0,
            disk: Some(1_000),
        };
        let mut kept = kept(vec![], finish);
        let mut busy = Sums::default();
        busy.add(1.0, Some(50_000.0));
        assert_eq!(judge_minute(&mut kept, Some(busy)), None);
        let mut idle = Sums::default();
        idle.add(1.0, Some(10.0));
        assert_eq!(judge_minute(&mut kept, Some(idle)), Some(DoneWhy::Quiet));
    }

    #[test]
    fn an_exit_is_done_whatever_the_finish() {
        let kept = kept(vec![], quiet(5));
        assert_eq!(judge_exit(&kept, false), Some(DoneWhy::Exited));
        assert_eq!(judge_exit(&kept, true), None);
    }

    #[test]
    fn the_deadline_gives_up_or_runs_the_steps() {
        let mut kept = kept(vec![], quiet(5));
        kept.plan.deadline = Some(Deadline {
            at: 1_000,
            then: AtDeadline::RunSteps,
        });
        assert_eq!(judge_deadline(&kept, 999), None);
        assert_eq!(judge_deadline(&kept, 1_000), Some(AtDeadline::RunSteps));
    }

    #[test]
    fn a_message_goes_out_then_an_acting_step_counts_down_and_runs() {
        let steps = vec![
            step(StepAction::Notify, false),
            step(StepAction::Stop, false),
        ];
        let mut kept = kept(steps, quiet(5));
        let effects = done(&mut kept, DoneWhy::Quiet, 0);
        assert_eq!(
            records(&effects),
            vec![
                Event::Done {
                    why: DoneWhy::Quiet
                },
                Event::Step {
                    step: 0,
                    outcome: Outcome::Ran { processes: None }
                },
                Event::CountingDown {
                    step: 1,
                    until: 60_000
                },
            ]
        );
        assert!(advance(&mut kept, 59_999).is_empty());
        assert_eq!(advance(&mut kept, 60_000), vec![Effect::Run(1)]);
        let finished = finish_step(&mut kept, 1, Outcome::Ran { processes: Some(4) }, 61_000);
        assert_eq!(
            records(&finished),
            vec![
                Event::Step {
                    step: 1,
                    outcome: Outcome::Ran { processes: Some(4) }
                },
                Event::Ended {
                    why: EndedWhy::Completed
                },
            ]
        );
        assert_eq!(kept.phase, Phase::Over);
    }

    #[test]
    fn a_skip_during_the_countdown_skips_only_that_step() {
        let steps = vec![
            step(StepAction::Stop, false),
            step(StepAction::Notify, false),
        ];
        let mut kept = kept(steps, quiet(5));
        done(&mut kept, DoneWhy::Exited, 0);
        kept.phase = Phase::Stepping {
            next: 0,
            pending: Pending::Skipped,
        };
        assert_eq!(
            records(&advance(&mut kept, 10)),
            vec![
                Event::Step {
                    step: 0,
                    outcome: Outcome::Skipped {
                        why: SkippedWhy::Skipped
                    }
                },
                Event::Step {
                    step: 1,
                    outcome: Outcome::Ran { processes: None }
                },
                Event::Ended {
                    why: EndedWhy::Completed
                },
            ]
        );
    }

    #[test]
    fn ask_first_waits_for_an_answer_then_gives_up_on_it() {
        let steps = vec![step(StepAction::Stop, true)];
        let mut kept = kept(steps, quiet(5));
        let effects = done(&mut kept, DoneWhy::Quiet, 0);
        assert_eq!(records(&effects)[1], Event::Asking { step: 0 });
        let hours = u64::from(followup::ASK_HOURS) * HOUR_MS;
        assert!(advance(&mut kept, hours - 1).is_empty());
        assert_eq!(
            records(&advance(&mut kept, hours)),
            vec![
                Event::Step {
                    step: 0,
                    outcome: Outcome::Skipped {
                        why: SkippedWhy::NoAnswer
                    }
                },
                Event::Ended {
                    why: EndedWhy::Completed
                },
            ]
        );
    }

    #[test]
    fn an_approved_step_runs_at_once() {
        let mut kept = kept(vec![step(StepAction::Stop, true)], quiet(5));
        done(&mut kept, DoneWhy::Quiet, 0);
        kept.phase = Phase::Stepping {
            next: 0,
            pending: Pending::Due,
        };
        assert_eq!(advance(&mut kept, 1), vec![Effect::Run(0)]);
    }

    #[test]
    fn plans_are_checked_before_anything_is_read() {
        let policy = PolicyState::Valid {
            policy: super::super::policy::Policy {
                source: super::super::policy::Source::File,
                allow: ["processes.read", "processes.signal", "power.suspend"]
                    .map(str::to_owned)
                    .into(),
                protect: std::collections::BTreeSet::new(),
                update: super::super::policy::UpdateMode::default(),
                hash: String::new(),
            },
        };
        let mut plan = kept(vec![step(StepAction::Stop, false)], quiet(5)).plan;
        assert!(check_plan(&plan, &policy, 0).is_ok());
        plan.steps = vec![step(
            StepAction::Power {
                action: PowerAction::Hibernate,
                skip_if_in_use: false,
            },
            false,
        )];
        assert_eq!(
            check_plan(&plan, &policy, 0).map_err(|e| e.code),
            Err(ErrorCode::PolicyDenied)
        );
        plan.steps = vec![
            step(
                StepAction::Power {
                    action: PowerAction::Suspend,
                    skip_if_in_use: true,
                },
                false,
            ),
            step(StepAction::Notify, false),
        ];
        assert_eq!(
            check_plan(&plan, &policy, 0).map_err(|e| e.code),
            Err(ErrorCode::BadRequest)
        );
        plan.steps = vec![step(StepAction::Notify, false)];
        plan.keep_awake = true;
        assert_eq!(
            check_plan(&plan, &policy, 0).map_err(|e| e.code),
            Err(ErrorCode::PolicyDenied)
        );
        plan.keep_awake = false;
        plan.id = "fu bad".to_owned();
        assert_eq!(
            check_plan(&plan, &policy, 0).map_err(|e| e.code),
            Err(ErrorCode::BadRequest)
        );
    }
}
