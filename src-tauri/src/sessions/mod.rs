//! Managed terminal switching protocol. Runtime files contain no credentials.
use crate::{model::AppResult, platform, store::Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[cfg(windows)]
mod batch;
#[cfg(windows)]
mod control;
#[cfg(windows)]
mod owner;
#[cfg(windows)]
mod runtime;
#[cfg(windows)]
pub use batch::{execute, preview, refresh, progress, retry};
#[cfg(windows)]
pub(crate) use owner::{is_cli_session, live_rows};
#[cfg(windows)]
pub use owner::{focus_window, helper, launch, list_windows};
#[cfg(windows)]
pub use control::helper as control_helper;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub window_id: String,
    pub name: String,
    pub cwd: String,
    pub session_id: String,
    pub pid: u32,
    pub kind: String,
    pub permission_mode: String,
    pub account_name: String,
    pub has_background_tasks: bool,
    #[serde(default)]
    pub can_focus: bool,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub plan_id: String,
    pub created_at: String,
    pub supported: bool,
    pub windows: Vec<Window>,
    pub blockers: Vec<String>,
    #[serde(default)]
    pub global_blockers: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failed {
    pub window_id: String,
    pub cwd: String,
    pub session_id: String,
    pub error: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Result {
    pub operation_id: String,
    pub switched: bool,
    pub switched_at: Option<String>,
    pub restored: usize,
    pub failed: Vec<Failed>,
    pub errors: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Progress {
    pub stage: String,
    pub text: String,
    pub completed: usize,
    pub total: usize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Permissions {
    Preserve,
    Default,
    BypassPermissions,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeSession {
    pub pid: u32,
    #[serde(deserialize_with = "number_string")]
    pub proc_start: String,
    pub session_id: String,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub waiting_for: Option<Value>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
}
fn number_string<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::String(s) => s,
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}
pub(crate) fn uuid(value: &str) -> AppResult<String> {
    uuid::Uuid::parse_str(value)
        .map(|v| v.to_string())
        .map_err(|_| "会话编号无效。".into())
}
pub(crate) fn directory(store: &Store) -> AppResult<PathBuf> {
    let p = store.root.join("sessions-v1");
    if !p.is_dir() {
        platform::private_directory(&p)?;
    }
    Ok(p)
}
pub fn mutation_guard(store: &Store) -> AppResult<platform::Lease> {
    platform::lock(
        &directory(store)?.join("operation.lock"),
        Duration::from_secs(120),
    )
}
pub(crate) fn rows(home: &Path) -> AppResult<Vec<NativeSession>> {
    let path = home.join(".claude/sessions");
    let entries = match fs::read_dir(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err("无法读取 Claude 会话登记。".into()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let p = entry.map_err(|_| "无法读取会话登记。")?.path();
        if p.extension().is_some_and(|s| s == "json") {
            // Native Claude may truncate/rewrite a live registration in place.
            // A persistent read error blocks the entire inventory.
            let mut row = platform::read_json::<NativeSession>(&p);
            for delay in [20, 50] {
                if row.is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(delay));
                row = platform::read_json(&p);
            }
            if let Some(row) = row? {
                result.push(row)
            }
        }
    }
    Ok(result)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Target {
    pub sid: String,
    pub cwd: PathBuf,
    pub chain: Vec<String>,
    pub empty: bool,
}
fn completed(row: &Value) -> bool {
    match row.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            row.get("isApiErrorMessage") != Some(&Value::Bool(true))
                && row
                    .pointer("/message/stop_reason")
                    .is_some_and(Value::is_string)
        }
        Some("user") => {
            if row.get("isMeta") == Some(&Value::Bool(true)) {
                return false;
            }
            let content = row.pointer("/message/content");
            let text = match content {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(a)) => a
                    .iter()
                    .filter(|v| v.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|v| v.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            !text.trim().is_empty()
                && !text.starts_with("<local-command-")
                && !text.starts_with("<command-name>")
                && !text.starts_with("<bash-")
        }
        _ => false,
    }
}
pub(crate) fn follow(home: &Path, sid: &str, cwd: &Path) -> AppResult<Target> {
    let mut at = uuid(sid)?;
    let mut chain = Vec::new();
    let mut working = cwd.to_path_buf();
    for _ in 0..12 {
        if chain.contains(&at) {
            return Err("对话接续记录成环，未自动恢复。".into());
        }
        chain.push(at.clone());
        let projects = home.join(".claude/projects");
        let mut found = Vec::new();
        if let Ok(entries) = fs::read_dir(&projects) {
            for e in entries {
                let p = e
                    .map_err(|_| "无法读取对话目录。")?
                    .path()
                    .join(format!("{at}.jsonl"));
                if p.is_file() {
                    found.push(p)
                }
            }
        }
        if found.is_empty() {
            return Ok(Target {
                sid: at,
                cwd: working,
                chain,
                empty: true,
            });
        }
        if found.len() != 1 {
            return Err("存在同编号的多份对话记录，未自动恢复。".into());
        }
        let file = fs::File::open(&found[0]).map_err(|_| "无法读取对话接续记录。")?;
        // Track the last semantically relevant record in a bounded-memory forward pass.
        let mut continuation = None;
        let mut meaningful = false;
        let mut superseded = false;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|_| "无法读取对话接续记录。")?;
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(p) = v.get("cwd").and_then(Value::as_str) {
                if Path::new(p).is_absolute() {
                    working = PathBuf::from(p);
                }
            }
            if v.get("type").and_then(Value::as_str) == Some("continued-in") {
                continuation = v
                    .get("continuedInSessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            if completed(&v) {
                meaningful = true;
                continuation = None;
            }
            if v.get("superseded") == Some(&Value::Bool(true))
                || v.get("type").and_then(Value::as_str) == Some("superseded")
            {
                superseded = true;
            }
        }
        if let Some(next) = continuation {
            at = uuid(&next)?;
            continue;
        }
        if superseded {
            return Err("对话记录已被接续替代，未自动恢复旧记录。".into());
        }
        return Ok(Target {
            sid: at,
            cwd: working,
            chain,
            empty: !meaningful,
        });
    }
    Err("对话接续层数过多，未自动恢复。".into())
}
pub(crate) fn restored_args(
    args: &[OsString],
    sid: &str,
    empty: bool,
    mode: &Permissions,
) -> Vec<OsString> {
    let mut result = Vec::new();
    let mut i = 0;
    let mut positional = false;
    while i < args.len() {
        let a = args[i].to_string_lossy();
        if a == "--" {
            break;
        } // Never replay an initial prompt after restoring.
        if ["--resume", "-r", "--session-id", "--name", "-n"].contains(&a.as_ref()) {
            i += 2;
            continue;
        }
        if ["--continue", "-c", "--fork-session"].contains(&a.as_ref())
            || ["--resume=", "--session-id=", "--name="]
                .iter()
                .any(|p| a.starts_with(p))
        {
            i += 1;
            continue;
        }
        if !matches!(mode, Permissions::Preserve) {
            if a == "--permission-mode" {
                i += 2;
                continue;
            }
            if a.starts_with("--permission-mode=")
                || a == "--dangerously-skip-permissions"
                || a == "--allow-dangerously-skip-permissions"
            {
                i += 1;
                continue;
            }
        }
        // Supported value-taking root flags. Positional prompts are intentionally omitted.
        if a.starts_with('-') {
            let with_value = [
                "--model",
                "--effort",
                "--agent",
                "--add-dir",
                "--allowedTools",
                "--allowed-tools",
                "--disallowedTools",
                "--disallowed-tools",
                "--tools",
                "--mcp-config",
                "--plugin-dir",
                "--permission-mode",
                "--system-prompt",
                "--system-prompt-file",
                "--append-system-prompt",
                "--append-system-prompt-file",
                "--debug-file",
            ]
            .contains(&a.as_ref());
            result.push(args[i].clone());
            if with_value && i + 1 < args.len() {
                i += 1;
                result.push(args[i].clone());
            }
        } else {
            positional = true;
        }
        i += 1;
    }
    let _ = positional;
    match mode {
        Permissions::Preserve => (),
        Permissions::Default => result.extend(["--permission-mode".into(), "default".into()]),
        Permissions::BypassPermissions => result.push("--dangerously-skip-permissions".into()),
    }
    if !empty {
        result.extend(["--resume".into(), sid.into()]);
    }
    result
}
pub(crate) fn rpc(
    store: &Store,
    window: &str,
    action: &str,
    data: Value,
    timeout: Duration,
) -> AppResult<Value> {
    let root = directory(store)?.join(uuid(window)?);
    let req = uuid::Uuid::new_v4().to_string();
    let output = root.join("responses").join(format!("{req}.json"));
    platform::write_json(
        &root.join("requests").join(format!("{req}.json")),
        &serde_json::json!({"id":req,"action":action,"data":data,
            "expiresAt":chrono::Utc::now().timestamp_millis() + timeout.as_millis() as i64}),
    )?;
    let end = Instant::now() + timeout;
    loop {
        if let Some(v) = platform::read_json::<Value>(&output)? {
            let _ = fs::remove_file(&output);
            if let Some(e) = v.get("error").and_then(Value::as_str) {
                return Err(e.into());
            }
            return Ok(v);
        }
        if Instant::now() > end {
            let _ = fs::remove_file(root.join("requests").join(format!("{req}.json")));
            return Err("终端启动器未响应，账号未继续切换。".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}
#[cfg(not(windows))]
pub fn preview(_: &crate::service::Manager, _: Option<String>) -> AppResult<Plan> {
    Ok(Plan {
        plan_id: uuid::Uuid::new_v4().to_string(),
        created_at: crate::model::now(),
        supported: false,
        windows: vec![],
        blockers: vec![],
        global_blockers: vec![],
    })
}
#[cfg(not(windows))]
pub fn refresh(_: &crate::service::Manager, _: String) -> AppResult<Plan> {
    Err("自动恢复当前仅支持 Windows。".into())
}
#[cfg(not(windows))]
pub fn execute(_: &crate::service::Manager, _: String, _: Permissions) -> AppResult<Result> {
    Err("自动恢复当前仅支持 Windows。".into())
}
#[cfg(not(windows))]
pub fn progress(_: &Store, _: String) -> AppResult<Progress> {
    Err("自动恢复当前仅支持 Windows。".into())
}
#[cfg(not(windows))]
pub fn retry(_: &crate::service::Manager, _: String, _: String) -> AppResult<Result> {
    Err("自动恢复当前仅支持 Windows。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn continuation_ignores_api_errors_and_tool_results() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join(".claude/projects/p");
        fs::create_dir_all(&p).unwrap();
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let records = [
            json!({"type":"continued-in","continuedInSessionId":b}),
            json!({"type":"assistant","isApiErrorMessage":true,"message":{"stop_reason":"end_turn"}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","content":"finished"}]}}),
        ];
        fs::write(
            p.join(format!("{a}.jsonl")),
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let target = follow(t.path(), &a, t.path()).unwrap();
        assert_eq!(target.sid, b);
        assert!(target.empty);
    }
    #[test]
    fn restore_preserves_permissions_without_replaying_prompts() {
        let args: Vec<OsString> = [
            "--dangerously-skip-permissions",
            "--model",
            "opus",
            "-c",
            "old prompt",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        let r = restored_args(&args, "sid", false, &Permissions::Preserve);
        assert_eq!(
            r,
            vec![
                OsString::from("--dangerously-skip-permissions"),
                "--model".into(),
                "opus".into(),
                "--resume".into(),
                "sid".into()
            ]
        );
        let r = restored_args(&args, "sid", false, &Permissions::Default);
        assert!(!r.contains(&"--dangerously-skip-permissions".into()));
        assert!(r.contains(&"default".into()));
    }
}
