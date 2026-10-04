//! systemd over D-Bus, for the `service.*` operations. It needs the system bus,
//! and root to act on units.

use std::collections::HashMap;

use cntrl_protocol::service::{
    JobResult, ServiceAction, ServiceKind, ServiceScope, ServiceState, ServiceStatus,
};
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

    /// Restarts `unit` and waits for systemd's verdict on the job.
    pub async fn restart(&self, unit: &str) -> Result<JobResult, HostError> {
        self.act(unit, ServiceAction::Restart).await
    }

    /// Takes `action` on `unit` (angle 11): start, stop and restart are jobs,
    /// whose verdict this waits for; enable and disable change the unit's
    /// files, then have systemd reload. Dropping the future stops the wait, not
    /// the job.
    pub async fn act(&self, unit: &str, action: ServiceAction) -> Result<JobResult, HostError> {
        let files = vec![unit.to_owned()];
        match action {
            ServiceAction::Enable => {
                self.manager
                    .enable_unit_files(files, false, false)
                    .await
                    .map_err(|e| unit_error(unit, e))?;
                self.manager.reload().await.map_err(failed)?;
                return Ok(JobResult::Done);
            }
            ServiceAction::Disable => {
                self.manager
                    .disable_unit_files(files, false)
                    .await
                    .map_err(|e| unit_error(unit, e))?;
                self.manager.reload().await.map_err(failed)?;
                return Ok(JobResult::Done);
            }
            ServiceAction::Start | ServiceAction::Stop | ServiceAction::Restart => {}
        }
        // Listen before asking, so a quick job's result can't slip past.
        let mut removed = self.manager.receive_job_removed().await.map_err(failed)?;
        // systemd sends job signals only to clients that subscribed. A repeat
        // subscription is refused, which is harmless.
        let _ = self.manager.subscribe().await;
        let (name, mode) = (unit.to_owned(), "replace".to_owned());
        let job = match action {
            ServiceAction::Start => self.manager.start_unit(name, mode).await,
            ServiceAction::Stop => self.manager.stop_unit(name, mode).await,
            _ => self.manager.restart_unit(name, mode).await,
        }
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
        // Whether each starts at boot, from its unit file. Failing to read it
        // only leaves enable and disable out.
        let files: HashMap<String, Option<bool>> = self
            .manager
            .list_unit_files_by_patterns(Vec::new(), vec!["*.service".to_owned()])
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(path, state)| {
                let name = path.rsplit('/').next()?.to_owned();
                Some((name, enablement(&state)))
            })
            .collect();
        let loaded: std::collections::HashSet<&str> = units
            .iter()
            .filter(|unit| unit.2 == "loaded")
            .map(|unit| unit.0.as_str())
            .collect();
        // systemd only lists the units it has loaded, and a stopped unit that
        // nothing pulls in isn't (angle 06 §2). Installed ones, enabled or
        // disabled, show too, so they can be started; static units and
        // templates (`name@.service`) are systemd's own plumbing and stay out.
        let mut unloaded: Vec<ServiceStatus> = files
            .iter()
            .filter(|(name, enabled)| {
                enabled.is_some() && !name.ends_with("@.service") && !loaded.contains(name.as_str())
            })
            .map(|(name, enabled)| ServiceStatus {
                unit: name.clone(),
                description: None,
                state: ServiceState::Stopped,
                detail: Some("inactive (not loaded)".to_owned()),
                pid: None,
                protected: false,
                scope: ServiceScope::System,
                user: None,
                kind: ServiceKind::Service,
                enabled: *enabled,
            })
            .collect();
        let mut services: Vec<ServiceStatus> = units
            .into_iter()
            .filter(|unit| unit.2 == "loaded")
            .map(|(unit, description, _, active, sub, ..)| ServiceStatus {
                state: unit_state(&active, &sub),
                detail: Some(format!("{active} ({sub})")),
                description: (!description.is_empty()).then_some(description),
                pid: None,
                protected: false,
                scope: ServiceScope::System,
                user: None,
                kind: ServiceKind::Service,
                enabled: files.get(&unit).copied().flatten(),
                unit,
            })
            .collect();
        services.append(&mut unloaded);
        services.sort_by(|a, b| a.unit.cmp(&b.unit));
        Ok(services)
    }
}

/// Whether a unit file's state means it starts at boot, where that can be
/// changed: `enabled` or `disabled`. Others (`static`, `indirect`, `masked`,
/// `generated`, …) can't be enabled or disabled from here.
fn enablement(state: &str) -> Option<bool> {
    match state {
        "enabled" => Some(true),
        "disabled" => Some(false),
        _ => None,
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
    fn only_enabled_or_disabled_units_can_change() {
        assert_eq!(enablement("enabled"), Some(true));
        assert_eq!(enablement("disabled"), Some(false));
        assert_eq!(enablement("static"), None);
        assert_eq!(enablement("masked"), None);
    }

    #[test]
    fn job_results_map_to_the_protocol() {
        assert_eq!(job_result("done"), JobResult::Done);
        assert_eq!(job_result("dependency"), JobResult::Dependency);
        assert_eq!(job_result("something-new"), JobResult::Unknown);
    }
}
