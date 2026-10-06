//! Windows' Service Control Manager (D58). The process connects through the
//! dispatcher, whose thread reports the service running, then stopping when
//! asked, which cancels the agent's token, then stopped with its exit code.
//! A process started by hand, from a terminal, runs in the foreground.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use tokio::signal::windows::{CtrlClose, ctrl_close};
use tokio_util::sync::CancellationToken;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_dispatcher;

use super::super::health::Health;

/// `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`: the Service Control Manager
/// didn't start this process.
const NOT_A_SERVICE: i32 = 1063;
/// How long stopping may take: the supervisor's grace, and a margin.
const STOP_WAIT: Duration = Duration::from_secs(15);

type Body = Box<dyn FnOnce(CancellationToken) -> ExitCode + Send>;

/// The service's name and main, for the dispatcher's thread.
static SERVICE: Mutex<Option<(&'static str, Body)>> = Mutex::new(None);
/// The main's exit code, back from the dispatcher's thread.
static EXIT: Mutex<Option<ExitCode>> = Mutex::new(None);
/// Whether the Service Control Manager runs this process.
static MANAGED: AtomicBool = AtomicBool::new(false);

/// Runs `body`, the service `name`'s main, under the Service Control
/// Manager, whose stop request cancels its token; or, started by hand, here.
pub fn run(
    name: &'static str,
    body: impl FnOnce(CancellationToken) -> ExitCode + Send + 'static,
) -> ExitCode {
    *lock(&SERVICE) = Some((name, Box::new(body)));
    match service_dispatcher::start(name, service_main) {
        Ok(()) => lock(&EXIT).take().unwrap_or(ExitCode::FAILURE),
        Err(windows_service::Error::Winapi(e)) if e.raw_os_error() == Some(NOT_A_SERVICE) => {
            match lock(&SERVICE).take() {
                Some((_, body)) => body(CancellationToken::new()),
                None => ExitCode::FAILURE,
            }
        }
        Err(e) => {
            let reason =
                std::error::Error::source(&e).map_or_else(|| e.to_string(), ToString::to_string);
            eprintln!("can't reach the Service Control Manager: {reason}");
            ExitCode::FAILURE
        }
    }
}

/// Whether the Service Control Manager runs this process, so its output
/// goes nowhere.
pub fn managed() -> bool {
    MANAGED.load(Ordering::Relaxed)
}

/// What the dispatcher's thread hears while the main runs.
enum Event {
    Stopping,
    Exited(ExitCode),
}

/// The service's entry, on the dispatcher's thread. It takes no start
/// parameters, so it reads none.
extern "system" fn service_main(_count: u32, _parameters: *mut *mut u16) {
    let Some((name, body)) = lock(&SERVICE).take() else {
        return;
    };
    MANAGED.store(true, Ordering::Relaxed);
    let stop = CancellationToken::new();
    let (tell, events) = mpsc::channel();
    let handler = {
        let (stop, tell) = (stop.clone(), tell.clone());
        move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stop.cancel();
                let _ = tell.send(Event::Stopping);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    // Without a handle there's no reporting anything; the Service Control
    // Manager gives up on the start.
    let Ok(status) = service_control_handler::register(name, handler) else {
        return;
    };
    report(status, ServiceState::Running, ServiceExitCode::NO_ERROR);
    std::thread::spawn(move || {
        let code = body(stop);
        let _ = tell.send(Event::Exited(code));
    });
    for event in events {
        match event {
            Event::Stopping => report(status, ServiceState::StopPending, ServiceExitCode::NO_ERROR),
            Event::Exited(code) => {
                // Any failure counts, so the recovery actions restart it.
                let exit = if code == ExitCode::SUCCESS {
                    ServiceExitCode::NO_ERROR
                } else {
                    ServiceExitCode::ServiceSpecific(1)
                };
                *lock(&EXIT) = Some(code);
                report(status, ServiceState::Stopped, exit);
                return;
            }
        }
    }
}

fn report(status: ServiceStatusHandle, state: ServiceState, exit_code: ServiceExitCode) {
    let (controls_accepted, wait_hint) = match state {
        ServiceState::Running => (
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            Duration::ZERO,
        ),
        ServiceState::StopPending => (ServiceControlAccept::empty(), STOP_WAIT),
        _ => (ServiceControlAccept::empty(), Duration::ZERO),
    };
    let reported = status.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted,
        exit_code,
        checkpoint: 0,
        wait_hint,
        process_id: None,
    });
    if let Err(e) = reported {
        tracing::warn!("can't tell the Service Control Manager the service is {state:?}: {e}");
    }
}

/// The dispatcher's thread reports readiness: the Service Control Manager
/// takes no status text.
pub fn ready(_status: &str) {}

/// The dispatcher's thread reports stopping when the stop request comes.
pub fn stopping() {}

/// The Service Control Manager has no watchdog; this waits to be cancelled.
pub async fn watchdog(_health: Arc<Health>, token: CancellationToken) -> Result<(), String> {
    token.cancelled().await;
    Ok(())
}

/// The console closing, when the agent runs in one; a service hears the
/// Service Control Manager instead.
pub struct Terminate(CtrlClose);

impl Terminate {
    pub fn listen() -> Result<Self, String> {
        ctrl_close()
            .map(Self)
            .map_err(|e| format!("can't listen for the console closing: {e}"))
    }

    /// Waits for the console to close, and says so.
    pub async fn recv(&mut self) -> &'static str {
        self.0.recv().await;
        "the console closed"
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
