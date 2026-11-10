//! Explicit CPA credential exports. Secrets stay in Rust and are only written to
//! a path chosen by the native save dialog; no network or credential refresh.
use crate::{crypto, model::AppResult, normal, platform, service::Manager};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

// CPA may refresh disabled OAuth credentials. A syntactically valid, unreachable
// proxy prevents falling back to the server's default exit before configuration.
const UNCONFIGURED_PROXY: &str = "http://configure-cpa-proxy.invalid:1";

#[derive(Serialize)]
pub struct ExportResult {
    pub path: String,
}

// This type must never be a command return value or implement Debug.
#[derive(Serialize)]
struct CpaCredential<'a> {
    #[serde(rename = "type")]
    provider: &'static str,
    proxy_url: &'static str,
    access_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expired: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_uuid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    organization_uuid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_setup_token: Option<bool>,
    disabled: bool,
}

fn encode(value: &CpaCredential<'_>) -> AppResult<Zeroizing<Vec<u8>>> {
    // Token lengths are bounded below. Reserve once so serialization does not
    // leave old allocations holding copies of either token.
    let mut bytes = Zeroizing::new(Vec::with_capacity(64 * 1024));
    serde_json::to_writer_pretty(&mut *bytes, value).map_err(|_| "无法生成 CPA 认证文件。")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn secret<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Zeroizing<String>>, D::Error> {
    Option::<String>::deserialize(d).map(|v| v.map(Zeroizing::new))
}

#[derive(Deserialize)]
struct CredentialFile {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OAuth>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuth {
    #[serde(default, deserialize_with = "secret")]
    access_token: Option<Zeroizing<String>>,
    #[serde(default, deserialize_with = "secret")]
    refresh_token: Option<Zeroizing<String>>,
    expires_at: Option<i64>,
    #[serde(default)]
    scopes: Vec<String>,
}

fn valid_secret(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 8192
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._~+/=".contains(&c))
}

fn same_identity(a: &normal::Identity, b: &normal::Identity) -> bool {
    a.cache_key() == b.cache_key() && a.email == b.email && a.account_uuid == b.account_uuid
}

fn oauth_document(
    home: &Path,
    selected: &normal::Identity,
    native: bool,
) -> AppResult<Zeroizing<Vec<u8>>> {
    let bytes = crate::credentials::read(home, native)?;
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let oauth = serde_json::from_slice::<CredentialFile>(body)
        .ok()
        .and_then(|v| v.oauth)
        .ok_or("普通登录凭据格式不正确，未导出。")?;
    let at = oauth
        .access_token
        .as_deref()
        .map(String::as_str)
        .filter(|v| valid_secret(v))
        .ok_or("普通登录缺少有效的 access token，未导出。")?;
    let rt = oauth
        .refresh_token
        .as_deref()
        .map(String::as_str)
        .filter(|v| valid_secret(v))
        .ok_or("普通登录缺少有效的 refresh token，未导出。")?;
    let expiry = oauth
        .expires_at
        .filter(|v| *v > 0)
        .and_then(|v| {
            if v >= 1_000_000_000_000 {
                chrono::DateTime::from_timestamp_millis(v)
            } else {
                chrono::DateTime::from_timestamp(v, 0)
            }
        })
        .ok_or("普通登录缺少有效的到期时间，未导出。")?
        .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
    if oauth.scopes.len() > 64
        || oauth
            .scopes
            .iter()
            .any(|s| s.len() > 128 || s.chars().any(char::is_whitespace))
    {
        return Err("普通登录权限字段格式不正确，未导出。".into());
    }
    let scope = oauth.scopes.join(" ");
    let result = encode(&CpaCredential {
        provider: "claude",
        proxy_url: UNCONFIGURED_PROXY,
        access_token: at,
        refresh_token: Some(rt),
        expired: Some(&expiry),
        email: selected.email.as_deref(),
        account_uuid: selected.account_uuid.as_deref(),
        organization_uuid: selected.org.as_deref(),
        scope: (!scope.is_empty()).then_some(scope.as_str()),
        is_setup_token: None,
        disabled: true,
    })?;
    // Claude Code owns token rotation. Reject a changing snapshot rather than
    // exporting mismatched identity/token files or writing anything back.
    if !same_identity(selected, &normal::identity(home)?)
        || *crate::credentials::read(home, native)? != *bytes
    {
        return Err("普通登录正在更新，请稍后重新导出。".into());
    }
    // An expired AT is intentionally preserved: CPA can use the exported RT.
    Ok(result)
}

fn file_name(label: &str, setup: bool) -> String {
    let safe: String = label
        .chars()
        .take(96)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim().trim_matches('.');
    format!(
        "claude-{}-{}.json",
        if setup { "setup" } else { "oauth" },
        if safe.is_empty() { "account" } else { safe }
    )
}

fn folded(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .trim_end_matches('/')
        .to_lowercase()
}
fn within(path: &Path, root: &Path) -> bool {
    let root = folded(root);
    let path = folded(path);
    path == root || path.starts_with(&(root + "/"))
}
fn destination(path: &Path, home: &Path, data_root: &Path) -> AppResult<PathBuf> {
    let path = platform::checked_path(path).map_err(|_| "保存路径不可用，或经过目录联接。")?;
    // Normalize protected roots too: macOS may spell the same temp directory
    // as /var/... or /private/var/.... Neither spelling may bypass protection.
    let home = platform::checked_path(home)?;
    let data_root = platform::checked_path(data_root)?;
    let json = path
        .extension()
        .is_some_and(|v| v.eq_ignore_ascii_case("json"));
    if !json
        || path
            .file_name()
            .is_none_or(|v| v.to_string_lossy().contains(':'))
    {
        return Err("请保存为 .json 文件。".into());
    }
    if within(&path, &home.join(".claude"))
        || folded(&path) == folded(&home.join(".claude.json"))
        || (within(&path, &data_root) && !within(&path, &data_root.join("cpa-exports")))
    {
        return Err("不能覆盖 Claude 登录文件或换号台的数据文件，请选择其他位置。".into());
    }
    if path
        .parent()
        .is_some_and(|p| folded(p).contains("onedrive"))
        || std::env::vars_os().any(|(key, value)| {
            key.to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("onedrive")
                && !value.is_empty()
                && within(&path, Path::new(&value))
        })
    {
        return Err("导出文件包含完整凭据，请选择 OneDrive 以外的位置。".into());
    }
    #[cfg(target_os = "macos")]
    if path.parent().is_some_and(|p| {
        let parent = folded(p);
        parent.contains("/library/cloudstorage/") || parent.contains("/library/mobile documents/")
    }) {
        return Err("导出文件包含完整凭据，请选择云同步目录以外的位置。".into());
    }
    if !path.parent().is_some_and(Path::is_dir) || path.is_dir() {
        return Err("保存目录不存在或文件路径不可用。".into());
    }
    Ok(path)
}

fn write_export(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = path.parent().ok_or("保存路径不正确。")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法创建导出文件。")?;
    platform::private_file(temp.path())?;
    temp.write_all(bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "无法写入导出文件，原文件未覆盖。")?;
    temp.persist(path)
        .map_err(|_| "无法保存导出文件，原文件未覆盖。")?;
    Ok(())
}

impl Manager {
    pub fn export_cpa_with_dialog(
        &self,
        id: Option<&str>,
        choose: impl FnOnce(&Path, &str) -> Option<PathBuf>,
    ) -> AppResult<Option<ExportResult>> {
        let home = self.normal_home()?;
        let selected_identity = normal::identity(&home)?;
        // Read metadata only before the dialog, and never hold the shared account
        // lock while waiting for the user. A cancelled export does not read tokens.
        let selected_account = if let Some(id) = id {
            let _lock = self.store.lock()?;
            Some(self.store.read()?.account(id)?.clone())
        } else {
            if selected_identity.cache_key().is_none() {
                return Err("未找到普通登录账号，请先在 Claude Code 中登录。".into());
            }
            None
        };
        let name = file_name(
            selected_account
                .as_ref()
                .map(|v| v.name.as_str())
                .or(selected_identity.email.as_deref())
                .unwrap_or("account"),
            id.is_some(),
        );
        let directory = self.store.root.join("cpa-exports");
        platform::private_directory(&directory)?;
        let Some(path) = choose(&directory, &name) else {
            return Ok(None);
        };
        let path = destination(&path, &home, &self.store.root)?;
        let bytes = if let Some(selected) = selected_account {
            let _lock = self.store.lock()?;
            let database = self.store.read()?;
            let account = database.account(&selected.id)?;
            if account.encrypted_token != selected.encrypted_token
                || account.org != selected.org
                || account.name != selected.name
            {
                return Err("账号资料已变化，请重新导出。".into());
            }
            let token = crypto::token(&account.encrypted_token)?;
            let token = crypto::validate_token(&token)?;
            let identity = normal::identity(&home)?;
            let email = if account.org.is_some() && account.org == identity.org {
                identity.email.as_deref()
            } else if account.name.contains('@') && !account.name.chars().any(char::is_whitespace) {
                Some(account.name.as_str())
            } else {
                None
            };
            encode(&CpaCredential {
                provider: "claude",
                proxy_url: UNCONFIGURED_PROXY,
                access_token: &token,
                refresh_token: None,
                expired: None,
                email,
                account_uuid: None,
                organization_uuid: account.org.as_deref(),
                scope: None,
                is_setup_token: Some(true),
                disabled: true,
            })?
        } else {
            if !same_identity(&selected_identity, &normal::identity(&home)?) {
                return Err("普通登录账号已变化，请重新导出。".into());
            }
            oauth_document(&home, &selected_identity, self.integrate)?
        };
        // The destination may have changed while the dialog or credential read ran.
        let path = destination(&path, &home, &self.store.root)?;
        write_export(&path, &bytes)?;
        Ok(Some(ExportResult {
            path: path.display().to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountRecord, Database, DEFAULT_PROXY};
    use serde_json::{json, Value};
    use std::fs;

    const AT: &str = "sk-ant-oat01-TEST_ONLY_export_000000";
    const RT: &str = "sk-ant-ort01-TEST_ONLY_export_000000";

    fn fixture() -> (tempfile::TempDir, Manager) {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::isolated(dir.path().to_path_buf());
        let home = manager.normal_home().unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude.json"),
            serde_json::to_vec(&json!({
                "oauthAccount": {
                    "emailAddress": "fixture@example.test", "accountUuid": "account-fixture",
                    "organizationUuid": "org-fixture", "organizationType": "claude_pro"
                },
                "unrelated": "must-not-export"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            home.join(".claude/.credentials.json"),
            serde_json::to_vec(&json!({
                "claudeAiOauth": {
                    "accessToken": AT, "refreshToken": RT, "expiresAt": 1735689600000_i64,
                    "scopes": ["user:inference", "user:profile"],
                    "subscriptionType": "pro", "unknownSecret": "must-not-export"
                }, "otherCredential": "must-not-export"
            }))
            .unwrap(),
        )
        .unwrap();
        let mut db = Database::empty();
        db.accounts.push(AccountRecord {
            id: "fixture-id".into(),
            name: "my account".into(),
            plan: "pro".into(),
            encrypted_token: crypto::seal(AT.as_bytes()).unwrap(),
            token_tail: "000000".into(),
            org: Some("org-fixture".into()),
            proxy: DEFAULT_PROXY.into(),
            expected_ip: "203.0.113.7".into(),
            note: "must-not-export".into(),
            added_at: "2026-10-31T00:00:00Z".into(),
        });
        manager.store.save(&db).unwrap();
        (dir, manager)
    }

    #[test]
    fn oauth_export_is_cpa_shaped_and_preserves_login_and_account_files() {
        let (_dir, manager) = fixture();
        let home = manager.normal_home().unwrap();
        let protected = [
            home.join(".claude.json"),
            home.join(".claude/.credentials.json"),
            manager.store.root.join("accounts.json"),
        ];
        let before: Vec<_> = protected
            .iter()
            .map(|p| {
                (
                    fs::read(p).unwrap(),
                    fs::metadata(p).unwrap().modified().unwrap(),
                )
            })
            .collect();
        let result = manager
            .export_cpa_with_dialog(None, |directory, name| {
                assert_eq!(name, "claude-oauth-fixture@example.test.json");
                Some(directory.join(name))
            })
            .unwrap()
            .unwrap();
        let bytes = fs::read(&result.path).unwrap();
        assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["type"], "claude");
        assert_eq!(v["access_token"], AT);
        assert_eq!(v["refresh_token"], RT);
        // The source AT expired in 2025. Export preserves its actual expiry and
        // does not invent a new lifetime or invoke any refresh operation.
        assert_eq!(v["expired"], "2025-01-01T00:00:00Z");
        assert_eq!(v["scope"], "user:inference user:profile");
        assert_eq!(v["account_uuid"], "account-fixture");
        assert_eq!(v["organization_uuid"], "org-fixture");
        assert_eq!(v["email"], "fixture@example.test");
        assert_eq!(v["disabled"], true);
        assert_eq!(v["proxy_url"], UNCONFIGURED_PROXY);
        for key in [
            "is_setup_token",
            "claudeAiOauth",
            "unknownSecret",
            "otherCredential",
            "last_refresh",
        ] {
            assert!(v.get(key).is_none(), "Unexpected field: {key}");
        }
        let dto = serde_json::to_value(&result).unwrap();
        assert_eq!(dto.as_object().unwrap().len(), 1);
        assert!(dto.get("path").is_some());
        for (p, original) in protected.iter().zip(before) {
            assert_eq!(fs::read(p).unwrap(), original.0);
            assert_eq!(fs::metadata(p).unwrap().modified().unwrap(), original.1);
        }
    }

    #[test]
    fn setup_export_decrypts_only_selected_token_and_omits_refresh_and_local_proxy() {
        let (_dir, manager) = fixture();
        let result = manager
            .export_cpa_with_dialog(Some("fixture-id"), |d, n| Some(d.join(n)))
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_slice(&fs::read(&result.path).unwrap()).unwrap();
        assert_eq!(v["type"], "claude");
        assert_eq!(v["access_token"], AT);
        assert_eq!(v["is_setup_token"], true);
        assert_eq!(v["disabled"], true);
        assert_eq!(v["proxy_url"], UNCONFIGURED_PROXY);
        assert_eq!(v["email"], "fixture@example.test");
        for key in [
            "refresh_token",
            "expired",
            "account_uuid",
            "claude_device_ids",
            "note",
        ] {
            assert!(v.get(key).is_none(), "Unexpected field: {key}");
        }
    }

    #[test]
    fn cancelled_export_reads_no_tokens_and_writes_no_export() {
        let (_dir, manager) = fixture();
        fs::remove_file(
            manager
                .normal_home()
                .unwrap()
                .join(".claude/.credentials.json"),
        )
        .unwrap();
        assert!(manager
            .export_cpa_with_dialog(None, |_, _| None)
            .unwrap()
            .is_none());
        let mut db = manager.store.read().unwrap();
        db.accounts[0].encrypted_token = "invalid-encrypted-fixture".into();
        manager.store.save(&db).unwrap();
        assert!(manager
            .export_cpa_with_dialog(Some("fixture-id"), |_, _| None)
            .unwrap()
            .is_none());
        assert_eq!(
            fs::read_dir(manager.store.root.join("cpa-exports"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn export_rejects_identity_or_token_change_during_save_dialog() {
        let (_dir, manager) = fixture();
        let error = manager.export_cpa_with_dialog(None, |d, n| {
            fs::write(manager.normal_home().unwrap().join(".claude.json"), br#"{"oauthAccount":{"emailAddress":"other@example.test","organizationUuid":"different-org"}}"#).unwrap();
            Some(d.join(n))
        }).err().unwrap();
        assert!(error.contains("账号已变化"));
        let error = manager
            .export_cpa_with_dialog(Some("fixture-id"), |d, n| {
                let mut db = manager.store.read().unwrap();
                db.accounts[0].encrypted_token = crypto::seal(b"different-fixture").unwrap();
                manager.store.save(&db).unwrap();
                Some(d.join(n))
            })
            .err()
            .unwrap();
        assert!(error.contains("账号资料已变化"));
        assert_eq!(
            fs::read_dir(manager.store.root.join("cpa-exports"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn export_rejects_missing_refresh_token_without_creating_output() {
        let (_dir, manager) = fixture();
        fs::write(manager.normal_home().unwrap().join(".claude/.credentials.json"), br#"{"claudeAiOauth":{"accessToken":"invalid-fixture-must-not-leak","expiresAt":1735689600}}"#).unwrap();
        let error = manager
            .export_cpa_with_dialog(None, |d, n| Some(d.join(n)))
            .err()
            .unwrap();
        assert!(error.contains("refresh token"));
        assert!(!error.contains("invalid-fixture"));
        assert_eq!(
            fs::read_dir(manager.store.root.join("cpa-exports"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn export_blocks_source_files_state_files_sync_folders_and_unsafe_names() {
        let (_dir, manager) = fixture();
        let home = manager.normal_home().unwrap();
        for p in [
            home.join(".claude/.credentials.json"),
            home.join(".claude.json"),
            manager.store.root.join("accounts.json"),
        ] {
            let before = fs::read(&p).unwrap();
            assert!(manager
                .export_cpa_with_dialog(None, |_, _| Some(p.clone()))
                .is_err());
            assert_eq!(fs::read(&p).unwrap(), before);
        }
        assert!(destination(
            &home.join("OneDrive/export.json"),
            &home,
            &manager.store.root
        )
        .is_err());
        assert!(destination(&home.join("export.txt"), &home, &manager.store.root).is_err());
        assert_eq!(
            file_name("../bad\\name:<test>\n", true),
            "claude-setup-_bad_name__test__.json"
        );
    }
}
