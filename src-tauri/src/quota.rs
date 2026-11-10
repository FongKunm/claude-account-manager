//! Local observations from Claude Code's documented status line / stream events.
//! Account selection is never used to attribute an incoming observation.
use crate::{crypto, model::*, normal, platform, store::Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub account_id: Option<String>,
    pub key: String,
    pub fingerprint: String,
    pub account_uuid: Option<String>,
    pub org: Option<String>,
    pub home: PathBuf,
    #[serde(default)]
    pub context_generation: Option<String>,
}
impl Binding {
    pub fn selected(store: &Store, home: &Path, id: Option<&str>) -> AppResult<Self> {
        if let Some(id) = id {
            let db = store.read()?;
            #[cfg(windows)]
            if let Some(a) = db.ordinary(id) {
                return Ok(Self {
                    account_id: Some(id.into()),
                    key: format!("ordinary:{id}:{}:{}", a.account_uuid, a.org),
                    fingerprint: crate::ordinary::verify_slot(store, a)?,
                    account_uuid: Some(a.account_uuid.clone()),
                    org: Some(a.org.clone()),
                    home: home.into(),
                    context_generation: None,
                });
            }
            let a = db.account(id)?;
            return Ok(Self {
                account_id: Some(id.into()),
                key: format!("token:{id}"),
                fingerprint: crypto::fingerprint(a.encrypted_token.as_bytes()),
                account_uuid: None,
                org: a.org.clone(),
                home: home.into(),
                context_generation: None,
            });
        }
        let identity = normal::identity(home)?;
        let uuid = identity
            .account_uuid
            .clone()
            .ok_or("未找到普通登录账号身份，请先在 Claude Code 登录。")?;
        let bytes = crate::credentials::read(home, true)?;
        Ok(Self {
            account_id: None,
            key: format!("normal:{uuid}:{}", identity.org.as_deref().unwrap_or("")),
            fingerprint: crypto::fingerprint(&bytes),
            account_uuid: Some(uuid),
            org: identity.org,
            home: home.into(),
            context_generation: None,
        })
    }
    pub fn current(&self, store: &Store) -> bool {
        #[cfg(windows)]
        if self
            .context_generation
            .as_deref()
            .is_some_and(|g| !crate::account_context::current(store, self, g))
        {
            return false;
        }
        if let Some(id) = self.account_id.as_deref() {
            return store
                .read()
                .ok()
                .and_then(|db| {
                    #[cfg(windows)]
                    if let Some(a) = db.ordinary(id) {
                        return Some(
                            self.account_uuid.as_ref() == Some(&a.account_uuid)
                                && self.org.as_ref() == Some(&a.org)
                                && crate::ordinary::verify_slot(store, a)
                                    .is_ok_and(|fp| fp == self.fingerprint),
                        );
                    }
                    db.account(id).ok().map(|a| {
                        crypto::fingerprint(a.encrypted_token.as_bytes()) == self.fingerprint
                    })
                })
                .unwrap_or(false);
        }
        crate::credentials::read(&self.home, true)
            .is_ok_and(|b| crypto::fingerprint(&b) == self.fingerprint)
            && normal::identity(&self.home)
                .is_ok_and(|i| i.account_uuid == self.account_uuid && i.org == self.org)
    }
    fn passive_current(&self, store: &Store) -> bool {
        #[cfg(windows)]
        if self.account_id.is_none() || self.account_uuid.is_some() {
            // Ordinary OAuth renewal rotates credentials, not account identity.
            // Only a managed generation can use this rule. Manual probes keep
            // current()'s exact credential check before and after the request.
            return self
                .context_generation
                .as_deref()
                .is_some_and(|g| crate::account_context::passive_current(store, self, g));
        }
        self.current(store)
    }
    fn cache_scope(&self) -> &str {
        #[cfg(windows)]
        if self.account_id.is_none() || self.account_uuid.is_some() {
            // key already includes the native account UUID and organization.
            // A renewed token must not make this account's observations vanish.
            return "normal-account-v1";
        }
        &self.fingerprint
    }
    fn file(&self, store: &Store) -> PathBuf {
        self.scoped_file(store, self.cache_scope())
    }
    fn scoped_file(&self, store: &Store, scope: &str) -> PathBuf {
        store.root.join("observed-usage").join(format!(
            "{}.json",
            crypto::fingerprint(format!("{}:{}", self.key, scope).as_bytes())
        ))
    }
    fn record(&self, store: &Store) -> AppResult<Option<Observed>> {
        let matches = |r: &Observed, scope: &str| r.key == self.key && r.fingerprint == scope;
        if let Some(r) = platform::read_json(&self.file(store))? {
            return Ok(matches(&r, self.cache_scope()).then_some(r));
        }
        // Upgrade only records carrying both this exact account/org and this
        // exact credential digest. Do not scan or guess an older owner.
        #[cfg(windows)]
        if self.account_id.is_none() {
            let legacy: Option<Observed> =
                platform::read_json(&self.scoped_file(store, &self.fingerprint))?;
            return Ok(legacy
                .filter(|r| matches(r, &self.fingerprint))
                .map(|mut r| {
                    r.fingerprint = self.cache_scope().into();
                    r
                }));
        }
        Ok(None)
    }
}
#[derive(Clone, Serialize, Deserialize, Default)]
struct Observed {
    key: String,
    fingerprint: String,
    buckets: BTreeMap<String, UsageBucket>,
    checked_at: String,
    source: String,
}

fn windows(value: &Value, percent: bool) -> Vec<UsageBucket> {
    [("five_hour", "5h"), ("seven_day", "7d")]
        .into_iter()
        .filter_map(|(input, key)| {
            let b = value.get(input)?;
            let n = b
                .get(if percent {
                    "used_percentage"
                } else {
                    "utilization"
                })?
                .as_f64()?;
            let reset = b
                .get(if percent { "resets_at" } else { "resetsAt" })?
                .as_i64()?;
            if !n.is_finite() || n < 0.0 || reset <= 0 {
                return None;
            }
            let utilization = (if percent { n } else { n * 100.0 }).clamp(0.0, 100.0);
            let time = chrono::DateTime::from_timestamp(reset, 0)?;
            Some(UsageBucket {
                key: key.into(),
                utilization,
                resets_at: Some(time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                status: b
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or(if utilization >= 100.0 {
                        "rejected"
                    } else {
                        "allowed"
                    })
                    .into(),
            })
        })
        .collect()
}
pub fn statusline(input: &Value) -> Vec<UsageBucket> {
    if input.get("remote").is_some() {
        return vec![];
    }
    input
        .get("rate_limits")
        .map(|v| windows(v, true))
        .unwrap_or_default()
}
pub fn stream_event(input: &Value) -> Vec<UsageBucket> {
    if input.get("type").and_then(Value::as_str) != Some("rate_limit_event") {
        return vec![];
    }
    input
        .pointer("/rate_limit_info/unifiedWindows")
        .map(|v| windows(v, false))
        .unwrap_or_default()
}
fn merge(record: &mut Observed, buckets: Vec<UsageBucket>) -> bool {
    let mut changed = false;
    for b in buckets {
        let accept = match record.buckets.get(&b.key) {
            None => true,
            Some(old) => {
                b.resets_at > old.resets_at
                    || (b.resets_at == old.resets_at && b.utilization > old.utilization)
            }
        };
        if accept {
            record.buckets.insert(b.key.clone(), b);
            changed = true;
        }
    }
    changed
}
pub fn observe(
    store: &Store,
    binding: &Binding,
    buckets: Vec<UsageBucket>,
    source: &str,
) -> AppResult<()> {
    let valid = || {
        if source == "claude-code" {
            binding.passive_current(store)
        } else {
            binding.current(store)
        }
    };
    if buckets.is_empty() || !valid() {
        return Ok(());
    }
    let directory = store.root.join("observed-usage");
    if !directory.is_dir() {
        platform::private_directory(&directory)?;
    }
    // Never wait on the manager's network/selection lock from a status-line child.
    let path = binding.file(store);
    let Ok(_lease) = platform::lock(&path.with_extension("lock"), Duration::from_millis(20)) else {
        return Ok(());
    };
    let mut record = binding.record(store)?.unwrap_or_else(|| Observed {
        key: binding.key.clone(),
        fingerprint: binding.cache_scope().into(),
        ..Default::default()
    });
    // A completed explicit query is a fresh observation even if usage did not
    // increase. Passive repeated status-line renders are not fresh requests.
    let explicit_fresh = source == "native-probe"
        && buckets.iter().all(|b| {
            record.buckets.get(&b.key).is_none_or(|old| {
                b.resets_at >= old.resets_at
                    && (b.resets_at != old.resets_at || b.utilization >= old.utilization)
            })
        });
    if merge(&mut record, buckets) || explicit_fresh {
        record.checked_at = now();
        record.source = source.into();
        if valid() {
            platform::write_json(&path, &record)?;
        }
    }
    Ok(())
}
pub fn cached(store: &Store, binding: &Binding) -> AppResult<Option<UsageCheck>> {
    let Some(record) = binding.record(store)? else {
        return Ok(None);
    };
    let buckets: Vec<_> = record.buckets.into_values().collect();
    let limited = buckets
        .iter()
        .any(|b| b.utilization >= 100.0 && b.resets_at.as_ref().is_some_and(|r| r > &now()));
    Ok(Some(UsageCheck {
        ok: true,
        checked_at: record.checked_at,
        state: if limited { "limited" } else { "ok" }.into(),
        error: None,
        buckets: Some(buckets),
        overage: None,
        source: Some(record.source),
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn stream_uses_both_actual_windows_not_limiting_top_level() {
        assert!(stream_event(&json!({"type":"rate_limit_event","rate_limit_info":{"utilization":0.9,"rateLimitType":"seven_day"}})).is_empty());
        let rows = stream_event(
            &json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","utilization":1,"unifiedWindows":{"five_hour":{"utilization":1.1,"resetsAt":2000000000},"seven_day":{"utilization":0.17,"resetsAt":2000500000}}}}),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].utilization, 100.0);
        assert_eq!(rows[1].utilization, 17.0);
    }
    #[test]
    fn merges_partial_out_of_order_windows_without_clearing_or_rolling_back() {
        let mut r = Observed::default();
        let a = statusline(
            &json!({"rate_limits":{"five_hour":{"used_percentage":77,"resets_at":2000000000},"seven_day":{"used_percentage":12,"resets_at":2000500000}}}),
        );
        assert!(merge(&mut r, a));
        assert!(!merge(
            &mut r,
            statusline(
                &json!({"rate_limits":{"five_hour":{"used_percentage":11,"resets_at":2000000000}}})
            )
        ));
        assert!(merge(
            &mut r,
            statusline(
                &json!({"rate_limits":{"five_hour":{"used_percentage":1,"resets_at":2000100000}}})
            )
        ));
        assert_eq!(r.buckets["5h"].utilization, 1.0);
        assert_eq!(r.buckets["7d"].utilization, 12.0);
        assert!(statusline(&json!({"remote":{},"rate_limits":{"five_hour":{"used_percentage":1,"resets_at":2000000000}}})).is_empty());
    }
}
