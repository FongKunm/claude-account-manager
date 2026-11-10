use crate::{crypto, model::AppResult, platform, store::Store};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};
const BEGIN: &str = "# >>> ClaudeAccountManager v2 >>>";
const END: &str = "# <<< ClaudeAccountManager v2 <<<";
const SHIM_BEGIN: &str = "# >>> ClaudeAccountManager command shim v1 >>>";
const SHIM_END: &str = "# <<< ClaudeAccountManager command shim v1 <<<";

fn command_shim(helper: &Path) -> Vec<u8> {
    let quoted = helper.to_string_lossy().replace('\'', "''");
    // PowerShell discovers .ps1 ahead of .exe in the same existing PATH entry,
    // even in a shell that started before this file was installed. Explicit
    // .exe invocation in the native helper avoids any recursion or replacement.
    let text = format!("{SHIM_BEGIN}\r\n# Load the selected account for this new Claude process.\r\n& '{quoted}' @args\r\n$global:LASTEXITCODE = $LASTEXITCODE\r\n{SHIM_END}\r\n");
    encode(&text, &Encoding::Utf8(true))
}

fn edit_command_shim(store: &Store, path: &Path, helper: &Path, install: bool) -> AppResult<()> {
    platform::checked_path(path)?;
    let expected = command_shim(helper);
    let existing = match fs::read(path) {
        Ok(value) => Some(value),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("无法检查 PowerShell 的 claude 启动入口。".into()),
    };
    if let Some(old) = existing.as_ref() {
        if old != &expected {
            return Err("已存在其他 claude.ps1 启动脚本，未覆盖；请先处理该脚本。".into());
        }
        if install {
            return Ok(());
        }
    } else if !install {
        return Ok(());
    }
    let backup = zeroize::Zeroizing::new(
        serde_json::to_vec(&serde_json::json!({
            "path":path, "existed":existing.is_some(), "bytes":existing.as_deref().unwrap_or(&[])
        }))
        .map_err(|_| "无法备份启动入口。")?,
    );
    store.backup("powershell-command-shim", &backup)?;
    if install {
        write_bytes(path, &expected)
    } else {
        fs::remove_file(path).map_err(|_| "无法移除 PowerShell 启动入口。".into())
    }
}

#[cfg(windows)]
fn edit_cmd_shim(store: &Store, native: &Path, helper: &Path, install: bool) -> AppResult<()> {
    // cmd's standard PATHEXT resolves .COM before .EXE. A PE executable may use
    // either extension, so this avoids changing PATH, AutoRun, or native Claude.
    let path = native.with_extension("com");
    platform::checked_path(&path)?;
    let manifest = store.root.join("cmd-shim.json");
    let installed: Option<serde_json::Value> = platform::read_json(&manifest)?;
    let old = match fs::read(&path) {
        Ok(v) => Some(v),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("无法检查 cmd 的 Claude 启动入口。".into()),
    };
    let bytes = if install {
        fs::read(helper).map_err(|_| "缺少终端启动器。")?
    } else {
        vec![]
    };
    if let Some(old) = &old {
        let fingerprint = crypto::fingerprint(old);
        let owned = installed.as_ref().is_some_and(|m| {
            m["path"] == serde_json::json!(path) && m["fingerprint"] == fingerprint
        });
        if !owned {
            return Err("已有其他 claude.com，未覆盖或删除；请先检查该启动入口。".into());
        }
        if install && *old == bytes {
            return Ok(());
        }
        store.backup("cmd-command-shim", old)?;
    } else if !install {
        return Ok(());
    }
    if install {
        write_bytes(&path, &bytes)?;
        platform::write_json(
            &manifest,
            &serde_json::json!({"path":path,"fingerprint":crypto::fingerprint(&bytes)}),
        )
    } else {
        fs::remove_file(&path).map_err(|_| "无法移除 cmd 启动入口。".to_owned())?;
        fs::remove_file(&manifest).map_err(|_| "无法清理 cmd 启动入口记录。".into())
    }
}

fn documents() -> AppResult<PathBuf> {
    #[cfg(windows)]
    {
        // CSIDL_PERSONAL respects the user's redirected Documents location.
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn SHGetFolderPathW(
                window: *mut std::ffi::c_void,
                csidl: i32,
                token: *mut std::ffi::c_void,
                flags: u32,
                path: *mut u16,
            ) -> i32;
        }
        let mut path = [0u16; 260];
        // SAFETY: the fixed buffer meets SHGetFolderPathW's MAX_PATH requirement.
        let result = unsafe {
            SHGetFolderPathW(
                std::ptr::null_mut(),
                5,
                std::ptr::null_mut(),
                0,
                path.as_mut_ptr(),
            )
        };
        if result < 0 {
            return Err("无法定位 PowerShell 配置目录。".into());
        }
        let end = path.iter().position(|c| *c == 0).unwrap_or(path.len());
        Ok(PathBuf::from(String::from_utf16_lossy(&path[..end])))
    }
    #[cfg(not(windows))]
    Err("此版本仅支持 Windows。".into())
}
fn profile_paths() -> AppResult<Vec<PathBuf>> {
    let docs = documents()?;
    // CurrentUserAllHosts also applies to the VS Code PowerShell host.
    Ok(vec![
        docs.join("WindowsPowerShell/profile.ps1"),
        docs.join("PowerShell/profile.ps1"),
    ])
}
fn block(helper: &Path) -> String {
    let quoted = helper.to_string_lossy().replace('\'', "''");
    format!("{BEGIN}\r\n# Read the latest account for each NEW claude process. No tokens in this file.\r\nfunction global:claude {{\r\n  & '{quoted}' @args\r\n  $global:LASTEXITCODE = $LASTEXITCODE\r\n}}\r\n{END}")
}
fn without_block(text: &str) -> AppResult<String> {
    let start = text.find(BEGIN);
    let end = text.find(END);
    match (start, end) {
        (None, None) => Ok(text.into()),
        (Some(start), Some(end))
            if end > start
                && !text[end + END.len()..].contains(BEGIN)
                && !text[start + BEGIN.len()..end].contains(BEGIN) =>
        {
            Ok(format!("{}{}", &text[..start], &text[end + END.len()..]))
        }
        _ => Err("PowerShell 配置标记不完整，未修改原文件。".into()),
    }
}
enum Encoding {
    Utf8(bool),
    Utf16,
}
fn decode(bytes: &[u8]) -> AppResult<(String, Encoding)> {
    if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
        if bytes.len() % 2 != 0 {
            return Err("PowerShell 配置编码不正确。".into());
        }
        let wide: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        return Ok((
            String::from_utf16(&wide).map_err(|_| "PowerShell 配置编码不正确。")?,
            Encoding::Utf16,
        ));
    }
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let body = if bom { &bytes[3..] } else { bytes };
    let text = String::from_utf8(body.to_vec())
        .map_err(|_| "PowerShell 配置不是 UTF-8/UTF-16；请先转换编码，原文件未修改。")?;
    Ok((text, Encoding::Utf8(bom)))
}
fn encode(text: &str, encoding: &Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8(bom) => {
            let mut bytes = if *bom {
                vec![0xef, 0xbb, 0xbf]
            } else {
                Vec::new()
            };
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
        Encoding::Utf16 => [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect(),
    }
}
fn write_bytes(path: &Path, bytes: &[u8]) -> AppResult<()> {
    // Profile files may be in redirected Documents/OneDrive. No credentials go
    // there; encrypted originals are backed up in LOCALAPPDATA before changes.
    let parent = path.parent().ok_or("PowerShell 配置路径不正确。")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建 PowerShell 配置目录。")?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法保存 PowerShell 配置。")?;
    temp.write_all(bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "无法保存 PowerShell 配置。")?;
    temp.persist(path)
        .map_err(|_| "无法替换 PowerShell 配置，原文件保留。")?;
    Ok(())
}
fn edit_profiles(store: &Store, paths: &[PathBuf], helper: Option<&Path>) -> AppResult<()> {
    let mut edits = Vec::new();
    for path in paths {
        platform::checked_path(path)?;
        let old = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("无法读取 PowerShell 配置。".into()),
        };
        let (text, encoding) = decode(old.as_deref().unwrap_or(&[]))?;
        let clean = without_block(&text)?;
        let next = match helper {
            Some(helper) => format!(
                "{}\r\n{}\r\n",
                clean.trim_end_matches(['\r', '\n']),
                block(helper)
            ),
            None => clean,
        };
        let next = encode(&next, &encoding);
        if old.as_deref() == Some(next.as_slice()) || (old.is_none() && helper.is_none()) {
            continue;
        }
        let backup = serde_json::json!({"path":path,"existed":old.is_some(),
            "bytes":old.as_deref().unwrap_or(&[])});
        let backup = zeroize::Zeroizing::new(
            serde_json::to_vec(&backup).map_err(|_| "无法备份 PowerShell 配置。")?,
        );
        store.backup("powershell-profile", &backup)?;
        edits.push((path.clone(), old, next));
    }
    for (index, (path, _, next)) in edits.iter().enumerate() {
        if let Err(error) = write_bytes(path, next) {
            // Restore only files this installation just edited.
            for (path, old, _) in edits[..index].iter().rev() {
                match old {
                    Some(bytes) => {
                        let _ = write_bytes(path, bytes);
                    }
                    None => {
                        let _ = fs::remove_file(path);
                    }
                }
            }
            return Err(error);
        }
    }
    Ok(())
}
pub fn install(store: &Store) -> AppResult<()> {
    let source = env::current_exe()
        .map_err(|_| "无法定位桌面程序。")?
        .with_file_name("cam-launcher.exe");
    let bytes = fs::read(&source).map_err(|_| "缺少 cam-launcher.exe，请使用完整的程序包。")?;
    let directory = store.root.join("bin");
    if !directory.is_dir() {
        platform::private_directory(&directory)?;
    }
    let target = directory.join("cam-launcher.exe");
    platform::checked_path(&target)?;
    let existing = fs::read(&target).ok();
    if existing
        .as_ref()
        .is_none_or(|current| crypto::fingerprint(current) != crypto::fingerprint(&bytes))
    {
        write_bytes(&target, &bytes)?;
    }
    let native = crate::client::executable()?;
    #[cfg(windows)]
    edit_cmd_shim(store, &native, &target, true)?;
    let shim = native.with_extension("ps1");
    edit_command_shim(store, &shim, &target, true)?;
    edit_profiles(store, &profile_paths()?, Some(&target))
}
pub fn uninstall(store: &Store) -> AppResult<()> {
    let native = crate::client::executable()?;
    #[cfg(windows)]
    edit_cmd_shim(
        store,
        &native,
        &store.root.join("bin/cam-launcher.exe"),
        false,
    )?;
    let shim = native.with_extension("ps1");
    edit_command_shim(
        store,
        &shim,
        &store.root.join("bin/cam-launcher.exe"),
        false,
    )?;
    edit_profiles(store, &profile_paths()?, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn cmd_resolves_owned_pe_com_before_native_exe_without_path_edits() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            root: dir.path().into(),
        };
        let native = dir.path().join("cam-cmd-fixture.exe");
        let system = PathBuf::from(env::var_os("SystemRoot").unwrap()).join("System32");
        let original = fs::read(system.join("hostname.exe")).unwrap();
        fs::write(&native, &original).unwrap();
        edit_cmd_shim(&store, &native, &system.join("cmd.exe"), true).unwrap();
        let result = std::process::Command::new(system.join("cmd.exe"))
            .current_dir(dir.path())
            .args(["/d", "/c", "cam-cmd-fixture /d /c exit 23"])
            .status()
            .unwrap();
        assert_eq!(result.code(), Some(23));
        assert_eq!(fs::read(&native).unwrap(), original);
        edit_cmd_shim(&store, &native, &system.join("cmd.exe"), true).unwrap();
        edit_cmd_shim(&store, &native, &system.join("cmd.exe"), false).unwrap();
        fs::write(native.with_extension("com"), b"user program").unwrap();
        assert!(edit_cmd_shim(&store, &native, &system.join("cmd.exe"), true).is_err());
        assert_eq!(
            fs::read(native.with_extension("com")).unwrap(),
            b"user program"
        );
    }
    #[cfg(windows)]
    #[test]
    fn existing_powershell_resolves_new_shim_without_reload_and_keeps_exit_code() {
        use base64::Engine;
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("cam-shim-fixture.exe");
        let shim = native.with_extension("ps1");
        let staged = dir.path().join("staged.txt");
        let system = PathBuf::from(env::var_os("SystemRoot").unwrap()).join("System32");
        fs::copy(system.join("hostname.exe"), &native).unwrap();
        fs::write(&staged, command_shim(&system.join("cmd.exe"))).unwrap();
        let quote = |path: &Path| path.to_string_lossy().replace('\'', "''");
        let script = format!(
            r#"$ErrorActionPreference = 'Stop'
$env:PATH = '{directory};' + $env:PATH
$env:CLAUDE_CODE_OAUTH_TOKEN = 'non-secret-parent-fixture'
$before = (Get-Command cam-shim-fixture).CommandType.ToString()
& cam-shim-fixture | Out-Null
Copy-Item -LiteralPath '{staged}' -Destination '{shim}'
$after = (Get-Command cam-shim-fixture).CommandType.ToString()
& cam-shim-fixture /d /c 'exit 23'
$code = $LASTEXITCODE
[pscustomobject]@{{before=$before; after=$after; code=$code; parentUnchanged=($env:CLAUDE_CODE_OAUTH_TOKEN -eq 'non-secret-parent-fixture')}} | ConvertTo-Json -Compress
"#,
            directory = quote(dir.path()),
            staged = quote(&staged),
            shim = quote(&shim)
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        for shell in ["powershell.exe", "pwsh.exe"] {
            if shim.exists() {
                fs::remove_file(&shim).unwrap();
            }
            let output = match std::process::Command::new(shell)
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                    &encoded,
                ])
                .output()
            {
                Ok(output) => output,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && shell == "pwsh.exe" =>
                {
                    continue
                }
                Err(error) => panic!("Cannot run PowerShell fixture: {error}"),
            };
            assert!(output.status.success(), "{shell} fixture failed");
            let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["before"], "Application", "{shell}");
            assert_eq!(result["after"], "ExternalScript", "{shell}");
            assert_eq!(result["code"], 23, "{shell}");
            assert_eq!(result["parentUnchanged"], true, "{shell}");
            assert_eq!(
                fs::read(&native).unwrap(),
                fs::read(system.join("hostname.exe")).unwrap()
            );
        }
    }
    #[test]
    fn command_shim_is_idempotent_preserves_native_exe_and_refuses_foreign_script() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            root: dir.path().into(),
        };
        let exe = dir.path().join("claude.exe");
        let shim = exe.with_extension("ps1");
        let helper = dir.path().join("user's helper.exe");
        fs::write(&exe, b"native executable fixture").unwrap();
        edit_command_shim(&store, &shim, &helper, true).unwrap();
        let first = fs::read(&shim).unwrap();
        edit_command_shim(&store, &shim, &helper, true).unwrap();
        assert_eq!(fs::read(&shim).unwrap(), first);
        assert_eq!(fs::read(&exe).unwrap(), b"native executable fixture");
        assert!(decode(&first).unwrap().0.contains("user''s helper.exe"));
        edit_command_shim(&store, &shim, &helper, false).unwrap();
        assert!(!shim.exists());
        assert!(exe.exists());
        fs::write(&shim, b"# user's existing script").unwrap();
        assert!(edit_command_shim(&store, &shim, &helper, true).is_err());
        assert!(edit_command_shim(&store, &shim, &helper, false).is_err());
        assert_eq!(fs::read(&shim).unwrap(), b"# user's existing script");
    }
    #[test]
    fn profile_edit_is_idempotent_preserves_content_and_can_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            root: dir.path().into(),
        };
        let profile = dir.path().join("profile.ps1");
        let original = "# 中文配置\r\n$custom=42\r\n";
        fs::write(&profile, encode(original, &Encoding::Utf16)).unwrap();
        let helper = Path::new("C:/test/user's tools/cam-launcher.exe");
        edit_profiles(&store, std::slice::from_ref(&profile), Some(helper)).unwrap();
        let first = fs::read(&profile).unwrap();
        edit_profiles(&store, std::slice::from_ref(&profile), Some(helper)).unwrap();
        assert!(first == fs::read(&profile).unwrap());
        let text = decode(&first).unwrap().0;
        assert!(text.contains("user''s tools"));
        assert!(text.contains("$custom=42"));
        edit_profiles(&store, std::slice::from_ref(&profile), None).unwrap();
        let text = decode(&fs::read(&profile).unwrap()).unwrap().0;
        assert!(!text.contains(BEGIN));
        assert!(text.contains("$custom=42"));
    }
    #[test]
    fn broken_markers_do_not_destroy_profile() {
        assert!(without_block(BEGIN).is_err());
        assert!(without_block(&format!("{BEGIN}{BEGIN}{END}")).is_err());
    }
}
