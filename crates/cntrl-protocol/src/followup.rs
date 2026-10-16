//! Follow-ups (D119, D121): a one-use plan for a job on a machine. Console sets
//! one up with `followup.start`; the machine watches the job, judges when it's
//! done, carries out the steps in order and reports each through the outbox,
//! so a report made offline reaches Console once the link is back. It never
//! fires twice, and it isn't an alert rule.
//!
//! CPU is a percentage of one core, as the process table gives it, summed over
//! the job's processes; disk is bytes a second, read and written together;
//! times are milliseconds since the Unix epoch unless a name says otherwise.

use serde::{Deserialize, Serialize};

use crate::power::PowerAction;
use crate::process::ProcessInfo;
use crate::service::ServiceScope;

/// Minutes in a row under every line that make a job done, as Console offers
/// by default.
pub const QUIET_MINUTES: u32 = 5;
/// How long a step that acts counts down before it runs, in seconds.
pub const COUNTDOWN_SECONDS: u32 = 60;
/// How long an ask-first step waits for an answer when the follow-up has no
/// deadline, in hours. Then the step is skipped.
pub const ASK_HOURS: u32 = 12;
/// Input this recent means someone's using the machine, in minutes.
pub const IN_USE_MINUTES: u32 = 5;
/// The most follow-ups a machine keeps at once; Console's plans allow fewer.
pub const MAX_FOLLOWUPS: usize = 10;
/// The longest a follow-up's ID may be.
pub const ID_MAX: usize = 64;

/// `followup.start`: what to watch, when it's done, what then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Plan {
    /// Chosen by Console (`fu_` and a ULID). Starting the same ID again
    /// answers with the follow-up already waiting, as a retry after a dropped
    /// link does; a different plan under it is refused.
    pub id: String,
    pub watch: Watch,
    pub finish: Finish,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<Deadline>,
    /// In order. Empty means it only reports that the job is done.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Keep the machine from sleeping until the follow-up ends; needs
    /// `power.inhibit`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_awake: bool,
}

/// What a follow-up watches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Watch {
    /// The app a process belongs to: the machine climbs from it to the app's
    /// top process ([`job_top`]) and watches that and everything under it.
    App { pid: u32, started: u64 },
    /// Exactly this process and everything it starts.
    Process { pid: u32, started: u64 },
    /// A container, by ID, its first 12 characters, or name.
    Container { id: String },
    #[serde(other)]
    Unknown,
}

/// When a follow-up's job is done.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Finish {
    /// When the job's top process, or the container, is gone.
    Exits,
    /// When the job has been under every line for `minutes` minutes in a row.
    /// A minute is busy if its CPU is over `cpu`, or its disk over `disk`;
    /// without `disk`, disk doesn't count. Starting processes alone doesn't
    /// make a minute busy, but the CPU of short ones the job ran does.
    Quiet {
        minutes: u32,
        cpu: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disk: Option<u64>,
    },
    #[serde(other)]
    Unknown,
}

/// A time to stop waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Deadline {
    pub at: u64,
    pub then: AtDeadline,
}

/// What happens at the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AtDeadline {
    /// Stop waiting and run nothing.
    GiveUp,
    /// Run the steps as if the job were done.
    RunSteps,
    #[serde(other)]
    Unknown,
}

/// One step, carried out when the job is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Step {
    /// What it does; `do` on the wire.
    #[serde(rename = "do")]
    pub what: StepAction,
    /// Wait for someone to approve it (`followup.approve`). Otherwise a step
    /// that acts counts down [`COUNTDOWN_SECONDS`] and runs unless skipped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask: bool,
}

/// What a step does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepAction {
    /// One message, which Console sends when the step's record arrives.
    Notify,
    /// Stops the job: its processes, asked to close and forced after a few
    /// seconds (on Windows they end at once), or the container.
    Stop,
    /// Asks a Mac app to quit, as the Dock asks, so it can save.
    QuitApp {
        app: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
    /// Stops or restarts a service.
    Service {
        unit: String,
        #[serde(default, skip_serializing_if = "ServiceScope::is_system")]
        scope: ServiceScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        action: StopOrRestart,
    },
    /// Stops or restarts a container.
    Container { id: String, action: StopOrRestart },
    /// Sleeps, hibernates, shuts down or restarts the machine.
    Power {
        action: PowerAction,
        /// Skip it while someone's using the machine, or when it can't tell.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        skip_if_in_use: bool,
    },
    #[serde(other)]
    Unknown,
}

/// Stop or restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StopOrRestart {
    Stop,
    Restart,
    #[serde(other)]
    Unknown,
}

impl StepAction {
    /// The capability the step needs when it runs, which the machine's policy
    /// must allow at setup and still allow then; none for a message. Stopping
    /// a container needs `containers.manage`, which the agent asks for when
    /// the follow-up watches one.
    pub fn capability(&self) -> Option<&'static str> {
        match self {
            Self::Notify | Self::Unknown => None,
            Self::Stop | Self::QuitApp { .. } => Some("processes.signal"),
            Self::Service { .. } => Some("services.manage"),
            Self::Container { .. } => Some("containers.manage"),
            Self::Power { action, .. } => Some(action.op()),
        }
    }

    /// Whether it changes the machine, and so counts down or asks first.
    pub fn acts(&self) -> bool {
        !matches!(self, Self::Notify)
    }
}

/// `followup.start`'s answer: the follow-up waiting, and what it watches as the
/// machine found it, which is what Console shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Started {
    pub id: String,
    pub scope: Scope,
}

/// What a follow-up watches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Scope {
    /// The top process's name, or the container's.
    pub name: String,
    /// The top process, for an app or a process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
    /// Processes in the job now, the top one included; 0 for a container.
    pub processes: u32,
}

/// `followup.approve` and `followup.skip`: one step of one follow-up, by its
/// place in the plan's steps, from 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StepRef {
    pub id: String,
    pub step: u32,
}

/// `followup.cancel`: one follow-up. Also the answer of approve, skip and
/// cancel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FollowUpRef {
    pub id: String,
}

/// The `followups` topic's parameters: none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FollowUpsParams {}

/// The `followups` topic: every follow-up the machine has, with where each one
/// is, sent as a subscription opens and every 15 s while it's open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FollowUpsSample {
    pub ts: u64,
    pub followups: Vec<FollowUpState>,
}

/// One follow-up as it stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FollowUpState {
    pub id: String,
    pub scope: Scope,
    /// When it was set up.
    pub since: u64,
    pub stage: Stage,
    /// The minute so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minute: Option<Minute>,
    /// Whole minutes in a row under every line.
    pub quiet_minutes: u32,
    /// Keeping the machine awake now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keeping_awake: bool,
}

/// Where a follow-up is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Stage {
    /// Watching the job.
    Waiting,
    /// Waiting for someone to approve or skip a step.
    Asking { step: u32 },
    /// A step runs at `until` unless someone skips it.
    CountingDown { step: u32, until: u64 },
    /// A step is running.
    Running { step: u32 },
    #[serde(other)]
    Unknown,
}

/// A minute's readings of the job, and whether they made it busy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Minute {
    /// Averaged over the minute's samples, the short processes it ran
    /// included where the OS keeps their time.
    pub cpu: f64,
    /// Where the machine can read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<u64>,
    pub busy: bool,
}

/// A `followup` outbox record: one thing that happened to a follow-up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FollowUpRecord {
    pub id: String,
    pub at: u64,
    pub event: Event,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Set up and watching.
    Started { scope: Scope },
    /// The job is done, or the deadline came and the steps run anyway.
    Done { why: DoneWhy },
    /// A step waits for someone to approve or skip it.
    Asking { step: u32 },
    /// A step runs at `until` unless someone skips it.
    CountingDown { step: u32, until: u64 },
    /// A step's outcome. A message's step is `ran` when Console should send it.
    Step { step: u32, outcome: Outcome },
    /// It's over: nothing more happens to it.
    Ended { why: EndedWhy },
    #[serde(other)]
    Unknown,
}

/// Why the job counts as done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DoneWhy {
    Exited,
    Quiet,
    /// The deadline came, and the plan said to run the steps anyway.
    Deadline,
    #[serde(other)]
    Unknown,
}

/// How a step went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Ran {
        /// Processes it stopped, for a stop.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        processes: Option<u32>,
    },
    Skipped {
        why: SkippedWhy,
    },
    /// The machine wouldn't: its policy no longer allows it, or the step's
    /// target is protected.
    Refused {
        reason: String,
    },
    Failed {
        reason: String,
    },
    #[serde(other)]
    Unknown,
}

/// Why a step didn't run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SkippedWhy {
    /// Someone skipped it.
    Skipped,
    /// Nobody approved it in time.
    NoAnswer,
    /// Someone was using the machine.
    InUse,
    /// The machine couldn't tell whether someone was using it.
    CantTell,
    /// There was nothing left to do: the job had already exited.
    NothingToDo,
    #[serde(other)]
    Unknown,
}

/// Why a follow-up ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EndedWhy {
    /// The job was done and every step had its turn.
    Completed,
    Cancelled,
    /// The deadline came, and the plan said to give up.
    GaveUp,
    /// The machine restarted while it waited, so nothing ran.
    Restarted,
    /// What it watched was gone before the follow-up started properly, as
    /// when a PID was reused.
    Gone,
    #[serde(other)]
    Unknown,
}

/// Names of programs that start others for people rather than being part of
/// one job: init and service managers, shells, terminals and multiplexers, the
/// desktop. Climbing from a process stops below them, so a helper finds its
/// app and `rustc` finds `cargo`, not the Terminal. Lowercase, without
/// Windows' `.exe`; codegen gives Console the same list.
pub const LAUNCHERS: &[&str] = &[
    // Init and service managers.
    "init",
    "systemd",
    "launchd",
    "runsv",
    "runsvdir",
    "s6-supervise",
    "supervisord",
    "tini",
    "dumb-init",
    "containerd-shim",
    "containerd-shim-runc-v2",
    "services",
    "svchost",
    "wininit",
    "winlogon",
    "smss",
    "csrss",
    "taskhostw",
    // Logins and privilege.
    "login",
    "sshd",
    "sshd-session",
    "mosh-server",
    "sudo",
    "su",
    "doas",
    // Shells.
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "tcsh",
    "csh",
    "nu",
    "pwsh",
    "powershell",
    "cmd",
    // Terminals and multiplexers.
    "terminal",
    "iterm2",
    "ghostty",
    "warp",
    "wezterm",
    "wezterm-gui",
    "alacritty",
    "kitty",
    "foot",
    "konsole",
    "xterm",
    "tilix",
    "gnome-terminal-server",
    "windowsterminal",
    "openconsole",
    "conhost",
    "tmux",
    "tmux: server",
    "screen",
    "zellij",
    // The desktop.
    "explorer",
    "gnome-shell",
    "plasmashell",
    "kwin_wayland",
    "kwin_x11",
];

/// Whether a process with this name starts others for people (see
/// [`LAUNCHERS`]).
pub fn is_launcher(name: &str) -> bool {
    let name = name.trim();
    let lower = name.to_ascii_lowercase();
    let bare = lower.strip_suffix(".exe").unwrap_or(&lower);
    LAUNCHERS.contains(&bare)
}

/// The top process of the job `pid` belongs to: climbing from it to its parent
/// while there is one in `table` that isn't a launcher and isn't PID 0 or 1.
/// `None` when `pid` isn't in the table.
pub fn job_top(table: &[ProcessInfo], pid: u32) -> Option<&ProcessInfo> {
    let find = |pid: u32| table.iter().find(|process| process.pid == pid);
    let mut current = find(pid)?;
    // A parent loop can't happen in a real table, but a bound costs nothing.
    for _ in 0..table.len() {
        let Some(parent) = current.parent.filter(|parent| *parent > 1).and_then(find) else {
            break;
        };
        if is_launcher(&parent.name) {
            break;
        }
        current = parent;
    }
    Some(current)
}

/// The job: `top` and every process under it, in `table`'s order.
pub fn job_members(table: &[ProcessInfo], top: u32) -> Vec<&ProcessInfo> {
    let mut members = std::collections::BTreeSet::from([top]);
    // Each pass adds the children of what's in already; a tree's depth bounds it.
    loop {
        let before = members.len();
        for process in table {
            if process
                .parent
                .is_some_and(|parent| members.contains(&parent))
            {
                members.insert(process.pid);
            }
        }
        if members.len() == before {
            break;
        }
    }
    table
        .iter()
        .filter(|process| members.contains(&process.pid))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent: Option<u32>, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent,
            name: name.to_owned(),
            user: None,
            cpu: 0.0,
            memory: 0,
            started: u64::from(pid),
            unit: None,
            kernel: false,
            system: false,
            protected: false,
        }
    }

    /// A Mac with T3 Code and its helpers, and a build in a terminal.
    fn table() -> Vec<ProcessInfo> {
        vec![
            process(1, None, "launchd"),
            process(100, Some(1), "T3 Code (Alpha)"),
            process(101, Some(100), "T3 Code (Alpha) Helper (Renderer)"),
            process(102, Some(100), "T3 Code (Alpha) Helper"),
            process(103, Some(102), "git"),
            process(200, Some(1), "Terminal"),
            process(201, Some(200), "login"),
            process(202, Some(201), "zsh"),
            process(203, Some(202), "cargo"),
            process(204, Some(203), "rustc"),
            process(300, Some(1), "explorer.exe"),
            process(301, Some(300), "T3 Code.exe"),
            process(302, Some(301), "T3 Code.exe"),
        ]
    }

    #[test]
    fn a_helper_finds_its_app() {
        let table = table();
        assert_eq!(job_top(&table, 103).map(|p| p.pid), Some(100));
        assert_eq!(job_top(&table, 101).map(|p| p.pid), Some(100));
    }

    #[test]
    fn a_build_stops_below_the_shell() {
        let table = table();
        assert_eq!(job_top(&table, 204).map(|p| p.pid), Some(203));
    }

    #[test]
    fn windows_names_match_without_exe_and_case() {
        let table = table();
        assert_eq!(job_top(&table, 302).map(|p| p.pid), Some(301));
        assert!(is_launcher("Explorer.EXE"));
        assert!(!is_launcher("T3 Code.exe"));
    }

    #[test]
    fn the_job_is_the_top_and_everything_under_it() {
        let table = table();
        let pids: Vec<u32> = job_members(&table, 100).iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![100, 101, 102, 103]);
        let build: Vec<u32> = job_members(&table, 203).iter().map(|p| p.pid).collect();
        assert_eq!(build, vec![203, 204]);
    }

    #[test]
    fn an_unknown_pid_has_no_job() {
        assert!(job_top(&table(), 999).is_none());
    }

    #[test]
    fn steps_name_their_capability() {
        assert_eq!(StepAction::Notify.capability(), None);
        assert_eq!(StepAction::Stop.capability(), Some("processes.signal"));
        let sleep = StepAction::Power {
            action: PowerAction::Suspend,
            skip_if_in_use: true,
        };
        assert_eq!(sleep.capability(), Some("power.suspend"));
        assert!(sleep.acts());
        assert!(!StepAction::Notify.acts());
    }
}
