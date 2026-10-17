mod bridge;
mod dashboard;
mod exit_pause;
mod history;
mod installation;
mod lifecycle;
mod policy;
mod processes;
mod report;
mod session_end;
mod settings;
mod shortcuts;
mod steam;
mod terminal;
use bridge::Bridge;
use serde_json::{Value, json};
use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
static STOP: AtomicBool = AtomicBool::new(false);
const UI_AGENT: &str = include_str!("../hook/ui-agent.js");
const UI_BRIDGE: &str = include_str!("../hook/ui-bridge.cjs");
const MAIN_AGENT: &str = include_str!("../hook/main-agent.js");
const NAPI_AGENT: &str = include_str!("../hook/napi-agent.js");
const NAPI_BRIDGE: &str = include_str!("../hook/napi-bridge.cjs");
const NATIVE_AGENT: &str = include_str!("../hook/agent.js");
const NATIVE_BRIDGE: &str = include_str!("../hook/bridge.cjs");
const HASH: &str = "aca4e862d4f26073c99dfbe77bb750effbedfe805b6b5bac34b284220d750153";
struct Options {
    watch: bool,
    json: bool,
    launch: bool,
    native: bool,
    cdp: bool,
    port: u16,
    interval: u64,
    node: PathBuf,
    exe: PathBuf,
    frida: Option<PathBuf>,
    explicit_exe: Option<PathBuf>,
    diagnose: bool,
    plain: bool,
}
fn options() -> Result<Options, Box<dyn Error>> {
    parse_options(env::args().skip(1))
}
fn parse_options(mut args: impl Iterator<Item = String>) -> Result<Options, Box<dyn Error>> {
    let mut o = Options {
        watch: true,
        json: false,
        launch: true,
        native: false,
        cdp: false,
        port: 9223,
        interval: 5000,
        node: env::var_os("LEIGOD_NODE")
            .map(PathBuf::from)
            .unwrap_or_else(|| "node".into()),
        exe: PathBuf::new(),
        frida: None,
        explicit_exe: None,
        diagnose: false,
        plain: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--watch" => o.watch = true,
            "--once" => o.watch = false,
            "--json" => o.json = true,
            "--launch" => o.launch = true,
            "--no-launch" => o.launch = false,
            "--diagnose" => o.diagnose = true,
            "--plain" => o.plain = true,
            "--native" => o.native = true,
            "--cdp" => o.cdp = true,
            "--port" => o.port = args.next().ok_or("--port requires a number")?.parse()?,
            "--interval-ms" => {
                o.interval = args
                    .next()
                    .ok_or("--interval-ms requires a number")?
                    .parse()?
            }
            "--node" => o.node = args.next().ok_or("--node requires a path")?.into(),
            "--leigod-exe" => {
                o.explicit_exe = Some(args.next().ok_or("--leigod-exe requires a path")?.into())
            }
            "--frida-module" => {
                o.frida = Some(args.next().ok_or("--frida-module requires a path")?.into())
            }
            "--help" | "-h" => {
                println!(
                    "leigod-status [--json | --plain] [--once] [--no-launch]\n  [--interval-ms 5000] [--node PATH] [--leigod-exe PATH] [--frida-module PATH]\n  [--diagnose] [--cdp --port 9223] [--native]\n\n默认自动寻找并启动雷神、动态注入、进入终端主页，每 5 秒重新采集。\n命令：home 主页；log 历史日志及导出；config 配置；quit 退出。\n默认确认游戏退出 10 分钟后停止加速并暂停计时；config 可修改。\n持续模式下启动器与雷神双向绑定，Ctrl+C / 关闭启动器会关闭雷神。\n--once 单次只读查询，不启用绑定及自动停止；--diagnose 只检查依赖。\n--json 输出 NDJSON；--plain 输出追加文本；重定向时使用追加输出。\n完整便携包无需预装 Node。发行版由 Rust 启动器申请管理员权限。"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    if o.port < 1024 || o.interval < 100 || o.interval > 60000 {
        return Err("Port must be >=1024; interval must be 100..60000 ms".into());
    }
    Ok(o)
}
fn ordinary_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
}
fn find_frida(o: &Options) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(p) = o
        .frida
        .clone()
        .or_else(|| env::var_os("LEIGOD_FRIDA_MODULE").map(PathBuf::from))
    {
        return Ok(p.canonicalize()?);
    }
    let mut candidates = vec![
        PathBuf::from(".runtime/node_modules/frida"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".runtime/node_modules/frida"),
    ];
    if let Some(directory) = env::current_exe()?.parent() {
        candidates.insert(0, directory.join("runtime/node_modules/frida"));
        candidates.insert(0, directory.join(".runtime/node_modules/frida"));
    }
    if let Some(local) = env::var_os("LOCALAPPDATA")
        && let Ok(entries) = std::fs::read_dir(PathBuf::from(local).join("npm-cache/_npx"))
    {
        let mut cached: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path().join("node_modules/frida"))
            .collect();
        cached.sort();
        candidates.extend(cached);
    }
    Ok(candidates
        .into_iter()
        .find(|p| p.join("package.json").is_file())
        .ok_or("Frida module missing; use --frida-module PATH")?
        .canonicalize()?)
}
fn find_node(o: &Options) -> Result<PathBuf, Box<dyn Error>> {
    if o.node.is_absolute() || o.node.components().count() > 1 {
        return Ok(o.node.canonicalize()?);
    }
    if o.node == Path::new("node")
        && let Some(directory) = env::current_exe()?.parent()
    {
        let bundled = directory.join("runtime/node.exe");
        if bundled.is_file() {
            return Ok(bundled.canonicalize()?);
        }
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(&o.node);
        for candidate in [candidate.clone(), candidate.with_extension("exe")] {
            if candidate.is_file() {
                return Ok(candidate.canonicalize()?);
            }
        }
    }
    Err("Node.js executable not found; use --node PATH".into())
}
fn main_process<'a>(
    inventory: &'a [processes::Process],
    exe: &Path,
) -> Option<&'a processes::Process> {
    let expected = ordinary_path(exe).replace('/', "\\");
    let candidates: Vec<_> = inventory
        .iter()
        .filter(|p| {
            p.name.eq_ignore_ascii_case("leigod.exe")
                && p.path
                    .as_ref()
                    .is_none_or(|path| path.replace('/', "\\").eq_ignore_ascii_case(&expected))
        })
        .collect();
    candidates
        .iter()
        .copied()
        .find(|p| !candidates.iter().any(|parent| parent.pid == p.parent_pid))
}
fn start_napi(
    o: &Options,
    pid: u32,
    ui_key: &str,
    ui_source: &str,
    resource: &str,
) -> Result<Bridge, Box<dyn Error>> {
    let key = format!("{ui_key}_main");
    let install = format!(
        "globalThis.LEIGOD_MAIN_CONFIG={};\n{MAIN_AGENT}",
        json!({"key":key,"uiKey":ui_key,"uiSource":ui_source,"resourcePath":resource,"intervalMs":o.interval,"pauseOnExit":o.watch})
    );
    let key_json = serde_json::to_string(&key)?;
    let snapshot = format!(
        "(()=>{{if(!globalThis[{key_json}]) (0,eval)({}); return globalThis[{key_json}].snapshot();}})()",
        serde_json::to_string(&install)?
    );
    let cleanup = format!("globalThis[{key_json}]?.dispose();");
    let source = format!(
        "globalThis.LEIGOD_NAPI_CONFIG={};\nglobalThis.LEIGOD_NAPI_CONFIG.transport=globalThis.LEIGOD_NAPI_TRANSPORT;\n{NAPI_AGENT}",
        json!({"installSource":install,"snapshotExpression":snapshot,"cleanupSource":cleanup,"intervalMs":o.interval})
    );
    Bridge::start_elevated(
        &find_node(o)?,
        NAPI_BRIDGE,
        json!({"fridaModule":ordinary_path(&find_frida(o)?),"pid":pid,"source":source,"intervalMs":o.interval,"controllerPid":std::process::id(),"pauseOnExit":o.watch}),
    )
}
fn normalize_snapshot(value: &Value) -> Value {
    if value["transport"] != "napi" {
        return value.clone();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    match value["observed_at_ms"].as_u64() {
        Some(at) if now.saturating_sub(at) <= 15000 => value["snapshot"].clone(),
        _ => {
            json!({"available":false,"state":"unknown","error":"Waiting for a current native addon callback"})
        }
    }
}
fn start_native(o: &Options, p: &processes::Process) -> Result<Bridge, Box<dyn Error>> {
    let exe = Path::new(p.path.as_ref().ok_or("Cannot query SDK path")?);
    let dll = exe
        .parent()
        .ok_or("SDK parent path missing")?
        .join("GateProxyEx.dll")
        .canonicalize()?;
    let path = ordinary_path(&dll);
    let source = format!(
        "const MONITOR_CONFIG={};\n{NATIVE_AGENT}",
        json!({"modulePath":path})
    );
    Bridge::start(
        &o.node,
        NATIVE_BRIDGE,
        json!({"fridaModule":ordinary_path(&find_frida(o)?),"pid":p.pid,"source":source,
        "intervalMs":o.interval,"verification":{"path":path,"sha256":HASH}}),
    )
}
fn basename(s: &str) -> &str {
    s.rsplit(['\\', '/']).next().unwrap_or(s)
}
fn compose(
    ui: &Value,
    native: &Value,
    inventory: &[processes::Process],
    app_running: bool,
) -> Value {
    let available = ui["available"].as_bool().unwrap_or(false);
    let mut state = if available {
        ui["state"].as_str().unwrap_or("unknown")
    } else if !app_running {
        "not_running"
    } else {
        "unknown"
    };
    let native_state = native["state"].as_str().unwrap_or("unknown");
    let mut reason = if available {
        "renderer_acc_store"
    } else if !app_running {
        "no_leigod_process"
    } else {
        "ui_hook_unavailable"
    };
    if available
        && ["active", "inactive"].contains(&native_state)
        && ["active", "inactive"].contains(&state)
        && native_state != state
    {
        state = "unknown";
        reason = "ui_and_native_state_disagree";
    }
    let game = if state == "active" {
        ui["game"].clone()
    } else {
        Value::Null
    };
    let configured: Vec<_> = game["configured_process_names"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let confirmed: std::collections::BTreeSet<u32> = if native_state == "active" {
        native["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["pid"].as_u64().and_then(|x| u32::try_from(x).ok()))
            .collect()
    } else {
        Default::default()
    };
    let processes: Vec<_> = if state == "active" {
        inventory.iter().filter(|p|configured.iter().any(|n|basename(n).eq_ignore_ascii_case(&p.name)))
        .map(|p|json!({"pid":p.pid,"name":p.name,"path":p.path,"traffic_confirmed":confirmed.contains(&p.pid),
            "evidence":if confirmed.contains(&p.pid){"configuration_and_native_traffic"}else{"configured_name_matches_running_process"}})).collect()
    } else {
        vec![]
    };
    let intercepted: Vec<_> = if state == "active" {
        inventory
            .iter()
            .filter(|p| confirmed.contains(&p.pid))
            .map(|p| json!(p))
            .collect()
    } else {
        vec![]
    };
    let not_running: Vec<_> = configured
        .iter()
        .filter(|name| {
            !inventory
                .iter()
                .any(|p| basename(name).eq_ignore_ascii_case(&p.name))
        })
        .map(|name| name.to_string())
        .collect();
    json!({"state":state,"accelerating":match state{"active"=>Some(true),"inactive"|"not_running"=>Some(false),_=>None},
        "reason":reason,"game":game,"processes":processes,"not_running_process_names":not_running,"intercepted_processes":intercepted,
        "processes_complete":false,"ui_state":ui["acc_status"],"ui_hook_available":available,"ui_error":ui["error"],
        "last_action":if available{ui["last_action"].clone()}else{Value::Null},
        "last_ipc_action":if available{ui["last_ipc_action"].clone()}else{Value::Null},
        "user":if available && ui["user_hook_available"]==true{ui["user"].clone()}else{Value::Null},
        "user_hook_available":available && ui["user_hook_available"]==true,
        "user_error":if available{ui["user_error"].clone()}else{json!("User observer unavailable")},
        "last_user_event":if available{ui["last_user_event"].clone()}else{Value::Null},
        "hook_transport":ui["transport"],"ipc_hook_available":ui["ipc_hook_available"],
        "native_state":native_state,"native_error":native["error"],"native_sdk_pid":native["sdk_pid"]})
}
fn emit(v: &Value, o: &Options) {
    if o.json {
        println!("{v}");
        return;
    }
    let label = match v["state"].as_str().unwrap_or("unknown") {
        "active" => "加速中",
        "inactive" => "未加速",
        "not_running" => "雷神未运行",
        _ => "未知",
    };
    println!("状态: {label} ({})", v["reason"].as_str().unwrap_or(""));
    if v["user_hook_available"] == true {
        let user = &v["user"];
        let login = match user["logged_in"].as_bool() {
            Some(true) => "已登录",
            Some(false) => "未登录",
            None => "未知",
        };
        println!("用户: {login}");
        if user["logged_in"] == true {
            println!(
                "  账号: {}  昵称: {}",
                user["account_number"].as_str().unwrap_or("未知"),
                user["nickname"].as_str().unwrap_or("未设置")
            );
            println!(
                "  剩余时长: {} 秒  计时状态: {}",
                user["time"]["total_remaining_seconds"],
                user["time"]["status"].as_str().unwrap_or("未知")
            );
        }
    }
    if !v["game"].is_null() {
        println!(
            "游戏: {} (ID {})",
            v["game"]["name"].as_str().unwrap_or("名称暂不可用"),
            v["game"]["id"].as_str().unwrap_or("?")
        );
    }
    for p in v["processes"].as_array().into_iter().flatten() {
        println!(
            "  PID {}  {}  流量已确认={}",
            p["pid"],
            p["name"].as_str().unwrap_or("?"),
            p["traffic_confirmed"]
        );
    }
    for name in v["not_running_process_names"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        println!("  进程 {name}：未启动（未检测到运行 PID）");
    }
    if v["state"] == "active"
        && v["game"]["configured_process_names"]
            .as_array()
            .is_none_or(|x| x.is_empty())
    {
        println!("  未从界面/本地游戏表读取到进程名。");
    }
    if let Some(apps) = v["steam"]["running_apps"].as_array() {
        if v["steam"]["runtime_state_available"] == true {
            if apps.is_empty() {
                println!("Steam：当前没有报告运行中的 App。");
            }
            for app in apps {
                println!(
                    "Steam 运行中：AppID {}  {}",
                    app["app_id"],
                    app["name"].as_str().unwrap_or("名称未缓存")
                );
            }
        } else {
            println!(
                "Steam：{}",
                v["steam"]["error"].as_str().unwrap_or("运行状态不可用")
            );
        }
    }
    if !v["steam_game"].is_null() {
        let evidence = &v["steam_game"];
        let label = match evidence["status"].as_str().unwrap_or("") {
            "steam_reported_running" => "Steam 报告此游戏正在运行",
            "process_detected_only" => "仅检测到安装目录下的进程",
            "not_reported_running" => "Steam 未报告此游戏运行",
            "no_app_id_mapping" => "本地目录未提供该游戏的 Steam AppID 映射",
            _ => "Steam 运行状态无法确认",
        };
        println!(
            "Steam 交叉验证：{label}；候选 AppID {}",
            evidence["app_ids"]
        );
        for process in evidence["processes"].as_array().into_iter().flatten() {
            println!(
                "  Steam 安装目录关联 PID {}  {}",
                process["pid"],
                process["name"].as_str().unwrap_or("未知")
            );
        }
    }
    if !v["ui_error"].is_null() {
        let error = v["ui_error"].as_str().unwrap_or("不可用");
        if error.starts_with("Waiting for") {
            println!("正在等待雷神初始化/原生回调，Hook 会自动连接。");
        } else {
            println!("界面 Hook: {error}");
        }
    }
}
fn run(mut o: Options) -> Result<(), Box<dyn Error>> {
    let initial = processes::list_processes()?;
    let install = installation::discover(o.explicit_exe.as_deref(), &initial)?;
    o.exe = install.exe;
    o.node = find_node(&o)?;
    if !o.cdp {
        o.frida = Some(find_frida(&o)?);
    }
    let already_running = main_process(&initial, &o.exe).is_some();
    let steam_reader = steam::Reader::new(&o.exe);
    if o.diagnose {
        println!(
            "{}",
            json!({"installation":ordinary_path(&o.exe),"discovered_from":install.source,"already_running":already_running,
            "node":ordinary_path(&o.node),"frida":o.frida.as_ref().map(|p|ordinary_path(p)),"auto_launch":o.launch,"watch":o.watch,
            "shortcut_targets":shortcuts::candidates().iter().map(|p|ordinary_path(p)).collect::<Vec<_>>(),"steam":steam_reader.snapshot(&initial)})
        );
        return Ok(());
    }
    #[cfg(windows)]
    if !o.cdp && unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() } == 0 {
        return Err(
            "附加雷神需要管理员权限。请运行 dist 中的发行版启动器，或以管理员身份运行调试版。"
                .into(),
        );
    }
    eprintln!(
        "雷神目录: {}（{}）",
        o.exe.parent().unwrap().display(),
        install.source
    );
    use std::io::IsTerminal;
    let interactive = o.watch
        && !o.json
        && !o.plain
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    let root = env::current_exe()?
        .parent()
        .ok_or("Missing launcher directory")?
        .to_path_buf();
    let config_path = root.join("config.json");
    let mut settings = settings::Settings::load(&config_path)?;
    let mut dashboard = if interactive {
        Some(dashboard::Dashboard::enter()?)
    } else {
        None
    };
    let mut history = if o.watch {
        Some(history::History::create(&root)?)
    } else {
        None
    };
    let mut binding = if o.watch && !o.cdp {
        Some(lifecycle::Binding::start(&o.exe)?)
    } else {
        None
    };
    if let Some(binding) = &mut binding
        && let Some(process) = main_process(&initial, &o.exe)
    {
        binding.arm(process.pid)?;
    }
    let session_end = if binding.is_some() {
        Some(session_end::SessionEnd::start(
            system_session_ending,
            0x3f0,
        )?)
    } else {
        None
    };
    let mut tracker = policy::Tracker::default();
    let mut latest = json!({"state":"unknown"});
    let mut control_result = Value::Null;
    let mut pending_control: Option<(String, Instant)> = None;
    if o.launch && !already_running {
        // Match the installed shortcut: the launcher starts the SDK and supplies
        // -launch/-launchpid/-sdkpid before forwarding the caller's arguments.
        let launcher = o
            .exe
            .parent()
            .ok_or("LeiGod executable directory missing")?
            .join("leigod_launcher.exe");
        if !launcher.is_file() {
            return Err("安装目录缺少 leigod_launcher.exe，无法正常初始化 SDK。".into());
        }
        let program = launcher;
        eprintln!("正在通过官方启动器打开雷神…");
        let mut cmd = Command::new(&program);
        cmd.current_dir(
            o.exe
                .parent()
                .ok_or("LeiGod executable directory missing")?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        match cmd.spawn() {
            Ok(_) => {}
            Err(e) => return Err(e.into()),
        }
    }
    if already_running {
        eprintln!("检测到雷神已运行，直接附加…");
    } else if !o.launch {
        eprintln!("雷神未运行，当前仅附加模式，等待客户端启动…");
    }
    let ui_key = format!(
        "__leigodReadOnlyMonitor_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
    );
    let ui_source = UI_AGENT.replace("__leigodReadOnlyMonitor_v1", &ui_key);
    let resource_path = ordinary_path(
        &o.exe
            .parent()
            .ok_or("LeiGod executable directory missing")?
            .join("resources"),
    );
    let mut ui_bridge = if o.cdp {
        Some(Bridge::start(
            &o.node,
            UI_BRIDGE,
            json!({"port":o.port,"source":ui_source,"key":ui_key,"resourcePath":resource_path,"intervalMs":o.interval,
            "activePortFile":env::var_os("APPDATA").map(PathBuf::from).map(|p|p.join("leigod/DevToolsActivePort"))}),
        )?)
    } else {
        None
    };
    // Declared after the UI bridge so errors also request pause before detach.
    let mut exit_guard = exit_pause::Guard::new(binding.is_some());
    let mut hooked_pid = None;
    let mut ui_retry = Instant::now();
    let mut ui = json!({"available":false,"state":"unknown","error":"Waiting for renderer"});
    let mut native = json!({"state":"unknown"});
    let mut native_bridge: Option<Bridge> = None;
    let mut native_pid = None;
    let mut last_ui = Instant::now();
    let mut last_native = Instant::now();
    let deadline = Instant::now()
        + Duration::from_secs(if o.launch {
            60
        } else if o.native {
            20
        } else {
            30
        });
    let mut received = false;
    let mut next_sample = Instant::now();
    let mut native_retry = Instant::now();
    let mut sample_number = 0u64;
    while !STOP.load(Ordering::Relaxed) {
        if binding
            .as_ref()
            .is_some_and(lifecycle::Binding::client_exited)
        {
            break;
        }
        if let (Some(dashboard), Some(history)) = (&mut dashboard, &history) {
            if dashboard.poll(&mut settings, &config_path, history)? {
                break;
            }
            latest["auto_stop"] = tracker.status(&settings, Instant::now());
            dashboard.draw(&latest)?;
        }
        if let Some(result) = ui_bridge.as_ref().and_then(Bridge::control_result)
            && pending_control
                .as_ref()
                .is_some_and(|(id, _)| result["id"].as_str() == Some(id))
        {
            control_result = result;
            tracker.complete(&control_result);
            pending_control = None;
            next_sample = Instant::now();
        }
        if pending_control
            .as_ref()
            .is_some_and(|(_, time)| time.elapsed() > Duration::from_secs(16))
        {
            control_result = json!({"ok":false,"error":"控制结果超时；状态未知，不自动重试"});
            tracker.complete(&control_result);
            pending_control = None;
            next_sample = Instant::now();
        }
        let next_ui = if let Some(b) = &ui_bridge {
            b.events.recv_timeout(Duration::from_millis(100))
        } else {
            thread::sleep(Duration::from_millis(100));
            Err(mpsc::RecvTimeoutError::Timeout)
        };
        match next_ui {
            Ok(e) => {
                last_ui = Instant::now();
                received = true;
                ui = if e["kind"] == "snapshot" {
                    normalize_snapshot(&e["snapshot"])
                } else if e["kind"] == "hook" {
                    let event = &e["payload"];
                    match event["event"].as_str().unwrap_or("") {
                        "snapshot" => event["snapshot"].clone(),
                        "napi_error" => {
                            json!({"available":false,"state":"unknown","error":event["error"]})
                        }
                        _ => ui.clone(),
                    }
                } else if e["kind"] == "attached" {
                    if o.launch
                        && let Some(pid) = hooked_pid
                    {
                        installation::activate(pid);
                    }
                    ui.clone()
                } else {
                    json!({"available":false,"state":"unknown","error":e["error"]})
                };
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                ui = json!({"available":false,"state":"unknown","error":"UI bridge exited"});
                received = true;
                if !o.cdp {
                    ui_bridge = None;
                    hooked_pid = None;
                    ui_retry = Instant::now() + Duration::from_secs(5);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if last_ui.elapsed() > Duration::from_millis(o.interval + 6000) {
            ui = json!({"available":false,"state":"unknown","error":"UI snapshot timed out"});
        }
        let mut native_disconnected = false;
        if let Some(b) = &native_bridge {
            loop {
                match b.events.try_recv() {
                    Ok(e) => {
                        last_native = Instant::now();
                        match e["kind"].as_str().unwrap_or("") {
                            "snapshot" => native = e["snapshot"].clone(),
                            "error" | "script_error" | "detached" => {
                                native = json!({"state":"unknown","error":e});
                                native_disconnected = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        native = json!({"state":"unknown","error":"Native bridge exited"});
                        native_disconnected = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
        }
        if native_bridge.is_some()
            && last_native.elapsed() > Duration::from_millis(o.interval + 15000)
        {
            native = json!({"state":"unknown","error":"Native hook timed out"});
            native_disconnected = true;
        }
        if native_disconnected {
            native_bridge = None;
            native_pid = None;
            native_retry = Instant::now() + Duration::from_secs(5);
        }
        if Instant::now() >= next_sample {
            let inventory = processes::list_processes()?;
            let running = main_process(&inventory, &o.exe).is_some();
            if !running && binding.as_ref().is_some_and(lifecycle::Binding::has_client) {
                break;
            }
            if let Some(binding) = &mut binding
                && let Some(process) = main_process(&inventory, &o.exe)
            {
                binding.arm(process.pid)?;
            }
            if !o.cdp {
                let target = main_process(&inventory, &o.exe);
                if hooked_pid.is_some() && target.map(|p| p.pid) != hooked_pid {
                    ui_bridge = None;
                    hooked_pid = None;
                    ui_retry = Instant::now();
                    ui = json!({"available":false,"state":"unknown","error":"Target restarted"});
                }
                if let Some(p) = target
                    && ui_bridge.is_none()
                    && Instant::now() >= ui_retry
                {
                    match start_napi(&o, p.pid, &ui_key, &ui_source, &resource_path) {
                        Ok(b) => {
                            if let Some(binding) = &binding
                                && let Some(endpoint) = b.exit_endpoint()
                            {
                                binding.set_endpoint(endpoint)?;
                            }
                            ui_bridge = Some(b);
                            hooked_pid = Some(p.pid);
                            last_ui = Instant::now();
                        }
                        Err(e) => {
                            ui = json!({"available":false,"state":"unknown","error":e.to_string()});
                            received = true;
                            ui_retry = Instant::now() + Duration::from_secs(30);
                        }
                    }
                }
            }
            if o.native {
                let targets: Vec<_> = inventory
                    .iter()
                    .filter(|p| p.name.eq_ignore_ascii_case("leishenSdk.exe"))
                    .collect();
                let target = if targets.len() == 1 {
                    Some(targets[0])
                } else {
                    None
                };
                if native_pid.is_some() && target.map(|p| p.pid) != native_pid {
                    native_bridge = None;
                    native_pid = None;
                    native = json!({"state":"unknown"});
                }
                if let Some(p) = target
                    && native_bridge.is_none()
                    && Instant::now() >= native_retry
                {
                    match start_native(&o, p) {
                        Ok(b) => {
                            native_bridge = Some(b);
                            native_pid = Some(p.pid);
                            last_native = Instant::now();
                        }
                        Err(e) => {
                            native = json!({"state":"unknown","error":e.to_string()});
                            native_retry = Instant::now() + Duration::from_secs(5);
                        }
                    }
                }
            }
            if !running {
                ui = json!({"available":false,"state":"unknown"});
            }
            let mut value = compose(&ui, &native, &inventory, running);
            let steam = steam_reader.snapshot(&inventory);
            let steam_game = steam_reader.current_game(&steam, value["game"]["id"].as_str());
            value["steam"] = serde_json::to_value(&steam)?;
            value["steam_game"] = steam_game;
            value["observed_process_names"] =
                json!(inventory.iter().map(|p| &p.name).collect::<Vec<_>>());
            if o.watch
                && !o.cdp
                && let Some(mut command) = tracker.update(&value, &settings, Instant::now())
            {
                let id = format!("{}-{}", std::process::id(), history::epoch_ms());
                command["id"] = json!(id);
                command["expires_at_ms"] = json!(history::epoch_ms() + 2000);
                match ui_bridge
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("Hook 尚未连接"))
                    .and_then(|bridge| bridge.send_control(&command))
                {
                    Ok(()) => {
                        pending_control = Some((id, Instant::now()));
                        control_result = json!({"status":"submitted"});
                    }
                    Err(error) => {
                        control_result = json!({"ok":false,"error":error.to_string()});
                        tracker.complete(&control_result);
                    }
                }
            }
            value
                .as_object_mut()
                .unwrap()
                .remove("observed_process_names");
            value["auto_stop"] = tracker.status(&settings, Instant::now());
            value["control_result"] = control_result.clone();
            value["sample_interval_ms"] = json!(o.interval);
            value["sampled_at_ms"] = json!(history::epoch_ms());
            sample_number += 1;
            value["sample_number"] = json!(sample_number);
            let ready = !o.launch || ui["available"] == true || Instant::now() >= deadline;
            if o.watch || ready && (received || !running || Instant::now() >= deadline) {
                if let Some(history) = &mut history {
                    let block = history.push(&value)?;
                    if let Some(dashboard) = &mut dashboard {
                        dashboard.append(&block)?;
                    }
                }
                if dashboard.is_none() {
                    emit(&value, &o);
                }
                latest = value.clone();
            }
            if !o.watch
                && ready
                && (ui["available"] == true || !running || Instant::now() >= deadline)
            {
                break;
            }
            next_sample = Instant::now() + Duration::from_millis(o.interval);
        }
        thread::sleep(Duration::from_millis(10));
    }
    if binding.is_some() {
        latest["auto_stop"]["message"] = json!("正在优先暂停时长，然后退出…");
        if let Some(dashboard) = &mut dashboard {
            let _ = dashboard.draw(&latest);
        }
        let result = exit_guard.finish();
        latest["exit_pause_result"] = result.clone();
        if let Some(history) = &mut history {
            let _ = history.push(&latest);
        }
        if result["ok"] != true {
            eprintln!("退出前未确认暂停时长：{}", result["error"]);
        }
    }
    drop(dashboard);
    drop(exit_guard);
    drop(native_bridge);
    drop(ui_bridge);
    drop(binding);
    drop(session_end);
    Ok(())
}
#[cfg(windows)]
unsafe extern "system" fn ctrl_handler(kind: u32) -> i32 {
    if kind == 0 || kind == 1 {
        STOP.store(true, Ordering::Relaxed);
        1
    } else if [2, 5, 6].contains(&kind) {
        STOP.store(true, Ordering::Relaxed);
        let _ = exit_pause::request_current(Duration::from_millis(3300));
        1
    } else {
        0
    }
}
fn system_session_ending() {
    STOP.store(true, Ordering::Relaxed);
    let _ = exit_pause::request_current(Duration::from_millis(3300));
}
fn main() {
    #[cfg(debug_assertions)]
    if env::args().nth(1).as_deref() == Some("--test-binding") {
        let exe = PathBuf::from(env::args_os().nth(2).expect("Fixture path"));
        let mut client = Command::new(&exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("Fixture spawn");
        let mut binding = lifecycle::Binding::start(&exe).expect("Fixture binding");
        binding.arm(client.id()).expect("Fixture arm");
        if let Some(path) = env::var_os("LEIGOD_EXIT_TEST_ENDPOINT") {
            let endpoint: exit_pause::Endpoint =
                serde_json::from_slice(&std::fs::read(path).expect("Fixture endpoint file"))
                    .expect("Fixture endpoint JSON");
            binding
                .set_endpoint(endpoint)
                .expect("Fixture endpoint registration");
        }
        println!("fixture_ready {}", client.id());
        if env::args().nth(3).as_deref() == Some("quit") {
            thread::sleep(Duration::from_millis(600));
            drop(binding);
            let _ = client.wait();
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while !binding.client_exited() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        drop(binding);
        let _ = client.wait();
        return;
    }
    #[cfg(debug_assertions)]
    if env::args().nth(1).as_deref() == Some("--test-terminal") {
        let root = PathBuf::from(env::args_os().nth(2).expect("Fixture data directory"));
        let path = root.join("config.json");
        let mut config = settings::Settings::load(&path).expect("Settings");
        let mut history = history::History::create(&root).expect("History");
        let mut dashboard = dashboard::Dashboard::enter().expect("Terminal");
        let value = json!({"state":"active","game":{"id":"fixture","name":"测试游戏","configured_process_names":["fixture-game.exe"]},"not_running_process_names":["fixture-game.exe"],"auto_stop":{"message":"测试模式（隔离数据）","remaining_seconds":598},"steam_game":{"steam_reported_running":false,"app_ids":[123]}});
        let mut sample = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            if dashboard.poll(&mut config, &path, &history).expect("Input") {
                break;
            }
            if Instant::now() >= sample {
                let block = history.push(&value).expect("Log");
                dashboard.append(&block).expect("Append");
                sample = Instant::now() + Duration::from_secs(5);
            }
            dashboard.draw(&value).expect("Draw");
            thread::sleep(Duration::from_millis(100));
        }
        return;
    }
    if env::args().nth(1).as_deref() == Some("--watchdog") {
        let result = env::args_os()
            .nth(2)
            .ok_or_else(|| std::io::Error::other("Missing watchdog config"))
            .and_then(|path| lifecycle::watchdog(Path::new(&path)));
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ctrl_handler), 1);
    }
    if let Err(e) = options().and_then(run) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_defaults_and_explicit_once_mode() {
        let defaults = parse_options(std::iter::empty()).unwrap();
        assert!(defaults.launch && defaults.watch);
        assert_eq!(defaults.interval, 5000);
        assert!(defaults.explicit_exe.is_none());
        let once = parse_options(
            ["--once", "--no-launch", "--leigod-exe", r"D:\Apps\LeiGod"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert!(!once.watch && !once.launch);
        assert_eq!(once.explicit_exe, Some(PathBuf::from(r"D:\Apps\LeiGod")));
    }
    #[test]
    fn attach_selects_main_process_in_the_discovered_installation() {
        let processes = vec![
            processes::Process {
                pid: 1,
                parent_pid: 0,
                name: "leigod.exe".into(),
                path: Some(r"C:\Other\leigod.exe".into()),
            },
            processes::Process {
                pid: 2,
                parent_pid: 0,
                name: "leigod.exe".into(),
                path: Some(r"D:\LeiGod\leigod.exe".into()),
            },
            processes::Process {
                pid: 3,
                parent_pid: 2,
                name: "leigod.exe".into(),
                path: Some(r"D:\LeiGod\leigod.exe".into()),
            },
        ];
        assert_eq!(
            main_process(&processes, Path::new(r"D:\LeiGod\leigod.exe"))
                .unwrap()
                .pid,
            2
        );
        assert!(main_process(&processes, Path::new(r"E:\Missing\leigod.exe")).is_none());
    }
    fn inventory() -> Vec<processes::Process> {
        vec![
            processes::Process {
                pid: 7,
                parent_pid: 0,
                name: "Game.exe".into(),
                path: None,
            },
            processes::Process {
                pid: 8,
                parent_pid: 0,
                name: "other.exe".into(),
                path: None,
            },
        ]
    }
    #[test]
    fn active_game_matches_only_configured_running_processes() {
        let ui = json!({"available":true,"state":"active","game":{"id":"1","name":"Game","configured_process_names":["game.exe"]}});
        let v = compose(&ui, &json!({"state":"unknown"}), &inventory(), true);
        assert_eq!(v["accelerating"], true);
        assert_eq!(v["game"]["name"], "Game");
        assert_eq!(v["processes"].as_array().unwrap().len(), 1);
        assert_eq!(v["processes"][0]["traffic_confirmed"], false);
    }
    #[test]
    fn game_configuration_is_displayed_even_without_a_running_game() {
        let ui = json!({"available":true,"state":"active","game":{"name":"Game","configured_process_names":["game.exe","Launcher.exe"]}});
        let absent = compose(&ui, &Value::Null, &[], true);
        assert!(absent["processes"].as_array().unwrap().is_empty());
        assert_eq!(
            absent["not_running_process_names"],
            json!(["game.exe", "Launcher.exe"])
        );
        let partial = compose(&ui, &Value::Null, &inventory(), true);
        assert_eq!(
            partial["not_running_process_names"],
            json!(["Launcher.exe"])
        );
        assert_eq!(partial["processes"][0]["pid"], 7);
    }
    #[test]
    fn unknown_and_stop_clear_game_and_processes() {
        for state in ["unknown", "inactive"] {
            let ui = json!({"available":true,"state":state,"game":{"name":"old","configured_process_names":["game.exe"]}});
            let v = compose(&ui, &Value::Null, &inventory(), true);
            assert!(v["game"].is_null());
            assert!(v["processes"].as_array().unwrap().is_empty());
            assert!(
                v["not_running_process_names"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[test]
    fn contradictory_evidence_is_unknown() {
        let v = compose(
            &json!({"available":true,"state":"active"}),
            &json!({"state":"inactive"}),
            &inventory(),
            true,
        );
        assert!(v["accelerating"].is_null());
    }
    #[test]
    fn user_is_independent_of_acceleration_but_cleared_when_observer_is_lost() {
        let user = json!({"logged_in":true,"account_number":"fixture","nickname":"Test"});
        let mut ui =
            json!({"available":true,"state":"inactive","user_hook_available":true,"user":user});
        let value = compose(&ui, &Value::Null, &inventory(), true);
        assert_eq!(value["user"]["account_number"], "fixture");
        assert_eq!(value["accelerating"], false);
        ui["available"] = json!(false);
        let unavailable = compose(&ui, &Value::Null, &inventory(), true);
        assert!(unavailable["user"].is_null());
        assert_eq!(unavailable["user_hook_available"], false);
    }
}
