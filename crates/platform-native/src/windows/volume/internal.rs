//! Read-only volume -> disk -> PnP observations. Device numbers are scoped to
//! this call; Windows may reuse them after device removal or a system restart.
//! No fixed-drive, bus-type or absence-of-hotplug heuristic supplies membership.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io,
    mem::{offset_of, size_of},
    os::windows::io::{AsRawHandle, FromRawHandle},
    ptr,
};
use windows_sys::Win32::{
    Devices::{
        DeviceAndDriverInstallation::{
            CM_Get_DevNode_PropertyW, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
            SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, CR_SUCCESS,
            DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SPINT_ACTIVE, SPINT_REMOVED,
            SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
        },
        Properties::{DEVPKEY_Device_InLocalMachineContainer, DEVPROP_TYPE_BOOLEAN},
    },
    Foundation::{
        ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        BusTypeFibre, BusTypeFileBackedVirtual, BusTypeMax, BusTypeSpaces, BusTypeUnknown,
        BusTypeVirtual, BusTypeiScsi, CreateFileW, GetFinalPathNameByHandleW, FILE_DEVICE_DISK,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        OPEN_EXISTING, VOLUME_NAME_GUID,
    },
    System::{
        Ioctl::{
            PropertyStandardQuery, StorageDeviceProperty, DISK_EXTENT, GUID_DEVINTERFACE_DISK,
            IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY,
            STORAGE_DESCRIPTOR_HEADER, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER,
            STORAGE_PROPERTY_QUERY, VOLUME_DISK_EXTENTS,
        },
        IO::DeviceIoControl,
    },
};

const MAX_QUERY_BYTES: usize = 1024 * 1024;
const MAX_QUERY_ATTEMPTS: usize = 8;
const MAX_INTERFACES: u32 = 65536;

#[derive(Debug)]
pub(super) struct Observation {
    pub(super) internal: Option<bool>,
    pub(super) disk_numbers: Vec<u32>,
    pub(super) diagnostic: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Extent {
    disk: u32,
    offset: i64,
    length: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeviceNumber {
    kind: u32,
    disk: u32,
    partition: u32,
}

struct MatchedDisk {
    file: File,
    number: DeviceNumber,
    devinst: u32,
    membership: Option<bool>,
    bus: u32,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn unknown(error: impl std::fmt::Display, disk_numbers: Vec<u32>) -> Observation {
    Observation {
        internal: None,
        disk_numbers,
        diagnostic: Some(error.to_string()),
    }
}

pub(super) fn observe(directory: &File) -> Observation {
    let volume = match volume_device_name(directory).and_then(|name| open_device(&name)) {
        Ok(volume) => volume,
        Err(error) => {
            return unknown(
                format!("Volume device observation unavailable: {error}"),
                vec![],
            )
        }
    };
    let before = match volume_extents(&volume) {
        Ok(extents) => extents,
        Err(error) => return unknown(format!("Volume disk extents unavailable: {error}"), vec![]),
    };
    let numbers: BTreeSet<_> = before.iter().map(|extent| extent.disk).collect();
    let disk_numbers: Vec<_> = numbers.iter().copied().collect();
    let disks = match matched_disks(&numbers) {
        Ok(disks) => disks,
        Err(error) => {
            return unknown(
                format!("Backing disk observation unavailable: {error}"),
                disk_numbers,
            )
        }
    };
    // Keep all matching handles until both the disk identities and the complete
    // volume extent map have been observed again. Never cache a disk-number map.
    for disk in disks.values() {
        match device_number(&disk.file) {
            Ok(after) if after == disk.number => {}
            Ok(_) => {
                return unknown(
                    "Backing disk identity changed during observation",
                    disk_numbers,
                )
            }
            Err(error) => {
                return unknown(
                    format!("Backing disk disappeared during observation: {error}"),
                    disk_numbers,
                )
            }
        }
    }
    match volume_extents(&volume) {
        Ok(after) if after == before => {}
        Ok(_) => {
            return unknown(
                "Volume disk extents changed during observation",
                disk_numbers,
            )
        }
        Err(error) => {
            return unknown(
                format!("Volume disk extents could not be rechecked: {error}"),
                disk_numbers,
            )
        }
    }
    let internal = aggregate(disks.values().map(|disk| disk.membership));
    Observation {
        internal,
        disk_numbers,
        diagnostic: internal.is_none().then(|| {
            "A backing disk has unknown or virtual storage topology; physical internal-device membership is unresolved".into()
        }),
    }
}

fn aggregate(memberships: impl IntoIterator<Item = Option<bool>>) -> Option<bool> {
    let mut any = false;
    let mut unknown = false;
    for membership in memberships {
        any = true;
        match membership {
            Some(false) => return Some(false),
            None => unknown = true,
            Some(true) => {}
        }
    }
    (any && !unknown).then_some(true)
}

fn volume_device_name(directory: &File) -> io::Result<Vec<u16>> {
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            directory.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_GUID,
        )
    };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize >= buffer.len() {
        return Err(invalid(
            "Volume GUID path exceeds the Windows namespace limit",
        ));
    }
    volume_name_from_final_path(&buffer[..length as usize])
}

fn volume_name_from_final_path(path: &[u16]) -> io::Result<Vec<u16>> {
    let prefix: Vec<_> = "\\\\?\\Volume{".encode_utf16().collect();
    if !path.starts_with(&prefix) {
        return Err(invalid(
            "Directory handle did not return a volume GUID path",
        ));
    }
    let end = path
        .iter()
        .enumerate()
        .skip(prefix.len())
        .find_map(|(index, unit)| (*unit == b'\\' as u16).then_some(index))
        .ok_or_else(|| invalid("Volume GUID path has no root separator"))?;
    if path[end - 1] != b'}' as u16 || path[..end].contains(&0) {
        return Err(invalid("Malformed volume GUID path"));
    }
    // A volume device is opened without its trailing root separator.
    Ok(path[..end].iter().copied().chain([0]).collect())
}

fn open_device(name: &[u16]) -> io::Result<File> {
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(raw) })
}

fn extent_buffer_size(count: u32) -> io::Result<usize> {
    if count == 0 {
        return Err(invalid("Volume reported no backing disk extents"));
    }
    let size = (count as usize)
        .checked_mul(size_of::<DISK_EXTENT>())
        .and_then(|bytes| bytes.checked_add(offset_of!(VOLUME_DISK_EXTENTS, Extents)))
        .filter(|bytes| *bytes <= MAX_QUERY_BYTES)
        .ok_or_else(|| invalid("Volume disk extents exceed the observation buffer limit"))?;
    Ok(size)
}

fn parse_extents(bytes: &[u8]) -> io::Result<Vec<Extent>> {
    let count = bytes
        .get(..4)
        .map(|value| u32::from_ne_bytes(value.try_into().unwrap()))
        .ok_or_else(|| invalid("Truncated volume disk extent header"))?;
    let required = extent_buffer_size(count)?;
    if bytes.len() < required {
        return Err(invalid("Truncated volume disk extents"));
    }
    let mut extents = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let offset = offset_of!(VOLUME_DISK_EXTENTS, Extents) + index * size_of::<DISK_EXTENT>();
        // The native buffer is byte-aligned; never create a misaligned reference.
        let extent =
            unsafe { ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<DISK_EXTENT>()) };
        extents.push(Extent {
            disk: extent.DiskNumber,
            offset: extent.StartingOffset,
            length: extent.ExtentLength,
        });
    }
    Ok(extents)
}

fn volume_extents(volume: &File) -> io::Result<Vec<Extent>> {
    let mut bytes = vec![0u8; size_of::<VOLUME_DISK_EXTENTS>()];
    for _ in 0..MAX_QUERY_ATTEMPTS {
        let mut returned = 0;
        let success = unsafe {
            DeviceIoControl(
                volume.as_raw_handle(),
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                ptr::null(),
                0,
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if success != 0 {
            let returned = returned as usize;
            if returned > bytes.len() {
                return Err(invalid("Volume extent reply exceeds its buffer"));
            }
            return parse_extents(&bytes[..returned]);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
            return Err(error);
        }
        let wanted = if returned >= 4 {
            extent_buffer_size(u32::from_ne_bytes(bytes[..4].try_into().unwrap()))?
        } else {
            bytes.len().saturating_mul(2)
        };
        let wanted = wanted.max(bytes.len().saturating_mul(2));
        if wanted > MAX_QUERY_BYTES {
            return Err(invalid(
                "Volume disk extents exceed the observation buffer limit",
            ));
        }
        bytes.resize(wanted, 0);
    }
    Err(invalid(
        "Volume disk extent map did not stabilize during observation",
    ))
}

fn ioctl(
    file: &File,
    code: u32,
    input: *const std::ffi::c_void,
    input_size: u32,
    output: &mut [u8],
) -> io::Result<usize> {
    let mut returned = 0;
    let success = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            input,
            input_size,
            output.as_mut_ptr().cast(),
            output.len() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    if returned as usize > output.len() {
        return Err(invalid("Storage reply exceeds its buffer"));
    }
    Ok(returned as usize)
}

fn device_number(file: &File) -> io::Result<DeviceNumber> {
    let mut bytes = [0u8; size_of::<STORAGE_DEVICE_NUMBER>()];
    let returned = ioctl(
        file,
        IOCTL_STORAGE_GET_DEVICE_NUMBER,
        ptr::null(),
        0,
        &mut bytes,
    )?;
    if returned < bytes.len() {
        return Err(invalid("Truncated storage device number"));
    }
    let number = unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<STORAGE_DEVICE_NUMBER>()) };
    Ok(DeviceNumber {
        kind: number.DeviceType,
        disk: number.DeviceNumber,
        partition: number.PartitionNumber,
    })
}

fn storage_bus(file: &File) -> io::Result<u32> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    let input = (&query as *const STORAGE_PROPERTY_QUERY).cast();
    let mut header = [0u8; size_of::<STORAGE_DESCRIPTOR_HEADER>()];
    let returned = ioctl(
        file,
        IOCTL_STORAGE_QUERY_PROPERTY,
        input,
        size_of::<STORAGE_PROPERTY_QUERY>() as u32,
        &mut header,
    )?;
    if returned < header.len() {
        return Err(invalid("Truncated storage descriptor header"));
    }
    let size = u32::from_ne_bytes(header[4..8].try_into().unwrap()) as usize;
    let bus_end = offset_of!(STORAGE_DEVICE_DESCRIPTOR, BusType) + size_of::<u32>();
    if !(bus_end..=MAX_QUERY_BYTES).contains(&size) {
        return Err(invalid("Storage descriptor size is invalid"));
    }
    let mut bytes = vec![0u8; size];
    let returned = ioctl(
        file,
        IOCTL_STORAGE_QUERY_PROPERTY,
        input,
        size_of::<STORAGE_PROPERTY_QUERY>() as u32,
        &mut bytes,
    )?;
    if returned < bus_end {
        return Err(invalid("Truncated storage device descriptor"));
    }
    let actual_size = u32::from_ne_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if actual_size < bus_end || actual_size > returned {
        return Err(invalid(
            "Storage device descriptor changed size or is incomplete",
        ));
    }
    Ok(u32::from_ne_bytes(
        bytes[bus_end - 4..bus_end].try_into().unwrap(),
    ))
}

fn parse_membership(property_type: u32, bytes: &[u8]) -> io::Result<bool> {
    if property_type != DEVPROP_TYPE_BOOLEAN {
        return Err(invalid(
            "Local-machine membership property has the wrong type",
        ));
    }
    match bytes {
        [0] => Ok(false),
        [255] => Ok(true),
        _ => Err(invalid(
            "Local-machine membership property is not a native Boolean",
        )),
    }
}

fn membership(devinst: u32) -> io::Result<bool> {
    let mut property_type = 0;
    let mut value = [0u8; 1];
    let mut length = value.len() as u32;
    let result = unsafe {
        CM_Get_DevNode_PropertyW(
            devinst,
            &DEVPKEY_Device_InLocalMachineContainer,
            &mut property_type,
            value.as_mut_ptr(),
            &mut length,
            0,
        )
    };
    if result != CR_SUCCESS {
        return Err(invalid(format!(
            "Local-machine membership query returned CONFIGRET {result}"
        )));
    }
    if length as usize != value.len() {
        return Err(invalid(
            "Local-machine membership property has the wrong length",
        ));
    }
    parse_membership(property_type, &value)
}

fn physical_membership(membership: bool, bus: u32) -> Option<bool> {
    if !membership {
        return Some(false);
    }
    // Logical virtual/fabric disks can be assigned to the local-machine
    // container. Their physical backing requires a separate topology query;
    // membership in that container alone does not prove internal media.
    if !(1..BusTypeMax as u32).contains(&bus)
        || [
            BusTypeUnknown,
            BusTypeVirtual,
            BusTypeFileBackedVirtual,
            BusTypeSpaces,
            BusTypeFibre,
            BusTypeiScsi,
        ]
        .contains(&(bus as i32))
    {
        None
    } else {
        Some(true)
    }
}

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn interface_detail(
    set: &DeviceSet,
    interface: &SP_DEVICE_INTERFACE_DATA,
) -> io::Result<(Vec<u16>, SP_DEVINFO_DATA)> {
    let mut required = 0;
    let success = unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set.0,
            interface,
            ptr::null_mut(),
            0,
            &mut required,
            ptr::null_mut(),
        )
    };
    if success == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(error);
        }
    }
    let path_offset = offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    if required as usize > MAX_QUERY_BYTES || (required as usize) < path_offset + 2 {
        return Err(invalid("Disk interface detail size is invalid"));
    }
    // x64 detail.cbSize is 8 while DevicePath begins at byte 4. Use the SDK
    // layout and aligned allocation, never an assumed pointer-size offset.
    let mut allocation = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
    let detail = allocation
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    unsafe {
        (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
    }
    let capacity = required;
    let mut device = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };
    let success = unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set.0,
            interface,
            detail,
            capacity,
            &mut required,
            &mut device,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    if required > capacity
        || (required as usize) < path_offset + 2
        || !(required as usize - path_offset).is_multiple_of(2)
    {
        return Err(invalid("Disk interface path length is invalid"));
    }
    let path = unsafe {
        std::slice::from_raw_parts(
            allocation
                .as_ptr()
                .cast::<u8>()
                .add(path_offset)
                .cast::<u16>(),
            (required as usize - path_offset) / 2,
        )
    };
    let end = path
        .iter()
        .position(|unit| *unit == 0)
        .filter(|end| *end > 0)
        .ok_or_else(|| invalid("Disk interface path is not terminated"))?;
    Ok((path[..=end].to_vec(), device))
}

fn matched_disks(wanted: &BTreeSet<u32>) -> io::Result<BTreeMap<u32, MatchedDisk>> {
    let raw = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_DISK,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if raw == INVALID_HANDLE_VALUE as isize {
        return Err(io::Error::last_os_error());
    }
    let set = DeviceSet(raw);
    let mut found: BTreeMap<u32, MatchedDisk> = BTreeMap::new();
    for index in 0..MAX_INTERFACES {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        let success = unsafe {
            SetupDiEnumDeviceInterfaces(
                set.0,
                ptr::null(),
                &GUID_DEVINTERFACE_DISK,
                index,
                &mut interface,
            )
        };
        if success == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                if found.len() != wanted.len() {
                    return Err(invalid(
                        "Not every backing disk could be matched to a present PnP disk interface",
                    ));
                }
                return Ok(found);
            }
            return Err(error);
        }
        if interface.Flags & SPINT_ACTIVE == 0 || interface.Flags & SPINT_REMOVED != 0 {
            continue;
        }
        // An inaccessible unrelated disk does not prevent observing the actual
        // backing disks. An inaccessible matching disk remains unmatched below.
        let Ok((name, device)) = interface_detail(&set, &interface) else {
            continue;
        };
        let Ok(file) = open_device(&name) else {
            continue;
        };
        let Ok(number) = device_number(&file) else {
            continue;
        };
        if number.kind != FILE_DEVICE_DISK || !wanted.contains(&number.disk) {
            continue;
        }
        let observed_membership = membership(device.DevInst)?;
        let bus = storage_bus(&file)?;
        if device_number(&file)? != number {
            return Err(invalid(
                "Disk identity changed while its PnP membership was queried",
            ));
        }
        if let Some(previous) = found.get(&number.disk) {
            if previous.devinst != device.DevInst
                || previous.bus != bus
                || previous.membership != physical_membership(observed_membership, bus)
            {
                return Err(invalid(
                    "Backing disk number has conflicting PnP interfaces",
                ));
            }
        } else {
            found.insert(
                number.disk,
                MatchedDisk {
                    file,
                    number,
                    devinst: device.DevInst,
                    membership: physical_membership(observed_membership, bus),
                    bus,
                },
            );
        }
    }
    Err(invalid(
        "Disk interface enumeration exceeded its observation limit",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent_bytes(entries: &[(u32, i64, i64)]) -> Vec<u8> {
        let mut bytes = vec![
            0u8;
            offset_of!(VOLUME_DISK_EXTENTS, Extents)
                + entries.len() * size_of::<DISK_EXTENT>()
        ];
        bytes[..4].copy_from_slice(&(entries.len() as u32).to_ne_bytes());
        for (index, (disk, offset, length)) in entries.iter().enumerate() {
            let extent = DISK_EXTENT {
                DiskNumber: *disk,
                StartingOffset: *offset,
                ExtentLength: *length,
            };
            let start = offset_of!(VOLUME_DISK_EXTENTS, Extents) + index * size_of::<DISK_EXTENT>();
            unsafe {
                ptr::write_unaligned(bytes.as_mut_ptr().add(start).cast::<DISK_EXTENT>(), extent);
            }
        }
        bytes
    }

    #[test]
    fn parses_complete_disk_extent_maps_without_dropping_additional_disks() {
        let bytes = extent_bytes(&[(2, 4096, 8192), (7, 16384, 32768), (2, 65536, 4096)]);
        let parsed = parse_extents(&bytes).unwrap();
        assert_eq!(
            parsed.iter().map(|entry| entry.disk).collect::<Vec<_>>(),
            [2, 7, 2]
        );
        assert_eq!(parsed[1].offset, 16384);
        assert_eq!(parsed[1].length, 32768);
        assert_eq!(
            parsed
                .iter()
                .map(|entry| entry.disk)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([2, 7])
        );
    }

    #[test]
    fn truncated_zero_and_oversized_extent_maps_cannot_supply_device_membership() {
        assert!(parse_extents(&[]).is_err());
        assert!(parse_extents(&[0; 4]).is_err());
        let bytes = extent_bytes(&[(0, 0, 4096)]);
        assert!(parse_extents(&bytes[..bytes.len() - 1]).is_err());
        assert!(parse_extents(&u32::MAX.to_ne_bytes()).is_err());
        assert!(extent_buffer_size(u32::MAX).is_err());
    }

    #[test]
    fn only_exact_native_boolean_properties_supply_local_machine_membership() {
        assert!(parse_membership(DEVPROP_TYPE_BOOLEAN, &[255]).unwrap());
        assert!(!parse_membership(DEVPROP_TYPE_BOOLEAN, &[0]).unwrap());
        for bytes in [&[][..], &[1][..], &[2][..], &[254][..], &[255, 0][..]] {
            assert!(parse_membership(DEVPROP_TYPE_BOOLEAN, bytes).is_err());
        }
        assert!(parse_membership(7, &[255]).is_err());
    }

    #[test]
    fn membership_aggregation_never_turns_missing_or_virtual_evidence_into_internal() {
        assert_eq!(aggregate([]), None);
        assert_eq!(aggregate([Some(true), Some(true)]), Some(true));
        assert_eq!(aggregate([Some(true), None]), None);
        assert_eq!(aggregate([None, Some(false)]), Some(false));
        assert_eq!(aggregate([Some(false), Some(true)]), Some(false));
        for bus in [
            BusTypeUnknown,
            BusTypeVirtual,
            BusTypeFileBackedVirtual,
            BusTypeSpaces,
            BusTypeFibre,
            BusTypeiScsi,
            BusTypeMax,
            127,
        ] {
            assert_eq!(physical_membership(true, bus as u32), None);
        }
        assert_eq!(physical_membership(true, u32::MAX), None);
        assert_eq!(
            physical_membership(false, BusTypeVirtual as u32),
            Some(false)
        );
        assert_eq!(physical_membership(true, 17), Some(true));
    }

    #[test]
    fn volume_device_names_come_only_from_guid_paths_returned_for_the_handle() {
        let value: Vec<_> =
            "\\\\?\\Volume{01234567-89ab-cdef-0123-456789abcdef}\\中文 space\\child"
                .encode_utf16()
                .collect();
        let parsed = volume_name_from_final_path(&value).unwrap();
        assert_eq!(
            parsed,
            "\\\\?\\Volume{01234567-89ab-cdef-0123-456789abcdef}\0"
                .encode_utf16()
                .collect::<Vec<_>>()
        );
        for value in [
            "C:\\child",
            "\\\\server\\share\\child",
            "\\\\?\\UNC\\server\\share\\child",
            "\\\\?\\Volume{unfinished",
        ] {
            assert!(
                volume_name_from_final_path(&value.encode_utf16().collect::<Vec<_>>()).is_err()
            );
        }
    }
}
