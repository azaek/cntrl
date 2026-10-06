//! GPUs on Windows (D59), read as Task Manager reads them and with no rights
//! beyond the agent's own: each display adapter's engines and memory from
//! the graphics kernel's statistics, its name and power state from its
//! device node, and its temperature from the driver's performance data,
//! asked for only while the device is powered on, so a sleeping GPU stays
//! asleep. Software and display-only adapters, such as the Basic Render
//! Driver, are left out.
//!
//! `D3DKMTQueryStatistics` is marked reserved for system use; System
//! Informer and LibreHardwareMonitor read GPUs through it (angle 14).

use std::collections::HashMap;
use std::ptr::null;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use cntrl_protocol::stats::GpuStats;
use windows_sys::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_ADAPTERTYPE, D3DKMT_CLOSEADAPTER,
    D3DKMT_OPENADAPTERFROMDEVICENAME, D3DKMT_QUERYADAPTERINFO, D3DKMT_QUERYSTATISTICS,
    D3DKMT_QUERYSTATISTICS_ADAPTER, D3DKMT_QUERYSTATISTICS_NODE, D3DKMT_QUERYSTATISTICS_QUERY_NODE,
    D3DKMT_QUERYSTATISTICS_QUERY_SEGMENT, D3DKMT_QUERYSTATISTICS_SEGMENT,
    D3DKMT_QUERYSTATISTICS_TYPE, D3DKMT_SEGMENTSIZEINFO, D3DKMTCloseAdapter,
    D3DKMTOpenAdapterFromDeviceName, D3DKMTQueryAdapterInfo, D3DKMTQueryStatistics,
    KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_ADAPTERTYPE, KMTQAITYPE_GETSEGMENTSIZE,
    KMTQUERYADAPTERINFOTYPE,
};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_DevNode_PropertyW,
    CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW,
    CM_Get_Device_Interface_PropertyW, CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CR_SUCCESS,
};
use windows_sys::Win32::Devices::Display::GUID_DISPLAY_DEVICE_ARRIVAL;
use windows_sys::Win32::Devices::Properties::{
    DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_InstanceId, DEVPKEY_Device_PowerData,
};
use windows_sys::Win32::Foundation::LUID;
use windows_sys::Win32::System::Power::{CM_POWER_DATA, DEVICE_POWER_STATE, PowerDeviceD0};

use crate::gpu::{busiest, decicelsius, device_name};

/// D3DKMT_ADAPTERTYPE's bits for an adapter that renders, and for one
/// that's software.
const RENDER_SUPPORTED: u32 = 1;
const SOFTWARE_DEVICE: u32 = 1 << 2;
/// Engine times older than this aren't compared with: a GPU's use is how
/// busy it was lately, not since someone last looked.
const STALE: Duration = Duration::from_secs(30);
/// With nothing recent to compare with, how long a read measures.
const FIRST_WINDOW: Duration = Duration::from_millis(250);

// The statistics' layout is the system's: an older Windows reads the
// engine or segment asked for at the same offset (System Informer checks
// this size too).
const _: () = assert!(size_of::<D3DKMT_QUERYSTATISTICS>() == 0x328);

/// Each engine's running time at the last read, kept to measure use.
#[derive(Debug, Default)]
pub(crate) struct EngineTimes(Mutex<Option<Times>>);

#[derive(Debug)]
struct Times {
    at: Instant,
    /// In 100 ns units, by adapter and engine.
    running: HashMap<(i64, u32), u64>,
}

/// The GPUs, with their use since the last read.
pub(crate) fn gpus(last: &EngineTimes) -> Vec<GpuStats> {
    let adapters: Vec<Adapter> = adapters().into_iter().filter(Adapter::is_gpu).collect();
    if adapters.is_empty() {
        return Vec::new();
    }
    let recent = lock(last).take().filter(|times| times.at.elapsed() < STALE);
    let earlier = recent.unwrap_or_else(|| {
        let times = engine_times(&adapters);
        thread::sleep(FIRST_WINDOW);
        times
    });
    let now = engine_times(&adapters);
    let gpus = adapters
        .iter()
        .map(|adapter| adapter.stats(&earlier, &now))
        .collect();
    *lock(last) = Some(now);
    gpus
}

/// An open display adapter, closed when dropped.
struct Adapter {
    handle: u32,
    luid: LUID,
    engines: u32,
    segments: u32,
    /// Its device node, for its name and power state.
    node: Option<u32>,
}

impl Drop for Adapter {
    fn drop(&mut self) {
        let close = D3DKMT_CLOSEADAPTER {
            hAdapter: self.handle,
        };
        // SAFETY: an adapter this opened.
        unsafe { D3DKMTCloseAdapter(&close) };
    }
}

impl Adapter {
    /// Whether it's a GPU: it renders, in hardware, on engines.
    fn is_gpu(&self) -> bool {
        if self.engines == 0 {
            return false;
        }
        let mut kind = D3DKMT_ADAPTERTYPE::default();
        // SAFETY: an adapter's type answers into D3DKMT_ADAPTERTYPE.
        if !unsafe { self.query(KMTQAITYPE_ADAPTERTYPE, &mut kind) } {
            return false;
        }
        // SAFETY: the bitfield and the value are the same 32 bits.
        let bits = unsafe { kind.Anonymous.Value };
        bits & RENDER_SUPPORTED != 0 && bits & SOFTWARE_DEVICE == 0
    }

    fn stats(&self, earlier: &Times, now: &Times) -> GpuStats {
        let id = key(self.luid);
        let engines = (0..self.engines).filter_map(|engine| {
            let time = |times: &Times| times.running.get(&(id, engine)).copied();
            Some((time(earlier)?, time(now)?))
        });
        let (memory_used, memory_total) = self.memory();
        GpuStats {
            name: self
                .node
                .and_then(description)
                .and_then(|text| device_name(&text))
                .unwrap_or_else(|| "GPU".to_owned()),
            busy: busiest(engines, now.at.saturating_duration_since(earlier.at)),
            memory_used,
            memory_total,
            temperature: self.temperature(),
            // Windows gives a share of the most it may draw, not watts.
            power: None,
        }
    }

    /// The dedicated memory in use, out of its size: the resident bytes of
    /// the segments that aren't apertures onto system memory, as Task
    /// Manager's dedicated memory. An integrated GPU without any reports
    /// neither.
    fn memory(&self) -> (Option<u64>, Option<u64>) {
        let mut sizes = D3DKMT_SEGMENTSIZEINFO::default();
        // SAFETY: segment sizes answer into D3DKMT_SEGMENTSIZEINFO.
        let total = unsafe { self.query(KMTQAITYPE_GETSEGMENTSIZE, &mut sizes) }
            .then_some(sizes.DedicatedVideoMemorySize)
            .filter(|&total| total > 0);
        if total.is_none() {
            return (None, None);
        }
        let used = (0..self.segments)
            .map(|id| segment(self.luid, id))
            .try_fold(0u64, |used, segment| {
                let (resident, aperture) = segment?;
                Some(if aperture { used } else { used + resident })
            });
        (used, total)
    }

    /// The driver's temperature, asked for only while the device is in D0:
    /// the driver answers, and asking a sleeping GPU could wake it.
    fn temperature(&self) -> Option<f64> {
        if self.node.and_then(power_state) != Some(PowerDeviceD0) {
            return None;
        }
        // The first physical adapter's, the only one but on linked GPUs.
        let mut data = D3DKMT_ADAPTER_PERFDATA::default();
        // SAFETY: performance data answers into D3DKMT_ADAPTER_PERFDATA.
        unsafe { self.query(KMTQAITYPE_ADAPTERPERFDATA, &mut data) }
            .then_some(data.Temperature)
            .and_then(decicelsius)
    }

    /// Asks the adapter's `kind` of information into `value`.
    ///
    /// # Safety
    ///
    /// `T` must be the structure `kind` answers into.
    unsafe fn query<T>(&self, kind: KMTQUERYADAPTERINFOTYPE, value: &mut T) -> bool {
        let mut query = D3DKMT_QUERYADAPTERINFO {
            hAdapter: self.handle,
            Type: kind,
            pPrivateDriverData: (value as *mut T).cast(),
            PrivateDriverDataSize: size_of::<T>() as u32,
        };
        // SAFETY: the query points at a `T` of the size it states, which
        // the caller has matched to `kind`.
        unsafe { D3DKMTQueryAdapterInfo(&mut query) >= 0 }
    }
}

/// The display adapters Windows lists, software ones included, each with
/// its engines and segments counted. One that can't be opened or counted
/// is left out.
fn adapters() -> Vec<Adapter> {
    interfaces()
        .iter()
        .filter_map(|interface| {
            let mut open = D3DKMT_OPENADAPTERFROMDEVICENAME {
                pDeviceName: interface.as_ptr(),
                ..D3DKMT_OPENADAPTERFROMDEVICENAME::default()
            };
            // SAFETY: a NUL-terminated device interface, and the handle and
            // LUID to fill.
            if unsafe { D3DKMTOpenAdapterFromDeviceName(&mut open) } < 0 {
                return None;
            }
            let mut adapter = Adapter {
                handle: open.hAdapter,
                luid: open.AdapterLuid,
                engines: 0,
                segments: 0,
                node: device_node(interface),
            };
            let mut query = statistics(D3DKMT_QUERYSTATISTICS_ADAPTER, adapter.luid);
            if !ask(&mut query) {
                return None;
            }
            // SAFETY: an adapter query answers with adapter information.
            let counts = unsafe { query.QueryResult.AdapterInformation };
            (adapter.engines, adapter.segments) = (counts.NodeCount, counts.NbSegments);
            Some(adapter)
        })
        .collect()
}

/// The display adapters' device interfaces, each NUL-terminated.
fn interfaces() -> Vec<Vec<u16>> {
    let mut length = 0u32;
    // SAFETY: a length to fill.
    let status = unsafe {
        CM_Get_Device_Interface_List_SizeW(
            &mut length,
            &GUID_DISPLAY_DEVICE_ARRIVAL,
            null(),
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if status != CR_SUCCESS || length == 0 {
        return Vec::new();
    }
    let mut list = vec![0u16; length as usize];
    // SAFETY: the list holds `length` characters. One that arrived in
    // between makes it too short, and the next read finds it.
    let status = unsafe {
        CM_Get_Device_Interface_ListW(
            &GUID_DISPLAY_DEVICE_ARRIVAL,
            null(),
            list.as_mut_ptr(),
            length,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if status != CR_SUCCESS {
        return Vec::new();
    }
    list.split(|&c| c == 0)
        .filter(|interface| !interface.is_empty())
        .map(|interface| interface.iter().copied().chain(Some(0)).collect())
        .collect()
}

fn engine_times(adapters: &[Adapter]) -> Times {
    let mut running = HashMap::new();
    for adapter in adapters {
        for engine in 0..adapter.engines {
            if let Some(time) = running_time(adapter.luid, engine) {
                running.insert((key(adapter.luid), engine), time);
            }
        }
    }
    Times {
        at: Instant::now(),
        running,
    }
}

/// How long the engine has run since the adapter started, in 100 ns units.
fn running_time(luid: LUID, engine: u32) -> Option<u64> {
    let mut query = statistics(D3DKMT_QUERYSTATISTICS_NODE, luid);
    query.Anonymous.QueryNode = D3DKMT_QUERYSTATISTICS_QUERY_NODE { NodeId: engine };
    if !ask(&mut query) {
        return None;
    }
    // SAFETY: a node query answers with node information.
    let time = unsafe {
        query
            .QueryResult
            .NodeInformation
            .GlobalInformation
            .RunningTime
    };
    u64::try_from(time).ok()
}

/// A memory segment's resident bytes, and whether it's an aperture onto
/// system memory.
fn segment(luid: LUID, id: u32) -> Option<(u64, bool)> {
    let mut query = statistics(D3DKMT_QUERYSTATISTICS_SEGMENT, luid);
    query.Anonymous.QuerySegment = D3DKMT_QUERYSTATISTICS_QUERY_SEGMENT { SegmentId: id };
    if !ask(&mut query) {
        return None;
    }
    // SAFETY: a segment query answers with segment information.
    let segment = unsafe { query.QueryResult.SegmentInformation };
    Some((segment.BytesResident, segment.Aperture != 0))
}

fn statistics(kind: D3DKMT_QUERYSTATISTICS_TYPE, luid: LUID) -> D3DKMT_QUERYSTATISTICS {
    D3DKMT_QUERYSTATISTICS {
        Type: kind,
        AdapterLuid: luid,
        ..D3DKMT_QUERYSTATISTICS::default()
    }
}

/// Asks the graphics kernel for the statistics `query` names, into it.
fn ask(query: &mut D3DKMT_QUERYSTATISTICS) -> bool {
    // SAFETY: a query with room for its answer, which the call writes
    // though its declaration says const.
    unsafe { D3DKMTQueryStatistics((query as *mut D3DKMT_QUERYSTATISTICS).cast_const()) >= 0 }
}

/// The device node behind a device interface.
fn device_node(interface: &[u16]) -> Option<u32> {
    // Device instance IDs are at most 200 characters.
    let mut id = [0u16; 256];
    let mut size = size_of_val(&id) as u32;
    let mut kind = 0;
    // SAFETY: a NUL-terminated interface, and room for its device's ID.
    let status = unsafe {
        CM_Get_Device_Interface_PropertyW(
            interface.as_ptr(),
            &DEVPKEY_Device_InstanceId,
            &mut kind,
            id.as_mut_ptr().cast(),
            &mut size,
            0,
        )
    };
    if status != CR_SUCCESS {
        return None;
    }
    let mut node = 0;
    // SAFETY: the NUL-terminated ID just read.
    let status = unsafe { CM_Locate_DevNodeW(&mut node, id.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
    (status == CR_SUCCESS).then_some(node)
}

/// The device's description, Device Manager's name for it.
fn description(node: u32) -> Option<String> {
    let mut text = [0u16; 256];
    let mut size = size_of_val(&text) as u32;
    let mut kind = 0;
    // SAFETY: room for `size` bytes.
    let status = unsafe {
        CM_Get_DevNode_PropertyW(
            node,
            &DEVPKEY_Device_DeviceDesc,
            &mut kind,
            text.as_mut_ptr().cast(),
            &mut size,
            0,
        )
    };
    if status != CR_SUCCESS {
        return None;
    }
    let length = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    Some(String::from_utf16_lossy(&text[..length]))
}

/// The device's power state as Windows last recorded it, read without
/// asking the device.
fn power_state(node: u32) -> Option<DEVICE_POWER_STATE> {
    let mut data = CM_POWER_DATA::default();
    let mut size = size_of::<CM_POWER_DATA>() as u32;
    let mut kind = 0;
    // SAFETY: room for the power data, its size stated.
    let status = unsafe {
        CM_Get_DevNode_PropertyW(
            node,
            &DEVPKEY_Device_PowerData,
            &mut kind,
            (&raw mut data).cast(),
            &mut size,
            0,
        )
    };
    (status == CR_SUCCESS).then_some(data.PD_MostRecentPowerState)
}

fn key(luid: LUID) -> i64 {
    (i64::from(luid.HighPart) << 32) | i64::from(luid.LowPart)
}

fn lock(times: &EngineTimes) -> MutexGuard<'_, Option<Times>> {
    times.0.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the agent reads of a GPU, it reads of every adapter Windows
    /// lists, software ones too: CI's machines have only those. CI also
    /// runs this as a virtual account, as the agent's service runs.
    #[test]
    fn reads_every_display_adapter() {
        let listed = interfaces().len();
        assert!(listed > 0, "Windows lists display adapters");
        let adapters = adapters();
        assert_eq!(adapters.len(), listed, "each opens, with its counts");
        for adapter in &adapters {
            let node = adapter.node.expect("a device node");
            assert!(description(node).is_some_and(|d| !d.is_empty()));
            assert!(power_state(node).is_some(), "a power state");
            for engine in 0..adapter.engines {
                assert!(running_time(adapter.luid, engine).is_some());
            }
            for id in 0..adapter.segments {
                assert!(segment(adapter.luid, id).is_some());
            }
            let mut kind = D3DKMT_ADAPTERTYPE::default();
            // SAFETY: as in `is_gpu`.
            assert!(unsafe { adapter.query(KMTQAITYPE_ADAPTERTYPE, &mut kind) });
        }

        let times = EngineTimes::default();
        let first = gpus(&times);
        let second = gpus(&times);
        assert_eq!(first.len(), second.len());
        for gpu in first.iter().chain(&second) {
            assert!(!gpu.name.is_empty());
            assert!(gpu.busy.is_some_and(|busy| (0.0..=1.0).contains(&busy)));
        }
    }
}
