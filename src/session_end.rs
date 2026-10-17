//! A hidden top-level window receives Windows logoff/shutdown broadcasts.
//! Message-only windows do not receive them. This never vetoes system shutdown.
use std::{io, thread};

pub struct SessionEnd {
    #[cfg(windows)]
    hwnd: isize,
    #[cfg(windows)]
    thread_id: u32,
    worker: Option<thread::JoinHandle<()>>,
}

impl SessionEnd {
    /// `on_end` runs on the message thread, once after a confirmed session end.
    /// It must finish within its own bounded shutdown budget and must not panic.
    pub fn start(on_end: fn(), priority: u32) -> io::Result<Self> {
        #[cfg(windows)]
        {
            if !(0x100..=0x3ff).contains(&priority) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Shutdown priority must be in the application range",
                ));
            }
            // Prefer this process before the accelerator (normally 0x280).
            // No SHUTDOWN_NORETRY flag, blocking reason, or shutdown veto.
            if unsafe {
                windows_sys::Win32::System::Threading::SetProcessShutdownParameters(priority, 0)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let worker = thread::Builder::new()
                .name("session-end".into())
                .spawn(move || win::message_thread(on_end, ready_tx))?;
            match ready_rx.recv() {
                Ok(Ok((hwnd, thread_id))) => Ok(Self {
                    hwnd,
                    thread_id,
                    worker: Some(worker),
                }),
                result => {
                    let _ = worker.join();
                    Err(match result {
                        Ok(Err(error)) => error,
                        _ => io::Error::other("Session-end message thread exited before ready"),
                    })
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (on_end, priority);
            Ok(Self { worker: None })
        }
    }
}

impl Drop for SessionEnd {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                PostMessageW, PostThreadMessageW, WM_CLOSE, WM_QUIT,
            };
            // Let the owning thread destroy its own window. The fallback also
            // works if the window was already destroyed while its queue lives.
            if PostMessageW(self.hwnd as _, WM_CLOSE, 0, 0) == 0 {
                PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
            }
        }
        if let Some(worker) = self.worker.take()
            && worker.thread().id() != thread::current().id()
        {
            let _ = worker.join();
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::{ptr, sync::mpsc::SyncSender};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::WindowsAndMessaging::*,
    };

    struct State {
        on_end: fn(),
        notified: bool,
    }

    // The State allocation belongs to the message thread and remains alive
    // through DestroyWindow. Only this window procedure reads or changes it.
    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            match message {
                WM_NCCREATE => {
                    let create = &*(lparam as *const CREATESTRUCTW);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
                    1
                }
                WM_QUERYENDSESSION => 1,
                WM_ENDSESSION => {
                    if wparam != 0 {
                        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
                        if let Some(state) = state.as_mut()
                            && !state.notified
                        {
                            state.notified = true;
                            // Never unwind across the system callback boundary.
                            let _ = std::panic::catch_unwind(state.on_end);
                        }
                    }
                    0
                }
                WM_CLOSE => {
                    DestroyWindow(hwnd);
                    0
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    0
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
                _ => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
    }

    pub(super) fn message_thread(on_end: fn(), ready: SyncSender<io::Result<(isize, u32)>>) {
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            if instance.is_null() {
                let _ = ready.send(Err(io::Error::last_os_error()));
                return;
            }
            let thread_id = GetCurrentThreadId();
            let class_name: Vec<u16> = format!(
                "LeiGodStatusSessionEnd_{}_{}",
                std::process::id(),
                thread_id
            )
            .encode_utf16()
            .chain(Some(0))
            .collect();
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                let _ = ready.send(Err(io::Error::last_os_error()));
                return;
            }
            let mut state = State {
                on_end,
                notified: false,
            };
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0, // No WS_VISIBLE, owner, or HWND_MESSAGE: hidden top-level.
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                (&mut state as *mut State).cast(),
            );
            if hwnd.is_null() {
                let error = io::Error::last_os_error();
                UnregisterClassW(class_name.as_ptr(), instance);
                let _ = ready.send(Err(error));
                return;
            }
            if ready.send(Ok((hwnd as isize, thread_id))).is_ok() {
                let mut message = MSG::default();
                loop {
                    let result = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
                    if result <= 0 {
                        break;
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            if IsWindow(hwnd) != 0 {
                DestroyWindow(hwnd);
            }
            UnregisterClassW(class_name.as_ptr(), instance);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsWindow, IsWindowVisible, SendMessageW, WM_ENDSESSION, WM_QUERYENDSESSION,
    };

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn on_end() {
        CALLS.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn owned_window_handles_cancel_confirm_and_cleanup_without_system_shutdown() {
        CALLS.store(0, Ordering::SeqCst);
        let observer = SessionEnd::start(on_end, 0x3f0).unwrap();
        let hwnd = observer.hwnd as _;
        unsafe {
            assert_ne!(IsWindow(hwnd), 0);
            assert_eq!(IsWindowVisible(hwnd), 0);
            assert_eq!(SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0), 1);
            assert_eq!(CALLS.load(Ordering::SeqCst), 0);
            SendMessageW(hwnd, WM_ENDSESSION, 0, 0);
            assert_eq!(CALLS.load(Ordering::SeqCst), 0);
            SendMessageW(hwnd, WM_ENDSESSION, 1, 0);
            SendMessageW(hwnd, WM_ENDSESSION, 1, 0);
            assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        }
        drop(observer);
        assert_eq!(unsafe { IsWindow(hwnd) }, 0);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }
}
