//! Read-only credential provider. No refresh, update, delete, or ACL modification
//! is ever performed on Claude Code's credential entry.
use crate::{model::AppResult, platform};
use std::{fs::File, io::Read, path::Path};
use zeroize::Zeroizing;

pub fn read(home: &Path, native: bool) -> AppResult<Zeroizing<Vec<u8>>> {
    #[cfg(target_os = "macos")]
    if native {
        if std::env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|v| !v.is_empty()) {
            return Err("普通登录只支持默认 Claude 配置目录，请先移除 CLAUDE_CONFIG_DIR。".into());
        }
        if let Some(bytes) = crate::macos_support::claude_credentials()? {
            return Ok(bytes);
        }
        // Fall back ONLY when the entry does not exist, never after a denied or
        // locked Keychain read (which could otherwise resurrect stale tokens).
    }
    #[cfg(not(target_os = "macos"))]
    let _ = native;
    read_file(home)
}

#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginGrant {
    pub refresh_token_expires_at: Option<i64>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub scopes: Option<Vec<String>>,
}
#[cfg(windows)]
impl LoginGrant {
    // The server may return a fixed deadline as whole remaining seconds.
    // Native refresh takes min(old, now + seconds): allow downward rounding
    // against the PREVIOUS accepted read, not an arbitrary older login.
    pub fn permits(&self, next: &Self) -> bool {
        self.permits_cumulative_rounding(next)
            && self.refresh_token_expires_at.is_none_or(|old| {
                next.refresh_token_expires_at.is_some_and(|new| old.abs_diff(new) <= 2_000)
            })
    }
    pub fn permits_cumulative_rounding(&self, next: &Self) -> bool {
        fn unchanged<T: PartialEq>(old: &Option<T>, new: &Option<T>) -> bool {
            old.as_ref().is_none_or(|v| Some(v) == new.as_ref())
        }
        self.refresh_token_expires_at.is_none_or(|old| {
            next.refresh_token_expires_at
                .is_some_and(|new| new <= old)
        }) && unchanged(&self.subscription_type, &next.subscription_type)
            && unchanged(&self.rate_limit_tier, &next.rate_limit_tier)
            && unchanged(&self.scopes, &next.scopes)
    }
}
/// Return a digest and the non-secret continuity hints from the SAME read.
/// The access token is borrowed only to reject empty/invalidated logins.
#[cfg(windows)]
pub(crate) fn login_snapshot(home: &Path) -> Option<(String, LoginGrant)> {
    #[derive(serde::Deserialize)]
    struct OAuth<'a> {
        #[serde(rename = "accessToken", borrow)]
        token: Option<&'a str>,
        #[serde(flatten)]
        grant: LoginGrant,
    }
    #[derive(serde::Deserialize)]
    struct Document<'a> {
        #[serde(rename = "claudeAiOauth", borrow)]
        oauth: Option<OAuth<'a>>,
    }
    let bytes = read(home, true).ok()?;
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let mut v = serde_json::from_slice::<Document<'_>>(body).ok()?.oauth?;
    v.token.filter(|t| !t.is_empty())?;
    if let Some(scopes) = &mut v.grant.scopes {
        scopes.sort_unstable();
        scopes.dedup();
    }
    Some((crate::crypto::fingerprint(&bytes), v.grant))
}

fn read_file(home: &Path) -> AppResult<Zeroizing<Vec<u8>>> {
    let path = platform::checked_path(&home.join(".claude/.credentials.json"))
        .map_err(|_| "无法只读打开普通登录凭据。")?;
    let file = File::open(path).map_err(|_| "未找到普通登录凭据，请先在 Claude Code 中登录。")?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(1024 * 1024 + 1));
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法只读打开普通登录凭据。")?;
    if bytes.len() > 1024 * 1024 {
        return Err("普通登录凭据文件格式不正确。".into());
    }
    Ok(bytes)
}
