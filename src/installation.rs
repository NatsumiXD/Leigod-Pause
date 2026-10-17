//! Read-only installation discovery. Never executes registry command strings.
use crate::processes::Process;
use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
};

pub struct Installation {
    pub exe: PathBuf,
    pub source: &'static str,
}

#[cfg(windows)]
pub fn activate(pid: u32) {
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::*,
    };
    struct Search {
        pid: u32,
        window: HWND,
    }
    unsafe extern "system" fn find(window: HWND, data: LPARAM) -> i32 {
        let search = unsafe { &mut *(data as *mut Search) };
        let mut owner_pid = 0;
        unsafe { GetWindowThreadProcessId(window, &mut owner_pid) };
        if owner_pid != search.pid
            || !unsafe { GetWindow(window, GW_OWNER) }.is_null()
            || unsafe { GetWindowTextLengthW(window) } == 0
        {
            return 1;
        }
        let mut class = [0u16; 128];
        let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
        if length > 0 && String::from_utf16_lossy(&class[..length as usize]) == "Chrome_WidgetWin_1"
        {
            search.window = window;
            return 0;
        }
        1
    }
    let mut search = Search {
        pid,
        window: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(find), &mut search as *mut Search as LPARAM) };
    if !search.window.is_null() {
        unsafe {
            ShowWindowAsync(
                search.window,
                if IsIconic(search.window) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            SetForegroundWindow(search.window);
        }
    }
}
#[cfg(not(windows))]
pub fn activate(_pid: u32) {}

fn command_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if let Some(quoted) = value.strip_prefix('"') {
        return quoted.find('"').map(|end| PathBuf::from(&quoted[..end]));
    }
    if let Some(end) = value.to_ascii_lowercase().find(".exe") {
        return Some(PathBuf::from(&value[..end + 4]));
    }
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn validated(candidate: &Path) -> Option<PathBuf> {
    let root = if candidate.is_dir() {
        candidate
    } else if candidate.is_file() {
        candidate.parent()?
    } else {
        return None;
    };
    let exe = root.join("leigod.exe");
    (exe.is_file() && root.join("resources").is_dir())
        .then(|| exe.canonicalize().ok())
        .flatten()
}

pub fn discover(
    explicit: Option<&Path>,
    processes: &[Process],
) -> Result<Installation, Box<dyn Error>> {
    if let Some(path) = explicit {
        return validated(path)
            .map(|exe| Installation {
                exe,
                source: "命令行指定",
            })
            .ok_or_else(|| format!("指定位置不是有效的雷神安装目录：{}", path.display()).into());
    }
    if let Some(path) = env::var_os("LEIGOD_EXE") {
        return validated(Path::new(&path))
            .map(|exe| Installation {
                exe,
                source: "LEIGOD_EXE",
            })
            .ok_or_else(|| "LEIGOD_EXE 未指向有效的雷神安装目录".into());
    }
    let mut candidates: Vec<(PathBuf, &'static str)> = processes
        .iter()
        .filter(|p| {
            p.name.eq_ignore_ascii_case("leigod.exe")
                || p.name.eq_ignore_ascii_case("leigod_launcher.exe")
        })
        .filter_map(|p| {
            p.path
                .as_ref()
                .map(|path| (PathBuf::from(path), "运行中的雷神"))
        })
        .collect();
    candidates.extend(registry_candidates().into_iter().map(|p| (p, "注册表")));
    candidates.extend(
        crate::shortcuts::candidates()
            .into_iter()
            .map(|p| (p, "开始菜单/桌面快捷方式")),
    );
    for variable in [
        "ProgramFiles(x86)",
        "ProgramFiles",
        "ProgramW6432",
        "LOCALAPPDATA",
    ] {
        if let Some(root) = env::var_os(variable) {
            for name in ["LeiGod_Acc", "LeiGod", "leigod"] {
                candidates.push((PathBuf::from(&root).join(name), "常见安装位置"));
            }
        }
    }
    for (candidate, source) in candidates {
        if let Some(exe) = validated(&candidate) {
            return Ok(Installation { exe, source });
        }
    }
    Err("未找到雷神加速器。请先安装，或用 --leigod-exe 指定安装目录/主程序。".into())
}

#[cfg(windows)]
fn registry_candidates() -> Vec<PathBuf> {
    use windows_sys::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn open(root: HKEY, path: &str, view: u32) -> Option<Key> {
        let mut key = std::ptr::null_mut();
        let status =
            unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ | view, &mut key) };
        (status == ERROR_SUCCESS).then_some(Key(key))
    }
    fn expand(value: &str) -> String {
        let mut result = String::new();
        let mut rest = value;
        while let Some(start) = rest.find('%') {
            result.push_str(&rest[..start]);
            rest = &rest[start + 1..];
            let Some(end) = rest.find('%') else {
                result.push('%');
                result.push_str(rest);
                return result;
            };
            let name = &rest[..end];
            result.push_str(&env::var(name).unwrap_or_else(|_| format!("%{name}%")));
            rest = &rest[end + 1..];
        }
        result.push_str(rest);
        result
    }
    fn value(key: &Key, name: &str) -> Option<String> {
        let mut kind = 0;
        let mut size = 0;
        if unsafe {
            RegQueryValueExW(
                key.0,
                wide(name).as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        } != ERROR_SUCCESS
            || ![REG_SZ, REG_EXPAND_SZ].contains(&kind)
            || size > 65536
        {
            return None;
        }
        let mut units = vec![0u16; size as usize / 2 + 1];
        if unsafe {
            RegQueryValueExW(
                key.0,
                wide(name).as_ptr(),
                std::ptr::null(),
                &mut kind,
                units.as_mut_ptr().cast(),
                &mut size,
            )
        } != ERROR_SUCCESS
        {
            return None;
        }
        let end = units.iter().position(|v| *v == 0).unwrap_or(units.len());
        Some(expand(&String::from_utf16_lossy(&units[..end])))
    }
    let mut paths = Vec::new();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            for executable in ["leigod.exe", "leigod_launcher.exe"] {
                if let Some(key) = open(
                    root,
                    &format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{executable}"),
                    view,
                ) {
                    if let Some(path) = value(&key, "").and_then(|v| command_path(&v)) {
                        paths.push(path);
                    }
                    if let Some(path) = value(&key, "Path") {
                        paths.push(path.into());
                    }
                }
            }
            let base = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
            let Some(uninstall) = open(root, base, view) else {
                continue;
            };
            for index in 0..16384 {
                let mut buffer = vec![0u16; 256];
                let mut length = buffer.len() as u32;
                let status = unsafe {
                    RegEnumKeyExW(
                        uninstall.0,
                        index,
                        buffer.as_mut_ptr(),
                        &mut length,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if status == windows_sys::Win32::Foundation::ERROR_NO_MORE_ITEMS {
                    break;
                }
                if status != ERROR_SUCCESS {
                    continue;
                }
                let name = String::from_utf16_lossy(&buffer[..length as usize]);
                let Some(key) = open(root, &format!(r"{base}\{name}"), view) else {
                    continue;
                };
                let display = value(&key, "DisplayName").unwrap_or_default();
                if !display.contains("雷神") && !display.to_ascii_lowercase().contains("leigod") {
                    continue;
                }
                if let Some(location) = value(&key, "InstallLocation") {
                    paths.push(location.into());
                }
                for field in ["DisplayIcon", "UninstallString"] {
                    if let Some(path) = value(&key, field).and_then(|v| command_path(&v)) {
                        paths.push(path);
                    }
                }
            }
        }
    }
    paths
}
#[cfg(not(windows))]
fn registry_candidates() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_commands_are_parsed_without_executing_arguments() {
        assert_eq!(
            command_path(r#""D:\Games\LeiGod\uninstall.exe" /S"#),
            Some(PathBuf::from(r"D:\Games\LeiGod\uninstall.exe"))
        );
        assert_eq!(
            command_path(r"D:\Program Files\LeiGod\leigod.exe,0"),
            Some(PathBuf::from(r"D:\Program Files\LeiGod\leigod.exe"))
        );
        assert!(command_path("").is_none());
    }
    #[test]
    fn explicit_installation_must_exist_and_contain_resources() {
        let root = env::temp_dir().join(format!("leigod-installation-test-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("leigod.exe"), b"fixture").unwrap();
        assert!(discover(Some(&root), &[]).is_err());
        std::fs::create_dir(root.join("resources")).unwrap();
        assert_eq!(
            discover(Some(&root), &[]).unwrap().exe,
            root.join("leigod.exe").canonicalize().unwrap()
        );
        assert!(discover(Some(&root.join("missing")), &[]).is_err());
        std::fs::remove_file(root.join("leigod.exe")).unwrap();
        std::fs::remove_dir(root.join("resources")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
