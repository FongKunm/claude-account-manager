//! Keep the native default data directory, separate only its global identity file.
//! Windows Claude 2.1.318: unset CONFIG_DIR uses HOME/.claude.json; setting it to
//! exactly HOME/.claude uses HOME/.claude/.claude.json. Ue() stays unchanged.
use crate::{
    credentials::{login_snapshot, LoginGrant},
    crypto,
    model::AppResult,
    platform,
    quota::Binding,
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const PREFS: &str = "installMethod autoUpdates autoUpdatesProtectedForNative theme verbose preferredNotifChannel shiftEnterKeyBindingInstalled editorMode hasUsedBackslashReturn autoCompactEnabled autoScrollEnabled showTurnDuration externalEditorContext showMessageTimestamps diffTool tipsHistory todoFeatureEnabled showExpandedTodos briefTranscript diffSidebarOpen messageIdleNotifThresholdMs autoConnectIde autoInstallIdeExtension fileCheckpointingEnabled terminalProgressBarEnabled showStatusInTerminalTab taskCompleteNotifEnabled inputNeededNotifEnabled agentPushNotifEnabled respectGitignore claudeInChromeDefaultEnabled hasCompletedClaudeInChromeOnboarding lspRecommendationDisabled lspRecommendationNeverPlugins lspRecommendationIgnoredCount copyFullResponse copyOnSelect leftArrowOpensAgents defaultToAgentsView prStatusFooterEnabled remoteControlAtStartup autoUploadSessions autoAddRemoteControlDaemonWorker remoteDialogSeen workflowSizeGuideline mcpServers hasCompletedOnboarding lastOnboardingVersion firstStartTime firstStartVersion numStartups lastReleaseNotesSeen migrationVersion opusProMigrationComplete sonnet1m45MigrationComplete officialMarketplaceAutoInstallAttempted officialMarketplaceAutoInstalled userID machineID cachedGrowthBookFeatures cachedGrowthBookFeaturesAt hasSeenTasksHint hasSeenAutoDefaultNotice hasSeenAutoModeEntryWarning hasSeenAutoModeOutsideReadPrompt bypassPermissionsModeAccepted";
const PROJECT_PREFS: &str = "allowedTools hasTrustDialogAccepted mcpServers enabledMcpjsonServers disabledMcpjsonServers mcpContextUris hasClaudeMdExternalIncludesApproved hasClaudeMdExternalIncludesWarningShown";
const ACCOUNT_KEYS: &str = "oauthAccount additionalModelOptionsCache additionalModelOptionsAnsweredAt additionalModelCostsCache modelAccessCache orgModelDefaultCache autoCompactWindowsCache clientDataCacheSlots clientDataCache cachedUsageUtilization passesEligibilityCache cachedExtraUsageDisabledReason hasAvailableSubscription subscriptionNoticeCount groveConfigCache groveChoiceSeenAt apiCreditLinkStatusCache promoStartupStatusCache cachedArtifactRoster githubWebConnectionStatusCache claudeAiMcpEverConnected replBridgePlaceholders bridgeOauthDeadExpiresAt bridgeOauthDeadFailCount claudeCodeFirstTokenDate hasRemoteEnvironment cachedExperimentData cachedExperimentFeatures startupPrefetchedAt";

#[derive(Clone, Serialize, Deserialize)]
struct Context {
    version: u32,
    home: PathBuf,
    selection: String,
    generation: String,
    #[serde(default)]
    normal: Option<NormalIdentity>,
    #[serde(default)]
    outer_account: Option<Value>,
    #[serde(default)]
    prepared_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    awaiting_grant: Option<AwaitingGrant>,
}
#[derive(Clone, Serialize, Deserialize)]
struct AwaitingGrant {
    digest: String,
    first_seen_ms: i64,
}
#[derive(Clone, Serialize, Deserialize)]
struct NormalIdentity {
    account: Value,
    credential_fingerprint: String,
    #[serde(default)]
    grant: Option<LoginGrant>,
}
type Snapshot = (String, LoginGrant);
impl NormalIdentity {
    fn new(account: Value, observed: &Snapshot) -> Self {
        Self {
            account,
            credential_fingerprint: observed.0.clone(),
            grant: Some(observed.1.clone()),
        }
    }
    fn permits(&self, observed: &Snapshot) -> bool {
        self.credential_fingerprint == observed.0
            || self.grant.as_ref().is_some_and(|g| g.permits(&observed.1))
    }
}
/// All ordinary-login consumers use this source: UI, exit binding, native probe
/// and passive attribution. It never writes the external global file.
pub(crate) fn normal_document(home: &Path) -> AppResult<Value> {
    let store = Store::discover()?;
    normal_document_at(&store, home)
}
fn normal_document_at(store: &Store, home: &Path) -> AppResult<Value> {
    if let Some(c) = platform::read_json::<Context>(&marker(store))? {
        if c.version == 1 && c.home == home {
            let Some(observed) = login_snapshot(home) else {
                return Ok(serde_json::json!({}));
            };
            let outer: Value =
                platform::read_json(&home.join(".claude.json"))?.unwrap_or(serde_json::json!({}));
            let inner: Value = platform::read_json(&home.join(".claude/.claude.json"))?
                .unwrap_or(serde_json::json!({}));
            return Ok(verified_account(&c, &outer, &inner, Some(&observed))
                .map(|a| serde_json::json!({"oauthAccount":a}))
                .unwrap_or(serde_json::json!({})));
        }
    }
    Ok(platform::read_json(&home.join(".claude.json"))?.unwrap_or(serde_json::json!({})))
}
/// A sibling restore can clear the managed profile between our two route checks.
/// Only accept that state while the same native profile is being repopulated.
pub(crate) fn awaiting_default_profile(store: &Store, home: &Path) -> AppResult<bool> {
    let Some(c) = platform::read_json::<Context>(&marker(store))? else {
        return Ok(false);
    };
    let outer: Option<Value> = platform::read_json(&home.join(".claude.json"))?;
    let inner: Option<Value> = platform::read_json(&home.join(".claude/.claude.json"))?;
    Ok(c.version == 1
        && c.home == home
        && c.selection == "normal"
        && c.normal.is_none()
        && c.prepared_at.is_some()
        && inner.is_some_and(|v| v.get("oauthAccount").is_none())
        && same_account(
            c.outer_account.as_ref(),
            outer.as_ref().and_then(|v| v.get("oauthAccount")),
        ))
}
fn same_account(a: Option<&Value>, b: Option<&Value>) -> bool {
    ["accountUuid", "organizationUuid"]
        .iter()
        .all(|k| a.and_then(|v| v.get(k)) == b.and_then(|v| v.get(k)))
}
fn normal_account(c: &Context, outer: &Value, inner: &Value) -> Option<Value> {
    if !same_account(c.outer_account.as_ref(), outer.get("oauthAccount")) {
        return None;
    }
    if c.selection == "normal" {
        let actual = inner.get("oauthAccount").filter(|a| {
            a.get("accountUuid")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        })?;
        if let Some(n) = &c.normal {
            // Native renewal changes tokens, not either profile's account/org.
            if !same_account(Some(&n.account), Some(actual)) {
                return None;
            }
        } else {
            // prepare cleared this slot while all old managed writers were
            // stopped. Native startup may refresh OAuth BEFORE fetching this
            // profile, so a launch-time token digest cannot identify it.
            let prepared = c.prepared_at?;
            if actual.get("profileFetchedAt").and_then(Value::as_i64)? < prepared {
                return None;
            }
        }
        Some(actual.clone())
    } else {
        c.normal.as_ref().map(|n| n.account.clone())
    }
}
fn marker(store: &Store) -> PathBuf {
    store.root.join("managed-config-v1.json")
}
/// Independent native identity evidence for a named login whose refresh
/// rounding has accumulated beyond the previous checkpoint. Do not trust the
/// slot's enrollment profile or an earlier profile copied during switching.
pub(crate) fn confirmed_ordinary_profile(
    store: &Store,
    account: &crate::model::OrdinaryRecord,
    slot: &Path,
) -> AppResult<bool> {
    let Some(c): Option<Context> = platform::read_json(&marker(store))? else { return Ok(false); };
    if c.version != 1 || c.selection != account.id
        || store.read()?.current.as_deref() != Some(&account.id)
    { return Ok(false); }
    let modified = std::fs::metadata(slot.join(".claude/.credentials.json"))
        .and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok());
    let Some(changed) = modified else { return Ok(false); };
    let inner: Value = platform::read_json(&c.home.join(".claude/.claude.json"))?.unwrap_or_default();
    let profile = &inner["oauthAccount"];
    Ok(crate::ordinary::matches(profile, account)
        && profile["profileFetchedAt"].as_i64().is_some_and(|at| {
            // Native 2.1.319 also fetches the profile just BEFORE saving the
            // rotated credentials (63 ms in the isolated native reproduction).
            // This bounded write-order allowance is not the cumulative token
            // deadline tolerance, and never accepts an enrollment-age profile.
            at >= changed.saturating_sub(2_000) && at <= chrono::Utc::now().timestamp_millis()
        }))
}
fn verified_account(
    c: &Context,
    outer: &Value,
    inner: &Value,
    observed: Option<&Snapshot>,
) -> Option<Value> {
    let observed = observed?;
    if c.normal.as_ref().is_some_and(|n| !n.permits(observed)) {
        return None;
    }
    normal_account(c, outer, inner)
}
fn selection(binding: &Binding) -> String {
    if let Some(id) = binding
        .account_id
        .as_deref()
        .filter(|_| binding.account_uuid.is_some())
    {
        return id.into();
    }
    if binding.account_id.is_some() {
        format!("{}:{}", binding.key, binding.fingerprint)
    } else {
        "normal".into()
    }
}
fn pick(value: &Value, keys: &str) -> Value {
    Value::Object(
        keys.split_whitespace()
            .filter_map(|k| value.get(k).map(|v| (k.into(), v.clone())))
            .collect(),
    )
}
fn seed(outer: &Value) -> Value {
    let mut result = pick(outer, PREFS);
    if let Some(projects) = outer.get("projects").and_then(Value::as_object) {
        result["projects"] = Value::Object(
            projects
                .iter()
                .map(|(k, v)| (k.clone(), pick(v, PROJECT_PREFS)))
                .collect(),
        );
    }
    result
}
fn clear_account(inner: &mut Value) -> AppResult<()> {
    let map = inner
        .as_object_mut()
        .ok_or("受管 Claude 资料文件格式无效。")?;
    for key in ACCOUNT_KEYS.split_whitespace() {
        map.remove(key);
    }
    Ok(())
}
fn credman(value: &Value) -> bool {
    value
        .pointer("/cachedGrowthBookFeatures/tengu_windows_credman")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
pub(crate) fn check_enrollment_backend(home: &Path) -> AppResult<()> {
    let outer: Value = platform::read_json(&home.join(".claude.json"))?.unwrap_or_default();
    let inner: Value =
        platform::read_json(&home.join(".claude/.claude.json"))?.unwrap_or_else(|| outer.clone());
    check_files(home, &outer, &inner)
}
fn check_files(home: &Path, outer: &Value, inner: &Value) -> AppResult<()> {
    if home.join(".claude/.config.json").exists() {
        return Err("检测到旧版 .claude/.config.json，无法分开受管账号资料；未修改文件。".into());
    }
    if credman(outer) != credman(inner) {
        return Err("受管与默认 Claude 的 Windows 凭据存储设置不同，未启动会话。".into());
    }
    if credman(inner) || std::env::var("CLAUDE_CODE_FORCE_WINDOWS_CREDMAN").is_ok_and(|v| v == "1")
    {
        return Err("新版批量切换尚未支持 Windows 凭据管理器后端，未启动会话。".into());
    }
    for value in [outer, inner] {
        if ["apiKeyHelper", "primaryApiKey"].iter().any(|k| {
            value
                .get(k)
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
        }) || value
            .get("env")
            .and_then(Value::as_object)
            .is_some_and(|v| !v.is_empty())
        {
            return Err("Claude 全局资料里有额外认证或环境配置，请先检查，未启动会话。".into());
        }
    }
    Ok(())
}
fn identity_matches(inner: &Value, binding: &Binding) -> bool {
    let account = inner.get("oauthAccount").filter(|v| !v.is_null());
    if binding.account_id.is_some() && binding.account_uuid.is_some() && account.is_none() {
        return false;
    }
    if binding.account_id.is_some() && binding.account_uuid.is_none() {
        return account.is_none();
    }
    account.is_none()
        || (account
            .and_then(|v| v.get("accountUuid"))
            .and_then(Value::as_str)
            == binding.account_uuid.as_deref()
            && account
                .and_then(|v| v.get("organizationUuid"))
                .and_then(Value::as_str)
                == binding.org.as_deref())
}

/// Caller holds the launcher/switch gate and store lock. The callback verifies
/// that no old native process/daemon can write the file while its identity resets.
fn prepare_with(
    store: &Store,
    binding: &Binding,
    quiescent: impl FnOnce() -> AppResult<()>,
) -> AppResult<String> {
    let _lease = platform::lock(
        &marker(store).with_extension("lock"),
        Duration::from_secs(10),
    )?;
    let home = &binding.home;
    let file = platform::checked_path(&home.join(".claude/.claude.json"))?;
    let outer: Value =
        platform::read_json(&home.join(".claude.json"))?.unwrap_or(Value::Object(Map::new()));
    let mut context: Option<Context> = platform::read_json(&marker(store))?;
    let existing: Option<Value> = platform::read_json(&file)?;
    if existing.is_some() && context.is_none() {
        return Err(".claude/.claude.json 已存在且不属于换号台，未覆盖；请先检查该文件。".into());
    }
    if existing.is_none() && context.is_some() {
        return Err("受管 Claude 资料文件已丢失，请先检查，未自动重建。".into());
    }
    if context
        .as_ref()
        .is_some_and(|c| c.version != 1 || c.home != *home)
    {
        return Err("受管账号资料标记不匹配，未修改文件。".into());
    }
    let mut inner = existing.unwrap_or_else(|| seed(&outer));
    check_files(home, &outer, &inner)?;
    let target = selection(binding);
    let observed = login_snapshot(home);
    let reuse = context
        .as_mut()
        .filter(|c| c.selection == target)
        .filter(|c| {
            binding.account_id.is_some()
                || verified_account(c, &outer, &inner, observed.as_ref()).is_some()
                || (c.normal.is_none()
                    && c.prepared_at.is_some()
                    && same_account(c.outer_account.as_ref(), outer.get("oauthAccount"))
                    && inner.get("oauthAccount").is_none())
        });
    if let Some(c) = reuse {
        if !identity_matches(&inner, binding)
            && (binding.account_id.is_some() || binding.account_uuid.is_some())
        {
            return Err("受管资料中的登录账号与当前选择不一致，请先退出受管会话再切换。".into());
        }
        if binding.account_id.is_none() && c.normal.is_none() {
            if let (Some(account), Some(snap)) =
                (normal_account(c, &outer, &inner), observed.as_ref())
            {
                c.normal = Some(NormalIdentity::new(account, snap));
                platform::write_json(&marker(store), c)?;
            }
        }
        return Ok(c.generation.clone());
    }
    quiescent()?;
    // Preserve only the last native ordinary profile and its credential digest
    // before clearing the shared managed identity for a setup-token account.
    let mut normal = None;
    if context.is_none() {
        if let Some(account) = outer.get("oauthAccount").filter(|v| !v.is_null()) {
            normal = observed
                .as_ref()
                .map(|snap| NormalIdentity::new(account.clone(), snap));
        }
    } else if let Some(c) = &context {
        if let Some(snap) = observed.as_ref() {
            normal = verified_account(c, &outer, &inner, Some(snap))
                .map(|account| NormalIdentity::new(account, snap));
        }
    }
    let prepared_at = chrono::Utc::now().timestamp_millis();
    clear_account(&mut inner)?;
    if let Some(id) = binding
        .account_id
        .as_deref()
        .filter(|_| binding.account_uuid.is_some())
    {
        let db = store.read()?;
        let a = db.ordinary(id).ok_or("找不到普通账号。")?;
        crate::ordinary::verify_slot(store, a)?;
        inner["oauthAccount"] = crate::ordinary::profile(&crate::ordinary::home(store, id)?)?;
    }
    platform::write_json(&file, &inner)?;
    let generation = uuid::Uuid::new_v4().to_string();
    platform::write_json(
        &marker(store),
        &Context {
            version: 1,
            home: home.clone(),
            selection: target,
            generation: generation.clone(),
            normal: if binding.account_id.is_none() {
                None
            } else {
                normal
            },
            outer_account: outer.get("oauthAccount").cloned(),
            prepared_at: binding.account_id.is_none().then_some(prepared_at),
            awaiting_grant: None,
        },
    )?;
    Ok(generation)
}
pub(crate) fn prepare(store: &Store, binding: &Binding, cli: &Path) -> AppResult<String> {
    prepare_with(store, binding, || {
        for row in crate::sessions::live_rows(&binding.home)? {
            if crate::sessions::is_cli_session(&row, cli) {
                return Err(format!("账号或普通登录凭据已变化，但 PID {} 的旧会话还在运行。请在全部旧窗口 /exit 后重进；换号台也可批量切到另一个号。",row.pid));
            }
        }
        let mut command = Command::new(cli);
        command.args(["daemon", "status"]);
        let result = platform::capture(command, Duration::from_secs(10))?;
        // Native `daemon status` returns exit code 1 for a stopped daemon.
        if !String::from_utf8_lossy(&result.stdout).starts_with("not running") {
            return Err("已有 Claude 后台服务尚未停止，请先通过换号台批量退出再切换。".into());
        }
        Ok(())
    })
}
pub(crate) fn current(store: &Store, binding: &Binding, generation: &str) -> bool {
    check_current(store, binding, generation, false)
}
/// A delayed observation may be accepted only against the latest digest
/// vouched for by a verified native process in this account generation.
pub(crate) fn passive_current(store: &Store, binding: &Binding, generation: &str) -> bool {
    check_current(store, binding, generation, true)
}
fn check_current(store: &Store, binding: &Binding, generation: &str, sealed: bool) -> bool {
    let Ok(Some(c)) = platform::read_json::<Context>(&marker(store)) else {
        return false;
    };
    if c.version != 1
        || c.home != binding.home
        || c.generation != generation
        || c.selection != selection(binding)
    {
        return false;
    }
    let Ok(Some(inner)) = platform::read_json::<Value>(&binding.home.join(".claude/.claude.json"))
    else {
        return false;
    };
    if !identity_matches(&inner, binding) {
        return false;
    }
    if binding.account_id.is_some() {
        if let Some(id) = binding
            .account_id
            .as_deref()
            .filter(|_| binding.account_uuid.is_some())
        {
            return store.read().is_ok_and(|db| {
                db.current.as_deref() == Some(id)
                    && db.ordinary(id).is_some_and(|a| {
                        crate::ordinary::matches(&inner["oauthAccount"], a)
                            && crate::ordinary::verify_slot(store, a).is_ok()
                    })
            });
        }
        return true;
    }
    let Ok(outer) = platform::read_json::<Value>(&binding.home.join(".claude.json")) else {
        return false;
    };
    let Some(snapshot) = login_snapshot(&binding.home) else {
        return false;
    };
    if sealed
        && (!c
            .normal
            .as_ref()
            .is_some_and(|n| n.credential_fingerprint == snapshot.0)
            || binding.account_uuid.as_deref().is_none_or(str::is_empty))
    {
        return false;
    }
    verified_account(
        &c,
        &outer.unwrap_or(serde_json::json!({})),
        &inner,
        Some(&snapshot),
    )
    .is_some()
}
pub(crate) fn seal_normal(store: &Store, binding: &Binding) -> AppResult<()> {
    if let Some(id) = binding
        .account_id
        .as_deref()
        .filter(|_| binding.account_uuid.is_some())
    {
        if binding
            .context_generation
            .as_deref()
            .is_some_and(|g| current(store, binding, g))
        {
            if let Some(a) = store.read()?.ordinary(id) {
                crate::ordinary::seal_renewal(store, a)?;
            }
        }
        return Ok(());
    }
    if binding.account_id.is_some() {
        return Ok(());
    }
    let Ok(_lease) = platform::lock(
        &marker(store).with_extension("lock"),
        Duration::from_millis(20),
    ) else {
        return Ok(());
    };
    let Some(mut c): Option<Context> = platform::read_json(&marker(store))? else {
        return Ok(());
    };
    if c.version != 1
        || c.home != binding.home
        || c.selection != "normal"
        || binding.context_generation.as_deref() != Some(c.generation.as_str())
    {
        return Ok(());
    }
    // Native sign-in publishes the new identity before the new credentials.
    // Capture credentials first and confirm the identity on the far side.
    let Some(snapshot) = login_snapshot(&binding.home) else {
        return Ok(());
    };
    let Some(inner): Option<Value> =
        platform::read_json(&binding.home.join(".claude/.claude.json"))?
    else {
        return Ok(());
    };
    let outer =
        platform::read_json(&binding.home.join(".claude.json"))?.unwrap_or(serde_json::json!({}));
    let Some(account) = normal_account(&c, &outer, &inner) else {
        return Ok(());
    };
    if c.normal.as_ref().is_some_and(|old| !old.permits(&snapshot)) {
        let now = chrono::Utc::now().timestamp_millis();
        if let Some(pending) = c.awaiting_grant.as_ref().filter(|p| p.digest == snapshot.0) {
            if !account
                .get("profileFetchedAt")
                .and_then(Value::as_i64)
                .is_some_and(|t| t > pending.first_seen_ms && t <= now)
            {
                return Ok(());
            }
        } else {
            c.awaiting_grant = Some(AwaitingGrant {
                digest: snapshot.0,
                first_seen_ms: now,
            });
            return platform::write_json(&marker(store), &c);
        }
    }
    if c.normal
        .as_ref()
        .is_some_and(|n| n.credential_fingerprint == snapshot.0 && n.grant.is_some())
        && c.awaiting_grant.is_none()
    {
        return Ok(());
    }
    c.awaiting_grant = None;
    c.normal = Some(NormalIdentity::new(account, &snapshot));
    platform::write_json(&marker(store), &c)
}
pub(crate) fn token_storage(store: &Store, id: &str) -> AppResult<PathBuf> {
    let path = store
        .root
        .join("token-storage")
        .join(crypto::fingerprint(id.as_bytes()));
    if !path.is_dir() {
        platform::private_directory(&path)?;
    }
    if platform::read_json::<Value>(&path.join(".credentials.json"))?
        .is_some_and(|v| v.get("claudeAiOauth").is_some_and(|v| !v.is_null()))
    {
        return Err("该令牌账号的专用存储里出现了普通登录凭据，请先检查；未启动会话。".into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (tempfile::TempDir, Store, Binding) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store {
            root: tmp.path().join("store"),
        };
        platform::private_directory(&store.root).unwrap();
        platform::private_directory(&tmp.path().join(".claude")).unwrap();
        let b = Binding {
            account_id: Some("B".into()),
            key: "token:B".into(),
            fingerprint: "fake".into(),
            account_uuid: None,
            org: None,
            home: tmp.path().into(),
            context_generation: None,
        };
        (tmp, store, b)
    }
    #[test]
    fn keeps_history_preferences_and_device_but_never_copies_account() {
        let (tmp, store, b) = fixture();
        let outer = json!({"oauthAccount":{"accountUuid":"A"},"userID":"same-device","machineID":"same-machine","mcpServers":{"local":{}},"projects":{"p":{"hasTrustDialogAccepted":true,"lastSessionId":"old","mcpServers":{}}},"cachedGrowthBookFeatures":{"tengu_windows_credman":false},"cachedUsageUtilization":{"A":99},"unknownAccountCache":"A","hasCompletedOnboarding":true});
        platform::write_json(&tmp.path().join(".claude.json"), &outer).unwrap();
        let epoch = prepare_with(&store, &b, || Ok(())).unwrap();
        let inner: Value = platform::read_json(&tmp.path().join(".claude/.claude.json"))
            .unwrap()
            .unwrap();
        assert!(inner.get("oauthAccount").is_none());
        assert!(inner.get("unknownAccountCache").is_none());
        assert_eq!(inner["userID"], "same-device");
        assert_eq!(inner["mcpServers"], outer["mcpServers"]);
        assert_eq!(inner["projects"]["p"]["hasTrustDialogAccepted"], true);
        assert!(current(&store, &b, &epoch));
        assert_eq!(
            platform::read_json::<Value>(&tmp.path().join(".claude.json"))
                .unwrap()
                .unwrap(),
            outer
        );
    }
    #[test]
    fn transition_waits_for_quiescence_then_revokes_old_generation() {
        let (_tmp, store, mut b) = fixture();
        let epoch = prepare_with(&store, &b, || Ok(())).unwrap();
        b.key = "token:C".into();
        b.account_id = Some("C".into());
        assert!(prepare_with(&store, &b, || Err("busy".into())).is_err());
        let next = prepare_with(&store, &b, || Ok(())).unwrap();
        assert_ne!(epoch, next);
        assert!(!current(&store, &b, &epoch));
        assert!(current(&store, &b, &next));
    }
    #[test]
    fn foreign_file_and_different_credential_backend_are_not_overwritten() {
        let (tmp, store, b) = fixture();
        let file = tmp.path().join(".claude/.claude.json");
        platform::write_json(&file, &json!({"userID":"not-ours"})).unwrap();
        assert!(prepare_with(&store, &b, || Ok(())).is_err());
        assert_eq!(
            platform::read_json::<Value>(&file).unwrap().unwrap()["userID"],
            "not-ours"
        );
        assert!(check_files(
            tmp.path(),
            &json!({}),
            &json!({"cachedGrowthBookFeatures":{"tengu_windows_credman":true}})
        )
        .is_err());
    }
    #[test]
    fn normal_observation_waits_for_matching_native_profile() {
        let (tmp, store, mut b) = fixture();
        b.account_id = None;
        b.account_uuid = Some("A".into());
        b.org = Some("org".into());
        b.key = "normal:A:org".into();
        platform::write_json(
            &tmp.path().join(".claude/.credentials.json"),
            &json!({"claudeAiOauth":{"accessToken":"synthetic-active"}}),
        )
        .unwrap();
        let epoch = prepare_with(&store, &b, || Ok(())).unwrap();
        assert!(!current(&store, &b, &epoch));
        assert!(awaiting_default_profile(&store, tmp.path()).unwrap());
        platform::write_json(
            &tmp.path().join(".claude/.claude.json"),
            &json!({"oauthAccount":{"accountUuid":"A","organizationUuid":"org","profileFetchedAt":chrono::Utc::now().timestamp_millis()}}),
        )
        .unwrap();
        assert!(current(&store, &b, &epoch));
        assert!(!awaiting_default_profile(&store, tmp.path()).unwrap());
        platform::write_json(
            &tmp.path().join(".claude/.claude.json"),
            &json!({"oauthAccount":{"accountUuid":"B","organizationUuid":"org","profileFetchedAt":chrono::Utc::now().timestamp_millis()}}),
        )
        .unwrap();
        assert!(!current(&store, &b, &epoch));
    }
    #[test]
    fn managed_login_identity_survives_switching_to_token_without_changing_outer() {
        let (tmp, store, mut b) = fixture();
        platform::write_json(
            &tmp.path().join(".claude.json"),
            &json!({"oauthAccount":{"accountUuid":"old-A"}}),
        )
        .unwrap();
        platform::write_json(
            &tmp.path().join(".claude/.credentials.json"),
            &json!({"claudeAiOauth":{"accessToken":"synthetic-only-old-login"}}),
        )
        .unwrap();
        b.account_id = None;
        b.key = "normal:old-A".into();
        b.account_uuid = Some("old-A".into());
        b.fingerprint = crypto::fingerprint(&crate::credentials::read(tmp.path(), true).unwrap());
        prepare_with(&store, &b, || Ok(())).unwrap();
        platform::write_json(
            &tmp.path().join(".claude/.claude.json"),
            &json!({"oauthAccount":{"accountUuid":"old-A","profileFetchedAt":chrono::Utc::now().timestamp_millis()}}),
        )
        .unwrap();
        // A native profile confirms the initial identity; then /login writes
        // another identity, not merely renewed bytes for the same account.
        prepare_with(&store, &b, || Err("must reuse".into())).unwrap();
        platform::write_json(
            &tmp.path().join(".claude/.credentials.json"),
            &json!({"claudeAiOauth":{"accessToken":"synthetic-only-new-login"}}),
        )
        .unwrap();
        platform::write_json(
            &tmp.path().join(".claude/.claude.json"),
            &json!({"oauthAccount":{"accountUuid":"new-A2","profileFetchedAt":chrono::Utc::now().timestamp_millis()}}),
        )
        .unwrap();
        assert!(normal_document_at(&store, tmp.path())
            .unwrap()
            .get("oauthAccount")
            .is_none());
        b.fingerprint = crypto::fingerprint(&crate::credentials::read(tmp.path(), true).unwrap());
        b.account_uuid = None;
        prepare_with(&store, &b, || Ok(())).unwrap();
        platform::write_json(
            &tmp.path().join(".claude/.claude.json"),
            &json!({"oauthAccount":{"accountUuid":"new-A2","profileFetchedAt":chrono::Utc::now().timestamp_millis()}}),
        )
        .unwrap();
        assert_eq!(
            normal_document_at(&store, tmp.path()).unwrap()["oauthAccount"]["accountUuid"],
            "new-A2"
        );
        b.account_id = Some("B".into());
        b.key = "token:B".into();
        b.account_uuid = None;
        prepare_with(&store, &b, || Ok(())).unwrap();
        assert_eq!(
            normal_document_at(&store, tmp.path()).unwrap()["oauthAccount"]["accountUuid"],
            "new-A2"
        );
        assert_eq!(
            platform::read_json::<Value>(&tmp.path().join(".claude.json"))
                .unwrap()
                .unwrap()["oauthAccount"]["accountUuid"],
            "old-A"
        );
        std::fs::remove_file(tmp.path().join(".claude/.credentials.json")).unwrap();
        assert!(normal_document_at(&store, tmp.path())
            .unwrap()
            .get("oauthAccount")
            .is_none());
    }
    #[test]
    fn renewal_keeps_identity_and_allows_new_window_but_external_login_requires_reset() {
        let (tmp, store, mut b) = fixture();
        let account = json!({"accountUuid":"A","organizationUuid":"org"});
        let file = tmp.path().join(".claude/.credentials.json");
        platform::write_json(
            &tmp.path().join(".claude.json"),
            &json!({"oauthAccount":account}),
        )
        .unwrap();
        platform::write_json(&file, &json!({"claudeAiOauth":{"accessToken":"fake-A1"}})).unwrap();
        b.account_id = None;
        b.account_uuid = Some("A".into());
        b.org = Some("org".into());
        b.fingerprint = crypto::fingerprint(&crate::credentials::read(tmp.path(), true).unwrap());
        let generation = prepare_with(&store, &b, || Ok(())).unwrap();
        platform::write_json(
            &tmp.path().join(".claude/.claude.json"),
            &json!({"oauthAccount":account}),
        )
        .unwrap();
        let mut c: Context = platform::read_json(&marker(&store)).unwrap().unwrap();
        c.normal = Some(NormalIdentity {
            account: account.clone(),
            credential_fingerprint: b.fingerprint.clone(),
            grant: Some(login_snapshot(tmp.path()).unwrap().1),
        });
        platform::write_json(&marker(&store), &c).unwrap();
        platform::write_json(&file,&json!({"claudeAiOauth":{"accessToken":"fake-A2","refreshToken":"fake-rotated","expiresAt":999999999}})).unwrap();
        b.fingerprint = crypto::fingerprint(&crate::credentials::read(tmp.path(), true).unwrap());
        assert_eq!(
            normal_document_at(&store, tmp.path()).unwrap()["oauthAccount"],
            account
        );
        assert_eq!(
            prepare_with(&store, &b, || Err("other windows are live".into())).unwrap(),
            generation
        );
        platform::write_json(
            &tmp.path().join(".claude.json"),
            &json!({"oauthAccount":{"accountUuid":"B","organizationUuid":"org-B"}}),
        )
        .unwrap();
        assert!(normal_document_at(&store, tmp.path())
            .unwrap()
            .get("oauthAccount")
            .is_none());
        assert!(prepare_with(&store, &b, || Err("other windows are live".into())).is_err());
    }
    #[test]
    fn startup_renewal_confirms_a_fresh_native_profile_before_binding_pending_window() {
        let (tmp, store, mut b) = fixture();
        let credential = tmp.path().join(".claude/.credentials.json");
        let inner = tmp.path().join(".claude/.claude.json");
        platform::write_json(
            &credential,
            &json!({"claudeAiOauth":{"accessToken":"fake-expired"}}),
        )
        .unwrap();
        b.account_id = None;
        b.account_uuid = None;
        b.key = "normal:pending".into();
        b.fingerprint = crypto::fingerprint(&crate::credentials::read(tmp.path(), true).unwrap());
        let launch_digest = b.fingerprint.clone();
        let generation = prepare_with(&store, &b, || Ok(())).unwrap();
        b.context_generation = Some(generation.clone());
        let c: Context = platform::read_json(&marker(&store)).unwrap().unwrap();
        let prepared = c.prepared_at.unwrap();
        platform::write_json(
            &credential,
            &json!({"claudeAiOauth":{"accessToken":"fake-renewed"}}),
        )
        .unwrap();
        platform::write_json(&inner, &json!({"oauthAccount":{"accountUuid":"A","organizationUuid":"org","profileFetchedAt":prepared-1}})).unwrap();
        assert!(normal_document_at(&store, tmp.path())
            .unwrap()
            .get("oauthAccount")
            .is_none());
        seal_normal(&store, &b).unwrap();
        assert!(platform::read_json::<Context>(&marker(&store))
            .unwrap()
            .unwrap()
            .normal
            .is_none());
        platform::write_json(&inner, &json!({"oauthAccount":{"accountUuid":"A","organizationUuid":"org","profileFetchedAt":prepared+1}})).unwrap();
        seal_normal(&store, &b).unwrap();
        assert_eq!(
            normal_document_at(&store, tmp.path()).unwrap()["oauthAccount"]["accountUuid"],
            "A"
        );
        assert!(platform::read_json::<Context>(&marker(&store))
            .unwrap()
            .unwrap()
            .normal
            .is_some());
        assert_eq!(b.fingerprint, launch_digest);
        assert!(!current(&store, &b, &generation));
        let mut next = b.clone();
        next.account_uuid = Some("A".into());
        next.org = Some("org".into());
        next.fingerprint =
            crypto::fingerprint(&crate::credentials::read(tmp.path(), true).unwrap());
        assert_ne!(next.fingerprint, b.fingerprint);
        assert_eq!(
            prepare_with(&store, &next, || Err("first window is live".into())).unwrap(),
            generation
        );
    }
    struct LiveNormal {
        _tmp: tempfile::TempDir,
        store: Store,
        ticket: Binding,
        profile: Value,
    }
    impl LiveNormal {
        fn new() -> Self {
            let (tmp, store, mut ticket) = fixture();
            ticket.account_id = None;
            ticket.account_uuid = Some("A".into());
            ticket.org = Some("same-org".into());
            ticket.key = "normal:A:same-org".into();
            let outer = json!({"oauthAccount":{"accountUuid":"A","organizationUuid":"same-org"}});
            platform::write_json(&tmp.path().join(".claude.json"), &outer).unwrap();
            let mut f = Self {
                _tmp: tmp,
                store,
                ticket,
                profile: outer,
            };
            f.rotate("old", 100_000, "pro");
            f.ticket.fingerprint = login_snapshot(&f.ticket.home).unwrap().0;
            f.ticket.context_generation =
                Some(prepare_with(&f.store, &f.ticket, || Ok(())).unwrap());
            f.profile["oauthAccount"]["profileFetchedAt"] =
                json!(chrono::Utc::now().timestamp_millis());
            f.set_inner(&f.profile);
            seal_normal(&f.store, &f.ticket).unwrap();
            f.post(11, "claude-code");
            assert_eq!(f.amount(), 11.);
            f
        }
        fn cred(&self) -> PathBuf {
            self.ticket.home.join(".claude/.credentials.json")
        }
        fn rotate(&self, token: &str, end: i64, plan: &str) {
            platform::write_json(
                &self.cred(),
                &json!({"claudeAiOauth":{
                "accessToken":token,"refreshToken":format!("synthetic-{token}"),"expiresAt":777777,
                "refreshTokenExpiresAt":end,"subscriptionType":plan,"rateLimitTier":"test",
                "scopes":["user:profile","user:inference"]}}),
            )
            .unwrap();
        }
        fn set_inner(&self, v: &Value) {
            platform::write_json(&self.ticket.home.join(".claude/.claude.json"), v).unwrap()
        }
        fn post(&self, n: u32, source: &str) {
            let buckets = crate::quota::statusline(
                &json!({"rate_limits":{"five_hour":{"used_percentage":n,"resets_at":2_000_000_000}}}),
            );
            crate::quota::observe(&self.store, &self.ticket, buckets, source).unwrap();
        }
        fn amount(&self) -> f64 {
            crate::quota::cached(&self.store, &self.ticket)
                .unwrap()
                .unwrap()
                .buckets
                .unwrap()[0]
                .utilization
        }
    }
    #[test]
    fn live_renewals_keep_one_account_cache_but_do_not_relax_manual_probe() {
        let f = LiveNormal::new();
        for (index, end) in [99_201, 98_611, 98_611].into_iter().enumerate() {
            f.rotate(&format!("renewed-{index}"), end, "pro");
            // A stale query and an unconfirmed new digest cannot write.
            f.post(99, "native-probe");
            let before = f.amount();
            f.post(88, "claude-code");
            assert_eq!(f.amount(), before);
            seal_normal(&f.store, &f.ticket).unwrap();
            f.post(22 + index as u32, "claude-code");
            assert_eq!(f.amount(), (22 + index) as f64);
            let mut current = f.ticket.clone();
            current.fingerprint = login_snapshot(&current.home).unwrap().0;
            assert_eq!(
                crate::quota::cached(&f.store, &current)
                    .unwrap()
                    .unwrap()
                    .buckets
                    .unwrap()[0]
                    .utilization,
                f.amount()
            );
            f.post(99, "native-probe");
            assert_ne!(f.amount(), 99.);
        }
    }
    #[test]
    fn inner_outer_account_org_and_generation_changes_cannot_relabel_delayed_observations() {
        let f = LiveNormal::new();
        for (inner, id, org) in [
            (true, "B", "same-org"),
            (false, "B", "same-org"),
            (true, "A", "other-org"),
        ] {
            let path = f.ticket.home.join(if inner {
                ".claude/.claude.json"
            } else {
                ".claude.json"
            });
            let original: Value = platform::read_json(&path).unwrap().unwrap();
            platform::write_json(
                &path,
                &json!({"oauthAccount":{"accountUuid":id,"organizationUuid":org}}),
            )
            .unwrap();
            // Native /login changes identity before it saves new credentials.
            f.rotate(id, 100_000, "pro");
            seal_normal(&f.store, &f.ticket).unwrap();
            f.post(77, "claude-code");
            assert_eq!(f.amount(), 11.);
            let mut other = f.ticket.clone();
            other.key = format!("normal:{id}:{org}");
            assert!(crate::quota::cached(&f.store, &other).unwrap().is_none());
            platform::write_json(&path, &original).unwrap();
        }
        let mut c: Context = platform::read_json(&marker(&f.store)).unwrap().unwrap();
        c.generation = "new-epoch".into();
        platform::write_json(&marker(&f.store), &c).unwrap();
        seal_normal(&f.store, &f.ticket).unwrap();
        f.post(88, "claude-code");
        assert_eq!(f.amount(), 11.);
    }
    #[test]
    fn copied_older_or_newer_grant_stays_quiet_until_native_reconfirms_same_account() {
        for (deadline, plan) in [(10_000, "pro"), (200_000, "pro"), (100_000, "other-plan")] {
            let f = LiveNormal::new();
            f.rotate("opaque-replacement", deadline, plan);
            seal_normal(&f.store, &f.ticket).unwrap();
            f.post(77, "claude-code");
            assert_eq!(f.amount(), 11.);
            assert!(normal_document_at(&f.store, &f.ticket.home)
                .unwrap()
                .get("oauthAccount")
                .is_none());
            // Simulate the native client independently confirming account B (same org).
            let mut profile = f.profile.clone();
            profile["oauthAccount"]["accountUuid"] = json!("B");
            f.set_inner(&profile);
            seal_normal(&f.store, &f.ticket).unwrap();
            f.post(77, "claude-code");
            assert_eq!(f.amount(), 11.);
            // A matching but old A profile cannot release the hold either.
            f.set_inner(&f.profile);
            seal_normal(&f.store, &f.ticket).unwrap();
            f.post(77, "claude-code");
            assert_eq!(f.amount(), 11.);
            let c: Context = platform::read_json(&marker(&f.store)).unwrap().unwrap();
            let t = c.awaiting_grant.unwrap().first_seen_ms;
            while chrono::Utc::now().timestamp_millis() <= t {
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut fresh = f.profile.clone();
            fresh["oauthAccount"]["profileFetchedAt"] =
                json!(chrono::Utc::now().timestamp_millis());
            f.set_inner(&fresh);
            seal_normal(&f.store, &f.ticket).unwrap();
            f.post(33, "claude-code");
            assert_eq!(f.amount(), 33.);
        }
    }
    #[test]
    fn logout_missing_token_or_unreadable_profile_cannot_publish() {
        let f = LiveNormal::new();
        platform::write_json(&f.cred(), &json!({"mcpOAuth":{"fixture":{}}})).unwrap();
        seal_normal(&f.store, &f.ticket).unwrap();
        f.post(91, "claude-code");
        assert_eq!(f.amount(), 11.);
        f.rotate("renewed", 100_000, "pro");
        std::fs::write(f.ticket.home.join(".claude.json"), b"{").unwrap();
        assert!(seal_normal(&f.store, &f.ticket).is_err());
        f.post(92, "claude-code");
        assert_eq!(f.amount(), 11.);
    }
}
