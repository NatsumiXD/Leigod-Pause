//! Separate watchdog survives console close / forced controller termination.
//! Only exact accelerator executable names within the selected install are closed.
use crate::processes::{self, Process};
use crate::{exit_pause, session_end};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[derive(Serialize, Deserialize)]
struct Config {
    controller_pid: u32,
    root: PathBuf,
    audit_path: PathBuf,
}
pub struct Binding {
    directory: PathBuf,
    child: Child,
    armed: bool,
    audit_path: PathBuf,
}
impl Binding {
    pub fn start(exe: &Path) -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("leigod-lifecycle-{}-{stamp}", std::process::id()));
        fs::create_dir(&directory)?;
        let config = directory.join("watchdog.json");
        let log_directory = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("Missing controller directory"))?
            .join("logs");
        fs::create_dir_all(&log_directory)?;
        let audit_path = log_directory.join(format!("exit-{}-{stamp}.json", std::process::id()));
        fs::write(
            &config,
            serde_json::to_vec(&Config {
                controller_pid: std::process::id(),
                root: exe
                    .parent()
                    .ok_or_else(|| io::Error::other("Missing install directory"))?
                    .to_path_buf(),
                audit_path: audit_path.clone(),
            })
            .map_err(io::Error::other)?,
        )?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--watchdog")
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !directory.join("ready").exists() {
            if child.try_wait()?.is_some() {
                return Err(io::Error::other("进程绑定监护器启动失败"));
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("进程绑定监护器启动超时"));
            }
            thread::sleep(Duration::from_millis(25));
        }
        Ok(Self {
            directory,
            child,
            armed: false,
            audit_path,
        })
    }
    pub fn arm(&mut self, pid: u32) -> io::Result<()> {
        if !self.armed {
            fs::write(self.directory.join("armed"), pid.to_string())?;
            self.armed = true;
        }
        Ok(())
    }
    pub fn client_exited(&self) -> bool {
        self.directory.join("client-exited").exists()
    }
    pub fn has_client(&self) -> bool {
        self.armed
    }
    pub fn set_endpoint(&self, mut endpoint: exit_pause::Endpoint) -> io::Result<()> {
        endpoint.receipt_path = Some(self.directory.join("pause-result.json"));
        endpoint.audit_path = Some(self.audit_path.clone());
        fs::write(
            self.directory.join("pause-transport.json"),
            serde_json::to_vec(&endpoint).map_err(io::Error::other)?,
        )?;
        exit_pause::register(endpoint);
        Ok(())
    }
}
static WATCHDOG_END: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn watchdog_session_ending() {
    WATCHDOG_END.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = exit_pause::request_current(Duration::from_millis(3300));
}
impl Drop for Binding {
    fn drop(&mut self) {
        if self.client_exited() {
            return;
        }
        let _ = fs::write(self.directory.join("shutdown"), b"close");
        let deadline = std::time::Instant::now() + Duration::from_secs(7);
        while self.child.try_wait().ok().flatten().is_none() && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(50));
        }
    }
}
fn belongs(process: &Process, root: &Path) -> bool {
    if !["leigod.exe", "leigod_launcher.exe", "leishensdk.exe"]
        .iter()
        .any(|name| process.name.eq_ignore_ascii_case(name))
    {
        return false;
    }
    let Some(path) = &process.path else {
        return false;
    };
    let normalize = |s: &str| {
        s.trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    normalize(path).starts_with(&(normalize(&root.to_string_lossy()) + "\\"))
}
#[cfg(windows)]
pub fn watchdog(config_path: &Path) -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
        },
    };
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    fn selected(handle: HANDLE, root: &Path) -> bool {
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        // Verify the opened handle, not only the earlier PID snapshot: a PID
        // may have been reused between enumeration and OpenProcess.
        if unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) } == 0 {
            return false;
        }
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        let name = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        belongs(
            &Process {
                pid: 0,
                parent_pid: 0,
                name,
                path: Some(path),
            },
            root,
        )
    }
    let bytes = fs::read(config_path)?;
    if bytes.len() > 32768 {
        return Err(io::Error::other("Invalid watchdog configuration"));
    }
    let config: Config = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let directory = config_path
        .parent()
        .ok_or_else(|| io::Error::other("Invalid watchdog path"))?;
    let inventory = processes::list_processes()?;
    if inventory
        .iter()
        .find(|p| p.pid == std::process::id())
        .is_none_or(|p| p.parent_pid != config.controller_pid)
    {
        return Err(io::Error::other("Watchdog controller mismatch"));
    }
    if !config.root.join("leigod.exe").is_file() || !config.root.join("resources").is_dir() {
        return Err(io::Error::other("Invalid watchdog install directory"));
    }
    let raw = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
            0,
            config.controller_pid,
        )
    };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let parent = Handle(raw);
    let _session_end = session_end::SessionEnd::start(watchdog_session_ending, 0x3e0)?;
    fs::write(directory.join("ready"), b"ready")?;
    let mut client: Option<Handle> = None;
    let mut armed = false;
    loop {
        if let Ok(bytes) = fs::read(directory.join("pause-transport.json"))
            && bytes.len() < 16384
            && let Ok(endpoint) = serde_json::from_slice::<exit_pause::Endpoint>(&bytes)
            && exit_pause::current().is_none_or(|old| old.id != endpoint.id)
        {
            exit_pause::register(endpoint);
        }
        if !armed
            && let Ok(pid) = fs::read_to_string(directory.join("armed"))
            && let Ok(pid) = pid.trim().parse::<u32>()
            && let Some(process) = processes::list_processes()?.into_iter().find(|p| {
                p.pid == pid
                    && p.name.eq_ignore_ascii_case("leigod.exe")
                    && belongs(p, &config.root)
            })
        {
            let raw = unsafe {
                OpenProcess(
                    PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    process.pid,
                )
            };
            if !raw.is_null() {
                let owned = Handle(raw);
                if selected(owned.0, &config.root) {
                    client = Some(owned);
                    armed = true;
                }
            }
        }
        let parent_exited = unsafe { WaitForSingleObject(parent.0, 0) } == WAIT_OBJECT_0;
        let client_exited = client
            .as_ref()
            .is_some_and(|client| unsafe { WaitForSingleObject(client.0, 0) } == WAIT_OBJECT_0);
        if parent_exited
            || directory.join("shutdown").exists()
            || client_exited
            || WATCHDOG_END.load(std::sync::atomic::Ordering::Relaxed)
        {
            // Keep the renderer alive until the pause attempt has completed or
            // its bounded exit budget elapsed, including forced parent death.
            let result = exit_pause::request_current(Duration::from_millis(3500));
            let _ = fs::write(&config.audit_path, result.to_string());
            for _ in 0..3 {
                // No /T tree kill: a game launched by the accelerator is never selected.
                for process in processes::list_processes()?
                    .into_iter()
                    .filter(|p| belongs(p, &config.root))
                {
                    let raw = unsafe {
                        OpenProcess(
                            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                            0,
                            process.pid,
                        )
                    };
                    if !raw.is_null() {
                        let owned = Handle(raw);
                        if selected(owned.0, &config.root) {
                            unsafe { TerminateProcess(owned.0, 0) };
                        }
                    }
                }
                thread::sleep(Duration::from_millis(200));
            }
            if client_exited && !parent_exited {
                let _ = fs::write(directory.join("client-exited"), b"closed");
                if unsafe { WaitForSingleObject(parent.0, 30000) } != WAIT_OBJECT_0 {
                    unsafe { TerminateProcess(parent.0, 0) };
                }
            }
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    for name in [
        "watchdog.json",
        "armed",
        "shutdown",
        "client-exited",
        "ready",
        "pause-transport.json",
        "pause-result.json",
    ] {
        let _ = fs::remove_file(directory.join(name));
    }
    let _ = fs::remove_dir(directory);
    Ok(())
}
#[cfg(not(windows))]
pub fn watchdog(_path: &Path) -> io::Result<()> {
    Err(io::Error::other("Windows required"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn games_and_other_installations_are_not_closed() {
        let mut p = Process {
            pid: 1,
            parent_pid: 0,
            name: "game.exe".into(),
            path: Some(r"D:\LeiGod\game.exe".into()),
        };
        assert!(!belongs(&p, Path::new(r"D:\LeiGod")));
        p.name = "leigod.exe".into();
        p.path = Some(r"D:\LeiGod-other\leigod.exe".into());
        assert!(!belongs(&p, Path::new(r"D:\LeiGod")));
        p.path = Some(r"D:\LeiGod\leigod.exe".into());
        assert!(belongs(&p, Path::new(r"D:\LeiGod")));
    }
}
