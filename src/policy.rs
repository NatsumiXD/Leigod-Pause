//! Steam corroborates process evidence when available; otherwise use observed
//! game processes. A game must be seen running before its exit can arm a timer.
use crate::settings::Settings;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct Tracker {
    key: Option<(String, String)>,
    armed: bool,
    closed_since: Option<Instant>,
    names: BTreeSet<String>,
    sent: bool,
    pub message: String,
    outcome: Option<String>,
    process_only: bool,
}
impl Tracker {
    pub fn update(&mut self, value: &Value, settings: &Settings, now: Instant) -> Option<Value> {
        let id = value["game"]["id"].as_str();
        let session = value["game"]["session"].as_str();
        if value["accelerating"] != true || id.is_none() || session.is_none() {
            self.key = None;
            self.armed = false;
            self.closed_since = None;
            self.names.clear();
            self.sent = false;
            self.outcome = None;
            self.process_only = false;
            self.message = if value["accelerating"] == false {
                "未加速"
            } else {
                "等待可靠的加速会话"
            }
            .into();
            return None;
        }
        let key = (id.unwrap().to_owned(), session.unwrap().to_owned());
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key.clone());
            self.armed = false;
            self.closed_since = None;
            self.names.clear();
            self.sent = false;
            self.outcome = None;
        }
        let mut names = BTreeSet::new();
        for process in value["processes"].as_array().into_iter().flatten().chain(
            value["steam_game"]["processes"]
                .as_array()
                .into_iter()
                .flatten(),
        ) {
            if let Some(name) = process["name"].as_str() {
                names.insert(name.to_ascii_lowercase());
            }
        }
        let steam = value["steam_game"]["steam_reported_running"].as_bool();
        self.process_only = steam.is_none();
        let inventory = value["observed_process_names"].as_array();
        let tracked_alive = inventory
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|name| self.names.contains(&name.to_ascii_lowercase()));
        if steam == Some(true) && !names.is_empty()
            || self.process_only && (!names.is_empty() || tracked_alive)
        {
            self.armed = true;
            self.names.extend(names);
            self.closed_since = None;
            self.sent = false;
            self.outcome = None;
            self.message = if self.process_only {
                "Steam 不可用，游戏运行中（仅进程）"
            } else {
                "游戏运行中（Steam + 进程）"
            }
            .into();
            return None;
        }
        if !settings.auto_stop_enabled {
            self.closed_since = None;
            self.message = "自动停止已禁用".into();
            return None;
        }
        if !self.armed {
            self.message = "尚未确认游戏运行，不启动退出倒计时".into();
            return None;
        }
        if inventory.is_none() {
            self.closed_since = None;
            self.message = "进程快照不可用，取消退出倒计时".into();
            return None;
        }
        if steam == Some(true) || !names.is_empty() || tracked_alive {
            self.closed_since = None;
            self.message = "运行证据不完整，暂停退出判断".into();
            return None;
        }
        if self.sent {
            self.message = self
                .outcome
                .clone()
                .unwrap_or_else(|| "停止加速并暂停计时：已提交，等待结果".into());
            return None;
        }
        let since = *self.closed_since.get_or_insert(now);
        let delay = Duration::from_secs(settings.exit_delay_minutes * 60);
        if now.duration_since(since) >= delay {
            self.sent = true;
            self.message = "正在停止加速并暂停计时".into();
            return Some(
                json!({"action":"stop_and_pause","expected_game_id":key.0,"expected_session":key.1}),
            );
        }
        self.message = if self.process_only {
            "Steam 不可用，相关进程已退出，等待停止并暂停计时"
        } else {
            "游戏已退出，等待停止并暂停计时"
        }
        .into();
        None
    }
    pub fn complete(&mut self, result: &Value) {
        self.outcome = Some(if result["ok"] == true {
            "已停止加速并暂停计时".into()
        } else {
            format!(
                "自动操作未确认完成：{}；不自动重试",
                result["error"].as_str().unwrap_or("未知错误")
            )
        });
        if let Some(message) = &self.outcome {
            self.message = message.clone();
        }
    }
    pub fn status(&self, settings: &Settings, now: Instant) -> Value {
        let remaining = self.closed_since.filter(|_| !self.sent).map(|since| {
            (settings.exit_delay_minutes * 60)
                .saturating_sub(now.saturating_duration_since(since).as_secs())
        });
        json!({"message":self.message,"armed":self.armed,"remaining_seconds":remaining,"submitted":self.sent,
            "evidence_mode":self.key.as_ref().map(|_|if self.process_only{"process_only"}else{"steam_and_process"})})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn report(steam: Value, present: bool) -> Value {
        json!({"accelerating":true,"game":{"id":"5158","session":"A"},"steam_game":{"steam_reported_running":steam,"processes":if present{vec![json!({"name":"game.exe","pid":1})]}else{vec![]}},"processes":[],"observed_process_names":[]})
    }
    #[test]
    fn never_started_game_does_not_arm() {
        let mut p = Tracker::default();
        let t = Instant::now();
        assert!(
            p.update(&report(json!(false), false), &Settings::default(), t)
                .is_none()
        );
        assert!(
            p.update(
                &report(json!(false), false),
                &Settings::default(),
                t + Duration::from_secs(900)
            )
            .is_none()
        );
    }
    #[test]
    fn close_requires_ten_minutes_and_only_submits_once() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        p.update(&report(json!(true), true), &c, t);
        assert!(p.update(&report(json!(false), false), &c, t).is_none());
        assert!(
            p.update(
                &report(json!(false), false),
                &c,
                t + Duration::from_secs(599)
            )
            .is_none()
        );
        assert_eq!(
            p.update(
                &report(json!(false), false),
                &c,
                t + Duration::from_secs(600)
            )
            .unwrap()["expected_session"],
            "A"
        );
        assert!(
            p.update(
                &report(json!(false), false),
                &c,
                t + Duration::from_secs(605)
            )
            .is_none()
        );
    }
    #[test]
    fn restart_or_unknown_acceleration_cancels_countdown() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        p.update(&report(json!(true), true), &c, t);
        p.update(&report(json!(false), false), &c, t);
        let mut unknown = report(Value::Null, false);
        unknown["accelerating"] = Value::Null;
        p.update(&unknown, &c, t + Duration::from_secs(599));
        assert!(
            p.update(
                &report(json!(false), false),
                &c,
                t + Duration::from_secs(601)
            )
            .is_none()
        );
        p.update(&report(json!(true), true), &c, t + Duration::from_secs(602));
        assert!(p.status(&c, t)["remaining_seconds"].is_null());
    }
    #[test]
    fn a_new_session_of_the_same_game_must_arm_again() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        p.update(&report(json!(true), true), &c, t);
        let mut next = report(json!(false), false);
        next["game"]["session"] = json!("B");
        assert!(p.update(&next, &c, t + Duration::from_secs(1200)).is_none());
        assert_eq!(p.status(&c, t)["armed"], false);
    }
    #[test]
    fn surviving_named_process_prevents_countdown() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        p.update(&report(json!(true), true), &c, t);
        let mut v = report(json!(false), false);
        v["observed_process_names"] = json!(["GAME.EXE"]);
        p.update(&v, &c, t);
        assert!(p.status(&c, t)["remaining_seconds"].is_null());
    }
    #[test]
    fn steam_absent_from_start_uses_configured_process_then_times_its_exit() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        let mut running = report(Value::Null, false);
        running["steam_game"] = Value::Null;
        running["processes"] = json!([{"name":"game.exe","pid":20}]);
        running["observed_process_names"] = json!(["game.exe"]);
        p.update(&running, &c, t);
        assert_eq!(p.status(&c, t)["armed"], true);
        assert_eq!(p.status(&c, t)["evidence_mode"], "process_only");
        let mut closed = report(Value::Null, false);
        closed["steam_game"] = Value::Null;
        assert!(p.update(&closed, &c, t + Duration::from_secs(5)).is_none());
        assert!(
            p.update(&closed, &c, t + Duration::from_secs(604))
                .is_none()
        );
        assert!(
            p.update(&closed, &c, t + Duration::from_secs(605))
                .is_some()
        );
    }
    #[test]
    fn losing_steam_does_not_cancel_an_existing_exit_countdown() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        p.update(&report(json!(true), true), &c, t);
        p.update(&report(json!(false), false), &c, t);
        p.update(
            &report(Value::Null, false),
            &c,
            t + Duration::from_secs(300),
        );
        assert_eq!(
            p.status(&c, t + Duration::from_secs(300))["remaining_seconds"],
            300
        );
        assert!(
            p.update(
                &report(Value::Null, false),
                &c,
                t + Duration::from_secs(600)
            )
            .is_some()
        );
    }
    #[test]
    fn process_only_restart_and_remembered_name_cancel_countdown() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        p.update(&report(Value::Null, true), &c, t);
        p.update(&report(Value::Null, false), &c, t);
        let mut remembered = report(Value::Null, false);
        remembered["observed_process_names"] = json!(["GAME.EXE"]);
        p.update(&remembered, &c, t + Duration::from_secs(599));
        assert!(p.status(&c, t)["remaining_seconds"].is_null());
        assert!(
            p.update(
                &report(Value::Null, false),
                &c,
                t + Duration::from_secs(601)
            )
            .is_none()
        );
    }
    #[test]
    fn available_steam_running_flag_still_prevents_exit_without_processes() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        p.update(&report(json!(true), true), &c, t);
        p.update(&report(Value::Null, false), &c, t);
        assert!(
            p.update(
                &report(json!(true), false),
                &c,
                t + Duration::from_secs(599)
            )
            .is_none()
        );
        assert!(p.status(&c, t)["remaining_seconds"].is_null());
        assert!(
            p.update(
                &report(Value::Null, false),
                &c,
                t + Duration::from_secs(601)
            )
            .is_none()
        );
    }
    #[test]
    fn no_process_observation_or_missing_inventory_cannot_prove_exit() {
        let mut p = Tracker::default();
        let c = Settings::default();
        let t = Instant::now();
        assert!(p.update(&report(Value::Null, false), &c, t).is_none());
        assert_eq!(p.status(&c, t)["armed"], false);
        p.update(&report(Value::Null, true), &c, t);
        p.update(&report(Value::Null, false), &c, t);
        let mut missing = report(Value::Null, false);
        missing
            .as_object_mut()
            .unwrap()
            .remove("observed_process_names");
        p.update(&missing, &c, t + Duration::from_secs(599));
        assert!(p.status(&c, t)["remaining_seconds"].is_null());
        assert!(
            p.update(
                &report(Value::Null, false),
                &c,
                t + Duration::from_secs(601)
            )
            .is_none()
        );
    }
}
