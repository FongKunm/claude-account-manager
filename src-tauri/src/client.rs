#[cfg(not(windows))]
use crate::registry::Environment;
use crate::{
    conflicts, crypto,
    model::*,
    network,
    registry::{EnvValue, UserEnvironment},
    service::Manager,
    store::Store,
};
use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
};

pub(crate) const PROXY_KEYS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];
// Local MCP/bridge services may remain local. Never inherit wildcard/domain
// exemptions from the parent shell: all non-loopback HTTP traffic uses the port.
pub(crate) const SESSION_NO_PROXY: &str = "localhost,127.0.0.1,::1";

fn validate_args(args: &[OsString]) -> AppResult<()> {
    if args.iter().any(|a| {
        let a = a.to_string_lossy();
        a == "--settings"
            || a.starts_with("--settings=")
            || a == "--setting-sources"
            || a.starts_with("--setting-sources=")
    }) {
        return Err("切号模式下不能用命令行覆盖 settings；请先切回普通登录。".into());
    }
    Ok(())
}

/// Configure only the new child. Claude re-applies settings.env on config
/// reload, so the environment alone is insufficient. An inline --settings
/// object keeps the route at session precedence without a mutable shared file.
/// Only loopback proxy addresses appear in argv; the token stays in child env.
#[cfg(any(not(windows), test))]
fn configure_selected(
    command: &mut Command,
    args: &[OsString],
    token: &str,
    proxy: &str,
) -> AppResult<()> {
    validate_args(args)?;
    let proxy = network::validate_proxy(proxy)?;
    let mut route = serde_json::Map::new();
    for key in PROXY_KEYS {
        command.env(key, &proxy);
        route.insert(key.into(), proxy.clone().into());
    }
    for key in ["NO_PROXY", "no_proxy"] {
        command.env(key, SESSION_NO_PROXY);
        route.insert(key.into(), SESSION_NO_PROXY.into());
    }
    command.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    // Prepend the owned option: user arguments may contain a positional `--`.
    command
        .arg("--settings")
        .arg(serde_json::json!({"env": route}).to_string())
        .args(args);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn executable() -> AppResult<PathBuf> {
    let preferred = conflicts::home()?.join(".local/bin/claude.exe");
    if preferred.is_file() {
        return Ok(preferred);
    }
    // Only native executables. Do not invoke .cmd through a shell with arbitrary arguments.
    for path in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = path.join("claude.exe");
        if path.is_file() {
            return Ok(path);
        }
    }
    Err("找不到原生 claude.exe，请先安装官方 Claude Code。".into())
}
#[cfg(target_os = "macos")]
pub(crate) fn executable() -> AppResult<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let mut candidates = vec![
        conflicts::home()?.join(".local/bin/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ];
    candidates.extend(
        env::split_paths(&env::var_os("PATH").unwrap_or_default()).map(|p| p.join("claude")),
    );
    let this = env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    for candidate in candidates {
        // Official native/Homebrew installations use symlinks. Resolve them for
        // execution only; data files and shell profiles still reject symlinks.
        let Ok(path) = candidate.canonicalize() else {
            continue;
        };
        if Some(&path) == this.as_ref() || path.file_name().is_some_and(|n| n == "cam-launcher") {
            continue;
        }
        if path
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return Ok(path);
        }
    }
    Err("找不到官方 Claude Code，请先在 Mac 安装原生命令行版。".into())
}
#[cfg(windows)]
pub fn launch_current(args: Vec<OsString>) -> AppResult<i32> {
    if management_command(&args) {
        // Native daemon/bootstrap verbs must remain argv[0]. They do not render
        // an interactive session and must not receive our root --settings first.
        let mut command = Command::new(executable()?);
        let verb = args[0].to_string_lossy();
        if matches!(
            verb.as_ref(),
            "mcp"
                | "auth"
                | "daemon"
                | "agents"
                | "attach"
                | "respawn"
                | "logs"
                | "stop"
                | "kill"
                | "rm"
        ) {
            let store = Store::discover()?;
            let _gate = crate::sessions::mutation_guard(&store)?;
            let _lock = store.lock()?;
            let db = store.read()?;
            let named = db
                .current
                .as_deref()
                .and_then(|id| db.ordinary(id))
                .is_some();
            if named && verb == "auth" && args.get(1).is_some_and(|a| a == "login" || a == "logout")
            {
                return Err("当前使用已登记的普通账号。请在换号台点「添加普通账号」登录，或直接选择已登记的账号。".into());
            }
            if named || verb == "mcp" {
                // The native verb must stay first. Its OAuth storage and process
                // environment still belong to the selected account.
                let cwd = env::current_dir().map_err(|_| "无法读取当前目录。")?;
                let (configured, _, _) =
                    managed_command(&store, &[], serde_json::json!({}), &cwd, None)?;
                for (key, value) in configured.get_envs() {
                    if let Some(value) = value {
                        command.env(key, value);
                    } else {
                        command.env_remove(key);
                    }
                }
            }
            // The command may wait for an OAuth browser; do not hold store/gate.
        }
        return command
            .args(&args)
            .status()
            .map(|s| s.code().unwrap_or(1))
            .map_err(|_| "无法运行 Claude Code 管理命令。".into());
    }
    crate::sessions::launch(args)
}
pub(crate) fn management_command(args: &[OsString]) -> bool {
    args.first().is_some_and(|a| {
        [
            "daemon",
            "agents",
            "mcp",
            "auth",
            "update",
            "install",
            "uninstall",
            "doctor",
            "setup-token",
            "plugin",
            "plugins",
            "completion",
            "--version",
            "-v",
            "--help",
            "-h",
            "attach",
            "logs",
            "stop",
            "kill",
            "rm",
            "respawn",
        ]
        .contains(&a.to_string_lossy().as_ref())
    })
}
#[cfg(not(windows))]
pub fn launch_current(args: Vec<OsString>) -> AppResult<i32> {
    let store = Store::discover()?;
    let environment = UserEnvironment;
    let mut command = Command::new(executable()?);
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    {
        let _lease = store.lock()?;
        store.recover(&environment)?;
        let db = store.read()?;
        if let Some(id) = &db.current {
            let account = db.account(id)?;
            let cwd = env::current_dir().map_err(|_| "无法读取当前项目路径。")?;
            conflicts::require_clean(&account.proxy, Some(&cwd))?;
            validate_args(&args)?;
            let check = network::network(&network::Curl, &account.proxy, &account.expected_ip);
            if !check.ok {
                return Err(check.error.unwrap_or_else(|| "出口检查失败。".into()));
            }
            let token = crypto::token(&account.encrypted_token)?;
            configure_selected(&mut command, &args, &token, &account.proxy)?;
        } else {
            command.args(&args);
            // An old PowerShell process may still have an earlier managed token.
            // Remove only values actually written by this app, never a manual token.
            for key in ENV_KEYS {
                if let Ok(value) = env::var(key) {
                    let hash = EnvValue::text(&value).fingerprint();
                    if db
                        .managed_hashes
                        .get(key)
                        .is_some_and(|known| known.contains(&hash))
                    {
                        match environment.read(key)? {
                            Some(latest) => {
                                command.env(key, latest.string()?.as_str());
                            }
                            None => {
                                command.env_remove(key);
                            }
                        }
                    }
                }
            }
        }
        // Spawn while holding the selection lock, then immediately release it.
        // Selection, credential and proxy were read under one lock. Subsequent
        // switches never mutate this child, its environment or its --settings.
        #[cfg(not(target_os = "macos"))]
        {
            let mut child = command.spawn().map_err(|_| "无法启动官方 Claude Code。")?;
            drop(_lease);
            child
                .wait()
                .map(|s| s.code().unwrap_or(1))
                .map_err(|_| "无法读取 Claude Code 退出状态。".into())
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::process::CommandExt;
            // Replace the helper so Ctrl-C, job control, TTY and exit status belong
            // to the official CLI. Its environment snapshot is already fixed.
            drop(_lease);
            let _ = command.exec();
            Err("无法启动官方 Claude Code。".into())
        }
    }
}
/// Pins the selected credential and fixed exit before any window is stopped.
/// Only local identifiers and a credential digest enter the restore request.
#[cfg(windows)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RestoreAccount {
    binding: crate::quota::Binding,
    proxy: String,
    expected_ip: String,
    normal_key: Option<String>,
}
#[cfg(windows)]
impl RestoreAccount {
    pub(crate) fn capture(store: &Store, db: &Database, id: Option<&str>) -> AppResult<Self> {
        let home = conflicts::home()?;
        let binding = crate::quota::Binding::selected(store, &home, id)?;
        let (proxy, expected_ip, normal_key) = match id {
            Some(id) => {
                let (p, ip) = db.route(id)?;
                (p.to_owned(), ip.to_owned(), None)
            }
            None => {
                let identity = crate::normal::identity(&home)?;
                if identity.account_uuid != binding.account_uuid || identity.org != binding.org {
                    return Err("普通登录身份已变化，请重新检查。".into());
                }
                let key = identity.cache_key().ok_or("普通登录的身份尚未确认。")?;
                let ip = store
                    .normal_checks()?
                    .get(&key)
                    .map(|r| r.expected_ip.clone())
                    .ok_or("普通登录尚无已确认的固定出口。")?;
                (normal_proxy(&home)?, ip, Some(key))
            }
        };
        network::validate_route(&proxy, &expected_ip)?;
        Ok(Self {
            binding,
            proxy,
            expected_ip,
            normal_key,
        })
    }
    fn verify(
        &self,
        store: &Store,
        db: &Database,
        revision: u64,
        cwd: &std::path::Path,
    ) -> AppResult<()> {
        let changed = "恢复目标的账号、凭据或固定出口已变化，未启动该窗口，请重新检查。";
        let b = &self.binding;
        if db.revision != revision || db.current != b.account_id || conflicts::home()? != b.home {
            return Err(changed.into());
        }
        match db.current.as_deref() {
            Some(id) => {
                if db.route(id)? != (&self.proxy, &self.expected_ip) {
                    return Err(changed.into());
                }
                let actual = crate::quota::Binding::selected(store, &b.home, Some(id))?;
                if actual.key != b.key
                    || actual.org != b.org
                    || actual.account_uuid != b.account_uuid
                    || actual.fingerprint != b.fingerprint
                {
                    return Err(changed.into());
                }
            }
            None => {
                let key = self.normal_key.as_deref().ok_or(changed)?;
                if normal_proxy(&b.home)? != self.proxy
                    || !store
                        .normal_checks()?
                        .get(key)
                        .is_some_and(|r| r.expected_ip == self.expected_ip)
                    || crypto::fingerprint(&crate::credentials::read(&b.home, true)?)
                        != b.fingerprint
                {
                    return Err(changed.into());
                }
                let current = crate::normal::identity(&b.home)?;
                if current.account_uuid.is_some() {
                    if current.account_uuid != b.account_uuid
                        || current.org != b.org
                        || current.cache_key().as_deref() != Some(key)
                    {
                        return Err(changed.into());
                    }
                } else if !crate::account_context::awaiting_default_profile(store, &b.home)? {
                    return Err(changed.into());
                }
            }
        }
        conflicts::require_clean(&self.proxy, Some(cwd))
    }
    pub(crate) fn check_window(
        &self,
        store: &Store,
        revision: u64,
        cwd: &std::path::Path,
    ) -> AppResult<()> {
        // The coordinator already committed the selection. This pass is read-only;
        // managed_command repeats it under the store lock immediately before launch.
        self.verify(store, &store.read()?, revision, cwd)?;
        // Two credential-free traces from this owner. No other window can reuse them.
        let check = network::network(&network::Curl, &self.proxy, &self.expected_ip);
        if !check.ok {
            return Err(check.error.unwrap_or_else(|| "窗口出口检查失败。".into()));
        }
        Ok(())
    }
    pub(crate) fn same_target(&self, other: &Self) -> bool {
        self.binding.account_id == other.binding.account_id
            && self.binding.key == other.binding.key
            && self.binding.org == other.binding.org
            && self.binding.account_uuid == other.binding.account_uuid
            && self.binding.home == other.binding.home
            && self.normal_key == other.normal_key
            && self.proxy == other.proxy
            && self.expected_ip == other.expected_ip
    }
}
#[cfg(windows)]
pub(crate) fn managed_command(
    store: &Store,
    args: &[OsString],
    mut settings: serde_json::Value,
    cwd: &std::path::Path,
    restore: Option<(&RestoreAccount, u64)>,
) -> AppResult<(Command, Option<crate::quota::Binding>, String)> {
    let db = store.read()?;
    let home = conflicts::home()?;
    validate_args(args)?;
    if let Some((account, revision)) = restore {
        account.verify(store, &db, revision, cwd)?;
    }
    let ordinary = db.current.as_deref().and_then(|id| db.ordinary(id));
    let (proxy, name, token, expected) = if let Some(a) = ordinary {
        (a.proxy.clone(), a.name.clone(), None, a.expected_ip.clone())
    } else if let Some(id) = db.current.as_deref() {
        let a = db.account(id)?;
        (
            a.proxy.clone(),
            a.name.clone(),
            Some(crypto::token(&a.encrypted_token)?),
            a.expected_ip.clone(),
        )
    } else {
        let identity = crate::normal::identity(&home)?;
        let (proxy, expected_ip) = if let Some((account, _)) = restore {
            (account.proxy.clone(), account.expected_ip.clone())
        } else {
            let check = identity
                .cache_key()
                .and_then(|k| store.normal_checks().ok()?.get(&k).cloned());
            (
                normal_proxy(&home)?,
                check.map(|c| c.expected_ip).unwrap_or_default(),
            )
        };
        (
            proxy,
            identity.email.unwrap_or_else(|| "普通登录".into()),
            None,
            expected_ip,
        )
    };
    if restore.is_none() {
        conflicts::require_clean(&proxy, Some(cwd))?;
        let check = network::bind_network(&network::Curl, &proxy, &expected);
        if !check.ok {
            return Err(check.error.unwrap_or_else(|| "出口检查失败。".into()));
        }
        if db.current.is_none() {
            crate::normal::remember_exit(store, &home, check.ip.as_deref())?;
        }
    }
    let cli = executable()?;
    let selected = match restore {
        Some((account, _)) => Ok(account.binding.clone()),
        None => crate::quota::Binding::selected(store, &home, db.current.as_deref()),
    };
    let pending_login = db.current.is_none() && selected.is_err();
    let mut binding = if pending_login {
        // Keep the official interactive login available after /logout.
        // Attribution stays pending until native profile matches this launch.
        crate::quota::Binding {
            account_id: None,
            key: "normal:pending".into(),
            fingerprint: crate::credentials::read(&home, true)
                .ok()
                .map(|b| crate::crypto::fingerprint(&b))
                .unwrap_or_default(),
            account_uuid: None,
            org: None,
            home: home.clone(),
            context_generation: None,
        }
    } else {
        selected?
    };
    binding.context_generation = Some(crate::account_context::prepare(store, &binding, &cli)?);
    let mut command = Command::new(cli);
    // Keep the native default spelling; only the global identity file moves.
    command.env("CLAUDE_CONFIG_DIR", home.join(".claude"));
    let mut route = serde_json::Map::new();
    for key in PROXY_KEYS {
        command.env(key, &proxy);
        route.insert(key.into(), proxy.clone().into());
    }
    for key in ["NO_PROXY", "no_proxy"] {
        command.env(key, SESSION_NO_PROXY);
        route.insert(key.into(), SESSION_NO_PROXY.into());
    }
    command.env("CLAUDE_CODE_DISABLE_BG_EXIT_HANDOFF", "1");
    route.insert("CLAUDE_CODE_DISABLE_BG_EXIT_HANDOFF".into(), "1".into());
    if crate::session_win::classic_console() {
        // Claude 2.1.318's first alternate-buffer frame misaligns conhost. Its
        // built-in classic renderer keeps the prompt inspectable without keys
        // or window resizing; Windows Terminal retains its chosen renderer.
        command.env("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN", "1");
        route.insert("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN".into(), "1".into());
    }
    if let Some(token) = token {
        command.env("CLAUDE_CODE_OAUTH_TOKEN", token.as_str());
        // On Windows the native daemon drops the environment OAuth token when
        // its credential store contains a refresh token. An account-private
        // empty OAuth store keeps native setup-token handling (expiresAt=null),
        // without copying /login credentials or splitting settings/history.
        let id = db.current.as_deref().ok_or("缺少所选账号。")?;
        let private = crate::account_context::token_storage(store, id)?;
        // Deliberately not settings.env: daemon startup preserves only the real
        // launch environment for this variable. MCP OAuth may live here normally.
        command.env("CLAUDE_SECURESTORAGE_CONFIG_DIR", private);
    } else {
        if let Ok(value) = env::var("CLAUDE_CODE_OAUTH_TOKEN") {
            let hash = EnvValue::text(&value).fingerprint();
            if !db
                .managed_hashes
                .get("CLAUDE_CODE_OAUTH_TOKEN")
                .is_some_and(|known| known.contains(&hash))
                && !value.is_empty()
            {
                return Err(
                    "当前终端有手动 Token，不能按普通登录自动恢复；请先清除该终端的手动 Token。"
                        .into(),
                );
            }
        }
        command.env_remove("CLAUDE_CODE_OAUTH_TOKEN");
        let credential_home = match ordinary {
            Some(a) => crate::ordinary::home(store, &a.id)?,
            None => home.clone(),
        };
        command.env(
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
            credential_home.join(".claude"),
        );
    }
    settings["env"] = route.into();
    command
        .arg("--settings")
        .arg(settings.to_string())
        .args(args);
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    Ok((command, Some(binding), name))
}
pub(crate) fn normal_proxy(home: &std::path::Path) -> AppResult<String> {
    let mut selected = None;
    for name in ["settings.json", "settings.local.json"] {
        if let Some(v) =
            crate::platform::read_json::<serde_json::Value>(&home.join(".claude").join(name))?
        {
            for key in [
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
                "HTTP_PROXY",
                "http_proxy",
            ] {
                if let Some(p) = v
                    .get("env")
                    .and_then(|v| v.get(key))
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.is_empty())
                {
                    selected = Some(p.to_owned());
                    break;
                }
            }
        }
    }
    network::validate_proxy(selected.as_deref().unwrap_or(DEFAULT_PROXY))
}
pub fn worker_entry() -> bool {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.first().is_some_and(|s| s == "--uninstall-hooks") {
        let result = Manager::discover().and_then(|manager| manager.uninstall(&UserEnvironment));
        if result.is_err() {
            std::process::exit(1);
        }
        return true;
    }
    false
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
