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
}
