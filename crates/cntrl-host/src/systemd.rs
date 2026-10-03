//! systemd over D-Bus, for the `service.*` operations. It needs the system bus,
//! and root to act on units.

use cntrl_protocol::service::{JobResult, ServiceKind, ServiceScope, ServiceState, ServiceStatus};
use futures_util::StreamExt;
use zbus_systemd::systemd1::ManagerProxy;

use crate::HostError;

/// A connection to systemd's manager on the system bus.
pub struct Systemd {
    manager: ManagerProxy<'static>,
}

impl Systemd {
    pub async fn connect() -> Result<Self, HostError> {
        let connection = zbus::Connection::system().await.map_err(failed)?;
        let manager = ManagerProxy::new(&connection).await.map_err(failed)?;
        Ok(Self { manager })
    }

    /// Restarts `unit` and waits for systemd's verdict on the job. Dropping the
    /// future stops the wait, not the job.
    pub async fn restart(&self, unit: &str) -> Result<JobResult, HostError> {
        // Listen before asking, so a quick job's result can't slip past.
        let mut removed = self.manager.receive_job_removed().await.map_err(failed)?;
        // systemd sends job signals only to clients that subscribed. A repeat
        // subscription is refused, which is harmless.
        let _ = self.manager.subscribe().await;
        let job = self
            .manager
            .restart_unit(unit.to_owned(), "replace".to_owned())
            .await
            .map_err(|e| unit_error(unit, e))?;
        while let Some(signal) = removed.next().await {
            let Ok(args) = signal.args() else { continue };
            if *args.job() == job {
                return Ok(job_result(args.result()));
            }
        }
        Err(HostError::Failed(
            "systemd stopped sending job results".to_owned(),
        ))
    }

    /// The service units systemd has loaded, by name, each with
    /// `protected: false` for the caller to fill in. Reading needs no root.
    pub async fn list(&self) -> Result<Vec<ServiceStatus>, HostError> {
        let units = self
            .manager
            .list_units_by_patterns(Vec::new(), vec!["*.service".to_owned()])
            .await
            .map_err(failed)?;
        let mut services: Vec<ServiceStatus> = units
            .into_iter()
            .filter(|unit| unit.2 == "loaded")
            .map(|(unit, description, _, active, sub, ..)| ServiceStatus {
                state: unit_state(&active, &sub),
                detail: Some(format!("{active} ({sub})")),
                description: (!description.is_empty()).then_some(description),
                unit,
                pid: None,
                protected: false,
                scope: ServiceScope::System,
                user: None,
                kind: ServiceKind::Service,
            })
            .collect();
        services.sort_by(|a, b| a.unit.cmp(&b.unit));
        Ok(services)
    }
}

/// A unit's ActiveState and SubState, as the protocol names the state.
fn unit_state(active: &str, sub: &str) -> ServiceState {
    match (active, sub) {
        ("active", "exited") => ServiceState::Exited,
        ("active" | "reloading", _) => ServiceState::Running,
        ("inactive", _) => ServiceState::Stopped,
        ("failed", _) => ServiceState::Failed,
        ("activating", _) => ServiceState::Starting,
        ("deactivating", _) => ServiceState::Stopping,
        _ => ServiceState::Unknown,
    }
}

/// systemd's `JobRemoved` result, as the protocol names it.
fn job_result(result: &str) -> JobResult {
    match result {
        "done" => JobResult::Done,
        "canceled" => JobResult::Canceled,
        "timeout" => JobResult::Timeout,
        "failed" => JobResult::Failed,
        "dependency" => JobResult::Dependency,
        "skipped" => JobResult::Skipped,
        _ => JobResult::Unknown,
    }
}

fn unit_error(unit: &str, error: zbus::Error) -> HostError {
    match &error {
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit" =>
        {
            HostError::NotFound(format!("{unit} doesn't exist"))
        }
        zbus::Error::MethodError(_, Some(detail), _) => HostError::Failed(detail.clone()),
        _ => failed(error),
    }
}

fn failed(error: zbus::Error) -> HostError {
    HostError::Failed(format!("systemd: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_states_map_to_the_protocol() {
        assert_eq!(unit_state("active", "running"), ServiceState::Running);
        assert_eq!(unit_state("active", "exited"), ServiceState::Exited);
        assert_eq!(unit_state("inactive", "dead"), ServiceState::Stopped);
        assert_eq!(unit_state("failed", "failed"), ServiceState::Failed);
        assert_eq!(unit_state("activating", "start"), ServiceState::Starting);
        assert_eq!(unit_state("maintenance", "x"), ServiceState::Unknown);
    }

    #[test]
    fn job_results_map_to_the_protocol() {
        assert_eq!(job_result("done"), JobResult::Done);
        assert_eq!(job_result("dependency"), JobResult::Dependency);
        assert_eq!(job_result("something-new"), JobResult::Unknown);
    }
}
