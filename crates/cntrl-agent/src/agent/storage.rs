//! The sampler behind the `storage` topic (angle 13): the physical disks every
//! 2 s, only while some session has a `storage` subscription, each with its
//! traffic since the reading before, and the volumes, which the host looks at
//! every 10 s. As `network` does, it keeps the latest sample in a watch
//! channel, and its first reading is a baseline, so the first event already
//! has rates.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cntrl_host::storage::{IoCounters, Storage, disk_sample};
use cntrl_protocol::storage::StorageSample;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use super::uplink::now_ms;

/// How often the disks are read while someone watches.
pub const INTERVAL: Duration = Duration::from_secs(2);
/// After the baseline, the first sample comes this soon.
const FIRST: Duration = Duration::from_secs(1);
/// The first sample waits this long at most for the host to look at the
/// volumes, which a Mac takes a moment over; without them a viewer would see
/// its disks as empty.
const VOLUMES_WAIT: Duration = Duration::from_secs(10);
/// How often the sampler looks for someone watching.
const CHECK: Duration = Duration::from_millis(250);

pub type Latest = watch::Sender<Option<Arc<StorageSample>>>;

/// The previous reading: when, each disk's counters, and whether a sample has
/// gone out since the baseline.
struct Previous {
    at: Instant,
    counters: HashMap<String, IoCounters>,
    sent: bool,
}

/// Reads the disks and volumes while anyone watches, until shutdown.
pub async fn run(
    latest: Arc<Latest>,
    storage: Arc<dyn Storage>,
    token: CancellationToken,
) -> Result<(), String> {
    let mut ticker = tokio::time::interval(CHECK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous: Option<Previous> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = token.cancelled() => return Ok(()),
        }
        if latest.receiver_count() == 0 {
            previous = None;
            continue;
        }
        if previous
            .as_ref()
            .is_some_and(|before| before.at.elapsed() < if before.sent { INTERVAL } else { FIRST })
        {
            continue;
        }
        // The first sample waits for the volumes, keeping its baseline, so
        // its rates still run from it.
        if previous
            .as_ref()
            .is_some_and(|before| !before.sent && before.at.elapsed() < VOLUMES_WAIT)
            && !storage.volumes_ready()
        {
            continue;
        }
        let host = Arc::clone(&storage);
        let Ok((disks, volumes)) =
            tokio::task::spawn_blocking(move || (host.disks(), host.volumes())).await
        else {
            continue;
        };
        let now = Instant::now();
        let counters = disks
            .iter()
            .map(|disk| (disk.name.clone(), disk.counters))
            .collect();
        let Some(before) = previous.replace(Previous {
            at: now,
            counters,
            sent: true,
        }) else {
            if let Some(baseline) = &mut previous {
                baseline.sent = false;
            }
            continue;
        };
        let elapsed = now.duration_since(before.at);
        let disks = disks
            .iter()
            .map(|disk| disk_sample(disk, before.counters.get(&disk.name), elapsed))
            .collect();
        latest.send_replace(Some(Arc::new(StorageSample {
            ts: now_ms(),
            disks,
            volumes,
        })));
    }
}

#[cfg(test)]
mod tests {
    use cntrl_host::storage::DiskReading;
    use cntrl_protocol::storage::{DiskKind, Volume};

    use super::*;

    /// A host whose volumes are looked at by `ready`, as a Mac's are a moment
    /// after the first ask.
    struct Host {
        ready: Instant,
    }

    impl Storage for Host {
        fn disks(&self) -> Vec<DiskReading> {
            vec![DiskReading {
                name: "disk0".to_owned(),
                model: None,
                size: 500,
                kind: DiskKind::Nvme,
                external: false,
                counters: IoCounters::default(),
            }]
        }

        fn volumes(&self) -> Vec<Volume> {
            if !self.volumes_ready() {
                return Vec::new();
            }
            vec![Volume {
                mount: "/".to_owned(),
                name: None,
                kind: "apfs".to_owned(),
                source: None,
                disk: Some("disk0".to_owned()),
                total: 400,
                used: 100,
                available: 300,
                inodes: None,
                inodes_used: None,
                read_only: false,
                network: false,
            }]
        }

        fn volumes_ready(&self) -> bool {
            Instant::now() >= self.ready
        }
    }

    /// The first sample's volumes and when it came, with the host's volumes
    /// ready after `after`.
    async fn first_sample(after: Duration) -> (usize, Duration) {
        let latest = Arc::new(watch::channel(None).0);
        let mut watching = latest.subscribe();
        let start = Instant::now();
        let host = Arc::new(Host {
            ready: start + after,
        });
        let token = CancellationToken::new();
        let sampler = tokio::spawn(run(Arc::clone(&latest), host, token.clone()));
        watching.changed().await.expect("a sample");
        let sample = watching.borrow().clone().expect("a sample");
        token.cancel();
        let _ = sampler.await;
        (sample.volumes.len(), start.elapsed())
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_sample_waits_for_the_volumes() {
        let (volumes, at) = first_sample(Duration::from_secs(3)).await;
        assert_eq!(volumes, 1, "came before the volumes were looked at");
        assert!(at >= Duration::from_secs(3) && at < VOLUMES_WAIT, "{at:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_sample_doesnt_wait_for_ever() {
        let (volumes, at) = first_sample(Duration::from_secs(60)).await;
        assert_eq!(volumes, 0);
        assert!(
            at >= VOLUMES_WAIT && at < VOLUMES_WAIT + Duration::from_secs(1),
            "{at:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ready_volumes_dont_hold_it_up() {
        let (volumes, at) = first_sample(Duration::ZERO).await;
        assert_eq!(volumes, 1);
        assert!(at < FIRST + Duration::from_millis(500), "{at:?}");
    }
}
