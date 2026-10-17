//! Shared, idempotent exit request. Root, close handler and watchdog all use the
//! same ID so competing exit paths cannot toggle billing twice.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub request_path: PathBuf,
    pub result_path: PathBuf,
    pub heartbeat_path: PathBuf,
    pub id: String,
    #[serde(default)]
    pub receipt_path: Option<PathBuf>,
    #[serde(default)]
    pub audit_path: Option<PathBuf>,
}
static CURRENT: OnceLock<Mutex<Option<Endpoint>>> = OnceLock::new();
pub fn register(endpoint: Endpoint) {
    if let Ok(mut current) = CURRENT.get_or_init(|| Mutex::new(None)).lock() {
        *current = Some(endpoint);
    }
}
pub fn current() -> Option<Endpoint> {
    CURRENT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()?
        .clone()
}
fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn read(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() > 16384 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn sanitize(value: &Value, id: &str) -> Option<Value> {
    if value["id"].as_str() != Some(id) || !value["ok"].is_boolean() {
        return None;
    }
    let ok = value["ok"] == true && value["paused"] == true;
    Some(
        json!({"id":id,"action":"pause_before_exit","ok":ok,"paused":value["paused"]==true,
        "error":if ok{Value::Null}else{json!(value["error"].as_str().unwrap_or("pause_not_confirmed").chars().filter(|c|c.is_ascii_alphanumeric()||*c=='_').take(64).collect::<String>())},"completed_at_ms":epoch_ms()}),
    )
}
fn record(endpoint: &Endpoint, value: Value) -> Value {
    if let Ok(bytes) = serde_json::to_vec(&value) {
        for path in [&endpoint.receipt_path, &endpoint.audit_path]
            .into_iter()
            .flatten()
        {
            let _ = fs::write(path, &bytes);
        }
    }
    value
}
pub fn unavailable() -> Value {
    json!({"action":"pause_before_exit","ok":false,"paused":false,"error":"pause_hook_unavailable","completed_at_ms":epoch_ms()})
}
pub fn request_current(budget: Duration) -> Value {
    current()
        .map(|e| request(&e, budget))
        .unwrap_or_else(unavailable)
}
pub fn request(endpoint: &Endpoint, budget: Duration) -> Value {
    if let Some(value) = read(&endpoint.result_path).and_then(|v| sanitize(&v, &endpoint.id)) {
        return record(endpoint, value);
    }
    if let Some(value) = endpoint
        .receipt_path
        .as_ref()
        .and_then(|p| read(p))
        .and_then(|v| sanitize(&v, &endpoint.id))
        && value["ok"] == true
    {
        return value;
    }
    let failure = |error: &str| {
        record(
            endpoint,
            json!({"id":endpoint.id,"action":"pause_before_exit","ok":false,"paused":false,"error":error,"completed_at_ms":epoch_ms()}),
        )
    };
    // Reuse an already accepted request; do not replay the business operation.
    let existing = read(&endpoint.request_path).is_some_and(|v| {
        v["id"].as_str() == Some(&endpoint.id) && v["action"] == "pause_before_exit"
    });
    if !existing {
        let command =
            json!({"id":endpoint.id,"action":"pause_before_exit","expires_at_ms":epoch_ms()+2000});
        if fs::write(&endpoint.request_path, command.to_string()).is_err() {
            return failure("exit_transport_unavailable");
        }
    }
    let deadline = Instant::now() + budget;
    loop {
        if let Some(value) = read(&endpoint.result_path).and_then(|v| sanitize(&v, &endpoint.id)) {
            return record(endpoint, value);
        }
        if Instant::now() >= deadline {
            return failure("pause_not_confirmed_before_exit");
        }
        // The injected observer can finish even if the controller's helper was
        // terminated together with its console. Its lease remains valid here.
        let _ = fs::write(&endpoint.heartbeat_path, b"exit-pause\n");
        thread::sleep(Duration::from_millis(40));
    }
}
pub struct Guard {
    enabled: bool,
}
impl Guard {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
    pub fn finish(&mut self) -> Value {
        self.enabled = false;
        request_current(Duration::from_millis(4000))
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = request_current(Duration::from_millis(3500));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn result_must_confirm_pause_not_only_a_success_flag() {
        assert_eq!(
            sanitize(&json!({"id":"e","ok":true,"paused":false}), "e").unwrap()["ok"],
            false
        );
        assert!(sanitize(&json!({"id":"other","ok":true,"paused":true}), "e").is_none());
    }
    #[test]
    fn repeated_exit_requests_reuse_the_same_confirmed_result() {
        let dir = std::env::temp_dir().join(format!(
            "leigod-exit-test-{}-{}",
            std::process::id(),
            epoch_ms()
        ));
        fs::create_dir(&dir).unwrap();
        let endpoint = Endpoint {
            request_path: dir.join("request"),
            result_path: dir.join("result"),
            heartbeat_path: dir.join("heartbeat"),
            id: "exit-test".into(),
            receipt_path: Some(dir.join("receipt")),
            audit_path: None,
        };
        fs::write(
            &endpoint.result_path,
            r#"{"id":"exit-test","ok":true,"paused":true,"secret":"never exported"}"#,
        )
        .unwrap();
        assert_eq!(request(&endpoint, Duration::ZERO)["ok"], true);
        fs::remove_file(&endpoint.result_path).unwrap();
        let result = request(&endpoint, Duration::ZERO);
        assert_eq!(result["paused"], true);
        assert!(!result.to_string().contains("secret"));
        assert!(!endpoint.request_path.exists());
        fs::remove_file(endpoint.receipt_path.unwrap()).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
