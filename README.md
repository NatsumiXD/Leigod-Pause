# Leigod-Pause

雷神终端启动器（Rust），支持游戏退出检测、自动停止加速，以及退出/关机时优先暂停时长。项目名称为 `Leigod-Pause`，兼容现有启动入口 `leigod-status.exe`。

完整便携版：解压 `LeiGodLauncher-portable.zip`，运行 `LeiGodLauncher/leigod-status.exe`。保留整个目录，不能只复制 EXE。无需安装 Node、npm、Python 或 Frida；内部会运行随包携带的私有 Node/Frida，隐藏启动并继承启动器权限。Windows 可能为 Rust 启动器显示一次管理员确认。

默认自动发现雷神安装目录，通过官方 `leigod_launcher.exe` 启动；已运行则直接附加。动态查找导出的 Node-API 和具有相应能力的界面 store，不依赖固定内存地址，也不修改雷神安装文件。首次安装 Hook 需要一次正常原生回调；如一直等待，可切换雷神页面。上游大幅修改接口后可能需要更新本程序，不能保证所有未来版本兼容；无法确认状态时会停止自动判断。

## 终端界面

默认使用键盘菜单：`↑/↓` 移动高亮选项，`Enter` 确认，`Esc` 返回。主页可选择历史日志、配置、退出。日志页底部同样使用方向键选择返回主页、配置、导出或退出，日志正文仍只追加。

配置页选中等待时间后按 `Enter` 输入分钟数，再按 `Enter` 确认输入；也可直接用 `←/→` 每次调整一分钟。选中自动停止开关后按 `Enter` 切换。最后选择“保存配置”生效；编辑时 `Esc` 取消输入，再按一次 `Esc` 丢弃草稿并返回主页。文字命令继续兼容。

主页在原位置刷新，状态、用户时长、游戏、配置进程名、运行 PID、Steam 证据及退出倒计时每 5 秒重新采集。标题栏显示当前加速和检测状态。未启动的游戏也显示已有配置的 EXE 名称；缺少本地配置时不会编造名称，改用 Steam 安装目录下的实际进程补充。

| 命令 | 行为 |
| --- | --- |
| `home` / `back` | 返回原地刷新的主页 |
| `log` | 导出 logs 目录中保留的所有会话历史，进入独立日志页；后续记录只追加，可向上滚动 |
| `config` | 打开配置表，输入字段编号，再输入值，`save` 保存、`cancel` 取消 |
| `quit` / `exit` / Ctrl+C | 优先暂停时长，再退出启动器并关闭绑定的雷神进程 |

也可通过文字命令修改等待时间为 5 分钟：依次输入 `config`、`1`、`5`、`save`，每条回车。配置保存在 EXE 同目录的 `config.json`：

```json
{"exit_delay_minutes": 10, "auto_stop_enabled": true}
```

等待时间允许 0–1440 分钟，0 表示下次确认退出时立即执行。修改在下一轮判断生效。日志自动持久化到 `logs/session-*.log` 和 `.ndjson`；`log` 额外生成 `export-*.log`，保留旧会话，不读取导出文件以免重复。日志包含已授权显示的账户编号、昵称和时长；不采集密码、令牌、Cookie 或完整登录资料。日志文件不会自动删除。

## 游戏退出后的动作

1. Steam 状态可用时，必须在当前加速会话中同时观察到 Steam 报告对应 AppID 运行，以及该游戏的配置进程或安装目录关联进程存在。Steam 未启动、状态不可读或没有 AppID 映射时，降级为仅根据实际游戏进程确认运行。两种模式都必须先监测到运行过；仅选择加速、仅安装了游戏或仅存在进程配置都不会启动退出倒计时。
2. 之后相关进程全部消失，且 Steam 明确报告未运行或已不可用，开始默认 10 分钟倒计时。Steam 不可用不会中断已有倒计时；游戏进程恢复运行会取消。Steam 状态可用且仍报告运行时，不判定游戏退出。进程快照不可用、Hook 状态不明确或加速会话丢失，仍会取消判断。曾观察到的同名进程在系统中仍存在时，也不会判断退出。
3. 到期重新采样，带游戏 ID、会话标识和短有效期发送一次控制命令。客户端再次验证同一会话后停止加速，确认界面恢复空闲，再明确调用暂停计时。已暂停时不会切换回计时。
4. 换游戏、重新加速同款游戏、切换账号、命令过期或接口失败都会中止相应动作并报告结果，不盲目重试。缺少 Steam AppID 映射时，仍可使用已实际观察到的配置进程进行退出判断；如果连游戏进程也没有观察到，则不启用倒计时。主页及标题会标注“仅进程”，JSON 的 auto_stop.evidence_mode 为 process_only。

执行结果分别报告 `stopped`、`paused`、`ok`、`error`。当前厂商的 stopAcc 内部不等待停止 IPC 返回，因此“已停止”的证据是客户端界面状态恢复 normal；暂停证据是账户 timeStatus 变为 pause，不能当作独立的网络流量或后台结算证明。请求派发后用户同时重新加速存在跨进程竞争窗口，无法原子撤销已经送出的远端请求。

## 双向进程绑定

默认持续监测模式下，启动器与选定安装目录的雷神双向绑定。正常退出、关闭终端窗口或强制结束启动器时，独立 Rust 监护进程会关闭该目录下的 leigod.exe、leigod_launcher.exe 和 leishenSdk.exe；雷神主进程真正退出时启动器也退出。窗口最小化到托盘不等于进程退出。不会沿父子进程树结束游戏，也不会选择其他安装目录的程序。

每次退出都优先请求暂停时长，确认后才关闭绑定进程；此行为独立于“游戏退出后自动停止”开关和等待分钟数。菜单退出、quit、Ctrl+C、关闭终端窗口，以及 Windows 正常关机/注销均接入退出处理。雷神自身正常退出时，也会在 Electron 退出事件中先尝试暂停。已暂停时不会切换回计时，退出与游戏自动停止共用控制锁，避免并发切换计时状态。

暂停等待有约 3–4 秒的上限，以客户端 `timeStatus: pause` 为确认依据。结果记录在启动器目录的 `logs/exit-*.json`；未安装好 Hook、接口失败或超时会明确记录未确认，随后继续退出。监护进程在启动器被强制结束时也会先尝试暂停，再关闭雷神。雷神本身被强杀、断电、系统强制终止或接口不可用时无法保证暂停成功。单次查询、诊断和 CDP 模式不启用退出控制。

## 命令行及构建

```powershell
# 默认终端 UI、自动启动、自动注入、双向绑定
.\leigod-status.exe
# 持续 NDJSON / 普通追加文本（仍启用绑定及自动停止）
.\leigod-status.exe --json
.\leigod-status.exe --plain
# 单次只读查询；不启用进程绑定及自动停止
.\leigod-status.exe --once --json --no-launch
# 只检查安装和便携依赖，不启动或注入
.\leigod-status.exe --diagnose
# 非标准安装位置
.\leigod-status.exe --leigod-exe 'D:\Apps\LeiGod\leigod.exe'
cargo build --offline --release --target-dir target/launcher
```

还支持 `--interval-ms`、`--node`、`--frida-module`、`--native` 和已有调试端口的 `--cdp --port`。`--cdp` 用于诊断/隔离测试，不启用绑定或自动停止。定制采样间隔时主页文字仍以默认 5 秒说明为准。重定向输入或输出时自动使用追加日志输出，无需终端能力。

自动发现使用运行进程、安装注册信息、快捷方式和常见目录，不扫描全部磁盘。Steam 只读当前用户运行标记、库清单和安装目录，不注入 Steam。用户及加速信息保持白名单输出；Steam/进程存在仅是运行证据，不能证明每条流量被加速。

发布包内 `THIRD-PARTY.json` 和 `runtime/*LICENSE*` / `Frida-COPYING*` 记录随包运行时版本、来源和许可。`runtime/node_modules` 可替换，未做静态合并。`package-portable.cjs` 从已有本机运行时复制依赖，不执行 npm 安装脚本。
## GitHub Actions 自动编译和便携包

工作流位于 `.github/workflows/build-portable.yml`。将本项目目录（`Cargo.toml`、`.github`、`src`、`hook` 等所在目录）作为 GitHub 仓库根目录提交。推送、Pull Request 或 Actions 页面手动运行 **Build Windows portable** 都会触发构建。手动运行入口需要先把工作流提交到仓库默认分支。

使用 Windows x64 runner，固定 Rust 1.99.0、Node 24.18.0 和 Frida 17.23.2；Rust 使用 `Cargo.lock`，Node 依赖使用 `ci/runtime/package-lock.json`。流程执行格式检查、Rust/JavaScript 测试、Clippy、发行版编译，再装配便携目录、验证私有 Node/Frida 可加载，并压缩上传。无需安装雷神、登录账户或配置额外 Secrets。

成功后，在该次运行的 **Artifacts → LeiGodLauncher-windows-x64-运行编号** 下载，包含：

- `LeiGodLauncher-portable-windows-x64.zip`：完整便携应用，包含私有 Node/Frida 和许可文件。
- `SHA256SUMS.txt`：便携 ZIP 的 SHA256 校验值。

构建产物保留 30 天。发布包使用全新默认配置，不收集本机历史日志；`dist`、`target`、`node_modules` 已加入 Git 忽略。

推送到 `main`，或在 `main` 手动运行工作流，全部构建步骤成功后还会自动发布 [GitHub Pre-release](https://github.com/NatsumiXD/Leigod-Pause/releases)，附上同一份便携 ZIP 和 SHA256 校验文件。其他分支和 Pull Request 只构建，不发布。预发布标签为 `pre-运行编号.重跑次数`（例如 `pre-12.1`），对应本次构建的提交；不会覆盖已有版本或将预发布设为正式 Latest。

发布使用独立作业和 GitHub 自带的 `GITHUB_TOKEN`，仅该作业具有 `contents: write` 权限，无需新增 Secret。它先校验下载产物的 SHA256，再建立草稿、上传两个附件，最后公开为预发布；上传失败时保持草稿。仓库或组织策略需要允许工作流写入仓库内容。

在本机复用相同打包步骤（PowerShell 7，输出目录必须为空或不存在）：

```powershell
npm ci --prefix ci/runtime --omit=dev --no-audit --no-fund
cargo build --locked --release --target x86_64-pc-windows-msvc --target-dir target/ci
./scripts/create-portable.ps1 `
  -Executable target/ci/x86_64-pc-windows-msvc/release/leigod-status.exe `
  -NodeExecutable (Get-Command node.exe).Source `
  -FridaDirectory ci/runtime/node_modules/frida `
  -OutputDirectory dist/ci
```

`create-portable.ps1` 拒绝复用非空输出目录，避免把原有个人配置和日志装进 ZIP。再次本地打包时可指定另一个空输出目录。调整 Node/Frida 固定版本时，需要同时更新依赖锁文件与 `licenses` 中对应版本的许可。
