//! The sampler behind the `containers` topic (D54, angle 19): the Docker or
//! Podman containers every 3 s, only while some session has a `containers`
//! subscription, each running one with what it used since the reading
//! before. privd reads them, since the engine's socket is root's. As
//! `network` does, it keeps the latest sample in a watch channel whose
//! receiver count says whether anyone is watching. The first reading is a
//! baseline, so the first event already has CPU and rates.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cntrl_protocol::containers::{ContainerNetwork, ContainersSample};
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::docker::{Counters, Listing, cpu_share};
use super::ipc::{self, Call};
use super::uplink::now_ms;

/// How often the containers are read while someone watches.
pub const INTERVAL: Duration = Duration::from_secs(3);
/// After the baseline, the first sample comes this soon.
const FIRST: Duration = Duration::from_secs(1);
/// How often the sampler looks for someone watching.
const CHECK: Duration = Duration::from_millis(250);
/// How long privd may take over the list and every running container's stats.
const READ_LIMIT: Duration = Duration::from_secs(20);

pub type Latest = watch::Sender<Option<Arc<ContainersSample>>>;

/// The previous reading: when, each running container's counters, and
/// whether a sample has gone out since the baseline.
struct Previous {
    at: Instant,
    counters: HashMap<String, Counters>,
    sent: bool,
}

/// Reads the containers while anyone watches, until shutdown.
pub async fn run(
    latest: Arc<Latest>,
    privd: PathBuf,
    token: CancellationToken,
) -> Result<(), String> {
    let mut ticker = tokio::time::interval(CHECK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous: Option<Previous> = None;
    let mut last_error: Option<String> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = token.cancelled() => return Ok(()),
        }
        if latest.receiver_count() == 0 {
            // Nobody's watching; whoever comes next gets a fresh baseline.
            previous = None;
            continue;
        }
        if previous
            .as_ref()
            .is_some_and(|before| before.at.elapsed() < if before.sent { INTERVAL } else { FIRST })
        {
            continue;
        }
        let listing = match read(&privd).await {
            Ok(listing) => {
                last_error = None;
                listing
            }
            Err(e) => {
                if last_error.as_deref() != Some(e.as_str()) {
                    warn!("can't read the containers: {e}");
                    last_error = Some(e.clone());
                }
                // Say why rather than show the last list as current.
                latest.send_replace(Some(Arc::new(ContainersSample {
                    ts: now_ms(),
                    note: Some(e),
                    ..ContainersSample::default()
                })));
                previous = Some(Previous {
                    at: Instant::now(),
                    counters: HashMap::new(),
                    sent: true,
                });
                continue;
            }
        };
        let now = Instant::now();
        let counters: HashMap<String, Counters> = listing
            .containers
            .iter()
            .filter_map(|listed| Some((listed.container.id.clone(), listed.counters?)))
            .collect();
        let Some(before) = previous.replace(Previous {
            at: now,
            counters,
            sent: true,
        }) else {
            // The baseline: nothing to compare with yet.
            if let Some(baseline) = &mut previous {
                baseline.sent = false;
            }
            continue;
        };
        latest.send_replace(Some(Arc::new(sample(listing, &before.counters))));
    }
}

/// The topic's event from privd's list and the counters read before.
fn sample(listing: Listing, before: &HashMap<String, Counters>) -> ContainersSample {
    let containers = listing
        .containers
        .into_iter()
        .map(|listed| {
            let mut container = listed.container;
            if let Some(now) = listed.counters {
                container.memory = now.memory;
                container.memory_limit = now.memory_limit;
                if let Some(earlier) = before.get(&container.id) {
                    container.cpu =
                        cpu_share(earlier, &now).map(|share| (share * 1e4).round() / 1e4);
                    container.network = rates(earlier, &now);
                }
            }
            container
        })
        .collect();
    ContainersSample {
        ts: now_ms(),
        engine: listing.engine,
        containers,
        note: listing.note,
    }
}

/// Bytes per second in and out between two readings; none when a counter
/// went back, as after a restart.
fn rates(earlier: &Counters, later: &Counters) -> Option<ContainerNetwork> {
    let ((rx0, tx0), (rx1, tx1)) = (earlier.network?, later.network?);
    let millis = later.at.checked_sub(earlier.at).filter(|ms| *ms > 0)?;
    let rate = |before: u64, after: u64| -> Option<u64> {
        Some(
            u64::try_from(u128::from(after.checked_sub(before)?) * 1000 / u128::from(millis))
                .unwrap_or(u64::MAX),
        )
    };
    Some(ContainerNetwork {
        received: rate(rx0, rx1)?,
        sent: rate(tx0, tx1)?,
    })
}

/// Asks privd for the list with every running container's counters.
async fn read(privd: &std::path::Path) -> Result<Listing, String> {
    let value = ipc::call_within(privd, Call::Containers { counters: true }, READ_LIMIT)
        .await
        .map_err(|e| e.msg)?;
    serde_json::from_value(value).map_err(|e| format!("privd's list didn't read: {e}"))
}

#[cfg(test)]
mod tests {
    use cntrl_protocol::containers::{Container, ContainerState};

    use super::super::docker::Listed;
    use super::*;

    fn counters(at: u64, cpu: u64, system: u64, network: (u64, u64)) -> Counters {
        Counters {
            at,
            cpu,
            system: Some(system),
            cpus: 4,
            memory: Some(200),
            memory_limit: Some(1_000),
            network: Some(network),
        }
    }

    fn listed(id: &str, counters: Option<Counters>) -> Listed {
        Listed {
            container: Container {
                id: id.to_owned(),
                name: id.to_owned(),
                image: "nginx".to_owned(),
                project: None,
                service: None,
                state: if counters.is_some() {
                    ContainerState::Running
                } else {
                    ContainerState::Exited
                },
                health: None,
                status: String::new(),
                created: 0,
                ports: Vec::new(),
                cpu: None,
                memory: None,
                memory_limit: None,
                network: None,
            },
            counters,
        }
    }

    #[test]
    fn running_containers_get_cpu_and_rates_against_the_reading_before() {
        let before = HashMap::from([(
            "web".to_owned(),
            counters(1_000, 1_000_000_000, 100_000_000_000, (1_000, 2_000)),
        )]);
        let listing = Listing {
            engine: None,
            containers: vec![
                listed(
                    "web",
                    Some(counters(
                        3_000,
                        3_000_000_000,
                        108_000_000_000,
                        (5_000, 2_000),
                    )),
                ),
                // New since the reading before: memory now, CPU next time.
                listed("db", Some(counters(3_000, 5, 108_000_000_000, (0, 0)))),
                listed("old", None),
            ],
            note: None,
        };
        let sample = sample(listing, &before);
        let web = &sample.containers[0];
        assert_eq!(web.cpu, Some(0.25));
        assert_eq!(web.memory, Some(200));
        assert_eq!(web.memory_limit, Some(1_000));
        assert_eq!(
            web.network,
            Some(ContainerNetwork {
                received: 2_000,
                sent: 0
            })
        );
        assert_eq!(sample.containers[1].cpu, None);
        assert_eq!(sample.containers[1].memory, Some(200));
        assert_eq!(sample.containers[2].memory, None);
    }
}
