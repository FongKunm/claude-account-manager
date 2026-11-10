//! Read-only ordinary-login identity, credentials, and OAuth response parsing.
//! Credential types deliberately implement neither Debug nor Serialize.
use crate::model::*;
#[cfg(not(windows))]
use crate::platform;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::path::Path;
use zeroize::Zeroizing;

pub const EXPIRED: &str = "登录令牌刚过期，在 Claude Code 里发一句话会自动续期，之后再查";
#[derive(Default)]
pub struct Identity {
    pub email: Option<String>,
    pub org: Option<String>,
    pub account_uuid: Option<String>,
    pub plan: String,
    pub billing_type: Option<String>,
}
impl Identity {
    pub fn cache_key(&self) -> Option<String> {
        self.org
            .as_ref()
            .map(|v| format!("org:{v}"))
            .or_else(|| self.email.as_ref().map(|v| format!("email:{v}")))
    }
}
/// Caller holds the store lock. Persist the first verified exit before an OAuth
/// process can send a request; later launches must compare against this binding.
pub(crate) fn remember_exit(
    store: &crate::store::Store,
    home: &Path,
    ip: Option<&str>,
) -> AppResult<()> {
    let ip = ip
        .filter(|s| !s.is_empty())
        .ok_or("未能核对普通登录出口。")?;
    let identity = identity(home)?;
    #[cfg(windows)]
    {
        let db = store.read()?;
        if !db.ordinary_accounts.is_empty() {
            let proxy = crate::client::normal_proxy(home)?;
            for a in &db.ordinary_accounts {
                let same_login = identity.account_uuid.as_ref() == Some(&a.account_uuid)
                    && identity.org.as_ref() == Some(&a.org);
                let same_ip = crate::ordinary::same_ip(ip, &a.expected_ip);
                if same_login != same_ip
                    || (!same_login && crate::ordinary::same_port(&proxy, &a.proxy))
                {
                    return Err(format!(
                        "本机普通登录的身份或出口与「{}」的绑定冲突。",
                        a.name
                    ));
                }
            }
        }
    }
    let Some(key) = identity.cache_key() else {
        return Ok(());
    };
    let mut all = store.normal_checks()?;
    let record = all.entry(key).or_default();
    if record.expected_ip.is_empty() {
        record.expected_ip = ip.into();
        store.save_normal_checks(&all)?;
    } else if record.expected_ip != ip {
        return Err("普通登录出口与绑定记录不同，未启动 Claude。".into());
    }
    Ok(())
}
fn plan(max: bool, pro: bool, tiers: &[&str]) -> String {
    if max {
        if tiers
            .iter()
            .any(|v| v.to_ascii_lowercase().contains("max_20x"))
        {
            "max20"
        } else if tiers
            .iter()
            .any(|v| v.to_ascii_lowercase().contains("max_5x"))
        {
            "max5"
        } else {
            "max"
        }
    } else if pro {
        "pro"
    } else {
        ""
    }
    .into()
}
pub fn identity(home: &Path) -> AppResult<Identity> {
    #[cfg(windows)]
    let file = crate::account_context::normal_document(home)?;
    #[cfg(not(windows))]
    let file = platform::read_json(&home.join(".claude.json"))?.unwrap_or(serde_json::json!({}));
    identity_document(file)
}
pub(crate) fn identity_document(file: Value) -> AppResult<Identity> {
    #[derive(Deserialize)]
    struct File {
        #[serde(rename = "oauthAccount")]
        account: Option<Account>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Account {
        email_address: Option<String>,
        organization_uuid: Option<String>,
        account_uuid: Option<String>,
        organization_type: Option<String>,
        organization_rate_limit_tier: Option<String>,
        user_rate_limit_tier: Option<String>,
        billing_type: Option<String>,
    }
    let file: File = serde_json::from_value(file).map_err(|_| "普通登录资料格式无效。")?;
    let Some(a) = file.account else {
        return Ok(Identity::default());
    };
    let tiers = [
        a.organization_rate_limit_tier.as_deref().unwrap_or(""),
        a.user_rate_limit_tier.as_deref().unwrap_or(""),
    ];
    let kind = a
        .organization_type
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let max = kind.contains("claude_max")
        || tiers
            .iter()
            .any(|v| v.to_ascii_lowercase().contains("max_"));
    Ok(Identity {
        email: a.email_address.filter(|v| !v.is_empty() && v.len() <= 320),
        org: a
            .organization_uuid
            .filter(|v| !v.is_empty() && v.len() <= 128),
        account_uuid: a.account_uuid.filter(|v| !v.is_empty() && v.len() <= 128),
        plan: plan(max, kind.contains("claude_pro"), &tiers),
        billing_type: a.billing_type.filter(|v| v.len() <= 128),
    })
}
pub struct AccessToken {
    pub value: Zeroizing<String>,
    pub expires_at_ms: i64,
}
impl AccessToken {
    pub fn fresh_at(&self, now_ms: i64) -> bool {
        self.expires_at_ms.saturating_sub(now_ms) > 60_000
    }
}
fn secret<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Zeroizing<String>>, D::Error> {
    Option::<String>::deserialize(d).map(|s| s.map(Zeroizing::new))
}
fn parse_credentials(bytes: &[u8], now_ms: i64) -> Result<AccessToken, Box<UsageCheck>> {
    #[derive(Deserialize)]
    struct File {
        #[serde(rename = "claudeAiOauth")]
        oauth: Option<OAuth>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OAuth {
        #[serde(default, deserialize_with = "secret")]
        access_token: Option<Zeroizing<String>>,
        expires_at: Option<i64>,
    }
    let invalid = || {
        UsageCheck::error(
            "invalid",
            "未找到有效的普通登录凭据，请先在 Claude Code 中登录",
        )
    };
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let oauth = serde_json::from_slice::<File>(body)
        .ok()
        .and_then(|f| f.oauth)
        .ok_or_else(invalid)?;
    let expires = oauth
        .expires_at
        .ok_or_else(|| UsageCheck::error("expired", EXPIRED))?;
    // Claude Code stores epoch milliseconds; accept older seconds-based records too.
    let expires_at_ms = if expires >= 1_000_000_000_000 {
        expires
    } else {
        expires.saturating_mul(1000)
    };
    if expires_at_ms.saturating_sub(now_ms) <= 60_000 {
        return Err(UsageCheck::error("expired", EXPIRED).into());
    }
    let value = oauth.access_token.ok_or_else(invalid)?;
    if value.is_empty()
        || value.len() > 8192
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._~+/=".contains(&c))
    {
        return Err(invalid().into());
    }
    Ok(AccessToken {
        value,
        expires_at_ms,
    })
}
pub fn credentials(home: &Path, native: bool) -> Result<AccessToken, Box<UsageCheck>> {
    let bytes = crate::credentials::read(home, native)
        .map_err(|error| UsageCheck::error("invalid", &error))?;
    parse_credentials(&bytes, chrono::Utc::now().timestamp_millis())
}
fn reset(value: Option<&Value>) -> Option<String> {
    let value = value?;
    let parsed = if let Some(text) = value.as_str() {
        chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|t| t.to_utc())
    } else {
        value.as_i64().and_then(|t| {
            if t >= 1_000_000_000_000 {
                chrono::DateTime::from_timestamp_millis(t)
            } else {
                chrono::DateTime::from_timestamp(t, 0)
            }
        })
    };
    parsed.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
}
fn bucket(key: &str, value: &Value, field: &str) -> Option<UsageBucket> {
    let percent = value.get(field)?.as_f64()?;
    if !percent.is_finite() || percent < 0.0 {
        return None;
    }
    let resets_at = reset(
        value
            .get("resets_at")
            .filter(|v| !v.is_null())
            .or_else(|| value.get("reset_at"))
            .or_else(|| value.get("reset")),
    );
    Some(UsageBucket {
        key: key.into(),
        utilization: percent.clamp(0.0, 100.0),
        resets_at,
        status: if percent >= 100.0 {
            "rejected"
        } else {
            "allowed"
        }
        .into(),
    })
}
pub fn usage(body: &str) -> AppResult<UsageCheck> {
    let value: Value = serde_json::from_str(body).map_err(|_| "普通登录额度响应格式不正确。")?;
    if ![
        "five_hour",
        "seven_day",
        "seven_day_opus",
        "seven_day_sonnet",
        "limits",
        "iguana_necktie",
    ]
    .iter()
    .any(|key| value.get(key).is_some())
    {
        return Err("未收到可识别的普通登录额度数据。".into());
    }
    let mut buckets = Vec::new();
    for (source, key) in [
        ("five_hour", "5h"),
        ("seven_day", "7d"),
        ("seven_day_opus", "7d_opus"),
        ("seven_day_sonnet", "7d_sonnet"),
    ] {
        if let Some(b) = value
            .get(source)
            .and_then(|v| bucket(key, v, "utilization"))
        {
            buckets.push(b);
        }
    }
    let candidates: Vec<_> = value
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|v| v.get("kind").and_then(Value::as_str) == Some("weekly_scoped"))
        .filter(|v| {
            v.pointer("/scope/model/display_name")
                .and_then(Value::as_str)
                .is_some_and(|name| {
                    ["fable", "fable 5"].contains(&name.trim().to_ascii_lowercase().as_str())
                })
        })
        .filter_map(|v| {
            bucket("7d_fable", v, "percent").map(|b| {
                (
                    v.get("is_active").and_then(Value::as_bool).unwrap_or(false),
                    b,
                )
            })
        })
        .collect();
    let fable = candidates
        .iter()
        .find(|(active, _)| *active)
        .or_else(|| candidates.first())
        .map(|(_, b)| b.clone())
        .or_else(|| {
            value.get("iguana_necktie").and_then(|v| {
                bucket("7d_fable", v, "utilization").or_else(|| bucket("7d_fable", v, "percent"))
            })
        });
    if let Some(b) = fable {
        buckets.push(b);
    }
    let overage = value
        .get("extra_usage")
        .filter(|v| v.is_object())
        .and_then(|v| {
            v.get("is_enabled")
                .and_then(Value::as_bool)
                .map(|enabled| Overage {
                    status: Some(if enabled { "allowed" } else { "rejected" }.into()),
                    disabled_reason: v
                        .get("disabled_reason")
                        .and_then(Value::as_str)
                        .filter(|s| s.len() <= 128)
                        .map(str::to_owned),
                })
        });
    let limited = buckets
        .iter()
        .any(|b| ["5h", "7d"].contains(&b.key.as_str()) && b.utilization >= 100.0);
    Ok(UsageCheck {
        ok: true,
        checked_at: now(),
        state: if limited { "limited" } else { "ok" }.into(),
        error: None,
        buckets: Some(buckets),
        overage,
        source: None,
    })
}
pub fn profile(body: &str) -> AppResult<NormalProfile> {
    let v: Value = serde_json::from_str(body).map_err(|_| "普通登录套餐响应格式不正确。")?;
    let field = |key: &str| {
        v.get(key)
            .or_else(|| v.get("organization").and_then(|o| o.get(key)))
            .or_else(|| v.get("account").and_then(|o| o.get(key)))
    };
    let max = field("has_claude_max").and_then(Value::as_bool);
    let pro = field("has_claude_pro").and_then(Value::as_bool);
    let tier = field("rate_limit_tier")
        .and_then(Value::as_str)
        .unwrap_or("");
    let kind = field("organization_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if max.is_none()
        && pro.is_none()
        && tier.is_empty()
        && kind.is_empty()
        && !v.get("organization").is_some_and(Value::is_object)
    {
        return Err("普通登录套餐响应格式不正确。".into());
    }
    Ok(NormalProfile {
        plan: plan(
            max.unwrap_or_else(|| {
                kind.contains("claude_max") || tier.to_ascii_lowercase().contains("max_")
            }),
            pro.unwrap_or_else(|| kind.contains("claude_pro")),
            &[tier],
        ),
        subscription_status: v
            .pointer("/organization/subscription_status")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 128)
            .map(str::to_owned),
        checked_at: now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    #[test]
    fn percentages_fable_priority_nulls_overage_and_past_reset() {
        let data = json!({
            "five_hour":{"utilization":43.2,"resets_at":"2020-01-01T00:00:00Z"},
            "seven_day":{"utilization":7,"resets_at":null},"seven_day_opus":null,
            "seven_day_sonnet":{"utilization":100,"resets_at":null},
            "limits":[
                {"kind":"weekly_scoped","scope":{"model":{"display_name":"fable"}},"is_active":false,"percent":91},
                {"kind":"weekly_scoped","scope":{"model":{"display_name":"FABLE 5"}},"is_active":true,"percent":24,"resets_at":"2030-01-01T00:00:00+00:00"}],
            "iguana_necktie":{"utilization":80},
            "extra_usage":{"is_enabled":false,"disabled_reason":"org_level_disabled"}});
        let usage = usage(&data.to_string()).unwrap();
        let buckets = usage.buckets.unwrap();
        assert_eq!(usage.state, "ok");
        assert_eq!(buckets[0].utilization, 43.2);
        assert!(buckets[0].resets_at.as_ref().unwrap().starts_with("2020"));
        assert_eq!(buckets.last().unwrap().key, "7d_fable");
        assert_eq!(buckets.last().unwrap().utilization, 24.0);
        assert!(!buckets.iter().any(|b| b.key == "7d_opus"));
        assert_eq!(
            usage.overage.unwrap().disabled_reason.as_deref(),
            Some("org_level_disabled")
        );
    }
    #[test]
    fn fable_legacy_fallback_and_core_limit() {
        let data = json!({"five_hour":{"utilization":100},"iguana_necktie":{"utilization":61}});
        let result = usage(&data.to_string()).unwrap();
        assert_eq!(result.state, "limited");
        assert_eq!(result.buckets.unwrap()[1].utilization, 61.0);
        assert!(usage("{}").is_err());
        assert!(usage(r#"{"five_hour":null,"seven_day":null}"#)
            .unwrap()
            .buckets
            .unwrap()
            .is_empty());
    }
    #[test]
    fn credentials_expiry_boundary_and_no_header_injection() {
        let now = 1_800_000_000_000i64;
        for remaining in [-1, 0, 59_999, 60_000] {
            let bytes=json!({"claudeAiOauth":{"accessToken":"fixture","expiresAt":now+remaining,"refreshToken":"never-copy"}}).to_string();
            let result = parse_credentials(bytes.as_bytes(), now).err().unwrap();
            assert_eq!(result.state, "expired");
            assert_eq!(result.error.as_deref(), Some(EXPIRED));
        }
        let bytes=json!({"claudeAiOauth":{"accessToken":"fixture","expiresAt":now+60_001,"refreshToken":"never-copy"}}).to_string();
        let token = parse_credentials(bytes.as_bytes(), now).ok().unwrap();
        assert_eq!(token.value.as_str(), "fixture");
        let bad=json!({"claudeAiOauth":{"accessToken":"fixture\r\nInjected: yes","expiresAt":now+90_000}}).to_string();
        assert!(parse_credentials(bad.as_bytes(), now).is_err());
    }
    #[test]
    fn local_identity_and_profile_plan_canceled_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".claude.json"),
            json!({"oauthAccount":{
            "emailAddress":"fixture@example.test","organizationUuid":"org-fixture",
            "organizationType":"claude_max","organizationRateLimitTier":"default_claude_max_20x",
            "billingType":"apple_subscription","ignoredSecret":"do-not-copy"}})
            .to_string(),
        )
        .unwrap();
        let id = identity(dir.path()).unwrap();
        assert_eq!(id.plan, "max20");
        assert_eq!(id.billing_type.as_deref(), Some("apple_subscription"));
        for (tier, expected) in [
            ("default_claude_max_20x", "max20"),
            ("default_claude_max_5x", "max5"),
            ("other", "max"),
        ] {
            let p=profile(&json!({"organization":{"has_claude_max":true,"rate_limit_tier":tier,"subscription_status":"canceled"}}).to_string()).unwrap();
            assert_eq!(p.plan, expected);
            assert_eq!(p.subscription_status.as_deref(), Some("canceled"));
        }
        assert_eq!(
            profile(r#"{"has_claude_pro":true,"organization":{"subscription_status":"canceled"}}"#)
                .unwrap()
                .plan,
            "pro"
        );
    }
}
