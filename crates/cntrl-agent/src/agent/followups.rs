//! Follow-ups (D119, D121): one-use plans the machine carries out when a job is
//! done. Console sets one up with `followup.start`, and may change it with
//! `followup.change` while it waits; this task watches the job, judges each
//! minute whether it's busy, and once the job exits or has been quiet long
//! enough, carries out the steps in order. A step that acts counts down, or
//! asks first and waits for an answer; each one runs through privd, which
//! checks the policy again and audits it, and only while the key that signed
//! the plan is still trusted where the machine requires signatures.
//! Everything that happens goes out as a `followup` record through the outbox,
//! so a report made offline reaches Console once the link is back, and the
//! `followups` topic says where each follow-up is. Follow-ups are saved in the
//! state directory with the boot they began in; one from an earlier boot ends
//! without running anything.
//!
//! A minute's CPU and disk come from the running totals of the job's
//! processes, read through privd at each look, so the short tasks a job runs
//! between two looks count where the OS keeps their time; the process table's
//! samples stand in where the totals can't be read. A countdown the machine
//! slept through doesn't run when it wakes: it starts over once Console can
//! reach the machine, or the follow-up ends as unreachable. A power step waits
//! for the other follow-ups' jobs, since the machine going would end them.
//!
//! The judging is written as pure functions over a saved follow-up (`judge_*`,
//! `advance`, `woke`, `used`), so the rules are tested without a clock or a
//! machine.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use cntrl_protocol::ErrorCode;
use cntrl_protocol::app::{AppQuitResult, QuitResult};
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
use tracing::{info, warn};

use super::docker;
use super::host;
use super::ipc::{self, Call, CallError, Request, Response};
use super::outbox::Outbox;
use super::policy::PolicyState;
use super::signers;
use super::uplink::{self, Uplink, now_ms};
use super::{containers, os, processes};

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
/// A look this long after the last means the machine slept, or the agent was
/// stopped: the task looks at least every [`TICK`], and the time a step takes
/// isn't counted.
const SLEPT_AFTER_MS: u64 = 45_000;
/// Running totals over less of a minute than this don't judge it; the
/// table's samples do.
const COUNTED_AT_LEAST_MS: u64 = 30_000;
/// How long before a look a process may have started and still be missing
/// from it: the table it goes by is read every 2 s.
const TABLE_LAG_MS: u64 = 3_000;
/// What keeping the machine awake says, in `pmset -g assertions`,
/// `systemd-inhibit --list` and `powercfg /requests`.
const AWAKE_REASON: &str = "A cntrl follow-up is waiting for a job";
/// How long after a power step ran nothing keeps the machine awake: privd
/// acts a moment after it answers, and a hold taken again would stop it.
const GOING_MS: u64 = 2 * MINUTE_MS;

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
    /// Who set it up, or last changed it: privd audits each step as theirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<Actor>,
    /// The key that signed its plan, where the machine required one: each
    /// step that acts checks it's still trusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signer: Option<String>,
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
    /// It runs at `until` unless someone skips it. `tries` carries the tries
    /// to reach Console after the machine slept through an earlier countdown.
    CountingDown {
        until: u64,
        #[serde(default)]
        tries: u32,
    },
    /// The machine slept through the countdown and woke at `woke`: it counts
    /// down again once Console has heard from the machine since, trying again
    /// at `next`, `tries` of [`followup::REACH_TRIES`] so far.
    Reaching {
        woke: u64,
        tries: u32,
        next: u64,
    },
    /// A power step, waiting for the other follow-ups' jobs to be done.
    WaitingFor,
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

/// What a follow-up's steps go by besides itself: when Console last heard
/// from the machine, and the follow-ups whose jobs still run.
#[derive(Debug, Clone, Copy, Default)]
struct Around<'a> {
    heard: u64,
    waiting: &'a [String],
}

impl Around<'_> {
    /// The other follow-ups whose jobs still run.
    fn others(&self, id: &str) -> Vec<String> {
        self.waiting
            .iter()
            .filter(|other| other.as_str() != id)
            .cloned()
            .collect()
    }
}

/// The follow-ups whose jobs still run.
fn waiting_ids(saved: &Saved) -> Vec<String> {
    saved
        .followups
        .iter()
        .filter(|kept| kept.phase == Phase::Waiting)
        .map(|kept| kept.plan.id.clone())
        .collect()
}

/// The CPU and disk of one minute so far, from the process table's samples,
/// for when the running totals can't be read, and for a container.
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
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let disk = self.disk.map(|sum| (sum / samples).round().max(0.0) as u64);
        Some(minute_of(self.cpu / samples, disk, finish))
    }
}

/// A minute's readings judged against the plan's lines: busy if the CPU or
/// the disk is over its line. Watching only for an exit, every minute counts
/// as busy.
fn minute_of(cpu: f64, disk: Option<u64>, finish: &Finish) -> Minute {
    let busy = match finish {
        Finish::Quiet {
            cpu: line,
            disk: disk_line,
            ..
        } => cpu > *line || disk_line.zip(disk).is_some_and(|(line, disk)| disk > line),
        Finish::Exits | Finish::Unknown => true,
    };
    Minute { cpu, disk, busy }
}

/// One process's running totals at a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counted {
    parent: Option<u32>,
    cpu_ms: u64,
    children_cpu_ms: Option<u64>,
    disk: Option<u64>,
}

/// A job's running totals at one look, by PID and start time.
#[derive(Debug, Clone, Default, PartialEq)]
struct Reading {
    at: u64,
    processes: HashMap<(u32, u64), Counted>,
}

/// What a job's running totals added up to over the minute so far.
#[derive(Debug, Clone, Default)]
struct Meter {
    last: Option<Reading>,
    cpu_ms: u64,
    disk: Option<u64>,
    /// How much of the minute the totals cover, in milliseconds.
    span: u64,
}

impl Meter {
    /// Takes a look's reading: what the job used since the last one counts
    /// toward the minute.
    fn look(&mut self, now: Reading) {
        if let Some(before) = self.last.take() {
            let (cpu, disk) = used(&before, &now, cntrl_host::usage::CHILDREN_DISK);
            self.cpu_ms += cpu;
            if let Some(disk) = disk {
                *self.disk.get_or_insert(0) += disk;
            }
            self.span += now.at.saturating_sub(before.at);
        }
        self.last = Some(now);
    }

    /// The minute so far, when the totals cover enough of it.
    #[allow(clippy::cast_precision_loss)]
    fn minute(&self, finish: &Finish) -> Option<Minute> {
        if self.span < COUNTED_AT_LEAST_MS {
            return None;
        }
        let cpu = self.cpu_ms as f64 / self.span as f64 * 100.0;
        let disk = self
            .disk
            .map(|bytes| bytes.saturating_mul(1000) / self.span);
        Some(minute_of(cpu, disk, finish))
    }

    /// A new minute, counted from the last look.
    fn next_minute(&mut self) {
        self.cpu_ms = 0;
        self.disk = None;
        self.span = 0;
    }
}

/// What a job used between two looks: CPU in milliseconds, and disk bytes
/// where they can be read. A process in both counts its growth, that of the
/// children it waited for included. One that started since counts all it
/// has. One that ended was taken into its parent's children's totals if its
/// parent waited for it, counting again what it had used by the first look,
/// so that much comes off the parent's growth. A parent that doesn't wait for
/// its children has no such growth, so nothing comes off it. Disk works the
/// same where the OS takes children's bytes into their parent's
/// (`children_disk`, Linux).
fn used(before: &Reading, now: &Reading, children_disk: bool) -> (u64, Option<u64>) {
    let mut cpu: u64 = 0;
    let mut disk: Option<u64> = None;
    // Each parent's growth that an ended child can account for, by PID.
    let mut children_growth: HashMap<u32, u64> = HashMap::new();
    let mut disk_growth: HashMap<u32, u64> = HashMap::new();
    for (key, counted) in &now.processes {
        match before.processes.get(key) {
            Some(was) => {
                let children = counted
                    .children_cpu_ms
                    .zip(was.children_cpu_ms)
                    .map_or(0, |(now, was)| now.saturating_sub(was));
                cpu += counted.cpu_ms.saturating_sub(was.cpu_ms) + children;
                *children_growth.entry(key.0).or_default() += children;
                if let (Some(now), Some(was)) = (counted.disk, was.disk) {
                    let grew = now.saturating_sub(was);
                    *disk.get_or_insert(0) += grew;
                    *disk_growth.entry(key.0).or_default() += grew;
                }
            }
            // Started since the last look, or just before it and missed by
            // its table: all of it is new.
            None if key.1.saturating_mul(1000) + TABLE_LAG_MS + 999 >= before.at => {
                cpu += counted.cpu_ms + counted.children_cpu_ms.unwrap_or(0);
                if let Some(bytes) = counted.disk {
                    *disk.get_or_insert(0) += bytes;
                }
            }
            // Here before the last look but not in it: counted from now on.
            None => {}
        }
    }
    for (key, was) in &before.processes {
        if now.processes.contains_key(key) {
            continue;
        }
        let Some(parent) = was.parent else {
            continue;
        };
        if let Some(growth) = children_growth.get_mut(&parent) {
            let off = (was.cpu_ms + was.children_cpu_ms.unwrap_or(0)).min(*growth);
            *growth -= off;
            cpu -= off;
        }
        if children_disk
            && let (Some(growth), Some(was_disk), Some(total)) =
                (disk_growth.get_mut(&parent), was.disk, disk.as_mut())
        {
            let off = was_disk.min(*growth);
            *growth -= off;
            *total -= off;
        }
    }
    (cpu, disk)
}

/// The follow-ups, shared with the uplink, which sets them up and answers them.
pub struct FollowUps {
    path: PathBuf,
    saved: Mutex<Saved>,
    changed: Notify,
    outbox: Arc<Outbox>,
    privd: PathBuf,
    latest: watch::Sender<Option<Arc<FollowUpsSample>>>,
    /// The machine kept awake for the follow-ups that asked, through privd.
    awake: std::sync::Mutex<Option<Awake>>,
    /// Until when nothing keeps the machine awake, since a power step ran.
    going_until: AtomicU64,
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
            awake: std::sync::Mutex::new(None),
            going_until: AtomicU64::new(0),
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
    /// already set up. `signer` is the key that signed the plan, where the
    /// machine requires one.
    pub async fn start(
        &self,
        plan: Plan,
        actor: Option<Actor>,
        signer: Option<String>,
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
            signer,
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

    /// Changes a follow-up while it waits (`followup.change`): the same job,
    /// a new plan, checked as a start's is. New lines, minutes or finish
    /// start the quiet count over; the same plan again answers as the change
    /// already made.
    pub async fn change(
        &self,
        plan: Plan,
        actor: Option<Actor>,
        signer: Option<String>,
        policy: &PolicyState,
    ) -> Result<Started, CallError> {
        check_plan(&plan, policy, now_ms())?;
        let (id, scope, recount) = {
            let mut saved = self.saved.lock().await;
            let kept = saved
                .followups
                .iter_mut()
                .find(|kept| kept.plan.id == plan.id && kept.phase != Phase::Over)
                .ok_or_else(|| CallError::new(ErrorCode::NotFound, "no such follow-up"))?;
            if kept.plan == plan {
                return Ok(Started {
                    id: plan.id,
                    scope: kept.scope.clone(),
                });
            }
            let recount = apply_change(kept, plan, actor, signer)?;
            let changed = (kept.plan.id.clone(), kept.scope.clone(), recount);
            save(&self.path, &saved).await;
            self.publish(&saved);
            changed
        };
        self.record(&id, now_ms(), Event::Changed { recount }).await;
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

    /// Skips a step that asks, counts down or waits (`followup.skip`).
    pub async fn skip(&self, id: &str, step: u32) -> Result<(), CallError> {
        self.answer(id, step, |pending| match pending {
            Pending::Fresh
            | Pending::Asking { .. }
            | Pending::CountingDown { .. }
            | Pending::Reaching { .. }
            | Pending::WaitingFor => Ok(Pending::Skipped),
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
        let sample = sample(saved, None, now_ms(), self.keeping_awake());
        self.latest.send_replace(Some(Arc::new(sample)));
    }

    /// Whether the machine is kept awake for the follow-ups now.
    fn keeping_awake(&self) -> bool {
        self.awake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(Awake::held)
    }

    /// Lets the machine sleep again: before a power step, which logind's
    /// inhibitor would hold off too, and once nothing asks to keep it awake.
    fn let_sleep(&self) {
        let awake = self
            .awake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(awake);
    }
}

/// A change to a waiting follow-up, applied: whether its quiet count starts
/// over. Its job can't change; once the job is done, only skipping or
/// cancelling is left.
fn apply_change(
    kept: &mut Kept,
    plan: Plan,
    actor: Option<Actor>,
    signer: Option<String>,
) -> Result<bool, CallError> {
    if kept.phase != Phase::Waiting {
        return Err(bad(
            "its job is done already; skip or cancel what's left instead",
        ));
    }
    if plan.watch != kept.plan.watch {
        return Err(bad(
            "a follow-up watches one job; set up another for this one",
        ));
    }
    let recount = plan.finish != kept.plan.finish;
    if recount {
        kept.quiet_minutes = 0;
        kept.minute = None;
    }
    kept.plan = plan;
    kept.actor = actor;
    kept.signer = signer;
    Ok(recount)
}

/// The machine kept awake for follow-ups: privd holds it while this
/// connection is open, and answers again only to end it.
struct Awake {
    held: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Awake {
    async fn take(privd: &Path) -> Result<Self, String> {
        let stream = os::connect(privd)
            .await
            .map_err(|e| format!("can't reach the agent's root helper: {e}"))?;
        let mut channel = ipc::channel(stream);
        let request = Request {
            id: 1,
            call: Call::KeepAwake {
                reason: AWAKE_REASON.to_owned(),
            },
        };
        ipc::send(&mut channel, &request).await?;
        let first = tokio::time::timeout(READ_LIMIT, ipc::receive::<_, Response>(&mut channel))
            .await
            .map_err(|_| "the agent's root helper didn't answer".to_owned())??;
        match first {
            Some(Response { err: Some(e), .. }) => return Err(e),
            Some(_) => {}
            None => return Err("the agent's root helper hung up".to_owned()),
        }
        let held = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&held);
        let task = tokio::spawn(async move {
            if let Ok(Some(Response { err: Some(e), .. })) =
                ipc::receive::<_, Response>(&mut channel).await
            {
                info!("the machine may sleep again: {e}");
            }
            flag.store(false, Ordering::Relaxed);
        });
        Ok(Self { held, task })
    }

    fn held(&self) -> bool {
        self.held.load(Ordering::Relaxed)
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        // The connection closes with the task, and privd lets the machine
        // sleep again.
        self.task.abort();
    }
}

/// The minute so far, for the topic: from the running totals where they
/// cover enough of it, or the table's samples.
struct Live<'a> {
    sums: &'a HashMap<String, Sums>,
    meters: &'a HashMap<String, Meter>,
}

impl Live<'_> {
    fn so_far(&self, kept: &Kept) -> Option<Minute> {
        let (id, finish) = (&kept.plan.id, &kept.plan.finish);
        self.meters
            .get(id)
            .and_then(|meter| meter.minute(finish))
            .or_else(|| self.sums.get(id).and_then(|sums| sums.minute(finish)))
    }
}

/// The `followups` topic's sample: each follow-up, with the minute so far
/// where the task has one.
fn sample(saved: &Saved, live: Option<&Live<'_>>, now: u64, keeping: bool) -> FollowUpsSample {
    let waiting = waiting_ids(saved);
    let followups = saved
        .followups
        .iter()
        .filter(|kept| kept.phase != Phase::Over)
        .map(|kept| FollowUpState {
            id: kept.plan.id.clone(),
            scope: kept.scope.clone(),
            since: kept.since,
            stage: stage_of(kept, &waiting),
            minute: live
                .and_then(|live| live.so_far(kept))
                .or_else(|| kept.minute.clone()),
            quiet_minutes: kept.quiet_minutes,
            keeping_awake: keeping && kept.plan.keep_awake,
        })
        .collect();
    FollowUpsSample { ts: now, followups }
}

fn stage_of(kept: &Kept, waiting: &[String]) -> Stage {
    match kept.phase {
        Phase::Waiting | Phase::Over => Stage::Waiting,
        Phase::Stepping { next, pending } => match pending {
            Pending::Asking { .. } => Stage::Asking { step: next },
            Pending::CountingDown { until, .. } => Stage::CountingDown { step: next, until },
            Pending::Reaching { tries, .. } => Stage::Reaching { step: next, tries },
            Pending::WaitingFor => Stage::WaitingFor {
                step: next,
                followups: Around { heard: 0, waiting }.others(&kept.plan.id),
            },
            Pending::Fresh | Pending::Due | Pending::Skipped => Stage::Running { step: next },
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
fn judge_minute(kept: &mut Kept, minute: Option<Minute>) -> Option<DoneWhy> {
    if kept.phase != Phase::Waiting {
        return None;
    }
    // A minute with no reading doesn't count either way.
    let minute = minute?;
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
fn done(kept: &mut Kept, why: DoneWhy, now: u64, around: Around<'_>) -> Vec<Effect> {
    kept.phase = Phase::Stepping {
        next: 0,
        pending: Pending::Fresh,
    };
    let mut effects = vec![Effect::Record(Event::Done { why })];
    effects.extend(advance(kept, now, around));
    effects
}

/// How long an ask-first step waits for an answer: until the deadline while
/// it's still ahead, and 12 hours without one, or once the deadline is what
/// started the steps.
fn ask_until(kept: &Kept, now: u64) -> u64 {
    match &kept.plan.deadline {
        Some(deadline) if deadline.at > now => deadline.at.max(now + MINUTE_MS),
        _ => now + u64::from(followup::ASK_HOURS) * HOUR_MS,
    }
}

/// Moves through the steps as far as they go without waiting: messages go
/// out, skipped steps pass, a power step waits for the other jobs, an acting
/// step starts its countdown or asks, one the machine slept through counts
/// down again once Console has heard from it, and one that's due is run.
/// Ends the follow-up after its last step.
fn advance(kept: &mut Kept, now: u64, around: Around<'_>) -> Vec<Effect> {
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
        let power = matches!(step.what, StepAction::Power { .. });
        let others = if power {
            around.others(&kept.plan.id)
        } else {
            Vec::new()
        };
        match pending {
            // The machine going would end the other jobs.
            Pending::Fresh | Pending::WaitingFor if !others.is_empty() => {
                if pending == Pending::Fresh {
                    effects.push(Effect::Record(Event::WaitingFor {
                        step: next,
                        followups: others,
                    }));
                }
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::WaitingFor,
                };
                return effects;
            }
            Pending::WaitingFor => {
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::Fresh,
                };
            }
            Pending::Fresh if step.ask => {
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::Asking {
                        until: ask_until(kept, now),
                    },
                };
                effects.push(Effect::Record(Event::Asking { step: next }));
                return effects;
            }
            Pending::Fresh => {
                let until = now + u64::from(followup::COUNTDOWN_SECONDS) * 1_000;
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::CountingDown { until, tries: 0 },
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
            Pending::CountingDown { until, .. } if now >= until => {
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::Due,
                };
            }
            // Console has heard from the machine since it woke: the person
            // can see the countdown, and stop it.
            Pending::Reaching { woke, tries, .. } if around.heard >= woke => {
                let until = now + u64::from(followup::COUNTDOWN_SECONDS) * 1_000;
                kept.phase = Phase::Stepping {
                    next,
                    pending: Pending::CountingDown { until, tries },
                };
                effects.push(Effect::Record(Event::CountingDown { step: next, until }));
                return effects;
            }
            Pending::Reaching {
                woke,
                tries,
                next: again,
            } if now >= again => {
                if tries >= followup::REACH_TRIES {
                    kept.phase = Phase::Over;
                    effects.push(Effect::Record(Event::Ended {
                        why: EndedWhy::Unreachable,
                    }));
                } else {
                    kept.phase = Phase::Stepping {
                        next,
                        pending: Pending::Reaching {
                            woke,
                            tries: tries + 1,
                            next: now + MINUTE_MS,
                        },
                    };
                }
                return effects;
            }
            Pending::Asking { .. } | Pending::CountingDown { .. } | Pending::Reaching { .. } => {
                return effects;
            }
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

/// The machine slept, or the agent was stopped, until `now`: a countdown it
/// slept through doesn't run when it wakes, as someone may have just woken
/// it. It starts over once Console can reach the machine, so the person can
/// see it and stop it; the wake is the first try.
fn woke(kept: &mut Kept, now: u64) -> Vec<Effect> {
    let Phase::Stepping { next, pending } = kept.phase else {
        return Vec::new();
    };
    let (tries, first) = match pending {
        Pending::CountingDown { tries, .. } => (tries, true),
        Pending::Reaching { tries, .. } => (tries, false),
        _ => return Vec::new(),
    };
    kept.phase = Phase::Stepping {
        next,
        pending: Pending::Reaching {
            woke: now,
            tries: tries + 1,
            next: now + MINUTE_MS,
        },
    };
    if first {
        vec![Effect::Record(Event::Slept { step: next })]
    } else {
        Vec::new()
    }
}

/// A step ran: its outcome on the record, then on to the next.
fn finish_step(
    kept: &mut Kept,
    step: u32,
    outcome: Outcome,
    now: u64,
    around: Around<'_>,
) -> Vec<Effect> {
    let mut effects = vec![Effect::Record(Event::Step { step, outcome })];
    if let Phase::Stepping { next, .. } = kept.phase
        && next == step
    {
        kept.phase = Phase::Stepping {
            next: next + 1,
            pending: Pending::Fresh,
        };
        effects.extend(advance(kept, now, around));
    }
    effects
}

/// Where the machine stands on signatures, as far as a follow-up goes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Required {
    No,
    /// These keys are trusted.
    Keys(HashSet<String>),
    /// It can't read whom it trusts.
    Unreadable(String),
}

impl Required {
    fn of(pin: &signers::Pin) -> Self {
        match pin {
            signers::Pin::Off => Self::No,
            signers::Pin::On(trust) => Self::Keys(trust.signers.keys().cloned().collect()),
            signers::Pin::Broken(reason) => Self::Unreadable(reason.clone()),
        }
    }
}

/// Whether a follow-up may still act (D121: "we prioritize security"): where
/// the machine requires signatures, only while the key that signed its plan
/// is still trusted. One set up unsigned before signatures were required, or
/// whose key has been removed since, has its steps refused.
fn still_trusted(required: &Required, signer: Option<&str>) -> Result<(), String> {
    match (required, signer) {
        (Required::No, _) => Ok(()),
        (Required::Unreadable(reason), _) => Err(format!(
            "this machine requires signed commands, and can't read whom it trusts: {reason}"
        )),
        (Required::Keys(keys), Some(key)) if keys.contains(key) => Ok(()),
        (Required::Keys(_), Some(_)) => {
            Err("the key that signed this follow-up isn't trusted any more".to_owned())
        }
        (Required::Keys(_), None) => Err(
            "this machine requires signed commands now, and this follow-up was set up without a signature"
                .to_owned(),
        ),
    }
}

/// What woke the task.
enum Look {
    Tick,
    Table,
    Engine,
    Changed,
}

/// Watches the follow-ups until shutdown.
pub async fn run(
    followups: Arc<FollowUps>,
    processes: Arc<processes::Latest>,
    containers: Arc<containers::Latest>,
    uplink: Arc<Uplink>,
    token: CancellationToken,
) -> Result<(), String> {
    end_other_boots(&followups).await;
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + TICK, TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut ticks: u32 = 0;
    let mut sums: HashMap<String, Sums> = HashMap::new();
    let mut meters: HashMap<String, Meter> = HashMap::new();
    let mut tables: Option<watch::Receiver<Option<Arc<processes::Table>>>> = None;
    let mut engines: Option<watch::Receiver<Option<Arc<ContainersSample>>>> = None;
    // What was present at the last reading, by follow-up.
    let mut present: HashMap<String, bool> = HashMap::new();
    let mut last_look = now_ms();
    let mut awake_retry = 0;
    let mut usage_error: Option<String> = None;
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
        let look = tokio::select! {
            _ = ticker.tick() => Look::Tick,
            Some(()) = next_change(&mut tables) => Look::Table,
            Some(()) = next_change(&mut engines) => Look::Engine,
            () = followups.changed.notified() => Look::Changed,
            () = token.cancelled() => return Ok(()),
        };
        let now = now_ms();
        if now.saturating_sub(last_look) > SLEPT_AFTER_MS {
            // What the minute had before the sleep isn't this minute's, and
            // time asleep never counts as quiet.
            sums.clear();
            meters.clear();
            let effects = {
                let mut saved = followups.saved.lock().await;
                let effects: Vec<(String, Effect)> = saved
                    .followups
                    .iter_mut()
                    .flat_map(|kept| {
                        let id = kept.plan.id.clone();
                        woke(kept, now)
                            .into_iter()
                            .map(move |effect| (id.clone(), effect))
                    })
                    .collect();
                if saved.followups.iter().any(|kept| {
                    matches!(
                        kept.phase,
                        Phase::Stepping {
                            pending: Pending::Reaching { .. },
                            ..
                        }
                    )
                }) {
                    save(&followups.path, &saved).await;
                }
                effects
            };
            if !effects.is_empty() {
                info!(
                    "the machine slept through a follow-up's countdown; it counts down again once Console can reach it"
                );
            }
            carry_out(&followups, effects, uplink.heard_ms()).await;
        }
        match look {
            Look::Tick => {
                ticks = ticks.wrapping_add(1);
                let minute_ends = ticks.is_multiple_of(TICKS_PER_MINUTE);
                let table = tables
                    .as_ref()
                    .and_then(|receiver| receiver.borrow().clone());
                if let Some(table) = table {
                    look_at_totals(&followups, &table, &mut meters, &mut usage_error).await;
                }
                let now = now_ms();
                let effects = {
                    let mut saved = followups.saved.lock().await;
                    let waiting = waiting_ids(&saved);
                    let around = Around {
                        heard: uplink.heard_ms(),
                        waiting: &waiting,
                    };
                    let mut effects = Vec::new();
                    for kept in &mut saved.followups {
                        let id = kept.plan.id.clone();
                        let mut found = Vec::new();
                        if let Some(why) =
                            judge_exit(kept, present.get(&id).copied().unwrap_or(true))
                        {
                            found.extend(done(kept, why, now, around));
                        }
                        match judge_deadline(kept, now) {
                            Some(AtDeadline::RunSteps) => {
                                found.extend(done(kept, DoneWhy::Deadline, now, around))
                            }
                            Some(AtDeadline::GiveUp | AtDeadline::Unknown) => {
                                kept.phase = Phase::Over;
                                found.push(Effect::Record(Event::Ended {
                                    why: EndedWhy::GaveUp,
                                }));
                            }
                            None => {}
                        }
                        if minute_ends {
                            let finish = kept.plan.finish.clone();
                            let minute = meters
                                .get(&id)
                                .and_then(|meter| meter.minute(&finish))
                                .or_else(|| sums.get(&id).and_then(|sums| sums.minute(&finish)));
                            if let Some(why) = judge_minute(kept, minute) {
                                found.extend(done(kept, why, now, around));
                            }
                            if let Some(meter) = meters.get_mut(&id) {
                                meter.next_minute();
                            }
                        }
                        // Countdowns and questions that ran out.
                        found.extend(advance(kept, now, around));
                        effects.extend(found.into_iter().map(|effect| (id.clone(), effect)));
                    }
                    if minute_ends {
                        sums.clear();
                    }
                    meters.retain(|id, _| {
                        saved
                            .followups
                            .iter()
                            .any(|kept| kept.plan.id == *id && kept.phase == Phase::Waiting)
                    });
                    // The quiet count is kept each minute, so an agent that
                    // restarts carries on counting.
                    if !effects.is_empty() || (minute_ends && !saved.followups.is_empty()) {
                        save(&followups.path, &saved).await;
                    }
                    effects
                };
                carry_out(&followups, effects, uplink.heard_ms()).await;
                keep_awake(&followups, now_ms(), &mut awake_retry, minute_ends).await;
                let saved = followups.saved.lock().await;
                let live = Live {
                    sums: &sums,
                    meters: &meters,
                };
                let sample = sample(&saved, Some(&live), now_ms(), followups.keeping_awake());
                followups.latest.send_replace(Some(Arc::new(sample)));
            }
            Look::Table => {
                if let Some(table) = tables
                    .as_ref()
                    .and_then(|receiver| receiver.borrow().clone())
                {
                    let saved = followups.saved.lock().await;
                    for kept in saved
                        .followups
                        .iter()
                        .filter(|kept| kept.phase == Phase::Waiting)
                    {
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
            Look::Engine => {
                if let Some(engine) = engines
                    .as_ref()
                    .and_then(|receiver| receiver.borrow().clone())
                {
                    let cores = std::thread::available_parallelism()
                        .map_or(1.0, |n| f64::from(u32::try_from(n.get()).unwrap_or(1)));
                    let saved = followups.saved.lock().await;
                    for kept in saved
                        .followups
                        .iter()
                        .filter(|kept| kept.phase == Phase::Waiting)
                    {
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
            Look::Changed => {
                let now = now_ms();
                let effects = {
                    let mut saved = followups.saved.lock().await;
                    let waiting = waiting_ids(&saved);
                    let around = Around {
                        heard: uplink.heard_ms(),
                        waiting: &waiting,
                    };
                    let effects: Vec<(String, Effect)> = saved
                        .followups
                        .iter_mut()
                        .flat_map(|kept| {
                            let id = kept.plan.id.clone();
                            advance(kept, now, around)
                                .into_iter()
                                .map(move |effect| (id.clone(), effect))
                        })
                        .collect();
                    if !effects.is_empty() {
                        save(&followups.path, &saved).await;
                    }
                    effects
                };
                carry_out(&followups, effects, uplink.heard_ms()).await;
                keep_awake(&followups, now_ms(), &mut awake_retry, false).await;
                let saved = followups.saved.lock().await;
                let live = Live {
                    sums: &sums,
                    meters: &meters,
                };
                let sample = sample(&saved, Some(&live), now_ms(), followups.keeping_awake());
                followups.latest.send_replace(Some(Arc::new(sample)));
            }
        }
        // After the work, so the time a step takes isn't taken for sleep.
        last_look = now_ms();
    }
}

/// Reads the running totals of each waiting job's processes through privd,
/// and adds what each job used since the last look to its minute. A job whose
/// totals can't be read is judged by the table's samples instead.
async fn look_at_totals(
    followups: &FollowUps,
    table: &processes::Table,
    meters: &mut HashMap<String, Meter>,
    usage_error: &mut Option<String>,
) {
    type Member = (u32, u64, Option<u32>);
    let jobs: Vec<(String, Vec<Member>)> = {
        let saved = followups.saved.lock().await;
        saved
            .followups
            .iter()
            .filter(|kept| {
                kept.phase == Phase::Waiting && !matches!(kept.plan.watch, Watch::Container { .. })
            })
            .map(|kept| {
                let job = members(&kept.scope, &table.processes)
                    .iter()
                    .map(|process| (process.pid, process.started, process.parent))
                    .collect();
                (kept.plan.id.clone(), job)
            })
            .collect()
    };
    let all: HashSet<(u32, u64)> = jobs
        .iter()
        .flat_map(|(_, job)| job.iter().map(|&(pid, started, _)| (pid, started)))
        .collect();
    if all.is_empty() {
        return;
    }
    let used = match processes::usage(&followups.privd, all.into_iter().collect()).await {
        Ok(used) => {
            *usage_error = None;
            used
        }
        Err(e) => {
            if usage_error.as_deref() != Some(e.as_str()) {
                warn!("can't read a follow-up's process totals; the table's samples judge it: {e}");
                *usage_error = Some(e);
            }
            return;
        }
    };
    let used: HashMap<(u32, u64), processes::Used> = used
        .into_iter()
        .map(|used| ((used.pid, used.started), used))
        .collect();
    let at = now_ms();
    for (id, job) in jobs {
        let processes = job
            .iter()
            .filter_map(|&(pid, started, parent)| {
                let used = used.get(&(pid, started))?;
                let counted = Counted {
                    parent,
                    cpu_ms: used.cpu_ms,
                    children_cpu_ms: used.children_cpu_ms,
                    disk: used.disk_bytes,
                };
                Some(((pid, started), counted))
            })
            .collect();
        meters
            .entry(id)
            .or_default()
            .look(Reading { at, processes });
    }
}

/// Keeps the machine awake while a follow-up that asked still has something
/// to do and the key that signed its plan is still trusted, and lets it sleep
/// otherwise. A hold that can't be taken is tried again a minute later;
/// `recheck` looks at the signatures again while it's held.
async fn keep_awake(followups: &FollowUps, now: u64, retry_at: &mut u64, recheck: bool) {
    if now < followups.going_until.load(Ordering::Relaxed) {
        followups.let_sleep();
        return;
    }
    let wanted: Vec<Option<String>> = {
        let saved = followups.saved.lock().await;
        saved
            .followups
            .iter()
            .filter(|kept| kept.plan.keep_awake && kept.phase != Phase::Over)
            .map(|kept| kept.signer.clone())
            .collect()
    };
    let held = followups.keeping_awake();
    if wanted.is_empty() {
        followups.let_sleep();
        return;
    }
    if (held && !recheck) || (!held && now < *retry_at) {
        return;
    }
    let required = Required::of(&uplink::signers_view(&followups.privd).await.pin());
    let trusted = wanted
        .iter()
        .any(|signer| still_trusted(&required, signer.as_deref()).is_ok());
    if !trusted {
        followups.let_sleep();
        return;
    }
    if held {
        return;
    }
    // A hold privd ended, as when the policy stopped allowing it, goes first.
    followups.let_sleep();
    match Awake::take(&followups.privd).await {
        Ok(awake) => {
            *followups
                .awake
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(awake);
        }
        Err(e) => {
            warn!("can't keep the machine awake for a follow-up: {e}");
            *retry_at = now + MINUTE_MS;
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
async fn carry_out(followups: &FollowUps, mut effects: Vec<(String, Effect)>, heard: u64) {
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
                        // logind's inhibitor would hold off the step itself.
                        followups.let_sleep();
                        // Gone from the saved list before the machine goes, so
                        // a restart can't run it twice; the records follow
                        // within privd's short delay.
                        remove(followups, &id).await;
                    }
                    let outcome = run_step(followups, &kept, step).await;
                    let now = now_ms();
                    if power {
                        if matches!(outcome, Outcome::Ran { .. }) {
                            followups
                                .going_until
                                .store(now + GOING_MS, Ordering::Relaxed);
                        }
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
                    let waiting = waiting_ids(&saved);
                    let around = Around {
                        heard,
                        waiting: &waiting,
                    };
                    if let Some(kept) = saved.followups.iter_mut().find(|kept| kept.plan.id == id) {
                        more.extend(
                            finish_step(kept, step, outcome, now, around)
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
/// as the person who set the follow-up up. A step that acts first checks the
/// key that signed the plan is still trusted where the machine requires
/// signatures.
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
    if what.acts() {
        let required = Required::of(&uplink::signers_view(privd).await.pin());
        if let Err(reason) = still_trusted(&required, kept.signer.as_deref()) {
            return Outcome::Refused { reason };
        }
    }
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
            return match ipc::call_within(privd, call, STEP_LIMIT).await {
                Ok(value) => match serde_json::from_value::<AppQuitResult>(value) {
                    Ok(AppQuitResult {
                        result: QuitResult::NotOpen,
                        ..
                    }) => Outcome::Skipped {
                        why: SkippedWhy::NothingToDo,
                    },
                    Ok(AppQuitResult {
                        result: QuitResult::StillOpen,
                        ..
                    }) => Outcome::Failed {
                        reason: "it's still open: it may be asking to save changes".to_owned(),
                    },
                    _ => Outcome::Ran { processes: None },
                },
                Err(e) => outcome_of(e),
            };
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
            signer: None,
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

    fn power(action: PowerAction) -> StepAction {
        StepAction::Power {
            action,
            skip_if_in_use: false,
        }
    }

    /// A minute's readings against the default lines: 6% CPU, no disk line.
    fn read(cpu: f64) -> Option<Minute> {
        Some(minute_of(cpu, None, &quiet(5)))
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

    fn counted(parent: Option<u32>, cpu_ms: u64, children: u64, disk: u64) -> Counted {
        Counted {
            parent,
            cpu_ms,
            children_cpu_ms: Some(children),
            disk: Some(disk),
        }
    }

    fn reading(at: u64, processes: &[((u32, u64), Counted)]) -> Reading {
        Reading {
            at,
            processes: processes.iter().copied().collect(),
        }
    }

    const ALONE: Around<'static> = Around {
        heard: 0,
        waiting: &[],
    };

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
        assert_eq!(judge_minute(&mut kept, read(2.0)), None);
        assert_eq!(judge_minute(&mut kept, read(2.0)), None);
        assert_eq!(judge_minute(&mut kept, read(40.0)), None);
        assert_eq!(kept.quiet_minutes, 0);
        // A minute with no reading counts neither way.
        assert_eq!(judge_minute(&mut kept, None), None);
        assert_eq!(judge_minute(&mut kept, read(1.0)), None);
        assert_eq!(judge_minute(&mut kept, read(1.0)), None);
        assert_eq!(judge_minute(&mut kept, read(1.0)), Some(DoneWhy::Quiet));
    }

    #[test]
    fn disk_over_its_line_keeps_a_quiet_cpu_busy() {
        let finish = Finish::Quiet {
            minutes: 1,
            cpu: 6.0,
            disk: Some(1_000),
        };
        let mut kept = kept(vec![], finish.clone());
        let mut busy = Sums::default();
        busy.add(1.0, Some(50_000.0));
        assert_eq!(judge_minute(&mut kept, busy.minute(&finish)), None);
        let mut idle = Sums::default();
        idle.add(1.0, Some(10.0));
        assert_eq!(
            judge_minute(&mut kept, idle.minute(&finish)),
            Some(DoneWhy::Quiet)
        );
    }

    #[test]
    fn short_tasks_count_once_whoever_waits_for_them() {
        // T3 Code (100) and a build under it (101); at the second look the
        // build has ended, a short task (700 ms) came and went, and a new
        // helper (102) has started. Over the 15 s between: T3 Code 100 ms,
        // the build's last 300 ms, the short task's 700 ms, the helper's 400.
        let before = reading(
            20_000,
            &[
                ((100, 10), counted(None, 1_000, 500, 10_000)),
                ((101, 11), counted(Some(100), 2_000, 0, 4_000)),
            ],
        );
        let linux = reading(
            35_000,
            &[
                // Linux takes the waited-for children's time and bytes in.
                ((100, 10), counted(None, 1_100, 500 + 2_300 + 700, 17_500)),
                ((102, 30), counted(Some(100), 400, 0, 600)),
            ],
        );
        assert_eq!(used(&before, &linux, true), (1_500, Some(4_100)));
        // macOS takes children's time in, but not their bytes: the build's
        // last 500 bytes and the short task's go uncounted.
        let mac = reading(
            35_000,
            &[
                ((100, 10), counted(None, 1_100, 500 + 2_300 + 700, 11_000)),
                ((102, 30), counted(Some(100), 400, 0, 600)),
            ],
        );
        assert_eq!(used(&before, &mac, false), (1_500, Some(1_600)));
        // A parent that never waits takes nothing in, so nothing comes off it.
        let unwaited = reading(35_000, &[((100, 10), counted(None, 1_100, 500, 11_000))]);
        assert_eq!(used(&before, &unwaited, false), (100, Some(1_000)));
    }

    #[test]
    fn a_minute_comes_from_the_totals_when_they_cover_enough_of_it() {
        let finish = quiet(5);
        let mut meter = Meter::default();
        meter.look(reading(0, &[((100, 0), counted(None, 0, 0, 0))]));
        meter.look(reading(15_000, &[((100, 0), counted(None, 1_500, 0, 0))]));
        // 15 s isn't enough: the table's samples judge it.
        assert_eq!(meter.minute(&finish), None);
        meter.look(reading(
            30_000,
            &[((100, 0), counted(None, 3_000, 0, 30_000))],
        ));
        let minute = meter.minute(&finish).expect("a minute");
        assert!((minute.cpu - 10.0).abs() < 1e-9, "{minute:?}");
        assert_eq!(minute.disk, Some(1_000));
        assert!(minute.busy);
        meter.next_minute();
        assert_eq!(meter.minute(&finish), None);
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
        let effects = done(&mut kept, DoneWhy::Quiet, 0, ALONE);
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
        assert!(advance(&mut kept, 59_999, ALONE).is_empty());
        assert_eq!(advance(&mut kept, 60_000, ALONE), vec![Effect::Run(1)]);
        let finished = finish_step(
            &mut kept,
            1,
            Outcome::Ran { processes: Some(4) },
            61_000,
            ALONE,
        );
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
        done(&mut kept, DoneWhy::Exited, 0, ALONE);
        kept.phase = Phase::Stepping {
            next: 0,
            pending: Pending::Skipped,
        };
        assert_eq!(
            records(&advance(&mut kept, 10, ALONE)),
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
        let effects = done(&mut kept, DoneWhy::Quiet, 0, ALONE);
        assert_eq!(records(&effects)[1], Event::Asking { step: 0 });
        let hours = u64::from(followup::ASK_HOURS) * HOUR_MS;
        assert!(advance(&mut kept, hours - 1, ALONE).is_empty());
        assert_eq!(
            records(&advance(&mut kept, hours, ALONE)),
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
    fn asking_waits_for_a_deadline_ahead_and_twelve_hours_after_one() {
        let mut early = kept(vec![step(StepAction::Stop, true)], quiet(5));
        early.plan.deadline = Some(Deadline {
            at: 10 * MINUTE_MS,
            then: AtDeadline::GiveUp,
        });
        done(&mut early, DoneWhy::Quiet, 0, ALONE);
        assert_eq!(
            early.phase,
            Phase::Stepping {
                next: 0,
                pending: Pending::Asking {
                    until: 10 * MINUTE_MS
                }
            }
        );
        // The deadline started the steps, so it's behind them: 12 hours.
        let mut late = kept(vec![step(StepAction::Stop, true)], quiet(5));
        late.plan.deadline = Some(Deadline {
            at: 1_000,
            then: AtDeadline::RunSteps,
        });
        done(&mut late, DoneWhy::Deadline, 5_000, ALONE);
        let hours = u64::from(followup::ASK_HOURS) * HOUR_MS;
        assert_eq!(
            late.phase,
            Phase::Stepping {
                next: 0,
                pending: Pending::Asking {
                    until: 5_000 + hours
                }
            }
        );
    }

    #[test]
    fn an_approved_step_runs_at_once() {
        let mut kept = kept(vec![step(StepAction::Stop, true)], quiet(5));
        done(&mut kept, DoneWhy::Quiet, 0, ALONE);
        kept.phase = Phase::Stepping {
            next: 0,
            pending: Pending::Due,
        };
        assert_eq!(advance(&mut kept, 1, ALONE), vec![Effect::Run(0)]);
    }

    #[test]
    fn a_countdown_slept_through_waits_for_console_then_counts_down_again() {
        let mut kept = kept(vec![step(power(PowerAction::Poweroff), false)], quiet(5));
        done(&mut kept, DoneWhy::Quiet, 0, ALONE);
        // The machine sleeps mid-countdown and wakes an hour later.
        assert_eq!(
            records(&woke(&mut kept, HOUR_MS)),
            vec![Event::Slept { step: 0 }]
        );
        // Console last heard from it before it slept: nothing runs, though
        // the countdown's end has long passed.
        let unheard = Around {
            heard: 30_000,
            waiting: &[],
        };
        assert!(advance(&mut kept, HOUR_MS + 5_000, unheard).is_empty());
        // Console hears from it again: the countdown starts over.
        let heard = Around {
            heard: HOUR_MS + 8_000,
            waiting: &[],
        };
        let until = HOUR_MS + 10_000 + 60_000;
        assert_eq!(
            records(&advance(&mut kept, HOUR_MS + 10_000, heard)),
            vec![Event::CountingDown { step: 0, until }]
        );
        assert!(advance(&mut kept, until - 1, heard).is_empty());
        assert_eq!(advance(&mut kept, until, heard), vec![Effect::Run(0)]);
    }

    #[test]
    fn a_machine_that_cant_reach_console_ends_its_follow_up_after_three_tries() {
        let mut kept = kept(vec![step(power(PowerAction::Suspend), false)], quiet(5));
        done(&mut kept, DoneWhy::Quiet, 0, ALONE);
        woke(&mut kept, HOUR_MS);
        // The wake was the first try; a minute apart come the rest.
        for minute in 1..u64::from(followup::REACH_TRIES) {
            assert!(advance(&mut kept, HOUR_MS + minute * MINUTE_MS, ALONE).is_empty());
            assert!(matches!(
                stage_of(&kept, &[]),
                Stage::Reaching { tries, .. } if u64::from(tries) == minute + 1
            ));
        }
        let at = HOUR_MS + u64::from(followup::REACH_TRIES) * MINUTE_MS;
        assert_eq!(
            records(&advance(&mut kept, at, ALONE)),
            vec![Event::Ended {
                why: EndedWhy::Unreachable
            }]
        );
        assert_eq!(kept.phase, Phase::Over);
    }

    #[test]
    fn sleeping_again_through_a_second_countdown_uses_a_try() {
        let mut kept = kept(vec![step(StepAction::Stop, false)], quiet(5));
        done(&mut kept, DoneWhy::Quiet, 0, ALONE);
        woke(&mut kept, HOUR_MS);
        let heard = Around {
            heard: HOUR_MS + 1,
            waiting: &[],
        };
        advance(&mut kept, HOUR_MS + 2, heard);
        // It slept through that one too: on the record again, and a try.
        assert_eq!(
            records(&woke(&mut kept, 2 * HOUR_MS)),
            vec![Event::Slept { step: 0 }]
        );
        assert!(matches!(
            stage_of(&kept, &[]),
            Stage::Reaching { tries: 2, .. }
        ));
    }

    #[test]
    fn a_power_step_waits_for_the_other_jobs() {
        let steps = vec![
            step(StepAction::Notify, false),
            step(power(PowerAction::Poweroff), false),
        ];
        let mut kept = kept(steps, quiet(5));
        // As the pass began, this follow-up's job still ran, and another's does.
        let waiting = ["fu_1".to_owned(), "fu_2".to_owned()];
        let around = Around {
            heard: 0,
            waiting: &waiting,
        };
        let effects = done(&mut kept, DoneWhy::Quiet, 0, around);
        assert_eq!(
            records(&effects)[2],
            Event::WaitingFor {
                step: 1,
                followups: vec!["fu_2".to_owned()]
            }
        );
        assert_eq!(
            stage_of(&kept, &waiting),
            Stage::WaitingFor {
                step: 1,
                followups: vec!["fu_2".to_owned()]
            }
        );
        // Still waiting, with nothing new on the record.
        assert!(advance(&mut kept, 60_000, around).is_empty());
        // The other job is done: the power step counts down.
        assert_eq!(
            records(&advance(&mut kept, 70_000, ALONE)),
            vec![Event::CountingDown {
                step: 1,
                until: 130_000
            }]
        );
    }

    #[test]
    fn a_change_starts_the_quiet_count_over_only_for_new_lines() {
        let mut kept = kept(vec![], quiet(5));
        kept.quiet_minutes = 3;
        let mut plan = kept.plan.clone();
        plan.steps = vec![step(StepAction::Notify, false)];
        let signer = Some("sk_1".to_owned());
        assert_eq!(
            apply_change(&mut kept, plan.clone(), None, signer),
            Ok(false)
        );
        assert_eq!(kept.quiet_minutes, 3);
        assert_eq!(kept.signer.as_deref(), Some("sk_1"));
        plan.finish = quiet(10);
        assert_eq!(apply_change(&mut kept, plan.clone(), None, None), Ok(true));
        assert_eq!((kept.quiet_minutes, kept.signer.as_deref()), (0, None));
        // Another job is another follow-up.
        let mut other = plan.clone();
        other.watch = Watch::Process { pid: 7, started: 1 };
        assert!(apply_change(&mut kept, other, None, None).is_err());
        // Once the job is done, only a skip or a cancel.
        kept.phase = Phase::Stepping {
            next: 0,
            pending: Pending::Fresh,
        };
        assert!(apply_change(&mut kept, plan, None, None).is_err());
    }

    #[test]
    fn steps_act_only_while_their_key_is_trusted() {
        assert!(still_trusted(&Required::No, None).is_ok());
        let keys = Required::Keys(HashSet::from(["sk_1".to_owned()]));
        assert!(still_trusted(&keys, Some("sk_1")).is_ok());
        // Removed since, or never signed before signatures were required.
        assert!(still_trusted(&keys, Some("sk_gone")).is_err());
        assert!(still_trusted(&keys, None).is_err());
        let broken = Required::Unreadable("the pinned log doesn't check out".to_owned());
        assert!(still_trusted(&broken, Some("sk_1")).is_err());
        assert_eq!(Required::of(&signers::Pin::Off), Required::No);
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
        plan.steps = vec![step(power(PowerAction::Hibernate), false)];
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
