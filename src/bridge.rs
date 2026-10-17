use serde_json::{Value, json};
use std::{
    error::Error,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
pub struct Bridge {
    child: Option<Child>,
    input: Option<ChildStdin>,
    pub events: mpsc::Receiver<Value>,
    reader: Option<thread::JoinHandle<()>>,
    #[cfg(windows)]
    elevated: Option<elevated::Elevated>,
}
impl Bridge {
    pub fn exit_endpoint(&self) -> Option<crate::exit_pause::Endpoint> {
        #[cfg(windows)]
        if let Some(helper) = &self.elevated {
            return Some(helper.exit_endpoint());
        }
        None
    }
    pub fn send_control(&self, command: &Value) -> std::io::Result<()> {
        #[cfg(windows)]
        if let Some(helper) = &self.elevated {
            return helper.send_control(command);
        }
        Err(std::io::Error::other("Control transport unavailable"))
    }
    pub fn control_result(&self) -> Option<Value> {
        #[cfg(windows)]
        if let Some(helper) = &self.elevated {
            return helper.control_result();
        }
        None
    }
    pub fn start(node: &Path, source: &str, configuration: Value) -> Result<Self, Box<dyn Error>> {
        let mut cmd = Command::new(node);
        cmd.args(["-e", source])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn()?;
        let mut input = child.stdin.take().ok_or("Node stdin unavailable")?;
        if let Err(e) = writeln!(input, "{configuration}") {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e.into());
        }
        let output = child.stdout.take().ok_or("Node stdout unavailable")?;
        let (tx, events) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let event = match line {
                    Ok(line) => serde_json::from_str(&line).unwrap_or_else(
                        |_| json!({"kind":"error","error":"Invalid bridge NDJSON"}),
                    ),
                    Err(e) => json!({"kind":"error","error":e.to_string()}),
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child: Some(child),
            input: Some(input),
            events,
            reader: Some(reader),
            #[cfg(windows)]
            elevated: None,
        })
    }

    /// Starts a hidden helper inheriting the release launcher's administrator token.
    /// Its private-file transport supplies lifecycle control and events without
    /// requiring ShellExecuteExW to inherit anonymous stdin/stdout pipes.
    #[cfg(windows)]
    pub fn start_elevated(
        node: &Path,
        source: &str,
        configuration: Value,
    ) -> Result<Self, Box<dyn Error>> {
        let helper = elevated::Elevated::start(node, source, configuration)?;
        let (tx, events) = mpsc::channel();
        let reader = helper.reader(tx)?;
        Ok(Self {
            child: None,
            input: None,
            events,
            reader: Some(reader),
            elevated: Some(helper),
        })
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(mut input) = self.input.take() {
            let _ = writeln!(input, "{{\"command\":\"stop\"}}");
        }
        if let Some(child) = self.child.as_mut() {
            let deadline = Instant::now() + Duration::from_secs(6);
            loop {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            let _ = child.wait();
        }
        #[cfg(windows)]
        if let Some(helper) = self.elevated.as_mut() {
            helper.stop_and_wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(windows)]
mod elevated {
    use super::*;
    use std::{
        ffi::OsStr,
        fs::{self, File},
        io::{self, Read},
        os::windows::ffi::OsStrExt,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
        System::Threading::WaitForSingleObject,
        UI::{
            Shell::{
                SEE_MASK_NO_CONSOLE, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
            },
            WindowsAndMessaging::SW_HIDE,
        },
    };

    struct ProcessHandle(HANDLE);
    // Windows process handles permit concurrent waits from any thread. The Arc
    // keeps the sole owning wrapper alive until all waiters finish.
    unsafe impl Send for ProcessHandle {}
    unsafe impl Sync for ProcessHandle {}
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            // SAFETY: this is the sole owner of ShellExecuteExW's process handle.
            unsafe { CloseHandle(self.0) };
        }
    }

    struct Files {
        directory: PathBuf,
        script: PathBuf,
        config: PathBuf,
        events: PathBuf,
        stop: PathBuf,
        snapshot: PathBuf,
        heartbeat: PathBuf,
        command: PathBuf,
        command_pending: PathBuf,
        result: PathBuf,
        exit_request: PathBuf,
        exit_result: PathBuf,
    }
    impl Files {
        fn create() -> io::Result<Self> {
            let epoch = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let root = std::path::absolute(std::env::temp_dir())?;
            // create_dir, rather than create_dir_all, ensures we never reuse
            // somebody else's directory, even in the event of a collision.
            for attempt in 0..16 {
                let directory = root.join(format!(
                    "leigod-status-{}-{epoch}-{attempt}",
                    std::process::id()
                ));
                match fs::create_dir(&directory) {
                    Ok(()) => {
                        return Ok(Self {
                            script: directory.join("helper.cjs"),
                            config: directory.join("config.json"),
                            events: directory.join("events.ndjson"),
                            stop: directory.join("stop"),
                            snapshot: directory.join("main-snapshot.json"),
                            heartbeat: directory.join("heartbeat"),
                            command: directory.join("command.json"),
                            command_pending: directory.join("command.tmp"),
                            result: directory.join("result.json"),
                            exit_request: directory.join("exit-request.json"),
                            exit_result: directory.join("exit-result.json"),
                            directory,
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Cannot create a unique bridge transport directory",
            ))
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            // Only remove the files that this helper owns. Unknown files keep
            // the leaf directory nonempty and are deliberately preserved.
            for path in [
                &self.script,
                &self.config,
                &self.events,
                &self.stop,
                &self.snapshot,
                &self.heartbeat,
                &self.command,
                &self.command_pending,
                &self.result,
                &self.exit_request,
                &self.exit_result,
            ] {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_dir(&self.directory);
        }
    }

    pub(super) struct Elevated {
        process: Arc<ProcessHandle>,
        files: Files,
        reader_stop: Arc<AtomicBool>,
        stopped: bool,
    }
    impl Elevated {
        pub(super) fn start(
            node: &Path,
            source: &str,
            mut configuration: Value,
        ) -> Result<Self, Box<dyn Error>> {
            if !configuration.is_object() {
                return Err("Bridge configuration must be a JSON object".into());
            }
            let files = Files::create()?;
            let events_path = files.events.to_str().ok_or("Non-Unicode transport path")?;
            let stop_path = files.stop.to_str().ok_or("Non-Unicode transport path")?;
            configuration["transport"] = json!({
                "eventsFile": events_path,
                "stopFile": stop_path,
                "snapshotFile": files.snapshot.to_str().ok_or("Non-Unicode snapshot path")?,
                "heartbeatFile": files.heartbeat.to_str().ok_or("Non-Unicode heartbeat path")?,
                "commandFile": files.command.to_str().ok_or("Non-Unicode command path")?,
                "resultFile": files.result.to_str().ok_or("Non-Unicode result path")?,
                "exitFile":files.exit_request.to_str().ok_or("Non-Unicode exit path")?,
                "exitResultFile":files.exit_result.to_str().ok_or("Non-Unicode exit result path")?,
                "exitCommandId":format!("exit-{}",files.directory.file_name().unwrap().to_string_lossy()),
            });
            fs::write(&files.script, source)?;
            fs::write(&files.config, serde_json::to_vec(&configuration)?)?;

            let verb = wide(OsStr::new("open"))?;
            let executable = wide(node.as_os_str())?;
            let directory = wide(files.directory.as_os_str())?;
            let mut parameters = quote_argument(files.script.as_os_str())?;
            parameters.push(b' ' as u16);
            parameters.extend(quote_argument(files.config.as_os_str())?);
            parameters.push(0);
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE,
                lpVerb: verb.as_ptr(),
                lpFile: executable.as_ptr(),
                lpParameters: parameters.as_ptr(),
                lpDirectory: directory.as_ptr(),
                nShow: SW_HIDE,
                ..Default::default()
            };
            // SAFETY: every string is NUL-terminated and remains live throughout
            // the call; info has the correct architecture-specific Win32 layout.
            if unsafe { ShellExecuteExW(&mut info) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
            if info.hProcess.is_null() {
                return Err("Windows did not return the elevated helper process handle".into());
            }
            Ok(Self {
                process: Arc::new(ProcessHandle(info.hProcess)),
                files,
                reader_stop: Arc::new(AtomicBool::new(false)),
                stopped: false,
            })
        }

        pub(super) fn exit_endpoint(&self) -> crate::exit_pause::Endpoint {
            crate::exit_pause::Endpoint {
                request_path: self.files.exit_request.clone(),
                result_path: self.files.exit_result.clone(),
                heartbeat_path: self.files.heartbeat.clone(),
                id: format!(
                    "exit-{}",
                    self.files.directory.file_name().unwrap().to_string_lossy()
                ),
                receipt_path: None,
                audit_path: None,
            }
        }
        pub(super) fn send_control(&self, command: &Value) -> io::Result<()> {
            fs::write(&self.files.command_pending, serde_json::to_vec(command)?)?;
            fs::rename(&self.files.command_pending, &self.files.command)
        }
        pub(super) fn control_result(&self) -> Option<Value> {
            let bytes = fs::read(&self.files.result).ok()?;
            if bytes.len() > 65536 {
                return None;
            }
            serde_json::from_slice(&bytes).ok()
        }
        pub(super) fn reader(&self, tx: mpsc::Sender<Value>) -> io::Result<thread::JoinHandle<()>> {
            let path = self.files.events.clone();
            let process = Arc::clone(&self.process);
            let stop = Arc::clone(&self.reader_stop);
            thread::Builder::new()
                .name("leigod-elevated-events".into())
                .spawn(move || {
                    let mut file: Option<File> = None;
                    let mut pending = Vec::new();
                    let mut exited = false;
                    loop {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        if file.is_none() {
                            match File::open(&path) {
                                Ok(opened) => file = Some(opened),
                                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                                Err(e) => {
                                    let _ = tx.send(json!({"kind":"error","error":e.to_string()}));
                                    break;
                                }
                            }
                        }
                        if let Some(file) = file.as_mut()
                            && let Err(e) = file.read_to_end(&mut pending)
                        {
                            let _ = tx.send(json!({"kind":"error","error":e.to_string()}));
                            break;
                        }
                        let mut consumed = 0;
                        while let Some(relative_end) =
                            pending[consumed..].iter().position(|&b| b == b'\n')
                        {
                            let end = consumed + relative_end;
                            if !pending[consumed..end].is_empty() {
                                let event = serde_json::from_slice(&pending[consumed..end])
                                    .unwrap_or_else(
                                        |_| json!({"kind":"error","error":"Invalid bridge NDJSON"}),
                                    );
                                if tx.send(event).is_err() {
                                    return;
                                }
                            }
                            consumed = end + 1;
                        }
                        if consumed > 0 {
                            pending.drain(..consumed);
                        }
                        // Once exit has been observed, drain the file once more:
                        // the final write may have happened between the preceding
                        // read and the process wait becoming signaled.
                        if exited {
                            if !pending.is_empty() {
                                let event = serde_json::from_slice(&pending).unwrap_or_else(
                                    |_| json!({"kind":"error","error":"Truncated bridge NDJSON"}),
                                );
                                let _ = tx.send(event);
                            }
                            break;
                        }
                        // SAFETY: the shared owning handle remains live in this
                        // thread. A zero timeout only polls process completion.
                        let state = unsafe { WaitForSingleObject(process.0, 0) };
                        if state == WAIT_OBJECT_0 {
                            exited = true;
                            continue;
                        }
                        if state == WAIT_FAILED {
                            let error = io::Error::last_os_error();
                            let _ = tx.send(json!({"kind":"error","error":error.to_string()}));
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                })
        }

        pub(super) fn stop_and_wait(&mut self) {
            if self.stopped {
                return;
            }
            self.stopped = true;
            let _ = fs::write(&self.files.stop, b"stop\n");
            // SAFETY: process remains owned by self. This waits only for the
            // helper launched above; it never terminates any process.
            unsafe { WaitForSingleObject(self.process.0, 10_000) };
            self.reader_stop.store(true, Ordering::Release);
        }
    }
    impl Drop for Elevated {
        fn drop(&mut self) {
            self.stop_and_wait();
        }
    }

    fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
        let mut units: Vec<u16> = value.encode_wide().collect();
        if units.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NUL in Windows path",
            ));
        }
        units.push(0);
        Ok(units)
    }

    // The Windows argv convention doubles backslashes before embedded quotes
    // and before the final quote. This preserves spaces and trailing slashes.
    fn quote_argument(value: &OsStr) -> io::Result<Vec<u16>> {
        let mut output = vec![b'"' as u16];
        let mut slashes = 0;
        for unit in value.encode_wide() {
            if unit == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "NUL in Windows argument",
                ));
            }
            if unit == b'\\' as u16 {
                slashes += 1;
            } else {
                let count = if unit == b'"' as u16 {
                    slashes * 2 + 1
                } else {
                    slashes
                };
                output.extend(std::iter::repeat_n(b'\\' as u16, count));
                slashes = 0;
                output.push(unit);
            }
        }
        output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        output.push(b'"' as u16);
        Ok(output)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn windows_arguments_preserve_spaces_quotes_and_trailing_slashes() {
            for (input, expected) in [
                (
                    r"C:\Program Files\helper.cjs",
                    r#""C:\Program Files\helper.cjs""#,
                ),
                (r"C:\directory\", r#""C:\directory\\""#),
                ("a\"b", "\"a\\\"b\""),
                ("", "\"\""),
            ] {
                let actual = quote_argument(OsStr::new(input)).unwrap();
                assert_eq!(String::from_utf16(&actual).unwrap(), expected);
            }
            assert!(quote_argument(OsStr::new("bad\0argument")).is_err());
        }
    }
}
