//! Read-only Steam evidence, independent from accelerator state and routing.
use crate::processes::Process;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Default)]
pub struct Reader {
    mapping: BTreeMap<String, BTreeSet<u32>>,
}
#[derive(Default, Clone, Serialize)]
pub struct App {
    pub app_id: u32,
    pub name: Option<String>,
    pub steam_reported_running: bool,
    pub evidence: Vec<String>,
    pub processes: Vec<Process>,
}
#[derive(Default, Serialize)]
pub struct Snapshot {
    pub client_running: bool,
    pub runtime_state_available: bool,
    pub coverage_complete: bool,
    pub running_apps: Vec<App>,
    pub process_only_apps: Vec<App>,
    pub error: Option<String>,
}
#[derive(Default)]
struct Registry {
    available: bool,
    active_pid: Option<u32>,
    root: Option<PathBuf>,
    running: BTreeSet<u32>,
}
struct Manifest {
    app_id: u32,
    name: Option<String>,
    directory: Option<PathBuf>,
}

impl Reader {
    pub fn new(leigod: &Path) -> Self {
        let mut reader = Self::default();
        if let Some(root) = leigod.parent() {
            for filename in ["game_path_updated.json", "game_path.json"] {
                let path = root.join("resources/leishenSdk").join(filename);
                if let Some(bytes) = read_bounded(&path, 16 * 1024 * 1024)
                    && let Ok(Value::Array(rows)) = serde_json::from_slice::<Value>(&bytes)
                {
                    reader.add_mapping(&rows);
                }
            }
        }
        reader
    }
    fn add_mapping(&mut self, rows: &[Value]) {
        for row in rows {
            let id = match &row["game_id"] {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            };
            if let (Some(id), Some(app_id)) =
                (id, row["info"]["reg_name"].as_str().and_then(steam_app_id))
            {
                self.mapping.entry(id).or_default().insert(app_id);
            }
        }
    }
    pub fn current_game(&self, snapshot: &Snapshot, id: Option<&str>) -> Value {
        let Some(id) = id else {
            return Value::Null;
        };
        let ids = self
            .mapping
            .get(id)
            .map(|v| v.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        snapshot.for_game(&ids)
    }
    pub fn snapshot(&self, inventory: &[Process]) -> Snapshot {
        collect(registry(), inventory)
    }
}
fn collect(registry: Registry, inventory: &[Process]) -> Snapshot {
    let client_running = inventory
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case("steam.exe"));
    let valid_pid = registry.active_pid.is_some_and(|pid| {
        inventory
            .iter()
            .any(|p| p.pid == pid && p.name.eq_ignore_ascii_case("steam.exe"))
    });
    let root = registry.root.or_else(|| {
        inventory
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case("steam.exe"))?
            .path
            .as_ref()
            .and_then(|p| Path::new(p).parent().map(Path::to_path_buf))
    });
    let manifests = root.as_deref().map(manifests).unwrap_or_default();
    let runtime_state_available = registry.available && valid_pid;
    let mut snapshot = Snapshot {
        client_running,
        runtime_state_available,
        error: if !client_running {
            Some("Steam 客户端未运行，忽略残留运行标志".into())
        } else if !runtime_state_available {
            Some("Steam 运行标志不可用或当前客户端 PID 无法核实".into())
        } else {
            None
        },
        ..Default::default()
    };
    let mut apps = BTreeMap::<u32, App>::new();
    if runtime_state_available {
        for app_id in registry.running {
            let app = apps.entry(app_id).or_insert_with(|| App {
                app_id,
                ..Default::default()
            });
            app.steam_reported_running = true;
            app.evidence.push("steam_registry_running".into());
        }
    }
    for manifest in manifests {
        let processes = manifest
            .directory
            .as_ref()
            .map(|directory| {
                inventory
                    .iter()
                    .filter(|p| {
                        p.path
                            .as_ref()
                            .is_some_and(|p| under_directory(Path::new(p), directory))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if processes.is_empty() && !apps.contains_key(&manifest.app_id) {
            continue;
        }
        let app = apps.entry(manifest.app_id).or_insert_with(|| App {
            app_id: manifest.app_id,
            ..Default::default()
        });
        app.name = manifest.name;
        if !processes.is_empty() {
            app.evidence
                .push("steam_manifest_install_directory_process".into());
        }
        for process in processes {
            if !app.processes.iter().any(|p| p.pid == process.pid) {
                app.processes.push(process);
            }
        }
    }
    for app in apps.into_values() {
        if app.steam_reported_running {
            snapshot.running_apps.push(app);
        } else {
            snapshot.process_only_apps.push(app);
        }
    }
    snapshot
}
impl Snapshot {
    pub fn for_game(&self, ids: &[u32]) -> Value {
        let matching = self
            .running_apps
            .iter()
            .filter(|a| ids.contains(&a.app_id))
            .map(|a| a.app_id)
            .collect::<Vec<_>>();
        let mut processes = BTreeMap::new();
        for app in self
            .running_apps
            .iter()
            .chain(&self.process_only_apps)
            .filter(|a| ids.contains(&a.app_id))
        {
            for process in &app.processes {
                processes.insert(process.pid, process.clone());
            }
        }
        let reported = if ids.is_empty() || !self.runtime_state_available {
            None
        } else {
            Some(!matching.is_empty())
        };
        let status = if ids.is_empty() {
            "no_app_id_mapping"
        } else if !matching.is_empty() {
            "steam_reported_running"
        } else if !processes.is_empty() {
            "process_detected_only"
        } else if reported == Some(false) {
            "not_reported_running"
        } else {
            "unavailable"
        };
        json!({"app_ids":ids,"mapping_source":"leigod_game_path_steam_app","steam_reported_running":reported,
            "matching_running_app_ids":matching,"processes":processes.into_values().collect::<Vec<_>>(),"status":status,
            "traffic_confirmed":false})
    }
}
fn steam_app_id(name: &str) -> Option<u32> {
    let lower = name.trim().to_ascii_lowercase();
    let id = lower.strip_prefix("steam app ")?.trim();
    if !id.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    id.parse::<u32>().ok().filter(|id| *id != 0)
}
fn under_directory(path: &Path, directory: &Path) -> bool {
    let normalize = |p: &Path| {
        p.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    normalize(path).starts_with(&(normalize(directory) + "\\"))
}
fn read_bounded(path: &Path, max: u64) -> Option<Vec<u8>> {
    if fs::metadata(path).ok()?.len() > max {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

// Only libraryfolders.vdf/appmanifest_*.acf are read; no userdata/login files.
fn manifests(root: &Path) -> Vec<Manifest> {
    let mut libraries = vec![root.to_path_buf()];
    if let Some(bytes) = read_bounded(&root.join("steamapps/libraryfolders.vdf"), 2 * 1024 * 1024)
        && let Some(vdf) = parse_vdf(&bytes)
        && let Some(folders) = vdf.object("libraryfolders")
    {
        for (key, value) in folders {
            if key.parse::<u32>().is_err() {
                continue;
            }
            let path = match value {
                Vdf::Text(s) => Some(s.as_str()),
                Vdf::Object(m) => m.get("path").and_then(Vdf::text),
            };
            if let Some(path) = path
                && libraries.len() < 64
            {
                libraries.push(PathBuf::from(path));
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    let mut visited = 0;
    for library in libraries {
        let apps = library.join("steamapps");
        let normalized = apps.to_string_lossy().to_ascii_lowercase();
        if !seen.insert(normalized) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&apps) else {
            continue;
        };
        for entry in entries.flatten().take(16384) {
            if visited >= 8192 {
                return result;
            }
            visited += 1;
            let filename = entry.file_name();
            let name = filename.to_string_lossy();
            if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
                continue;
            }
            let Some(bytes) = read_bounded(&entry.path(), 128 * 1024) else {
                continue;
            };
            let Some(vdf) = parse_vdf(&bytes) else {
                continue;
            };
            let Some(state) = vdf.object("AppState") else {
                continue;
            };
            let Some(app_id) = state
                .get("appid")
                .and_then(Vdf::text)
                .and_then(|s| s.parse::<u32>().ok())
                .filter(|id| *id != 0)
            else {
                continue;
            };
            let directory = state
                .get("installdir")
                .and_then(Vdf::text)
                .filter(|s| !s.is_empty())
                .map(Path::new)
                .filter(|p| p.components().all(|c| matches!(c, Component::Normal(_))))
                .map(|p| apps.join("common").join(p))
                .and_then(|p| p.canonicalize().ok());
            result.push(Manifest {
                app_id,
                name: state.get("name").and_then(Vdf::text).map(str::to_owned),
                directory,
            });
        }
    }
    result
}

enum Vdf {
    Text(String),
    Object(BTreeMap<String, Vdf>),
}
impl Vdf {
    fn text(&self) -> Option<&str> {
        if let Self::Text(text) = self {
            Some(text)
        } else {
            None
        }
    }
    fn object(&self, key: &str) -> Option<&BTreeMap<String, Vdf>> {
        if let Self::Object(map) = self
            && let Some(Self::Object(object)) = map.get(key)
        {
            return Some(object);
        }
        None
    }
}
fn parse_vdf(bytes: &[u8]) -> Option<Vdf> {
    fn tokens(bytes: &[u8]) -> Option<Vec<String>> {
        let mut p = 0;
        let mut out = Vec::new();
        while p < bytes.len() {
            if bytes[p].is_ascii_whitespace() {
                p += 1;
                continue;
            }
            if bytes.get(p..p + 2) == Some(b"//") {
                while p < bytes.len() && bytes[p] != b'\n' {
                    p += 1;
                }
                continue;
            }
            if b"{}".contains(&bytes[p]) {
                out.push(char::from(bytes[p]).to_string());
                p += 1;
                continue;
            }
            if bytes[p] != b'"' {
                return None;
            }
            p += 1;
            let mut value = Vec::new();
            let mut closed = false;
            while p < bytes.len() {
                let b = bytes[p];
                p += 1;
                if b == b'"' {
                    closed = true;
                    break;
                }
                if b == b'\\' {
                    let escaped = *bytes.get(p)?;
                    p += 1;
                    match escaped {
                        b'\\' | b'"' => value.push(escaped),
                        b'n' => value.push(b'\n'),
                        b't' => value.push(b'\t'),
                        _ => {
                            value.push(b'\\');
                            value.push(escaped);
                        }
                    }
                } else {
                    value.push(b);
                }
            }
            if !closed {
                return None;
            }
            out.push(String::from_utf8(value).ok()?);
            if out.len() > 100000 {
                return None;
            }
        }
        Some(out)
    }
    fn object(tokens: &[String], index: &mut usize, depth: usize) -> Option<Vdf> {
        if depth > 32 {
            return None;
        }
        let mut map = BTreeMap::new();
        while *index < tokens.len() {
            let key = tokens.get(*index)?.clone();
            *index += 1;
            if key == "}" {
                return (depth > 0).then_some(Vdf::Object(map));
            }
            if key == "{" {
                return None;
            }
            let next = tokens.get(*index)?;
            *index += 1;
            let value = if next == "{" {
                object(tokens, index, depth + 1)?
            } else if next == "}" {
                return None;
            } else {
                Vdf::Text(next.clone())
            };
            map.insert(key, value);
        }
        (depth == 0).then_some(Vdf::Object(map))
    }
    let tokens = tokens(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes))?;
    let mut index = 0;
    let root = object(&tokens, &mut index, 0)?;
    (index == tokens.len()).then_some(root)
}

#[cfg(windows)]
fn registry() -> Registry {
    use windows_sys::Win32::{
        Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS},
        System::Registry::*,
    };
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn open(path: &str) -> Option<Key> {
        let mut key = std::ptr::null_mut();
        (unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(path).as_ptr(),
                0,
                KEY_READ,
                &mut key,
            )
        } == ERROR_SUCCESS)
            .then_some(Key(key))
    }
    fn dword(key: &Key, name: &str) -> Option<u32> {
        let mut kind = 0;
        let mut value = 0u32;
        let mut size = 4;
        (unsafe {
            RegQueryValueExW(
                key.0,
                wide(name).as_ptr(),
                std::ptr::null(),
                &mut kind,
                (&mut value as *mut u32).cast(),
                &mut size,
            )
        } == ERROR_SUCCESS
            && kind == REG_DWORD
            && size == 4)
            .then_some(value)
    }
    fn string(key: &Key, name: &str) -> Option<String> {
        let mut kind = 0;
        let mut data = vec![0u16; 32768];
        let mut size = (data.len() * 2) as u32;
        if unsafe {
            RegQueryValueExW(
                key.0,
                wide(name).as_ptr(),
                std::ptr::null(),
                &mut kind,
                data.as_mut_ptr().cast(),
                &mut size,
            )
        } != ERROR_SUCCESS
            || kind != REG_SZ
        {
            return None;
        }
        let end = data.iter().position(|v| *v == 0).unwrap_or(data.len());
        Some(String::from_utf16_lossy(&data[..end]))
    }
    let mut state = Registry::default();
    if let Some(key) = open(r"Software\Valve\Steam") {
        state.root = string(&key, "SteamPath").map(PathBuf::from);
        if let Some(id) = dword(&key, "RunningAppID").filter(|id| *id != 0) {
            state.running.insert(id);
        }
    }
    if let Some(key) = open(r"Software\Valve\Steam\ActiveProcess") {
        state.active_pid = dword(&key, "pid");
    }
    let Some(apps) = open(r"Software\Valve\Steam\Apps") else {
        return state;
    };
    state.available = true;
    for index in 0..32768 {
        let mut name = [0u16; 64];
        let mut len = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                apps.0,
                index,
                name.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            state.available = false;
            break;
        }
        let name = String::from_utf16_lossy(&name[..len as usize]);
        if let Ok(app_id) = name.parse::<u32>()
            && app_id != 0
            && let Some(key) = open(&format!(r"Software\Valve\Steam\Apps\{name}"))
            && dword(&key, "Running") == Some(1)
        {
            state.running.insert(app_id);
        }
    }
    state
}
#[cfg(not(windows))]
fn registry() -> Registry {
    Registry::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_app_mapping_does_not_guess_from_names() {
        let mut reader = Reader::default();
        reader.add_mapping(&[
            json!({"game_id":5158,"info":{"reg_name":"steam app 3097560"}}),
            json!({"game_id":1,"info":{"reg_name":"not steam app 999"}}),
        ]);
        assert_eq!(
            reader.current_game(&Snapshot::default(), Some("5158"))["app_ids"],
            json!([3097560])
        );
        assert_eq!(
            reader.current_game(&Snapshot::default(), Some("1"))["steam_reported_running"],
            Value::Null
        );
    }
    #[test]
    fn app_running_and_pid_evidence_are_distinct() {
        let snapshot = Snapshot {
            client_running: true,
            runtime_state_available: true,
            running_apps: vec![App {
                app_id: 3097560,
                steam_reported_running: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let value = snapshot.for_game(&[3097560]);
        assert_eq!(value["steam_reported_running"], true);
        assert_eq!(value["processes"], json!([]));
        assert_eq!(value["traffic_confirmed"], false);
        assert_eq!(
            snapshot.for_game(&[648800])["steam_reported_running"],
            false
        );
        let unavailable = Snapshot::default().for_game(&[3097560]);
        assert!(unavailable["steam_reported_running"].is_null());
    }
    #[test]
    fn directory_match_requires_a_component_boundary() {
        assert!(under_directory(
            Path::new(r"D:\SteamLibrary\steamapps\common\Raft\Raft.exe"),
            Path::new(r"d:\steamlibrary\steamapps\common\raft")
        ));
        assert!(!under_directory(
            Path::new(r"D:\SteamLibrary\steamapps\common\Raft-copy\Raft.exe"),
            Path::new(r"D:\SteamLibrary\steamapps\common\Raft")
        ));
    }
    #[test]
    fn vdf_parses_escaped_paths_and_rejects_truncation() {
        let vdf = parse_vdf(br#""libraryfolders" { "1" { "path" "D:\\SteamLibrary" } }"#).unwrap();
        let folders = vdf.object("libraryfolders").unwrap();
        if let Vdf::Object(values) = &folders["1"] {
            assert_eq!(values["path"].text(), Some(r"D:\SteamLibrary"));
        } else {
            panic!("Missing folder");
        }
        assert!(parse_vdf(br#""AppState" { "appid" "123" "#).is_none());
    }
    #[test]
    fn stale_running_flags_are_ignored_without_the_current_steam_pid() {
        let inventory = [Process {
            pid: 10,
            parent_pid: 0,
            name: "steam.exe".into(),
            path: None,
        }];
        let stale = collect(
            Registry {
                available: true,
                active_pid: Some(9),
                running: [3097560].into(),
                ..Default::default()
            },
            &inventory,
        );
        assert!(!stale.runtime_state_available);
        assert!(stale.running_apps.is_empty());
        let current = collect(
            Registry {
                available: true,
                active_pid: Some(10),
                running: [3097560].into(),
                ..Default::default()
            },
            &inventory,
        );
        assert!(current.runtime_state_available);
        assert_eq!(current.running_apps[0].app_id, 3097560);
    }
    #[test]
    fn installed_manifest_is_not_running_until_a_matching_process_is_present() {
        let root = std::env::temp_dir().join(format!("steam-evidence-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        // Windows CI can expose TEMP through an 8.3 name or directory alias.
        // manifests() resolves its install directory, so the synthetic process
        // must use the same resolved path rather than the original TEMP spelling.
        let root = root.canonicalize().unwrap();
        let apps = root.join("steamapps");
        let common = apps.join("common");
        let game = common.join("FixtureGame");
        fs::create_dir(&apps).unwrap();
        fs::create_dir(&common).unwrap();
        fs::create_dir(&game).unwrap();
        let manifest = apps.join("appmanifest_3097560.acf");
        fs::write(
            &manifest,
            br#""AppState" { "appid" "3097560" "name" "Fixture" "installdir" "FixtureGame" }"#,
        )
        .unwrap();
        let steam = Process {
            pid: 10,
            parent_pid: 0,
            name: "steam.exe".into(),
            path: None,
        };
        let registry = || Registry {
            available: true,
            active_pid: Some(10),
            root: Some(root.clone()),
            ..Default::default()
        };
        let absent = collect(registry(), std::slice::from_ref(&steam));
        assert!(absent.running_apps.is_empty() && absent.process_only_apps.is_empty());
        let present = collect(
            registry(),
            &[
                steam,
                Process {
                    pid: 20,
                    parent_pid: 10,
                    name: "Fixture.exe".into(),
                    path: Some(game.join("Fixture.exe").to_string_lossy().into_owned()),
                },
            ],
        );
        assert!(present.running_apps.is_empty());
        assert_eq!(present.process_only_apps.len(), 1);
        assert_eq!(present.process_only_apps[0].processes.len(), 1);
        assert_eq!(present.process_only_apps[0].processes[0].pid, 20);
        assert_eq!(
            present.for_game(&[3097560])["status"],
            "process_detected_only"
        );
        fs::remove_file(manifest).unwrap();
        fs::remove_dir(game).unwrap();
        fs::remove_dir(common).unwrap();
        fs::remove_dir(apps).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
