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
    /// Restarts one systemd unit and waits for the job's result.
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
}
