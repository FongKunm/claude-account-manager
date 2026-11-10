use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub type AppResult<T> = Result<T, String>;
pub const DEFAULT_PROXY: &str = "http://127.0.0.1:7897";
pub const ENV_KEYS: [&str; 3] = ["CLAUDE_CODE_OAUTH_TOKEN", "HTTPS_PROXY", "HTTP_PROXY"];
// v3 selects a per-process launch snapshot. Older helpers must refuse to open
// it rather than falling back to their global-environment switching behavior.
pub const DATABASE_SCHEMA: u32 = 3;
// Older launchers must refuse to open a store containing private OAuth logins.
pub const ORDINARY_SCHEMA: u32 = 4;
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// No Debug on types that can carry a token or an encrypted credential.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Database {
    #[serde(default = "schema")]
    pub schema: u32,
    #[serde(default)]
    pub revision: u64,
    pub current: Option<String>,
    #[serde(default)]
    pub accounts: Vec<AccountRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ordinary_accounts: Vec<OrdinaryRecord>,
    pub owned_env: Option<String>,
    #[serde(default)]
    pub managed_hashes: BTreeMap<String, BTreeSet<String>>,
}
fn schema() -> u32 {
    2
}
impl Database {
    pub fn empty() -> Self {
        Self {
            schema: DATABASE_SCHEMA,
            ..Self::default()
        }
    }
    pub fn account(&self, id: &str) -> AppResult<&AccountRecord> {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| "找不到这个账号。".into())
    }
    pub fn ordinary(&self, id: &str) -> Option<&OrdinaryRecord> {
        self.ordinary_accounts.iter().find(|a| a.id == id)
    }
    pub fn route(&self, id: &str) -> AppResult<(&str, &str)> {
        if let Some(a) = self.ordinary(id) {
            return Ok((&a.proxy, &a.expected_ip));
        }
        let a = self.account(id)?;
        Ok((&a.proxy, &a.expected_ip))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrdinaryRecord {
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub plan: String,
    pub id: String,
    pub name: String,
    pub proxy: String,
    pub expected_ip: String,
    pub account_uuid: String,
    pub org: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_token_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrdinaryView {
    pub proxy: String,
    pub note: String,
    pub source_token_id: Option<String>,
    pub last_network_check: Option<NetworkCheck>,
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub normal: NormalLogin,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub plan: String,
    pub token_tail: String,
    pub encrypted_token: String,
    pub org: Option<String>,
    pub proxy: String,
    pub expected_ip: String,
    pub note: String,
    pub added_at: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountInput {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub plan: String,
    pub token: Option<String>,
    pub proxy: String,
    pub expected_ip: String,
    #[serde(default)]
    pub note: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NetworkCheck {
    pub ok: bool,
    pub checked_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    pub key: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
    pub status: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageCheck {
    pub ok: bool,
    pub checked_at: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buckets: Option<Vec<UsageBucket>>,
    #[serde(default)]
    pub overage: Option<Overage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Overage {
    pub status: Option<String>,
    pub disabled_reason: Option<String>,
}
impl UsageCheck {
    pub fn error(state: &str, error: &str) -> Self {
        Self {
            ok: false,
            checked_at: now(),
            state: state.into(),
            error: Some(error.into()),
            buckets: None,
            overage: None,
            source: None,
        }
    }
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Checks {
    pub last_network_check: Option<NetworkCheck>,
    pub last_usage_check: Option<UsageCheck>,
}
pub type CheckStore = BTreeMap<String, Checks>;
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub plan: String,
    pub token_tail: String,
    pub org: Option<String>,
    pub proxy: String,
    pub expected_ip: String,
    pub note: String,
    pub added_at: String,
    pub current: bool,
    pub last_network_check: Option<NetworkCheck>,
    pub last_usage_check: Option<UsageCheck>,
}
impl Account {
    pub fn view(a: &AccountRecord, current: bool, checks: &Checks) -> Self {
        Self {
            id: a.id.clone(),
            name: a.name.clone(),
            plan: a.plan.clone(),
            token_tail: a.token_tail.clone(),
            org: a.org.clone(),
            proxy: a.proxy.clone(),
            expected_ip: a.expected_ip.clone(),
            note: a.note.clone(),
            added_at: a.added_at.clone(),
            current,
            last_network_check: checks.last_network_check.clone(),
            last_usage_check: checks.last_usage_check.clone(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalLogin {
    pub email: Option<String>,
    pub org: Option<String>,
    pub current: bool,
    pub plan: String,
    pub subscription_status: Option<String>,
    pub billing_type: Option<String>,
    pub expected_ip: String,
    pub last_usage_check: Option<UsageCheck>,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NormalRecord {
    #[serde(default)]
    pub expected_ip: String,
    pub profile: Option<NormalProfile>,
    pub last_usage_check: Option<UsageCheck>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalProfile {
    pub plan: String,
    pub subscription_status: Option<String>,
    pub checked_at: String,
}
pub type NormalStore = BTreeMap<String, NormalRecord>;
#[derive(Serialize)]
pub struct DetectedExit {
    pub ip: String,
}
#[derive(Serialize, Clone)]
pub struct Conflict {
    pub source: String,
    pub key: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub version: String,
    pub data_root: String,
    pub default_proxy: String,
    pub accounts: Vec<Account>,
    pub ordinary_accounts: Vec<OrdinaryView>,
    pub normal: NormalLogin,
    pub conflicts: Vec<Conflict>,
}
#[derive(Serialize)]
pub struct AddResult {
    pub account: Account,
    pub replaced: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub fell_back_to_normal: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchResult {
    pub switched_at: String,
}
