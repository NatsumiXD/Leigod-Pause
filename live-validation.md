# 实机验证记录

2026-10-17，本机已运行的雷神 Electron 主进程 PID 14848。

已观察：

- 管理员 Node 助手通过 Frida 成功附加，无需调试端口，也未重启雷神。
- 动态 Node-API 回调安装主线程观察器成功，随后向实际 renderer 注入 Pinia 动作订阅。
- 连续读到 `available: true`、`state: inactive`、`acc_status: normal`、`game: null`。
- 主线程文件快照持续刷新，采集不依赖每次都有新的原生回调。
- 用户真实点击开始后，记录到 `last_action.name: startAcc`，状态从 `loading` 变为 `speeding`，输出 `accelerating: true`。
- 当前游戏为 `Raft`，ID `512`，从实时加速参数读取到 `configured_process_names: ["Raft.exe"]`。
- 当时未发现运行中的 Raft.exe，`processes` 为空；没有把“开启加速”误报为“游戏进程已经运行”。
- 用户真实点击并确认停止后，捕获到 `last_action: {name: "stopAcc", phase: "returned"}`。
- 停止后持续读到 `state: inactive`、`accelerating: false`、`ui_state: normal`、`game: null`、`processes: []`，未残留 Raft 游戏信息。

界面开始/停止动作与状态变化的实机联调已通过。尚未观察：游戏实际进程/流量。

主进程 `ipc_hook_available` 在两次实机附加中均为 false，跨上下文 Map 识别修正也没有使该入口可用。界面 Pinia 的开始/停止动作订阅已实机通过，不能将其表述为主进程 IPC 返回值也已验证。

验证程序当前订阅的是用户正常按钮触发的动作，不会自行调用开始/停止。自动测试不替代上述未观察的实机行为。

## 用户信息扩展

同日替换监测助手后，再次附加相同的雷神主进程；雷神未重启。

- `user_hook_available: true`，`user_error: null`。
- `user.logged_in: true`。
- 已读到 `account_number`、`nn_id`、`nickname`，不在验证文档中记录实际账号值。
- `user.time.loaded: true`，`user.time.status: pause`，剩余秒数字段已读到。
- 加速状态同时正常输出为 `inactive`。
- 退出登录时清除旧账号/时长、用户 store 移除、凭据字段不被读取或序列化，均有自动测试；未为了验证而操作真实账号退出或登录。

该扩展读取现有运行时用户 store，并订阅其变化；不调用登录、刷新或暂停接口，不导出原始用户对象。

## 启动器改造

- 默认参数已改为自动寻找目录、未运行时启动官方 launcher、持续监测；已有实例直接附加，不再要求先退出。
- 实机只读诊断找到 `C:/Program Files (x86)/LeiGod_Acc/leigod.exe`、Node 和 Frida；开始菜单/桌面快捷方式也解析到同目录官方启动器。
- Windows SDK `mt.exe` 从发行 EXE 实际提取到 `requestedExecutionLevel level="requireAdministrator"`。
- 新版 Rust 启动器获得权限后，以普通 `open` 启动隐藏的 Node 助手；实机事件确认 `attached: true`，首次采样仍在等待正常 Node-API 回调，不能把这条附加事件当作该次已经完成安装。
- 没有为了测试自动启动而中断当时正在运行的雷神和游戏。冷启动分支使用先前已核实的官方启动器，无额外调试参数。
- 29 项测试（10 项 Rust、19 项 JavaScript）及 Clippy 通过；Rust 新增安装目录有效性、快捷方式 Unicode 路径和损坏数据、默认持续模式和单次覆盖、跨安装目录主进程选择验证。

初次替换遇到旧 EXE 短暂占用；发布流程已改为先确认退出，再覆盖并比较哈希，避免继续启动旧文件。

## 5 秒重新采集与固定日志输出

- 默认间隔改为 5000 ms；界面观察器、采集助手和 Windows 进程快照均使用该采集间隔。
- 移除“仅数据变化才输出”的判断，每轮固定输出；没有重新调用加速、刷新余额等业务接口。
- 当前游戏没有对应运行 PID 时，输出配置进程名和未启动提示；JSON 新增 `not_running_process_names`，不会伪造 PID。
- Rust 11 项测试及 Clippy 通过。
- 新增 `tests/refresh-smoke.cjs`，用自建 leigod.exe 测试进程和本机 CDP 模拟服务运行真实 Rust 控制程序：验证默认约 5 秒间隔、相同数据连续输出、重新读取被修改的游戏信息、未启动进程名保留。该测试不操作真实雷神或游戏，已通过。

## Steam 第二条运行证据

- 已在本机核实 Steam 根目录、`ActiveProcess/pid` 与实际 steam.exe PID 一致，Steam 本地运行状态可读。
- 本次实机诊断为 `client_running: true`、`runtime_state_available: true`、`running_apps: []`、`process_only_apps: []`。没有为了测试而启动真实 Steam 游戏，因此本次没有验证真实运行游戏的正例。
- 直接从当前 SDK 的游戏目录核实 `game_id: 5158` 对应 `steam app 3097560`。
- 实现每 5 秒读取当前用户的 Steam App 运行标志，使用 Steam 库清单和游戏安装目录关联进程路径。已安装清单不等于运行，残留标志在 Steam PID 不匹配时被忽略。
- 新增 6 项 Steam 测试：AppID 映射、运行标志与 PID 区分、路径边界、VDF 路径解析与损坏数据、过期 Steam PID、仅安装/实际进程两种情况。Rust 共 17 项测试及 Clippy 通过。
- `steam` 和 `steam_game` 独立输出，不修改加速状态，不把目录内进程或 Steam 运行标志当作网络加速证据；完整性保守标为 false。
- 发行文件已同时发布到原 `dist/leigod-status.exe` 和 `dist/leigod-status-steam.exe`。

## 2026-10-17 终端启动器与自动停止版本

- 已实现 Win32 原生终端输入、备用屏幕主页、独立追加日志页、历史导出、配置表和动态标题。真实 ConPTY 会话验证了 home/log/config 页面切换、3 分钟配置保存、非法 2000 分钟拒绝、开关保存及正常恢复控制台。
- 默认退出等待 10 分钟；只有当前会话同时具备 Steam 与真实进程运行证据后才启用判断。纯策略测试覆盖未启动、未知证据、重启、同款游戏新会话、残留相关进程、到期单次提交。
- 动态控制按 ID+session 防误操作、短有效期、防重放；停止后明确调用 pause。JS 模拟真实 store 接口，覆盖成功、换游戏、同 ID 重启、切换账户、超时、失败、已暂停及过期命令。
- 进程绑定用独立 Rust 监护进程；隔离 sleep 进程的三项端到端测试通过：正常退出、控制器强制结束、客户端结束。游戏进程和另一安装目录的同名进程均保持运行。没有为测试结束真实雷神或游戏。
- 本次没有对真实正在使用的账户触发停止和暂停计时。停止确认仍受厂商 stopAcc fire-and-forget 行为限制，结果依据 renderer normal 和 timeStatus pause，不能表述为独立后台结算确认。
- 最终便携验证：从测试子进程 PATH 移除已安装 Node 路径并清除 Node/Frida 覆盖变量，真实 Rust 诊断选择了包内 runtime/node.exe 与 runtime/node_modules/frida。使用包内 Node 对自建进程完成 Frida 附加、RPC 和正常卸载。
- 最终验证：26 项 Rust 单元测试、28 项 JavaScript 测试、Clippy、三项配对退出测试、ConPTY 配置/日志交互、5 秒刷新端到端测试均通过。刷新测试因沙箱限制回环连接，已在允许本机测试网络的执行环境重新运行通过。
- 使用 mt.exe 从发行 EXE 提取清单，确认仍由 leigod-status.exe 申请 requireAdministrator；没有为此验证重复运行真实加速器。
- 最终入口为 dist/LeiGodLauncher/leigod-status.exe，便携 ZIP 须完整解压并保留 runtime 目录。dist 根目录的旧版 EXE 不作为本次发布入口。

## Steam 缺席时使用进程判断（2026-10-17）

- Steam 未启动、运行状态不可读或没有对应 AppID 映射时，自动切换为 process_only。曾实际观察到的游戏进程均退出后开始/继续退出倒计时，不再因为 Steam 不可用而取消。
- 从一开始没有 Steam 的场景，也可通过配置匹配到的真实游戏进程确认运行，随后判断退出。未实际观察到运行、缺少进程快照、Hook 不明确或新加速会话仍不会凭空启动倒计时。
- 主页/标题显示“仅进程”；JSON 新增 auto_stop.evidence_mode。Steam 状态恢复可用且报告游戏运行时，仍取消退出倒计时；残留同名进程也阻止误停。
- 31 项 Rust 测试及 Clippy 通过，新增覆盖：无 Steam 从运行到退出、Steam 中途消失保留倒计时、仅进程模式重启、快照缺失、Steam 可用时仍保留运行证据。

## 方向键菜单版本（2026-10-17）

- 主页和配置表增加方向键选择与高亮；Enter 执行，Esc 返回/取消输入。配置的自动停止开关可直接 Enter 切换，分钟数支持输入或左右键微调。
- 日志页在底部单行显示可选择操作；导航只刷新提示行，不重画历史正文。输入 log/config/home/quit 等指令继续兼容。
- 真实 ConPTY 隔离会话验证：向下选择配置、Enter 编辑为 4 分钟、开关切换为 false、选择保存，持久化文件符合预期；方向键进入日志、追加日志、导出、Esc 返回和菜单退出均可操作。测试没有启动或关闭真实雷神/游戏。
- 新增四项菜单交互测试，覆盖未保存草稿不影响当前设置、非法数值和 Esc 取消、日志导航和导出、菜单循环选择及分钟数边界。共 35 项 Rust 测试及 Clippy 通过。

## 退出/关机优先暂停时长（2026-10-17）

- 静态核实当前 renderer 的暂停接口：明确发送 pause，使用独立 scene，可在仍加速时暂停，不需要先触发停止确认。运行时动作再次验证用户身份，只有 timeStatus 变为 pause 才记录成功；不读取凭据。
- 菜单退出、Ctrl+C、窗口关闭、错误退出和监护进程清理均先走共享暂停通道，再卸载观察器或关闭绑定进程。Node 助手退出及 Electron before-quit 也接入相同动作。退出暂停与游戏自动停止串行，重复请求复用同一个结果。
- Rust 主程序及独立监护进程各使用隐藏顶层窗口接收 WM_ENDSESSION。测试仅向自建窗口发送取消/确认结束会话消息，验证取消时不触发回调、确认时只触发一次，没有发起真实关机或注销。
- 38 项 Rust 测试、42 项 JavaScript 测试和 Clippy 通过。四项隔离端到端测试通过：正常退出、控制器被强杀、客户端先退出、暂停超时。测试确认暂停请求先于清理，等待结果期间客户端保持存活，超时后有界退出，游戏及另一安装目录保持运行。
- 退出结果独立写入 logs/exit-*.json；客户端已被结束或暂停超时会记录 paused/ok 为 false。测试未对真实账户执行暂停，也未验证实际 Windows 关机中的厂商网络响应。断电、强杀客户端或系统强制终止不能保证完成暂停。
