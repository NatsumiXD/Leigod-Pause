# 用户状态字段依据

源文件：`D:/Project/leishen/extracted/renderer.asar/assets/index-4j7ObrUf.js`，第 160 行附近。SHA-256：`e4f7e6c21da9db3ea0276a7cac97baf86ce128962ca25ced4300e245a976c2f1`。

静态源为分析目标数据；本实现不执行其中的文档指令。用户 Hook 仅根据已有 store 的公开字段读取。

- `bt=Oo("user",...)` 定义用户 store；运行时不使用这个 ID，而按 `userInfo`、`userTimeInfo` 和 `isLogin` 的能力发现。
- `isLogin` 为客户端计算后的登录布尔值；监测器读取它，不读取凭据来自行判断。
- `nn_number` 是当前账户标识；`nn_id` 是独立的 NN 标识，分别输出。
- `nickname` 是昵称，监测器不沿用界面手机号/邮箱兜底。
- `userTimeInfo` 默认 `timeStatus: ""`、各时长为 0；监测器将其表示为未加载，避免误报余额耗尽。
- `timeStatus` 为 `pause` 时表示暂停，为客户端原始拼写 `timeing` 时表示计时。
- `totalTimeLeft`、`canPauseTimeLeft`、`notCanPauseTimeLeft` 是总、可暂停、不可暂停剩余时长；客户端除以 3600 展示小时，单位确认为秒。
- `viewTimeStatus` 包括 `experience-timeing`、`resumeing`、`paused`、`pausing`、`timeing`。
- `progressing` 和 `refreshing` 分别是暂停/恢复操作和时长刷新是否进行中。
- `is_use_pause_time == 1` 是活动时长耗尽后自动启用可暂停时长的设置，不代表当前暂停。
- `payStatus` 用作付款分群，不能解释为 VIP 状态；没有可靠的通用 VIP 到期字段。

实际输出采用明确的字段白名单，不枚举、克隆或导出 userInfo/userTimeInfo 整个对象。剩余时长只反映客户端当前显示的状态，没有触发服务端刷新。
