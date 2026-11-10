//! macOS-specific integration plus portable logic tested on Windows.
//! The encryption key belongs to this app, never to Claude Code.
#[cfg(all(target_os = "macos", not(test)))]
use crate::store::Store;
use crate::{
    model::AppResult,
    platform,
    registry::{EnvValue, Snapshot},
};
use aes_gcm::{
    aead::{rand_core::RngCore, AeadInPlace, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::path::Path;
#[cfg(target_os = "macos")]
use std::time::Duration;
use zeroize::Zeroizing;

const PREFIX: &str = "mac-keychain-aes256gcm-v1:";
const AAD: &[u8] = b"ClaudeAccountManager/mac-keychain-aes256gcm/v1";

fn encrypt(key: &[u8], bytes: &[u8]) -> AppResult<String> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("加密数据过大。".into());
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "钥匙串密钥格式不正确。")?;
    let mut nonce = [0u8; 12];
    OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| "无法生成安全随机数。")?;
    // Reserve space for the authentication tag before copying the secret.
    let mut body = Zeroizing::new(Vec::with_capacity(bytes.len() + 16));
    body.extend_from_slice(bytes);
    cipher
        .encrypt_in_place(Nonce::from_slice(&nonce), AAD, &mut *body)
        .map_err(|_| "凭据加密失败。")?;
    let mut envelope = nonce.to_vec();
    envelope.extend_from_slice(&body);
    Ok(format!("{PREFIX}{}", STANDARD.encode(envelope)))
}

fn decrypt(key: &[u8], value: &str) -> AppResult<Zeroizing<Vec<u8>>> {
    let raw = value
        .strip_prefix(PREFIX)
        .ok_or("此凭据不属于本机 Mac；请在 Mac 重新添加 setup token。")?;
    if raw.len() > 24 * 1024 * 1024 {
        return Err("凭据加密数据过大。".into());
    }
    let mut body = Zeroizing::new(STANDARD.decode(raw).map_err(|_| "凭据加密格式不正确。")?);
    if body.len() < 28 {
        return Err("凭据加密格式不正确。".into());
    }
    let nonce: [u8; 12] = body[..12].try_into().map_err(|_| "凭据加密格式不正确。")?;
    body.drain(..12);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "钥匙串密钥格式不正确。")?;
    cipher
        .decrypt_in_place(Nonce::from_slice(&nonce), AAD, &mut *body)
        .map_err(|_| "凭据无法解密，请使用保存令牌时的 Mac 用户和钥匙串。")?;
    Ok(body)
}

#[cfg(all(target_os = "macos", not(test)))]
fn master_key(create: bool) -> AppResult<Zeroizing<Vec<u8>>> {
    use security_framework::passwords::{generic_password, set_generic_password, PasswordOptions};
    const SERVICE: &str = "local.claude-account-manager.master-key.v1";
    const ACCOUNT: &str = "credential-encryption";
    let store = Store::discover()?;
    // Separate from manager.lock: serializes first creation across app/helper.
    let _guard = platform::lock(&store.root.join("keychain.lock"), Duration::from_secs(30))?;
    let read = || generic_password(PasswordOptions::new_generic_password(SERVICE, ACCOUNT));
    let bytes = match read() {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(error) if error.code() == -25300 && create => {
            let mut key = Zeroizing::new(vec![0u8; 32]);
            OsRng
                .try_fill_bytes(&mut key)
                .map_err(|_| "无法生成安全随机数。")?;
            set_generic_password(SERVICE, ACCOUNT, &key)
                .map_err(|_| "无法保存换号台密钥，请解锁登录钥匙串并允许访问。")?;
            let saved = Zeroizing::new(read().map_err(|_| "无法验证钥匙串密钥。")?);
            if *saved != *key {
                return Err("钥匙串密钥验证失败，未保存令牌。".into());
            }
            key
        }
        Err(error) if error.code() == -25300 => {
            return Err("换号台密钥不在本机钥匙串中，请重新添加 setup token。".into())
        }
        Err(_) => return Err("无法读取换号台密钥，请解锁登录钥匙串并允许访问。".into()),
    };
    if bytes.len() != 32 {
        return Err("换号台钥匙串密钥格式不正确。".into());
    }
    Ok(bytes)
}

// Unit tests never read or write the user's actual Keychain.
#[cfg(all(target_os = "macos", test))]
fn master_key(_: bool) -> AppResult<Zeroizing<Vec<u8>>> {
    Ok(Zeroizing::new(vec![0x5a; 32]))
}
#[cfg(target_os = "macos")]
pub fn seal(bytes: &[u8]) -> AppResult<String> {
    encrypt(&master_key(true)?, bytes)
}
#[cfg(target_os = "macos")]
pub fn unseal(value: &str) -> AppResult<Zeroizing<Vec<u8>>> {
    decrypt(&master_key(false)?, value)
}

#[cfg(target_os = "macos")]
pub fn claude_credentials() -> AppResult<Option<Zeroizing<Vec<u8>>>> {
    use security_framework::passwords::{generic_password, PasswordOptions};
    let mut command = std::process::Command::new("/usr/bin/id");
    command.arg("-un");
    let result = platform::capture(command, Duration::from_secs(5));
    let username = match result {
        Ok(r) if r.success => {
            String::from_utf8(r.stdout).map_err(|_| "无法确定本机钥匙串用户。")?
        }
        _ => return Err("无法确定本机钥匙串用户。".into()),
    };
    // Query exactly the default CLI entry and OS user; never enumerate secrets.
    match generic_password(PasswordOptions::new_generic_password(
        "Claude Code-credentials",
        username.trim(),
    )) {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            if bytes.is_empty() || bytes.len() > 1024 * 1024 {
                return Err("普通登录钥匙串凭据格式不正确。".into());
            }
            Ok(Some(bytes))
        }
        Err(error) if error.code() == -25300 => Ok(None),
        Err(_) => Err("无法只读访问 Claude Code 钥匙串，请解锁并允许本程序访问后重试。".into()),
    }
}

/// Private, encrypted launch snapshot replaces HKCU\Environment on macOS.
/// It is never installed in launchctl, shell rc files or a plaintext env file.
pub struct LaunchEnvironment {
    pub root: std::path::PathBuf,
}
impl LaunchEnvironment {
    fn load(&self) -> AppResult<Snapshot> {
        let value: Option<String> =
            platform::read_json(&self.root.join("launch-environment.json"))?;
        match value {
            Some(v) => serde_json::from_slice(&crate::crypto::unseal(&v)?)
                .map_err(|_| "Mac 启动配置损坏，已停止。".into()),
            None => Ok(Snapshot::new()),
        }
    }
}
impl crate::registry::Environment for LaunchEnvironment {
    fn read(&self, name: &str) -> AppResult<Option<EnvValue>> {
        if !crate::model::ENV_KEYS.contains(&name) {
            return Ok(None);
        }
        Ok(self.load()?.remove(name).flatten())
    }
    fn write(&self, name: &str, value: Option<&EnvValue>) -> AppResult<()> {
        if !crate::model::ENV_KEYS.contains(&name) {
            return Err("不允许修改此启动配置。".into());
        }
        let mut values = self.load()?;
        values.insert(name.into(), value.cloned());
        platform::write_json(
            &self.root.join("launch-environment.json"),
            &crate::registry::seal_json(&values)?,
        )
    }
}

pub const START: &str = "# >>> ClaudeAccountManager macOS v1 >>>";
pub const END: &str = "# <<< ClaudeAccountManager macOS v1 <<<";

pub fn shell_quote(value: &str) -> AppResult<String> {
    if value.contains(['\n', '\r', '\0']) {
        return Err("终端路径含不支持的字符。".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
pub fn without_block(text: &str) -> AppResult<String> {
    let starts = text.match_indices(START).collect::<Vec<_>>();
    let ends = text.match_indices(END).collect::<Vec<_>>();
    if starts.is_empty() && ends.is_empty() {
        return Ok(text.into());
    }
    if starts.len() != 1 || ends.len() != 1 || starts[0].0 > ends[0].0 {
        return Err("终端配置标记不完整，请先检查配置文件。".into());
    }
    let from = starts[0].0;
    let to = ends[0].0 + END.len();
    if (from != 0 && text.as_bytes()[from - 1] != b'\n')
        || (!text[to..].is_empty() && !text[to..].starts_with(['\r', '\n']))
    {
        return Err("终端配置标记格式不正确，未覆盖。".into());
    }
    let tail = text[to..]
        .strip_prefix("\r\n")
        .or_else(|| text[to..].strip_prefix('\n'))
        .unwrap_or(&text[to..]);
    Ok(format!("{}{}", &text[..from], tail))
}
pub fn profile_text(original: &str, script: &Path) -> AppResult<String> {
    let mut text = without_block(original)?;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!(
        "{START}\n. {}\n{END}\n",
        shell_quote(&script.to_string_lossy())?
    ));
    Ok(text)
}
pub fn function_text(helper: &Path) -> AppResult<String> {
    Ok(format!(
        "{START}\nfunction claude {{\n    {} \"$@\"\n}}\n{END}\n",
        shell_quote(&helper.to_string_lossy())?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Environment;
    #[cfg(target_os = "macos")]
    #[test]
    fn actual_function_switches_in_same_zsh_and_bash_and_preserves_args() {
        use std::{fs, os::unix::fs::PermissionsExt, process::Command};
        let dir = tempfile::tempdir().unwrap();
        let helper_dir = dir.path().join("space and 'quote");
        fs::create_dir(&helper_dir).unwrap();
        let helper = helper_dir.join("cam-launcher-fixture");
        fs::write(
            &helper,
            "#!/bin/sh\nprintf '%s\\n' \"$(cat \"$CAM_FIXTURE/selection\")\" \"$@\"\nexit 23\n",
        )
        .unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            dir.path().join("terminal.sh"),
            function_text(&helper).unwrap(),
        )
        .unwrap();
        fs::write(
            dir.path().join("check.sh"),
            r#"
set -eu
source "$CAM_FIXTURE/terminal.sh"
export CLAUDE_CODE_OAUTH_TOKEN=fixture-parent-unchanged
printf '%s' account-a > "$CAM_FIXTURE/selection"
fixture_exit=0
first="$(claude --resume 'session with spaces' 'quote"and$literal')" || fixture_exit=$?
[ "$fixture_exit" = 23 ]
printf '%s' account-b > "$CAM_FIXTURE/selection"
fixture_exit=0
second="$(claude --resume 'session with spaces' 'quote"and$literal')" || fixture_exit=$?
[ "$fixture_exit" = 23 ]
[ "$CLAUDE_CODE_OAUTH_TOKEN" = fixture-parent-unchanged ]
[ "$first" = "$(printf '%s\n' account-a --resume 'session with spaces' 'quote"and$literal')" ]
[ "$second" = "$(printf '%s\n' account-b --resume 'session with spaces' 'quote"and$literal')" ]
"#,
        )
        .unwrap();
        for shell in ["/bin/bash", "/bin/zsh"] {
            let output = Command::new(shell)
                .env_clear()
                .env("HOME", dir.path())
                .env("ZDOTDIR", dir.path())
                .env("PATH", "/usr/bin:/bin")
                .env("CAM_FIXTURE", dir.path())
                .arg("-f")
                .arg(dir.path().join("check.sh"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    #[test]
    fn encrypted_envelope_authenticates_key_nonce_and_ciphertext() {
        let key = [42u8; 32];
        let first = encrypt(&key, b"fixture-secret").unwrap();
        let second = encrypt(&key, b"fixture-secret").unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("fixture-secret"));
        assert_eq!(decrypt(&key, &first).unwrap().as_slice(), b"fixture-secret");
        assert!(decrypt(&[41; 32], &first).is_err());
        let mut raw = STANDARD
            .decode(first.strip_prefix(PREFIX).unwrap())
            .unwrap();
        raw[0] ^= 1;
        assert!(decrypt(&key, &format!("{PREFIX}{}", STANDARD.encode(raw))).is_err());
        assert!(decrypt(&key, "dpapi-v1:abc").is_err());
        assert!(decrypt(&key, &format!("{PREFIX}AA==")).is_err());
    }
    #[test]
    fn shell_integration_is_idempotent_and_preserves_other_config() {
        let original = "# user comment\nexport KEEP_ME='yes'\n";
        let once = profile_text(original, Path::new("/Users/a b's/Library/test.sh")).unwrap();
        assert_eq!(
            once,
            profile_text(&once, Path::new("/Users/a b's/Library/test.sh")).unwrap()
        );
        assert_eq!(without_block(&once).unwrap(), original);
        assert!(once.contains("a b'\\''s"));
        assert!(without_block(START).is_err());
        assert!(without_block(&format!("{START}\n{START}\n{END}\n")).is_err());
        assert!(shell_quote("line\nbreak").is_err());
        let function = function_text(Path::new("/Users/test/bin/cam-launcher")).unwrap();
        assert!(function.contains("\"$@\""));
        assert!(!function.contains("export "));
        assert!(!function.contains("eval "));
    }
    #[test]
    fn launch_snapshot_is_encrypted_and_independent_of_shell_environment() {
        let temp = tempfile::tempdir().unwrap();
        let environment = LaunchEnvironment {
            root: temp.path().into(),
        };
        let key = "CLAUDE_CODE_OAUTH_TOKEN";
        let value = EnvValue::text("test-fixture-secret");
        environment.write(key, Some(&value)).unwrap();
        assert!(environment.read(key).unwrap() == Some(value));
        let on_disk = std::fs::read_to_string(temp.path().join("launch-environment.json")).unwrap();
        assert!(!on_disk.contains("test-fixture-secret"));
        assert!(environment.write("PATH", None).is_err());
        environment.write(key, None).unwrap();
        assert!(environment.read(key).unwrap().is_none());
    }
}
