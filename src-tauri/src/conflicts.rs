use crate::{
    model::*,
    platform,
    registry::{Environment, UserEnvironment},
};
use serde_json::Value;
use std::{
    env,
    path::{Path, PathBuf},
};

const AUTH: [&str; 14] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_API_KEY_HELPER",
    "CLAUDE_CODE_CLIENT_CERT",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
    "CCR_OAUTH_TOKEN_FILE",
    "CLAUDE_CODE_ACCOUNT_UUID",
    "CLAUDE_CODE_USER_EMAIL",
    "CLAUDE_CODE_ORGANIZATION_UUID",
    "CLAUDE_CODE_REMOTE",
];
fn active(key: &str, value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    if key.starts_with("CLAUDE_CODE_USE_") {
        !["0", "false", "no"].contains(&value.to_ascii_lowercase().as_str())
    } else {
        true
    }
}
pub fn home() -> AppResult<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "找不到用户主目录。".into())
}
pub fn settings_paths(project: Option<&Path>) -> AppResult<Vec<PathBuf>> {
    let base = home()?.join(".claude");
    let mut paths = vec![base.join("settings.json"), base.join("settings.local.json")];
    #[cfg(windows)]
    if let Some(program) = env::var_os("ProgramFiles") {
        paths.push(PathBuf::from(program).join("ClaudeCode/managed-settings.json"));
    }
    #[cfg(target_os = "macos")]
    paths.push(PathBuf::from(
        "/Library/Application Support/ClaudeCode/managed-settings.json",
    ));
    // Check project/parent files when the terminal helper knows the actual cwd.
    if let Some(project) = project {
        for parent in project.ancestors() {
            paths.push(parent.join(".claude/settings.json"));
            paths.push(parent.join(".claude/settings.local.json"));
        }
    }
    // Managed drop-ins have the same precedence as managed-settings.json.
    // Never let an uninspected policy file override the per-process route.
    let policy_directories: Vec<_> = paths
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "managed-settings.json"))
        .filter_map(|p| p.parent().map(|p| p.join("managed-settings.d")))
        .collect();
    for directory in policy_directories {
        paths.extend(managed_dropins(&directory)?);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}
fn managed_dropins(directory: &Path) -> AppResult<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("无法检查 Claude Code 托管配置目录，已停止启动。".into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "无法检查 Claude Code 托管配置文件。")?;
        if entry.path().extension().is_some_and(|v| v == "json") {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}
fn settings_conflicts(path: &Path, settings: &Value, proxy: Option<&str>) -> Vec<Conflict> {
    let mut found = Vec::new();
    let source = path.display().to_string();
    // Ordinary user/project proxy values are overridden by this child's inline
    // settings. Managed policy has higher precedence and must never be bypassed.
    let managed = path
        .file_name()
        .is_some_and(|n| n == "managed-settings.json")
        || path
            .components()
            .any(|c| c.as_os_str() == "managed-settings.d");
    if settings
        .get("apiKeyHelper")
        .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    {
        found.push(Conflict {
            source: source.clone(),
            key: "apiKeyHelper".into(),
        });
    }
    if let Some(map) = settings.get("env").and_then(Value::as_object) {
        for (key, value) in map {
            let key_upper = key.to_ascii_uppercase();
            let content = value.as_str().unwrap_or("");
            let auth = (AUTH.contains(&key_upper.as_str())
                || key_upper == "CLAUDE_CODE_OAUTH_TOKEN"
                || key_upper == "CLAUDE_SECURESTORAGE_CONFIG_DIR"
                || key_upper == "CLAUDE_CONFIG_DIR")
                && active(&key_upper, content);
            let wrong_proxy = managed
                && proxy.is_some_and(|expected| {
                    ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"].contains(&key_upper.as_str())
                        && content.trim_end_matches('/') != expected.trim_end_matches('/')
                });
            let bypass = managed
                && proxy.is_some()
                && key_upper == "NO_PROXY"
                && bypasses_anthropic(content);
            if auth || wrong_proxy || bypass {
                found.push(Conflict {
                    source: source.clone(),
                    key: format!("env.{key}"),
                });
            }
        }
    }
    found
}
pub fn bypasses_anthropic(value: &str) -> bool {
    value.split([',', ' ', '\t', '\n']).any(|s| {
        let s = s.trim().trim_start_matches('.').to_ascii_lowercase();
        s == "*"
            || ["api.anthropic.com", "claude.ai", "claude.com"]
                .iter()
                .any(|host| !s.is_empty() && (*host == s || host.ends_with(&format!(".{s}"))))
    })
}
pub fn scan(proxy: Option<&str>, project: Option<&Path>) -> AppResult<Vec<Conflict>> {
    let mut found = Vec::new();
    let registry = UserEnvironment;
    for key in AUTH {
        if let Some(value) = registry.read(key)? {
            if active(key, &value.string()?) {
                found.push(Conflict {
                    source: "用户环境变量".into(),
                    key: key.into(),
                });
            }
        }
        if env::var(key).ok().is_some_and(|v| active(key, &v)) {
            found.push(Conflict {
                source: "当前进程环境变量".into(),
                key: key.into(),
            });
        }
    }
    #[cfg(windows)]
    {
        use winreg::{enums::*, RegKey};
        if let Ok(system) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            KEY_READ,
        ) {
            for key in AUTH {
                if system
                    .get_value::<String, _>(key)
                    .ok()
                    .is_some_and(|v| active(key, &v))
                {
                    found.push(Conflict {
                        source: "系统环境变量".into(),
                        key: key.into(),
                    });
                }
            }
            if proxy.is_some()
                && system
                    .get_value::<String, _>("NO_PROXY")
                    .ok()
                    .is_some_and(|s| bypasses_anthropic(&s))
            {
                found.push(Conflict {
                    source: "系统环境变量".into(),
                    key: "NO_PROXY".into(),
                });
            }
        }
    }
    if env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|s| !s.is_empty())
        || registry.read("CLAUDE_CONFIG_DIR")?.is_some()
    {
        found.push(Conflict {
            source: "环境变量".into(),
            key: "CLAUDE_CONFIG_DIR".into(),
        });
    }
    if proxy.is_some() {
        #[cfg(target_os = "macos")]
        if env::var("no_proxy")
            .ok()
            .is_some_and(|s| bypasses_anthropic(&s))
        {
            found.push(Conflict {
                source: "当前进程环境变量".into(),
                key: "no_proxy".into(),
            });
        }
        if env::var("NO_PROXY")
            .ok()
            .is_some_and(|s| bypasses_anthropic(&s))
        {
            found.push(Conflict {
                source: "当前进程环境变量".into(),
                key: "NO_PROXY".into(),
            });
        }
        if let Some(value) = registry.read("NO_PROXY")? {
            if bypasses_anthropic(&value.string()?) {
                found.push(Conflict {
                    source: "用户环境变量".into(),
                    key: "NO_PROXY".into(),
                });
            }
        }
    }
    if platform::reject_managed_registry().is_err() {
        found.push(Conflict {
            source: "Claude Code 管理策略".into(),
            key: "注册表策略需核验".into(),
        });
    }
    for path in settings_paths(project)? {
        match platform::read_json::<Value>(&path) {
            Ok(Some(settings)) => found.extend(settings_conflicts(&path, &settings, proxy)),
            Ok(None) => (),
            Err(_) => found.push(Conflict {
                source: path.display().to_string(),
                key: "配置无法读取或格式不正确".into(),
            }),
        }
    }
    Ok(found)
}
pub fn require_clean(proxy: &str, project: Option<&Path>) -> AppResult<()> {
    let conflicts = scan(Some(proxy), project)?;
    if let Some(first) = conflicts.first() {
        return Err(format!(
            "存在配置冲突：{} 的 {}。请先处理再切换。",
            first.source, first.key
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_auth_and_route_without_disclosing_values() {
        let value = serde_json::json!({"apiKeyHelper":"secret-command","env":{
            "CLAUDE_CODE_OAUTH_TOKEN":"secret-token","HTTPS_PROXY":"http://127.0.0.1:9000",
            "CLAUDE_CODE_USE_VERTEX":"0","NO_PROXY":"localhost,.anthropic.com"}});
        let found = settings_conflicts(Path::new("fixture.json"), &value, Some(DEFAULT_PROXY));
        assert_eq!(found.len(), 2);
        let managed = settings_conflicts(
            Path::new("managed-settings.json"),
            &value,
            Some(DEFAULT_PROXY),
        );
        assert_eq!(managed.len(), 4);
        let json = serde_json::to_string(&found).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("9000"));
        assert!(!bypasses_anthropic(
            "localhost,127.0.0.1,::1,example.org,.example.org"
        ));
    }
    #[test]
    fn managed_dropins_are_checked_without_exposing_values() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("managed-settings.d");
        assert!(managed_dropins(&directory).unwrap().is_empty());
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("policy.json"), b"{}").unwrap();
        std::fs::write(directory.join("README.txt"), b"ignored").unwrap();
        let paths = managed_dropins(&directory).unwrap();
        assert_eq!(paths, vec![directory.join("policy.json")]);
        let policy = serde_json::json!({"env":{"HTTPS_PROXY":"","NO_PROXY":"*"}});
        let conflicts = settings_conflicts(&paths[0], &policy, Some(DEFAULT_PROXY));
        assert_eq!(conflicts.len(), 2);
    }
}
