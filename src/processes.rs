//! Read-only process inventory. A process can be listed even when its full
//! image path is unavailable because it exited or access was denied.

#[derive(Debug, Clone, serde::Serialize)]
pub struct Process {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub path: Option<String>,
}

#[cfg(windows)]
mod platform {
    use super::Process;
    use std::io;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    // Constructed only after checking the API's failure sentinel.
    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: this wrapper owns a valid handle returned by Win32, and
            // closes it exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn image_path(pid: u32) -> Option<String> {
        // SAFETY: OpenProcess takes only scalar parameters. This requests query
        // access, with no inheritance and no mutation of the target process.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            return None;
        }
        let process = OwnedHandle(raw);
        // Windows extended-length paths contain at most 32,767 UTF-16 units.
        let mut buffer = vec![0u16; 32_768];
        let mut length = buffer.len() as u32;
        // SAFETY: the handle remains open and buffer has `length` writable
        // UTF-16 units. On success, length excludes the terminating NUL.
        let ok =
            unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut length) };
        if ok == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }

    pub fn list_processes() -> io::Result<Vec<Process>> {
        // SAFETY: a process snapshot does not dereference caller pointers.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = OwnedHandle(raw);
        // SAFETY: PROCESSENTRY32W contains only integer and fixed-array fields;
        // zero is valid for each. Win32 requires dwSize before enumeration.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut processes = Vec::new();
        // SAFETY: snapshot is live and entry points to a correctly sized,
        // writable PROCESSENTRY32W.
        let mut ok = unsafe { Process32FirstW(snapshot.0, &mut entry) };
        loop {
            if ok == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    break;
                }
                return Err(error);
            }
            let end = entry
                .szExeFile
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            processes.push(Process {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
                name: String::from_utf16_lossy(&entry.szExeFile[..end]),
                path: image_path(entry.th32ProcessID),
            });
            // SAFETY: the owned snapshot and mutable entry remain valid.
            ok = unsafe { Process32NextW(snapshot.0, &mut entry) };
        }
        Ok(processes)
    }
}

#[cfg(windows)]
pub use platform::list_processes;

#[cfg(not(windows))]
pub fn list_processes() -> std::io::Result<Vec<Process>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Process enumeration requires Windows",
    ))
}
