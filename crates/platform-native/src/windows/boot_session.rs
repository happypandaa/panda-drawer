//! The current process's Windows boot ID, without wall-clock reconstruction.
//!
//! Microsoft documents ProcessTelemetryIdInformation (64), its complete fixed
//! header, and the BootId member. This source is deliberately distinct from the
//! PrefetchParameters registry counter and from BCD boot application GUIDs.
//! https://learn.microsoft.com/windows/win32/api/winternl/nf-winternl-ntqueryinformationprocess
//! https://learn.microsoft.com/windows/win32/devnotes/process_telemetry_id_information_type
use std::{ffi::c_void, io, mem};
use windows_sys::Win32::System::{
    LibraryLoader::{GetModuleHandleW, GetProcAddress},
    Threading::{GetCurrentProcess, GetCurrentProcessId},
};

pub const SOURCE: &str = "windows-process-telemetry-class64";
const CLASS: i32 = 64;
const HEADER_SIZE: usize = 96;
const MAX_BUFFER: usize = 1024 * 1024;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xc000_0004u32 as i32;
const STATUS_BUFFER_TOO_SMALL: i32 = 0xc000_0023u32 as i32;

// Public Microsoft layout, not the undocumented SystemBootEnvironmentInformation.
#[repr(C)]
#[allow(dead_code)] // The complete ABI is retained to pin size and member offsets.
struct TelemetryHeader {
    header_size: u32,
    process_id: u32,
    process_start_key: u64,
    create_time: u64,
    create_interrupt_time: u64,
    create_unbiased_interrupt_time: u64,
    process_sequence_number: u64,
    session_create_time: u64,
    session_id: u32,
    boot_id: u32,
    image_checksum: u32,
    image_time_date_stamp: u32,
    user_sid_offset: u32,
    image_path_offset: u32,
    package_name_offset: u32,
    relative_app_name_offset: u32,
    command_line_offset: u32,
}
const _: () = assert!(mem::size_of::<TelemetryHeader>() == HEADER_SIZE);
const _: () = assert!(mem::offset_of!(TelemetryHeader, process_id) == 4);
const _: () = assert!(mem::offset_of!(TelemetryHeader, boot_id) == 60);

type Query = unsafe extern "system" fn(
    windows_sys::Win32::Foundation::HANDLE,
    i32,
    *mut c_void,
    u32,
    *mut u32,
) -> i32;

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

fn extract(bytes: &[u8], returned: usize, process_id: u32) -> io::Result<u32> {
    if returned < HEADER_SIZE || returned > bytes.len() {
        return Err(invalid("Unexpected process telemetry return length"));
    }
    let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    if word(0) as usize != HEADER_SIZE || word(4) != process_id {
        return Err(invalid(
            "Unexpected process telemetry header or process identity",
        ));
    }
    // Zero is the first valid boot ID. Never interpret a PID or timestamp as a
    // boot ID and never fall back to the unrelated registry counter.
    Ok(word(60))
}

pub fn current_boot_id() -> io::Result<u32> {
    let name: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    if module.is_null() {
        return Err(io::Error::last_os_error());
    }
    let entry = unsafe { GetProcAddress(module, c"NtQueryInformationProcess".as_ptr().cast()) }
        .ok_or_else(io::Error::last_os_error)?;
    // Runtime resolution is the documented way to tolerate an unavailable or
    // changed NtQueryInformationProcess implementation.
    let query: Query = unsafe { mem::transmute(entry) };
    let mut buffer = vec![0u64; 64 * 1024 / mem::size_of::<u64>()];
    loop {
        let length = buffer.len() * mem::size_of::<u64>();
        let mut returned = 0u32;
        let status = unsafe {
            query(
                GetCurrentProcess(),
                CLASS,
                buffer.as_mut_ptr().cast(),
                length as u32,
                &mut returned,
            )
        };
        if [STATUS_INFO_LENGTH_MISMATCH, STATUS_BUFFER_TOO_SMALL].contains(&status)
            && returned as usize > length
            && returned as usize <= MAX_BUFFER
        {
            buffer.fill(0);
            buffer.resize((returned as usize).div_ceil(mem::size_of::<u64>()), 0);
            continue;
        }
        let result = if status < 0 {
            Err(io::Error::other(format!(
                "Process boot observation failed (NTSTATUS 0x{:08x})",
                status as u32
            )))
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast(), length) };
            extract(bytes, returned as usize, unsafe { GetCurrentProcessId() })
        };
        // Variable trailing data may contain our own SID, paths or command
        // line. It never leaves this function or appears in an error/report.
        buffer.fill(0);
        return result;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_documented_bounded_current_process_header_yields_a_boot_id() {
        let mut bytes = [0u8; HEADER_SIZE];
        bytes[..4].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&42u32.to_le_bytes());
        assert_eq!(extract(&bytes, HEADER_SIZE, 42).unwrap(), 0);
        bytes[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(extract(&bytes, HEADER_SIZE, 42).unwrap(), u32::MAX);
        assert!(extract(&bytes, HEADER_SIZE - 1, 42).is_err());
        assert!(extract(&bytes, HEADER_SIZE + 1, 42).is_err());
        assert!(extract(&bytes, HEADER_SIZE, 43).is_err());
        bytes[..4].copy_from_slice(&104u32.to_le_bytes());
        assert!(extract(&bytes, HEADER_SIZE, 42).is_err());
    }

    #[test]
    fn actual_current_process_and_real_child_observe_the_same_boot() {
        let first = current_boot_id().unwrap();
        assert_eq!(first, current_boot_id().unwrap());
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows::boot_session::tests::real_child_boot_observation",
                "--ignored",
                "--color",
                "never",
            ])
            .env("PANDA_DISPOSABLE_BOOT_OBSERVATION", first.to_string())
            .env_remove("TERM")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    #[test]
    #[ignore = "subprocess fixture; started by actual_current_process_and_real_child_observe_the_same_boot"]
    fn real_child_boot_observation() {
        let expected: u32 = std::env::var("PANDA_DISPOSABLE_BOOT_OBSERVATION")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(current_boot_id().unwrap(), expected);
    }
}
