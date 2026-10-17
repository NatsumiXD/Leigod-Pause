use serde_json::Value;
use std::fmt::Write;
pub fn label(v: &Value) -> &'static str {
    match v["state"].as_str() {
        Some("active") => "加速中",
        Some("inactive") => "未加速",
        Some("not_running") => "雷神未运行",
        _ => "等待检测",
    }
}
pub fn text(v: &Value) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "状态: {}", label(v));
    if let Some(message) = v["auto_stop"]["message"].as_str() {
        let _ = writeln!(s, "检测: {message}");
    }
    if let Some(seconds) = v["auto_stop"]["remaining_seconds"].as_u64() {
        let _ = writeln!(
            s,
            "停止加速并暂停计时倒计时: {:02}:{:02}",
            seconds / 60,
            seconds % 60
        );
    }
    let user = &v["user"];
    let _ = writeln!(
        s,
        "用户: {}",
        match user["logged_in"].as_bool() {
            Some(true) => "已登录",
            Some(false) => "未登录",
            _ => "等待读取",
        }
    );
    if user["logged_in"] == true {
        let _ = writeln!(
            s,
            "  账号: {}  昵称: {}",
            user["account_number"].as_str().unwrap_or("?"),
            user["nickname"].as_str().unwrap_or("?")
        );
        let _ = writeln!(
            s,
            "  剩余: {} 秒  计时: {}",
            user["time"]["total_remaining_seconds"],
            user["time"]["status"].as_str().unwrap_or("未知")
        );
    }
    if !v["game"].is_null() {
        let _ = writeln!(
            s,
            "游戏: {} (ID {})",
            v["game"]["name"].as_str().unwrap_or("名称待获取"),
            v["game"]["id"].as_str().unwrap_or("?")
        );
    }
    for p in v["processes"].as_array().into_iter().flatten() {
        let _ = writeln!(
            s,
            "  PID {}  {}",
            p["pid"],
            p["name"].as_str().unwrap_or("?")
        );
    }
    for n in v["not_running_process_names"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let _ = writeln!(s, "  {n}: 未启动");
    }
    if v["state"] == "active"
        && v["game"]["configured_process_names"]
            .as_array()
            .is_none_or(|a| a.is_empty())
    {
        s.push_str("  本地未提供游戏进程配置，使用 Steam 安装目录关联进程补充。\n");
    }
    let sg = &v["steam_game"];
    let _ = writeln!(
        s,
        "Steam: {}  AppID {}",
        match sg["steam_reported_running"].as_bool() {
            Some(true) => "此游戏运行中",
            Some(false) => "未报告此游戏运行",
            _ => "状态/映射不可用",
        },
        sg["app_ids"]
    );
    for p in sg["processes"].as_array().into_iter().flatten() {
        let _ = writeln!(
            s,
            "  Steam 关联 PID {}  {}",
            p["pid"],
            p["name"].as_str().unwrap_or("?")
        );
    }
    if let Some(error) = v["ui_error"].as_str() {
        let _ = writeln!(s, "Hook: {error}");
    }
    if !v["control_result"].is_null() {
        let _ = writeln!(s, "自动操作结果: {}", v["control_result"]);
    }
    if !v["exit_pause_result"].is_null() {
        let _ = writeln!(s, "退出前暂停: {}", v["exit_pause_result"]);
    }
    s
}
