//! The sampler behind the `network` topic (angle 13): the interfaces, routes
//! and name servers every 2 s, only while some session has a `network`
//! subscription, each interface with its traffic since the reading before. As
//! `processes` does, it keeps the latest sample in a watch channel whose
//! receiver count says whether anyone is watching. The first reading is a
//! baseline, so the first event already has rates.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cntrl_host::network::{NetworkReader, interface_sample};
use cntrl_protocol::network::NetworkSample;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::uplink::now_ms;

/// How often the network is read while someone watches.
pub const INTERVAL: Duration = Duration::from_secs(2);
/// After the baseline, the first sample comes this soon.
const FIRST: Duration = Duration::from_secs(1);
/// How often the sampler looks for someone watching.
const CHECK: Duration = Duration::from_millis(250);

pub type Latest = watch::Sender<Option<Arc<NetworkSample>>>;

/// The previous reading: when, each interface's byte totals, and whether a
/// sample has gone out since the baseline.
struct Previous {
    at: Instant,
    totals: HashMap<String, (u64, u64)>,
    sent: bool,
}

/// Reads the network while anyone watches, until shutdown.
pub async fn run(latest: Arc<Latest>, token: CancellationToken) -> Result<(), String> {
    let reader = Arc::new(Mutex::new(NetworkReader::default()));
    let mut ticker = tokio::time::interval(CHECK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous: Option<Previous> = None;
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
        let shared = Arc::clone(&reader);
        let Ok(reading) = tokio::task::spawn_blocking(move || {
            shared.lock().unwrap_or_else(PoisonError::into_inner).read()
        })
        .await
        else {
            continue;
        };
        let now = Instant::now();
        let totals = reading
            .interfaces
            .iter()
            .map(|interface| {
                (
                    interface.name.clone(),
                    (interface.received_total, interface.sent_total),
                )
            })
            .collect();
        let Some(before) = previous.replace(Previous {
            at: now,
            totals,
            sent: true,
        }) else {
            // The baseline: nothing to compare with yet.
            if let Some(baseline) = &mut previous {
                baseline.sent = false;
            }
            continue;
        };
        let elapsed = now.duration_since(before.at);
        let interfaces = reading
            .interfaces
            .iter()
            .map(|interface| {
                interface_sample(
                    interface,
                    before.totals.get(&interface.name).copied(),
                    elapsed,
                )
            })
            .collect();
        latest.send_replace(Some(Arc::new(NetworkSample {
            ts: now_ms(),
            interfaces,
            routes: reading.routes,
            dns: reading.dns,
        })));
    }
}
