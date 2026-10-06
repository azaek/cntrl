//! The Storage tab on Windows (D58), readable by any account: the physical
//! disks through storage queries on `\\.\PhysicalDriveN`, which ask for no
//! access to their data, with I/O counters from IOCTL_DISK_PERFORMANCE and
//! health from the disk's own failure prediction and temperature, and an
//! NVMe disk's health log, which privd reads as SYSTEM; and the volumes by
//! drive letter, each with its label, filesystem, space and the disk it's
//! on.

use std::ptr::{null, null_mut};

use cntrl_protocol::storage::{DiskHealth, DiskKind, DisksHealth, HealthStatus, Volume};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BusType1394, BusTypeFileBackedVirtual, BusTypeMmc, BusTypeNvme, BusTypeSd, BusTypeUsb,
    BusTypeVirtual, CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW,
    GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows_sys::Win32::Storage::Nvme::NVME_LOG_PAGE_HEALTH_INFO;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    DEVICE_SEEK_PENALTY_DESCRIPTOR, DISK_GEOMETRY_EX, DISK_PERFORMANCE,
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_PERFORMANCE, IOCTL_STORAGE_PREDICT_FAILURE,
    IOCTL_STORAGE_QUERY_PROPERTY, NVMeDataTypeLogPage, PropertyStandardQuery, ProtocolTypeNvme,
    STORAGE_DEVICE_DESCRIPTOR, STORAGE_PREDICT_FAILURE, STORAGE_PROPERTY_ID,
    STORAGE_PROPERTY_QUERY, STORAGE_PROTOCOL_DATA_DESCRIPTOR, STORAGE_PROTOCOL_SPECIFIC_DATA,
    STORAGE_TEMPERATURE_DATA_DESCRIPTOR, StorageDeviceProperty,
    StorageDeviceProtocolSpecificProperty, StorageDeviceSeekPenaltyProperty,
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

/// What each disk, by name, says of its health, judged as smartctl's is on
/// Linux: failing when it predicts its own failure or an NVMe disk raises a
/// critical warning; a warning when it's past its warning temperature or its
/// rated life, or has had media errors. A disk that answers none of it can't
/// say. Opened to read, as privd can, an NVMe disk also gives its wear and
/// hours.
pub fn health(disks: &[String]) -> DisksHealth {
    let disks = disks.iter().map(|name| disk_health(name)).collect();
    DisksHealth { disks, note: None }
}

fn disk_health(name: &str) -> DiskHealth {
    let path = format!(r"\\.\{name}");
    let device = Device::open_reading(&path).or_else(|| Device::open(&path));
    let failing = device.as_ref().and_then(Device::predicts_failure);
    let nvme = device.as_ref().and_then(Device::nvme_health);
    let temperature = device.as_ref().and_then(Device::temperature);
    let mut failures: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    if failing == Some(true) {
        failures.push("it predicts its own failure".to_owned());
    }
    if let Some(log) = &nvme {
        let words = [
            (0x01, "spare capacity below its threshold"),
            (0x02, "temperature outside its limits"),
            (0x04, "reliability degraded"),
            (0x08, "media read-only"),
            (0x10, "backup memory failed"),
        ];
        failures.extend(
            words
                .iter()
                .filter(|(bit, _)| log.critical_warning & bit != 0)
                .map(|(_, words)| (*words).to_owned()),
        );
        if log.percentage_used >= 100 {
            warnings.push("past its rated life".to_owned());
        }
        if log.media_errors > 0 {
            warnings.push(format!("{} media errors", log.media_errors));
        }
    }
    if temperature.is_some_and(|(now, warning)| warning > 0 && now >= warning) {
        warnings.push("past its warning temperature".to_owned());
    }
    let status = if !failures.is_empty() {
        HealthStatus::Failing
    } else if !warnings.is_empty() {
        HealthStatus::Warning
    } else if failing.is_some() || nvme.is_some() {
        HealthStatus::Ok
    } else {
        HealthStatus::Unknown
    };
    let problems: Vec<String> = failures.into_iter().chain(warnings).collect();
    DiskHealth {
        disk: name.to_owned(),
        status,
        temperature: nvme
            .as_ref()
            .and_then(|log| log.celsius())
            .or(temperature.map(|(now, _)| f64::from(now))),
        power_on_hours: nvme.as_ref().map(|log| log.power_on_hours),
        wear: nvme.as_ref().map(|log| u32::from(log.percentage_used)),
        detail: (!problems.is_empty()).then(|| problems.join(", ")),
    }
}

/// What an NVMe disk's SMART / Health Information log says (log page 2).
#[derive(Debug, Clone, Copy)]
struct NvmeHealth {
    critical_warning: u8,
    /// Kelvin, as the log keeps it; 0 when it doesn't say.
    composite_temperature: u16,
    /// Of its rated life; it can pass 100.
    percentage_used: u8,
    power_on_hours: u64,
    media_errors: u64,
}

impl NvmeHealth {
    /// From the log's 512 bytes, as the NVMe specification lays them out.
    fn parse(log: &[u8]) -> Option<Self> {
        let low_64 = |at: usize| -> Option<u64> {
            Some(u64::from_le_bytes(log.get(at..at + 8)?.try_into().ok()?))
        };
        Some(Self {
            critical_warning: *log.first()?,
            composite_temperature: u16::from_le_bytes(log.get(1..3)?.try_into().ok()?),
            percentage_used: *log.get(5)?,
            power_on_hours: low_64(128)?,
            media_errors: low_64(160)?,
        })
    }

    fn celsius(&self) -> Option<f64> {
        (self.composite_temperature > 0).then(|| f64::from(self.composite_temperature) - 273.15)
    }
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
    /// Opened for queries alone, as any account may.
    fn open(path: &str) -> Option<Self> {
        Self::open_with(path, 0)
    }

    /// Opened to read and write, which an NVMe disk's health log needs and
    /// only an administrator gets; nothing is written.
    fn open_reading(path: &str) -> Option<Self> {
        Self::open_with(path, GENERIC_READ | GENERIC_WRITE)
    }

    fn open_with(path: &str, access: u32) -> Option<Self> {
        let path = wide(path);
        // SAFETY: a NUL-terminated path.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        (handle != INVALID_HANDLE_VALUE && !handle.is_null()).then_some(Self(handle))
    }

    /// An NVMe disk's health log, through a protocol query, as Microsoft's
    /// "Working with NVMe drives" shows.
    fn nvme_health(&self) -> Option<NvmeHealth> {
        // The query and the data share the buffer: the query's header, then
        // the protocol's request where its parameters go, then the log.
        const HEADER: usize = 8;
        const REQUEST: usize = size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>();
        const LOG: usize = 512;
        let mut buffer = [0u32; (HEADER + REQUEST + LOG) / 4];
        let bytes = size_of_val(&buffer);
        let request = STORAGE_PROTOCOL_SPECIFIC_DATA {
            ProtocolType: ProtocolTypeNvme,
            DataType: u32::try_from(NVMeDataTypeLogPage).ok()?,
            ProtocolDataRequestValue: u32::try_from(NVME_LOG_PAGE_HEALTH_INFO).ok()?,
            ProtocolDataRequestSubValue: 0,
            ProtocolDataOffset: u32::try_from(REQUEST).ok()?,
            ProtocolDataLength: u32::try_from(LOG).ok()?,
            FixedProtocolReturnData: 0,
            ProtocolDataRequestSubValue2: 0,
            ProtocolDataRequestSubValue3: 0,
            ProtocolDataRequestSubValue4: 0,
        };
        // SAFETY: the buffer holds the query's two ids, then the request.
        unsafe {
            let base = buffer.as_mut_ptr();
            base.cast::<STORAGE_PROPERTY_ID>()
                .write(StorageDeviceProtocolSpecificProperty);
            base.add(1).cast::<i32>().write(PropertyStandardQuery);
            base.add(2)
                .cast::<STORAGE_PROTOCOL_SPECIFIC_DATA>()
                .write(request);
        }
        let returned = self.control(
            IOCTL_STORAGE_QUERY_PROPERTY,
            buffer.as_ptr().cast(),
            bytes,
            buffer.as_mut_ptr().cast(),
            bytes,
        )?;
        // SAFETY: the call wrote a protocol data descriptor at the start.
        let answer = unsafe {
            buffer
                .as_ptr()
                .cast::<STORAGE_PROTOCOL_DATA_DESCRIPTOR>()
                .read()
        };
        let data = answer.ProtocolSpecificData;
        let offset = HEADER + usize::try_from(data.ProtocolDataOffset).ok()?;
        let length = usize::try_from(data.ProtocolDataLength).ok()?;
        if data.ProtocolDataOffset < u32::try_from(REQUEST).ok()?
            || length < LOG
            || returned < offset + LOG
        {
            return None;
        }
        // SAFETY: `returned` bytes, the log among them, were written.
        let all = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), bytes) };
        NvmeHealth::parse(all.get(offset..offset + LOG)?)
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
    fn reads_an_nvme_health_log() {
        let mut log = [0u8; 512];
        // Reliability degraded, at 310 K, 7% of its life used.
        log[0] = 0x04;
        log[1..3].copy_from_slice(&310u16.to_le_bytes());
        log[5] = 7;
        log[128..136].copy_from_slice(&1234u64.to_le_bytes());
        log[160..168].copy_from_slice(&2u64.to_le_bytes());
        let health = NvmeHealth::parse(&log).expect("a log");
        assert_eq!(health.critical_warning, 0x04);
        assert_eq!(health.percentage_used, 7);
        assert_eq!(health.power_on_hours, 1234);
        assert_eq!(health.media_errors, 2);
        let celsius = health.celsius().expect("a temperature");
        assert!((celsius - 36.85).abs() < 0.01, "{celsius}");
        assert!(NvmeHealth::parse(&log[..100]).is_none());
    }

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
        let names: Vec<String> = disks.iter().map(|disk| disk.name.clone()).collect();
        let health = health(&names);
        assert_eq!(health.disks.len(), names.len());
    }
}
