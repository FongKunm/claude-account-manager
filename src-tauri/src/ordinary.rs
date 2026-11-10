//! Named native OAuth logins. Each login has a permanent secure storage directory;
//! Claude itself exchanges and rotates credentials there. Switching only selects it.
use crate::{
    client, conflicts, credentials, model::*, network, normal, platform, quota,
    registry::UserEnvironment, service::Manager, session_win::Job, store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    pub login_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
}

fn uuid(id: &str) -> AppResult<Uuid> {
    Uuid::parse_str(id)
        .ok()
        .filter(|v| v.to_string() == id)
        .ok_or_else(|| "登录编号无效。".into())
}
fn base(store: &Store) -> PathBuf {
    store.root.join("ordinary-logins")
}
pub fn home(store: &Store, id: &str) -> AppResult<PathBuf> {
    let id = uuid(id.strip_prefix("login-").ok_or("普通账号编号无效。")?)?;
    platform::checked_path(&base(store).join(id.to_string()))
}
fn pending(store: &Store, id: &str) -> AppResult<PathBuf> {
    platform::checked_path(&base(store).join(format!("adding-{}", uuid(id)?)))
}
fn lock_file(store: &Store) -> PathBuf {
    store.root.join("ordinary-enrollment.lock")
}

#[derive(Serialize, Deserialize)]
struct Seal {
    digest: String,
    grant: credentials::LoginGrant,
}
pub fn profile(dir: &Path) -> AppResult<Value> {
    let doc: Value =
        platform::read_json(&dir.join(".claude/.claude.json"))?.ok_or("缺少官方登录资料。")?;
    doc.get("oauthAccount")
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(|| "未收到官方登录账号资料。".into())
}
pub fn matches(profile: &Value, a: &OrdinaryRecord) -> bool {
    profile["accountUuid"].as_str() == Some(&a.account_uuid)
        && profile["organizationUuid"].as_str() == Some(&a.org)
}
pub fn verify_slot(store: &Store, a: &OrdinaryRecord) -> AppResult<String> {
    let dir = home(store, &a.id)?;
    let _lock = platform::lock(&dir.join("grant.lock"), Duration::from_secs(2))?;
    if !matches(&profile(&dir)?, a) {
        return Err("普通登录身份已变化，已停止使用这个账号。".into());
    }
    let (digest, grant) =
        credentials::login_snapshot(&dir).ok_or("该账号的普通登录凭据已失效。")?;
    let seal: Seal =
        platform::read_json(&dir.join("grant.json"))?.ok_or("普通账号缺少登录核对记录。")?;
    if seal.digest != digest {
        // A dormant slot can miss several native refreshes. Their whole-second
        // rounding accumulates, so the enrollment deadline is not a permanent
        // identity test. A fresh native profile from this credential rotation
        // can independently confirm the account. Other grant changes still fail.
        if !seal.grant.permits(&grant)
            && !(seal.grant.permits_cumulative_rounding(&grant)
                && crate::account_context::confirmed_ordinary_profile(store, a, &dir)?)
        {
            return Err("该账号的登录授权已改变，尚未获得同一账号的新身份确认；未改凭据，请保留已打开的对话。".into());
        }
        if credentials::login_snapshot(&dir).as_ref() != Some(&(digest.clone(), grant.clone()))
            || !matches(&profile(&dir)?, a)
        {
            return Err("Claude 正在更新登录资料，请稍后重试。".into());
        }
        // Checkpoint every accepted rotation, including launches and quota
        // reads. Waiting for a later status callback leaves a stale baseline.
        platform::write_json(&dir.join("grant.json"), &Seal { digest: digest.clone(), grant })?;
    }
    Ok(digest)
}
/// Only the manager's continuity checkpoint is updated; native credentials and
/// profiles are always left to Claude Code.
pub fn seal_renewal(store: &Store, a: &OrdinaryRecord) -> AppResult<()> {
    verify_slot(store, a).map(|_| ())
}
pub fn view(
    store: &Store,
    shared: &Path,
    a: &OrdinaryRecord,
    current: bool,
) -> AppResult<OrdinaryView> {
    let doc = profile(&home(store, &a.id)?).ok().filter(|p| matches(p, a));
    let identity = normal::identity_document(json!({"oauthAccount":doc}))?;
    let usage = quota::Binding::selected(store, shared, Some(&a.id))
        .ok()
        .map(|b| quota::cached(store, &b))
        .transpose()?
        .flatten();
    Ok(OrdinaryView {
        proxy: a.proxy.clone(),
        note: a.note.clone(),
        source_token_id: a.source_token_id.clone(),
        last_network_check: None,
        id: a.id.clone(),
        name: a.name.clone(),
        normal: NormalLogin {
            email: identity.email,
            org: Some(a.org.clone()),
            current,
            plan: if a.plan.is_empty() { identity.plan } else { a.plan.clone() },
            billing_type: identity.billing_type,
            subscription_status: None,
            expected_ip: a.expected_ip.clone(),
            last_usage_check: usage,
        },
    })
}

pub fn check_slot_routes(
    db: &Database,
    proxy: &str,
    ip: &str,
    editing_token: Option<&str>,
) -> AppResult<()> {
    for a in &db.ordinary_accounts {
        if editing_token.is_some() && a.source_token_id.as_deref() == editing_token {
            if proxy != a.proxy || !same_ip(ip, &a.expected_ip) {
                return Err(
                    "这张令牌卡已经与同号的普通登录共用固定出口，不能更换端口或 IP。".into(),
                );
            }
        } else if same_port(proxy, &a.proxy) || same_ip(ip, &a.expected_ip) {
            return Err(format!(
                "专用端口或出口 IP 已绑定「{}」，请保留各账号原有的出口。",
                a.name
            ));
        }
    }
    Ok(())
}
fn enrollment_route(
    db: &Database,
    proxy: &str,
    ip: &str,
    source_id: Option<&str>,
) -> AppResult<()> {
    check_slot_routes(db, proxy, ip, None)?;
    if let Some(id) = source_id {
        let a = db.account(id)?;
        if a.org.as_deref().is_none_or(str::is_empty) {
            return Err(
                "这张令牌卡没有已核对的组织编号，暂时不能登记为同一个普通账号。请保留原出口。"
                    .into(),
            );
        }
        if !same_port(proxy, &a.proxy) || !same_ip(ip, &a.expected_ip) {
            return Err("已有账号只能沿用原令牌卡的专用端口和出口 IP。".into());
        }
    }
    for a in &db.accounts {
        if source_id != Some(a.id.as_str())
            && (same_port(proxy, &a.proxy) || same_ip(ip, &a.expected_ip))
        {
            return Err(format!("该端口或出口已绑定「{}」。如果是同一个个人账号，请选择这张原令牌卡；请勿为已有账号更换出口。", a.name));
        }
    }
    Ok(())
}
fn enrollment_identity(
    db: &Database,
    org: &str,
    subscription: Option<&str>,
    source_id: Option<&str>,
) -> AppResult<()> {
    if let Some(id) = source_id {
        let a = db.account(id)?;
        if a.org.as_deref() != Some(org)
            || !matches!(subscription, Some("pro" | "max" | "max5" | "max20"))
        {
            return Err(
                "官方登录资料与选定的个人令牌卡不符，未登记，两张卡的原记录均未更动。".into(),
            );
        }
    }
    if let Some(a) = db
        .accounts
        .iter()
        .find(|a| a.org.as_deref() == Some(org) && source_id != Some(a.id.as_str()))
    {
        return Err(format!(
            "该登录组织已在「{}」登记。请选原卡并沿用原端口和出口。",
            a.name
        ));
    }
    Ok(())
}
pub(crate) fn same_port(a: &str, b: &str) -> bool {
    let port = |s: &str| url::Url::parse(s).ok()?.port_or_known_default();
    port(a).is_some_and(|p| Some(p) == port(b))
}
pub(crate) fn same_ip(a: &str, b: &str) -> bool {
    let addr = |s: &str| s.parse::<std::net::IpAddr>().ok().map(|v| v.to_canonical());
    addr(a).is_some_and(|v| Some(v) == addr(b))
}
fn check_route(proxy: &str, ip: &str) -> AppResult<()> {
    let check = network::network(&network::Curl, proxy, ip);
    if check.ok {
        Ok(())
    } else {
        Err(check.error.unwrap_or_else(|| "出口核对失败。".into()))
    }
}

// The native CLI calls BROWSER with its own PKCE link. This helper hands that
// link to the local dialog. It never opens a browser or handles an auth code.
pub fn browser_handoff(args: &[std::ffi::OsString]) -> Option<AppResult<()>> {
    let id = std::env::var("CAM_ORDINARY_BROWSER").ok()?;
    Some((|| {
        if args.len() != 1 {
            return Err("登录浏览器参数无效。".into());
        }
        let raw = args[0].to_str().ok_or("登录链接格式无效。")?;
        let url = url::Url::parse(raw).map_err(|_| "官方登录链接格式无效。")?;
        if raw.len() > 16384
            || url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some("claude.com" | "claude.ai" | "platform.claude.com" | "console.anthropic.com")
            )
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || !url.query_pairs().any(|(k, v)| {
                k == "redirect_uri"
                    && url::Url::parse(&v).is_ok_and(|u| {
                        u.scheme() == "http"
                            && matches!(u.host_str(), Some("localhost" | "127.0.0.1"))
                            && u.port().is_some()
                            && u.path() == "/callback"
                    })
            })
        {
            return Err("没有获得带本机回调的官方登录链接。".into());
        }
        let dir = pending(&Store::discover()?, &id)?;
        if std::env::var_os("USERPROFILE").map(PathBuf::from).as_ref() != Some(&dir) {
            return Err("登录目录不匹配。".into());
        }
        platform::write_json(&dir.join("browser-link.json"), &raw)
    })())
}

fn native_login(id: &str, dir: &Path, proxy: &str) -> AppResult<(Job, std::process::Child)> {
    let config = dir.join(".claude");
    platform::private_directory(&config)?;
    let cli = client::executable()?;
    let helper = std::env::current_exe()
        .map_err(|_| "找不到登录助手。")?
        .with_file_name("cam-launcher.exe");
    if !helper.is_file() {
        return Err("请将 cam-launcher.exe 放在换号台旁边。".into());
    }
    let mut cmd = Command::new(cli);
    cmd.env_clear();
    for k in [
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATH",
        "PATHEXT",
        "TEMP",
        "TMP",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROCESSOR_ARCHITECTURE",
        "NUMBER_OF_PROCESSORS",
        "CLAUDE_CODE_GIT_BASH_PATH",
    ] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    #[cfg(feature = "synthetic-tests")]
    {
        crate::lab::check_proxy(proxy)?;
        cmd.env("NODE_EXTRA_CA_CERTS", crate::lab::config()?.ca);
    }
    for k in client::PROXY_KEYS {
        cmd.env(k, proxy);
    }
    cmd.env("HOME", dir)
        .env("USERPROFILE", dir)
        .env("CLAUDE_CONFIG_DIR", &config)
        .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", &config)
        .env("NO_PROXY", client::SESSION_NO_PROXY)
        .env("BROWSER", helper)
        .env("CAM_ORDINARY_BROWSER", id)
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1")
        .current_dir(dir)
        .args(["auth", "login", "--claudeai"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let job = Job::new()?;
    let (child, _) = job.spawn_hidden(&mut cmd)?;
    Ok((job, child))
}

fn commit(
    m: &Manager,
    dir: &Path,
    id: &str,
    name: String,
    note: String,
    plan: String,
    proxy: String,
    ip: String,
    source_token_id: Option<String>,
) -> AppResult<()> {
    check_route(&proxy, &ip)?;
    let p = profile(dir)?;
    let identity = normal::identity_document(json!({"oauthAccount":&p}))?;
    let account_uuid = identity.account_uuid.ok_or("官方登录没有返回账号编号。")?;
    let org = identity.org.ok_or("官方登录没有返回组织编号。")?;
    let (digest, grant) = credentials::login_snapshot(dir).ok_or("官方登录未写入有效凭据。")?;
    let bytes = credentials::read(dir, false)?;
    #[derive(Deserialize)]
    struct Login<'a> {
        #[serde(rename = "refreshToken")]
        refresh: &'a str,
    }
    #[derive(Deserialize)]
    struct File<'a> {
        #[serde(rename = "claudeAiOauth", borrow)]
        oauth: Login<'a>,
    }
    if !serde_json::from_slice::<File>(&bytes).is_ok_and(|v| !v.oauth.refresh.is_empty()) {
        return Err("此次登录没有可续期的凭据，未登记账号。".into());
    }
    let store = &m.store;
    let _gate = crate::sessions::mutation_guard(store)?;
    let _lock = store.lock()?;
    store.recover(&UserEnvironment)?;
    let before = store.read()?;
    enrollment_route(&before, &proxy, &ip, source_token_id.as_deref())?;
    if let Some(id) = &source_token_id {
        if before.account(id)?.proxy != proxy {
            return Err("登记期间原令牌卡的代理已变化，请重新开始登录。".into());
        }
    }
    enrollment_identity(
        &before,
        &org,
        grant.subscription_type.as_deref(),
        source_token_id.as_deref(),
    )?;
    if before
        .ordinary_accounts
        .iter()
        .any(|a| a.account_uuid == account_uuid)
    {
        return Err("这个普通账号已登记。".into());
    }
    let shared = m.normal_home()?;
    let legacy = normal::identity(&shared)?;
    if let Some(key) = legacy.cache_key() {
        let same_login = legacy.account_uuid.as_ref() == Some(&account_uuid)
            && legacy.org.as_ref() == Some(&org);
        if same_port(&client::normal_proxy(&shared)?, &proxy) && !same_login {
            return Err("此端口正供本机普通登录使用，登录的账号与它不同，未登记。".into());
        }
        if let Some(check) = store.normal_checks()?.get(&key) {
            if !check.expected_ip.is_empty() && (same_ip(&check.expected_ip, &ip) != same_login) {
                return Err("本机普通登录的账号与固定出口记录不符，未登记。".into());
            }
        }
    }
    let _ = fs::remove_file(dir.join("browser-link.json"));
    platform::write_json(&dir.join("grant.json"), &Seal { digest, grant })?;
    let a = OrdinaryRecord {
        id: format!("login-{id}"),
        name,
        note,
        plan,
        proxy,
        expected_ip: ip,
        account_uuid,
        org,
        source_token_id,
    };
    let target = home(store, &a.id)?;
    let mut after = before.clone();
    after.ordinary_accounts.push(a);
    fs::rename(dir, &target).map_err(|_| "无法保存普通登录的专用目录。")?;
    if let Err(e) = store.select(&UserEnvironment, &before, after) {
        // A committed row owns its directory even if post-commit cleanup failed.
        if store.read()?.ordinary(&format!("login-{id}")).is_none() {
            fs::rename(&target, dir).map_err(|_| "未能回滚登录目录。")?;
        }
        return Err(e);
    }
    Ok(())
}

pub fn begin(
    m: Manager,
    name: String,
    proxy: String,
    expected: String,
    source_token_id: Option<String>,
) -> AppResult<Started> {
    begin_with_details(m, name, proxy, expected, source_token_id, String::new(), String::new())
}

pub fn begin_with_details(m: Manager, name: String, proxy: String, expected: String, source_token_id: Option<String>, note: String, plan: String) -> AppResult<Started> {
    if note.chars().count() > 2000 || !["", "pro", "max5", "max20"].contains(&plan.as_str()) {
        return Err("请检查备注长度及套餐选项。".into());
    }
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("请填写 1 到 100 字的账号名称。".into());
    }
    let (proxy, ip) = network::validate_route(&proxy, &expected)?;
    crate::account_context::check_enrollment_backend(&m.normal_home()?)?;
    let store = &m.store;
    let lease = platform::lock(&lock_file(store), Duration::from_millis(5))
        .map_err(|_| "已有一个普通账号正在登录。")?;
    let db = store.read()?;
    enrollment_route(&db, &proxy, &ip, source_token_id.as_deref())?;
    // A paired login always uses the stored listener, including its exact
    // loopback address; an alternate 127.x address can host a different listener.
    let proxy = if let Some(id) = &source_token_id {
        db.account(id)?.proxy.clone()
    } else {
        proxy
    };
    conflicts::require_clean(&proxy, None)?;
    check_route(&proxy, &ip)?;
    let root = base(store);
    if !root.is_dir() {
        platform::private_directory(&root)?;
    }
    // At most one writer holds the lease. A process crash closes its native Job.
    // Only remove the single staging path named by our last pending registration.
    if let Some(old) = platform::read_json::<String>(&root.join("latest.json"))? {
        let old_dir = pending(store, &old)?;
        if old_dir.is_dir() {
            fs::remove_dir_all(old_dir).map_err(|_| "上一次登录的临时目录尚在使用。")?;
        }
        let _ = fs::remove_file(root.join(format!("status-{old}.json")));
    }
    let id = Uuid::new_v4().to_string();
    let dir = pending(store, &id)?;
    platform::private_directory(&dir)?;
    platform::write_json(
        &root.join(format!("status-{id}.json")),
        &Status {
            status: "waiting".into(),
            error: None,
            login_url: None,
        },
    )?;
    platform::write_json(&root.join("latest.json"), &id)?;
    let started = Started {
        login_id: id.clone(),
    };
    std::thread::spawn(move || {
        let _lease = lease;
        let result = (|| {
            let (job, mut child) = native_login(&id, &dir, &proxy)?;
            let until = Instant::now() + Duration::from_secs(600);
            while child
                .try_wait()
                .map_err(|_| "无法读取登录进程状态。")?
                .is_none()
            {
                if dir.join("cancel").exists() || Instant::now() >= until {
                    job.stop()?;
                    let _ = child.wait();
                    return Err("登录已取消或超时。".into());
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            if !child.wait().map_err(|_| "无法确认登录结果。")?.success() {
                return Err("官方登录未完成。请核对浏览器、账号和节点后重试。".into());
            }
            job.stop()?;
            let _finish = platform::lock(&root.join("finish.lock"), Duration::from_secs(30))?;
            if dir.join("cancel").exists() {
                return Err("登录已取消。".into());
            }
            commit(&m, &dir, &id, name, note, plan, proxy, ip, source_token_id)
        })();
        if dir.is_dir() {
            let _ = fs::remove_dir_all(&dir);
        }
        let s = Status {
            status: if result.is_ok() { "complete" } else { "failed" }.into(),
            error: result.err(),
            login_url: None,
        };
        let _ = platform::write_json(&root.join(format!("status-{id}.json")), &s);
    });
    Ok(started)
}
pub fn status(store: &Store, id: &str) -> AppResult<Status> {
    let dir = pending(store, id)?;
    let root = base(store);
    if platform::read_json::<String>(&root.join("latest.json"))?.as_deref() != Some(id) {
        return Err("登录编号已失效。".into());
    }
    let path = root.join(format!("status-{id}.json"));
    let mut s: Status = platform::read_json(&path)?.ok_or("找不到登录状态。")?;
    if s.status == "waiting" {
        if let Ok(_lease) = platform::lock(&lock_file(store), Duration::from_millis(1)) {
            // The writer may have finished while this reader was acquiring the
            // lease. Preserve its final result, including a specific rejection.
            s = platform::read_json(&path)?.ok_or("找不到登录状态。")?;
            if s.status != "waiting" {
                return Ok(s);
            }
            let committed = store.read()?.ordinary(&format!("login-{id}")).is_some();
            s.status = if committed { "complete" } else { "failed" }.into();
            s.error = (!committed).then(|| "上次登录已中断，请重试。".into());
        } else {
            s.login_url = platform::read_json(&dir.join("browser-link.json"))?;
        }
    }
    Ok(s)
}
pub fn cancel(store: &Store, id: &str) -> AppResult<()> {
    let dir = pending(store, id)?;
    let root = base(store);
    let _finish = platform::lock(&root.join("finish.lock"), Duration::from_secs(30))?;
    if platform::read_json::<String>(&root.join("latest.json"))?.as_deref() != Some(id) {
        return Err("登录编号已失效。".into());
    }
    if dir.is_dir() {
        platform::write_json(&dir.join("cancel"), &true)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn saved() -> OrdinaryRecord {
        OrdinaryRecord {
            id: format!("login-{}", Uuid::new_v4()),
            name: "A".into(),
            note: String::new(),
            plan: String::new(),
            proxy: "http://127.0.0.1:31001".into(),
            expected_ip: "203.0.113.11".into(),
            account_uuid: "account-a".into(),
            org: "org-a".into(),
            source_token_id: None,
        }
    }

    #[test]
    fn each_card_owns_its_listener_and_canonical_exit() {
        let mut db = Database::empty();
        db.ordinary_accounts.push(saved());
        assert!(check_slot_routes(&db, "http://localhost:31001", "203.0.113.22", None).is_err());
        assert!(check_slot_routes(&db, "http://[::1]:31002", "::ffff:203.0.113.11", None).is_err());
        assert!(check_slot_routes(&db, "http://127.0.0.1:31002", "203.0.113.22", None).is_ok());
        assert!(!same_ip("", "not an IP"));
    }

    #[test]
    fn personal_token_pair_requires_explicit_source_original_route_and_official_org() {
        let a = saved();
        let token = AccountRecord {
            id: "existing".into(),
            name: "Existing".into(),
            plan: "pro".into(),
            token_tail: "fake".into(),
            encrypted_token: "fixture".into(),
            org: Some(a.org.clone()),
            proxy: a.proxy.clone(),
            expected_ip: a.expected_ip.clone(),
            note: String::new(),
            added_at: String::new(),
        };
        let mut db = Database::empty();
        db.accounts.push(token);
        let own = Some("existing");
        assert!(enrollment_route(&db, &a.proxy, &a.expected_ip, None).is_err());
        assert!(enrollment_route(&db, &a.proxy, &a.expected_ip, own).is_ok());
        assert!(enrollment_route(&db, "http://127.0.0.1:31004", &a.expected_ip, own).is_err());
        assert!(enrollment_identity(&db, &a.org, Some("pro"), None).is_err());
        assert!(enrollment_identity(&db, &a.org, Some("team"), own).is_err());
        assert!(enrollment_identity(&db, "another-org", Some("max"), own).is_err());
        assert!(enrollment_identity(&db, &a.org, Some("max"), own).is_ok());
        db.ordinary_accounts.push(OrdinaryRecord {
            source_token_id: own.map(String::from),
            ..a.clone()
        });
        assert!(check_slot_routes(&db, &a.proxy, &a.expected_ip, own).is_ok());
        assert!(check_slot_routes(&db, "http://127.0.0.1:31004", "203.0.113.84", own).is_err());
        assert!(check_slot_routes(&db, &a.proxy, &a.expected_ip, None).is_err());
        db.accounts[0].org = None;
        assert!(enrollment_route(
            &Database {
                ordinary_accounts: vec![],
                ..db
            },
            &a.proxy,
            &a.expected_ip,
            own
        )
        .is_err());
    }

    #[test]
    fn native_renewal_keeps_account_and_quota_separate_from_default_login() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store {
            root: temp.path().into(),
        };
        let mut db = Database::empty();
        let a = saved();
        let dir = home(&store, &a.id).unwrap();
        fs::create_dir_all(dir.join(".claude")).unwrap();
        let file = dir.join(".claude/.credentials.json");
        let grant = |token: &str, end: i64| {
            json!({"claudeAiOauth": {
                "accessToken":token, "refreshTokenExpiresAt":end, "subscriptionType":"pro",
                "scopes":["user:profile", "user:inference"]
            }})
        };
        let p = dir.join(".claude/.claude.json");
        platform::write_json(
            &p,
            &json!({"oauthAccount":{"accountUuid":"account-a","organizationUuid":"org-a"}}),
        )
        .unwrap();
        platform::write_json(&file, &grant("fixture-a1", 80_000)).unwrap();
        let (digest, initial) = credentials::login_snapshot(&dir).unwrap();
        platform::write_json(
            &dir.join("grant.json"),
            &Seal {
                digest,
                grant: initial,
            },
        )
        .unwrap();
        db.ordinary_accounts.push(a.clone());
        store.save(&db).unwrap();
        let before = quota::Binding::selected(&store, temp.path(), Some(&a.id)).unwrap();
        platform::write_json(&file, &grant("fixture-a2", 79_300)).unwrap();
        seal_renewal(&store, &a).unwrap();
        let after = quota::Binding::selected(&store, temp.path(), Some(&a.id)).unwrap();
        assert_ne!(before.fingerprint, after.fingerprint);
        assert_eq!(before.key, after.key);
        assert_ne!(after.key, "normal:account-a:org-a");
        // A launch/inspection must advance the baseline even when no status
        // callback runs between rotations. Total rounding exceeds two seconds.
        for (index, end) in [78_400, 77_688, 76_900].into_iter().enumerate() {
            platform::write_json(&file, &grant(&format!("fixture-{index}"), end)).unwrap();
            verify_slot(&store, &a).unwrap();
        }
        platform::write_json(&file, &grant("fixture-b", 120_000)).unwrap();
        assert!(verify_slot(&store, &a).is_err());
        platform::write_json(&file, &grant("fixture-a2", 79_300)).unwrap();
        platform::write_json(
            &p,
            &json!({"oauthAccount":{"accountUuid":"account-b","organizationUuid":"org-a"}}),
        )
        .unwrap();
        assert!(verify_slot(&store, &a).is_err());
    }

    #[test]
    fn missed_renewals_require_a_new_native_identity_then_all_launches_can_resume() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store { root: temp.path().join("store") };
        fs::create_dir_all(&store.root).unwrap();
        let a = saved();
        let dir = home(&store, &a.id).unwrap();
        fs::create_dir_all(dir.join(".claude")).unwrap();
        let shared = temp.path().join("shared");
        fs::create_dir_all(shared.join(".claude")).unwrap();
        let identity = json!({"accountUuid":a.account_uuid,"organizationUuid":a.org});
        platform::write_json(&dir.join(".claude/.claude.json"), &json!({"oauthAccount":identity})).unwrap();
        let cred = dir.join(".claude/.credentials.json");
        let credentials = |end| json!({"claudeAiOauth":{"accessToken":"synthetic-only","refreshTokenExpiresAt":end,"subscriptionType":"pro"}});
        platform::write_json(&cred, &credentials(80_000)).unwrap();
        let (digest, grant) = credentials::login_snapshot(&dir).unwrap();
        platform::write_json(&dir.join("grant.json"), &Seal { digest, grant }).unwrap();
        let mut db = Database::empty();
        db.current = Some(a.id.clone());
        db.ordinary_accounts.push(a.clone());
        store.save(&db).unwrap();
        platform::write_json(&store.root.join("managed-config-v1.json"), &json!({
            "version":1,"home":shared,"selection":a.id,"generation":"test-generation"
        })).unwrap();
        platform::write_json(&cred, &credentials(77_688)).unwrap();
        let original = fs::read(&cred).unwrap();
        let profile = shared.join(".claude/.claude.json");
        let mut old = identity.clone();
        old["profileFetchedAt"] = json!(1);
        platform::write_json(&profile, &json!({"oauthAccount":old})).unwrap();
        assert!(verify_slot(&store, &a).is_err(), "enrollment/cached identity is insufficient");
        std::thread::sleep(Duration::from_millis(3));
        let mut fresh = identity;
        fresh["profileFetchedAt"] = json!(chrono::Utc::now().timestamp_millis());
        fresh["accountUuid"] = json!("different-account");
        platform::write_json(&profile, &json!({"oauthAccount":fresh})).unwrap();
        assert!(verify_slot(&store, &a).is_err());
        fresh["accountUuid"] = json!(a.account_uuid);
        fresh["profileFetchedAt"] = json!(chrono::Utc::now().timestamp_millis() - 3_000);
        platform::write_json(&profile, &json!({"oauthAccount":fresh})).unwrap();
        assert!(verify_slot(&store, &a).is_err(), "a profile from before this rotation stays stale");
        fresh["profileFetchedAt"] = json!(chrono::Utc::now().timestamp_millis() - 500);
        platform::write_json(&profile, &json!({"oauthAccount":fresh})).unwrap();
        for _ in 0..3 { verify_slot(&store, &a).unwrap(); }
        assert_eq!(original, fs::read(&cred).unwrap(), "never rewrite native credentials");
        assert_eq!(platform::read_json::<Seal>(&dir.join("grant.json")).unwrap().unwrap().grant.refresh_token_expires_at, Some(77_688));
        platform::write_json(&cred, &credentials(120_000)).unwrap();
        assert!(verify_slot(&store, &a).is_err(), "fresh profile cannot excuse a different login lifetime");
        let mut changed_plan = credentials(75_000);
        changed_plan["claudeAiOauth"]["subscriptionType"] = json!("max");
        platform::write_json(&cred, &changed_plan).unwrap();
        assert!(verify_slot(&store, &a).is_err(), "fresh profile cannot excuse an unrelated grant change");
    }
}
