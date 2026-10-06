//! The Storage tab on Windows (D58), readable by any account: the physical
//! disks through storage queries on `\\.\PhysicalDriveN`, which ask for no
//! access to their data, with I/O counters from IOCTL_DISK_PERFORMANCE and
//! health from the disk's own failure prediction and temperature; and the
//! volumes by drive letter, each with its label, filesystem, space and the
//! disk it's on.

use std::ptr::{null, null_mut};

use cntrl_protocol::storage::{DiskHealth, DiskKind, DisksHealth, HealthStatus, Volume};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BusType1394, BusTypeFileBackedVirtual, BusTypeMmc, BusTypeNvme, BusTypeSd, BusTypeUsb,
    BusTypeVirtual, CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW,
    GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    DEVICE_SEEK_PENALTY_DESCRIPTOR, DISK_GEOMETRY_EX, DISK_PERFORMANCE,
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_PERFORMANCE, IOCTL_STORAGE_PREDICT_FAILURE,
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR,
    STORAGE_PREDICT_FAILURE, STORAGE_PROPERTY_ID, STORAGE_PROPERTY_QUERY,
    STORAGE_TEMPERATURE_DATA_DESCRIPTOR, StorageDeviceProperty, StorageDeviceSeekPenaltyProperty,
    StorageDeviceTemperatureProperty, VOLUME_DISK_EXTENTS,
};

use crate::storage::{DiskReading, IoCounters, Storage, VIRTUAL_MAKERS};

/// GetDriveTypeW's answers: drives with fixed and removable media; network
/// shares, CD drives and RAM disks are left out.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
/// GetVolumeInformationW's flag for a volume mounted read-only.
const READ_ONLY_VOLUME: u32 = 0x0008_0000;
/// The most physical disks looked for.
const MOST_DISKS: u32 = 64;

/// Disks and volumes.
#[derive(Debug, Default)]
pub struct WinStorage;

impl Storage for WinStorage {
    fn disks(&self) -> Vec<DiskReading> {
        (0..MOST_DISKS).filter_map(disk).collect()
    }

    fn volumes(&self) -> Vec<Volume> {
        volumes()
    }
}

fn disk(number: u32) -> Option<DiskReading> {
    let device = Device::open(&format!(r"\\.\PhysicalDrive{number}"))?;
    let descriptor = device.property(StorageDeviceProperty)?;
    // SAFETY: the bytes start with the descriptor; read without assuming
    // their alignment.
    let header = unsafe {
        descriptor
            .as_ptr()
            .cast::<STORAGE_DEVICE_DESCRIPTOR>()
            .read_unaligned()
    };
    let text = |offset: u32| -> Option<String> {
        let start = usize::try_from(offset).ok().filter(|start| *start > 0)?;
        let bytes = descriptor.get(start..)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Some(String::from_utf8_lossy(&bytes[..end]).trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    let model = match (text(header.VendorIdOffset), text(header.ProductIdOffset)) {
        (Some(vendor), Some(product)) => Some(format!("{vendor} {product}")),
        (vendor, product) => vendor.or(product),
    };
    let bus = header.BusType;
    let made_by_hypervisor = model
        .as_deref()
        .is_some_and(|model| VIRTUAL_MAKERS.iter().any(|maker| model.contains(maker)));
    let kind = if bus == BusTypeVirtual || bus == BusTypeFileBackedVirtual || made_by_hypervisor {
        DiskKind::Virtual
    } else if bus == BusTypeNvme {
        DiskKind::Nvme
    } else {
        match device.seek_penalty() {
            Some(true) => DiskKind::Hdd,
            Some(false) => DiskKind::Ssd,
            None => DiskKind::Unknown,
        }
    };
    let counters = device
        .performance()
        .map_or_else(IoCounters::default, |counted| {
            // Times come in 100 ns.
            let ms = |ticks: i64| u64::try_from(ticks / 10_000).unwrap_or(0);
            IoCounters {
                read: u64::try_from(counted.BytesRead).unwrap_or(0),
                written: u64::try_from(counted.BytesWritten).unwrap_or(0),
                reads: Some(u64::from(counted.ReadCount)),
                writes: Some(u64::from(counted.WriteCount)),
                time_ms: Some(ms(counted.ReadTime.saturating_add(counted.WriteTime))),
                // The time since it was counted from that it wasn't idle.
                busy_ms: Some(ms(counted.QueryTime.saturating_sub(counted.IdleTime))),
            }
        });
    Some(DiskReading {
        name: format!("PhysicalDrive{number}"),
        model,
        size: device.size()?,
        kind,
        external: header.RemovableMedia
            || [BusTypeUsb, BusType1394, BusTypeSd, BusTypeMmc].contains(&bus),
        counters,
    })
}

/// What each disk, by name, says of its health: failing when its own
/// prediction says so, and a warning when it's past its warning temperature.
/// A disk that answers neither can't say.
pub fn health(disks: &[String]) -> DisksHealth {
    let disks = disks
        .iter()
        .map(|name| {
            let device = Device::open(&format!(r"\\.\{name}"));
            let failing = device.as_ref().and_then(Device::predicts_failure);
            let temperature = device.as_ref().and_then(Device::temperature);
            let hot = temperature.is_some_and(|(now, warning)| warning > 0 && now >= warning);
            let (status, detail) = match (failing, hot) {
                (Some(true), _) => (
                    HealthStatus::Failing,
                    Some("it predicts its own failure".to_owned()),
                ),
                (_, true) => (
                    HealthStatus::Warning,
                    Some("it's past its warning temperature".to_owned()),
                ),
                (Some(false), false) => (HealthStatus::Ok, None),
                (None, false) => (HealthStatus::Unknown, None),
            };
            DiskHealth {
                disk: name.clone(),
                status,
                temperature: temperature.map(|(now, _)| f64::from(now)),
                power_on_hours: None,
                wear: None,
                detail,
            }
        })
        .collect();
    DisksHealth { disks, note: None }
}

fn volumes() -> Vec<Volume> {
    let mut buffer = [0u16; 512];
    let size = u32::try_from(buffer.len()).unwrap_or(0);
    // SAFETY: the buffer holds `size` characters.
    let length = unsafe { GetLogicalDriveStringsW(size, buffer.as_mut_ptr()) } as usize;
    if length == 0 || length > buffer.len() {
        return Vec::new();
    }
    // Roots such as `C:\`, each ending in a NUL.
    buffer[..length]
        .split(|&c| c == 0)
        .filter(|root| !root.is_empty())
        .filter_map(|root| volume(&String::from_utf16_lossy(root)))
        .collect()
}

fn volume(root: &str) -> Option<Volume> {
    let path = wide(root);
    // SAFETY: a NUL-terminated root.
    let drive = unsafe { GetDriveTypeW(path.as_ptr()) };
    if drive != DRIVE_FIXED && drive != DRIVE_REMOVABLE {
        return None;
    }
    let mut label = [0u16; 261];
    let mut filesystem = [0u16; 261];
    let mut flags = 0u32;
    // SAFETY: the buffers hold 261 characters each. A drive without media,
    // such as an empty card reader, fails, and is left out.
    let read = unsafe {
        GetVolumeInformationW(
            path.as_ptr(),
            label.as_mut_ptr(),
            261,
            null_mut(),
            null_mut(),
            &mut flags,
            filesystem.as_mut_ptr(),
            261,
        )
    };
    if read == 0 {
        return None;
    }
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: three counts to fill.
    if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } == 0
        || total == 0
    {
        return None;
    }
    let name = until_nul(&label);
    Some(Volume {
        mount: root.to_owned(),
        name: (!name.is_empty()).then_some(name),
        kind: until_nul(&filesystem),
        source: None,
        disk: disk_of(root),
        total,
        used: total.saturating_sub(free),
        available,
        inodes: None,
        inodes_used: None,
        read_only: flags & READ_ONLY_VOLUME != 0,
        network: false,
    })
}

/// The physical disk a volume starts on, as `PhysicalDriveN`.
fn disk_of(root: &str) -> Option<String> {
    let device = Device::open(&format!(r"\\.\{}", root.trim_end_matches('\\')))?;
    // Room for a volume spread over a few disks.
    let mut buffer = [0u64; 32];
    let bytes = device.control(
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        null(),
        0,
        buffer.as_mut_ptr().cast(),
        size_of_val(&buffer),
    )?;
    if bytes < size_of::<VOLUME_DISK_EXTENTS>() {
        return None;
    }
    // SAFETY: the call wrote the extents at the buffer's start, which is
    // aligned for them.
    let extents = unsafe { &*buffer.as_ptr().cast::<VOLUME_DISK_EXTENTS>() };
    (extents.NumberOfDiskExtents > 0)
        .then(|| format!("PhysicalDrive{}", extents.Extents[0].DiskNumber))
}

/// A disk or a volume opened for queries only, closed when dropped.
struct Device(HANDLE);

impl Device {
    fn open(path: &str) -> Option<Self> {
        let path = wide(path);
        // SAFETY: a NUL-terminated path; no access to the data is asked for.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        (handle != INVALID_HANDLE_VALUE && !handle.is_null()).then_some(Self(handle))
    }

    /// One DeviceIoControl call; the bytes it returned.
    fn control(
        &self,
        code: u32,
        input: *const core::ffi::c_void,
        input_size: usize,
        output: *mut core::ffi::c_void,
        output_size: usize,
    ) -> Option<usize> {
        let mut returned = 0u32;
        // SAFETY: the callers pass buffers of the sizes given; no overlapped
        // I/O.
        let done = unsafe {
            DeviceIoControl(
                self.0,
                code,
                input,
                u32::try_from(input_size).ok()?,
                output,
                u32::try_from(output_size).ok()?,
                &mut returned,
                null_mut(),
            )
        };
        (done != 0).then_some(returned as usize)
    }

    /// A storage property's descriptor, as bytes.
    fn property(&self, id: STORAGE_PROPERTY_ID) -> Option<Vec<u8>> {
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: id,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut buffer = vec![0u8; 1024];
        let bytes = self.control(
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&raw const query).cast(),
            size_of::<STORAGE_PROPERTY_QUERY>(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )?;
        buffer.truncate(bytes);
        Some(buffer)
    }

    /// Whether reads wait on a head to move: a spinning disk.
    fn seek_penalty(&self) -> Option<bool> {
        let bytes = self.property(StorageDeviceSeekPenaltyProperty)?;
        if bytes.len() < size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() {
            return None;
        }
        // SAFETY: as long as the descriptor, read without assuming alignment.
        let descriptor = unsafe {
            bytes
                .as_ptr()
                .cast::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()
                .read_unaligned()
        };
        Some(descriptor.IncursSeekPenalty)
    }

    fn size(&self) -> Option<u64> {
        // The geometry, then room for the partition and detection data.
        let mut buffer = [0u64; 64];
        let bytes = self.control(
            IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
            null(),
            0,
            buffer.as_mut_ptr().cast(),
            size_of_val(&buffer),
        )?;
        if bytes < size_of::<DISK_GEOMETRY_EX>() - 8 {
            return None;
        }
        // SAFETY: the call wrote the geometry at the buffer's start.
        let geometry = unsafe { &*buffer.as_ptr().cast::<DISK_GEOMETRY_EX>() };
        u64::try_from(geometry.DiskSize)
            .ok()
            .filter(|size| *size > 0)
    }

    /// Whether the disk predicts its own failure, as SMART's thresholds or
    /// NVMe's critical warning say.
    fn predicts_failure(&self) -> Option<bool> {
        let mut predicted = STORAGE_PREDICT_FAILURE {
            PredictFailure: 0,
            VendorSpecific: [0; 512],
        };
        self.control(
            IOCTL_STORAGE_PREDICT_FAILURE,
            null(),
            0,
            (&raw mut predicted).cast(),
            size_of::<STORAGE_PREDICT_FAILURE>(),
        )?;
        Some(predicted.PredictFailure != 0)
    }

    /// The disk's temperature now and where it warns, in degrees Celsius.
    fn temperature(&self) -> Option<(i16, i16)> {
        let bytes = self.property(StorageDeviceTemperatureProperty)?;
        if bytes.len() < size_of::<STORAGE_TEMPERATURE_DATA_DESCRIPTOR>() {
            return None;
        }
        // SAFETY: as long as the descriptor, its first reading included,
        // read without assuming alignment.
        let data = unsafe {
            bytes
                .as_ptr()
                .cast::<STORAGE_TEMPERATURE_DATA_DESCRIPTOR>()
                .read_unaligned()
        };
        (data.InfoCount > 0).then(|| (data.TemperatureInfo[0].Temperature, data.WarningTemperature))
    }

    fn performance(&self) -> Option<DISK_PERFORMANCE> {
        let mut counted = DISK_PERFORMANCE::default();
        let bytes = self.control(
            IOCTL_DISK_PERFORMANCE,
            null(),
            0,
            (&raw mut counted).cast(),
            size_of::<DISK_PERFORMANCE>(),
        )?;
        (bytes >= size_of::<DISK_PERFORMANCE>()).then_some(counted)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: an open handle that this owns.
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn until_nul(text: &[u16]) -> String {
    let length = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_machines_storage() {
        let storage = WinStorage;
        let volumes = storage.volumes();
        let system = volumes
            .iter()
            .find(|volume| volume.mount.eq_ignore_ascii_case(r"C:\"))
            .expect("C:");
        assert!(system.total > 0 && system.used <= system.total);
        assert!(!system.kind.is_empty());
        let disks = storage.disks();
        assert!(!disks.is_empty(), "no physical disks");
        assert!(disks.iter().all(|disk| disk.size > 0));
        assert!(
            system
                .disk
                .as_ref()
                .is_some_and(|name| disks.iter().any(|disk| &disk.name == name)),
            "{system:?} {disks:?}"
        );
    }
}
