//! The sampler behind the `storage` topic (angle 13): the physical disks every
//! 2 s, only while some session has a `storage` subscription, each with its
//! traffic since the reading before, and the volumes, which the host looks at
//! every 10 s. As `network` does, it keeps the latest sample in a watch
//! channel, and its first reading is a baseline, so the first event already
//! has rates.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cntrl_host::storage::{IoCounters, Storage, disk_sample};
use cntrl_protocol::storage::StorageSample;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::uplink::now_ms;

/// How often the disks are read while someone watches.
pub const INTERVAL: Duration = Duration::from_secs(2);
/// After the baseline, the first sample comes this soon.
const FIRST: Duration = Duration::from_secs(1);
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
