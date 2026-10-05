//! Every operation and topic of protocol v1.

use crate::registry::{define_ops, define_topics};

define_ops! {
    /// The machine and its OS.
    SystemInfo = "system.info" {
        params: crate::system::NoParams,
        result: crate::system::SystemInfo,
        capability: "system.read",
        since: 1,
    },
    /// The services the service manager knows, system-wide and in users'
    /// sessions, with each one's state and whether the device policy protects it.
    ServiceList = "service.list" {
        params: crate::system::NoParams,
        result: crate::service::ServiceList,
        capability: "services.read",
        since: 1,
    },
    /// Asks an app open in a user's session to quit, or force-quits it, and
    /// waits for it to close.
    AppQuit = "app.quit" {
        params: crate::app::AppQuit,
        result: crate::app::AppQuitResult,
        capability: "processes.signal",
        since: 1,
    },
    /// Stops a process, if it's still the one that started at `started`:
    /// SIGTERM and a wait, or with `force`, SIGKILL.
    ProcessSignal = "process.signal" {
        params: crate::process::ProcessSignal,
        result: crate::process::ProcessSignalResult,
        capability: "processes.signal",
        since: 1,
    },
    /// Restarts one service, a systemd unit or on macOS a launchd job, and waits
    /// for the result.
    ServiceRestart = "service.restart" {
        params: crate::service::ServiceRef,
        result: crate::service::ServiceJob,
        capability: "services.manage",
        since: 1,
    },
    /// Starts one service and waits for the result. On a Mac a job that was
    /// stopped is loaded again first.
    ServiceStart = "service.start" {
        params: crate::service::ServiceRef,
        result: crate::service::ServiceJob,
        capability: "services.manage",
        since: 1,
    },
    /// Stops one service and waits for the result. On a Mac the job is
    /// unloaded, since launchd starts a kept-alive job again after a signal.
    ServiceStop = "service.stop" {
        params: crate::service::ServiceRef,
        result: crate::service::ServiceJob,
        capability: "services.manage",
        since: 1,
    },
    /// Makes one service start at boot; on a Mac it also loads it now.
    ServiceEnable = "service.enable" {
        params: crate::service::ServiceRef,
        result: crate::service::ServiceJob,
        capability: "services.manage",
        since: 1,
    },
    /// Stops one service starting at boot; on a Mac it also unloads it now.
    ServiceDisable = "service.disable" {
        params: crate::service::ServiceRef,
        result: crate::service::ServiceJob,
        capability: "services.manage",
        since: 1,
    },
    /// What the machine can do about power, what an action would interrupt and
    /// whether the machine comes back after it.
    PowerInfo = "power.info" {
        params: crate::system::NoParams,
        result: crate::power::PowerInfo,
        capability: "power.read",
        since: 1,
    },
    /// Restarts the machine. It answers, then restarts a moment later.
    PowerReboot = "power.reboot" {
        params: crate::system::NoParams,
        result: crate::power::PowerStarted,
        capability: "power.reboot",
        since: 1,
    },
    /// Shuts the machine down. It answers, then shuts down a moment later.
    PowerPoweroff = "power.poweroff" {
        params: crate::system::NoParams,
        result: crate::power::PowerStarted,
        capability: "power.poweroff",
        since: 1,
    },
    /// Puts the machine to sleep. It answers, then sleeps a moment later.
    PowerSuspend = "power.suspend" {
        params: crate::system::NoParams,
        result: crate::power::PowerStarted,
        capability: "power.suspend",
        since: 1,
    },
    /// Hibernates the machine, where it can. It answers, then hibernates a
    /// moment later.
    PowerHibernate = "power.hibernate" {
        params: crate::system::NoParams,
        result: crate::power::PowerStarted,
        capability: "power.hibernate",
        since: 1,
    },
}

define_topics! {
    /// Live host stats, sampled only while someone is subscribed.
    Stats = "stats" {
        params: crate::stats::StatsParams,
        event: crate::stats::StatsSample,
        capability: "system.read",
        since: 1,
    },
    /// The process table, highest CPU or memory first, read every 2 s only
    /// while someone is subscribed.
    Processes = "processes" {
        params: crate::process::ProcessesParams,
        event: crate::process::ProcessesSample,
        capability: "processes.read",
        since: 1,
    },
    /// A service's or the system's log: the latest lines, then new ones as
    /// they come, only while someone is subscribed (angle 11).
    Logs = "logs" {
        params: crate::logs::LogsParams,
        event: crate::logs::LogsBatch,
        capability: "logs.read",
        since: 1,
    },
}
