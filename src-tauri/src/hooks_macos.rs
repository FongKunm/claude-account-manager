//! Terminal integration owns a marked rc block and a private helper copy only.
//! No changes to the official claude binary, login credentials, PATH or launchctl.
use crate::{client, conflicts, macos_support as mac, model::AppResult, platform, store::Store};
use std::{
    env, fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

fn profiles(home: &Path, zdir: Option<&Path>) -> Vec<PathBuf> {
    let zdir = zdir.unwrap_or(home);
    // Do not create .bash_profile if doing so would shadow an existing login rc.
    let bash_login = [".bash_profile", ".bash_login", ".profile"]
        .iter()
        .map(|name| home.join(name))
        .find(|p| p.exists())
        .unwrap_or_else(|| home.join(".bash_profile"));
    let mut result = vec![zdir.join(".zshrc"), home.join(".bashrc"), bash_login];
    result.sort();
    result.dedup();
    result
}
fn read(path: &Path) -> AppResult<String> {
    let path = platform::checked_path(path)?;
    if !path.exists() {
        return Ok(String::new());
    }
    let metadata = fs::metadata(&path).map_err(|_| "无法检查终端配置。")?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("终端配置过大或不是普通文件，未覆盖。".into());
    }
    fs::read_to_string(path).map_err(|_| "终端配置必须是 UTF-8 文本，未覆盖。".into())
}
fn write(path: &Path, bytes: &[u8], mode: u32) -> AppResult<()> {
    let path = platform::checked_path(path)?;
    let parent = path.parent().ok_or("终端配置路径不正确。")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建终端配置目录。")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法准备终端配置。")?;
    fs::set_permissions(file.path(), fs::Permissions::from_mode(mode))
        .map_err(|_| "无法设置终端文件权限。")?;
    file.write_all(bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "无法写入终端配置。")?;
    file.persist(&path).map_err(|_| "无法更新终端配置。")?;
    Ok(())
}
fn update_profiles(
    store: &Store,
    home: &Path,
    zdir: Option<&Path>,
    install: bool,
) -> AppResult<()> {
    let script = store.root.join("terminal.sh");
    // Validate all destinations and marked blocks before changing any rc file.
    let tracking = store.root.join("mac-terminal-profiles.json");
    let mut paths: Vec<PathBuf> = platform::read_json(&tracking)?.unwrap_or_default();
    paths.extend(profiles(home, zdir));
    paths.sort();
    paths.dedup();
    let changes = paths
        .iter()
        .cloned()
        .map(|path| {
            let original = read(&path)?;
            let updated = if install {
                mac::profile_text(&original, &script)?
            } else {
                mac::without_block(&original)?
            };
            let mode = fs::metadata(&path)
                .map(|v| v.permissions().mode() & 0o777)
                .unwrap_or(0o600);
            Ok((path, original, updated, mode))
        })
        .collect::<AppResult<Vec<_>>>()?;
    for (path, original, updated, _) in &changes {
        if original != updated && path.exists() {
            store.backup("mac-shell-profile", original.as_bytes())?;
        }
    }
    for (index, (path, original, updated, mode)) in changes.iter().enumerate() {
        if original == updated {
            continue;
        }
        if write(path, updated.as_bytes(), *mode).is_err() {
            for (path, original, updated, mode) in &changes[..index] {
                if original != updated {
                    let _ = write(path, original.as_bytes(), *mode);
                }
            }
            return Err("无法更新终端配置；已尝试恢复，原配置备份保存在数据目录。".into());
        }
    }
    // Remember custom ZDOTDIR paths even when the GUI later runs without that
    // terminal's environment. Uninstall can then remove every owned block.
    platform::write_json(&tracking, &if install { paths } else { vec![] })
}

pub fn install(store: &Store) -> AppResult<()> {
    client::executable()?;
    let current = env::current_exe().map_err(|_| "无法定位终端启动器。")?;
    let source = current
        .parent()
        .ok_or("程序目录不正确。")?
        .join("cam-launcher");
    if !source.is_file() {
        return Err("Mac 程序包缺少 cam-launcher，请使用构建脚本重新打包。".into());
    }
    let directory = store.root.join("bin");
    platform::private_directory(&directory)?;
    let target = directory.join("cam-launcher");
    platform::checked_path(&target)?;
    let bytes = fs::read(&source).map_err(|_| "无法读取 Mac 终端启动器。")?;
    if fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
        write(&target, &bytes, 0o700)?;
    }
    let script = store.root.join("terminal.sh");
    let existing = read(&script)?;
    if !existing.is_empty() && !existing.starts_with(mac::START) {
        return Err("终端入口文件不属于本程序，未覆盖。".into());
    }
    let text = mac::function_text(&target)?;
    if text != existing {
        write(&script, text.as_bytes(), 0o600)?;
    }
    let zdir = env::var_os("ZDOTDIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    update_profiles(store, &conflicts::home()?, zdir.as_deref(), true)
}

pub fn uninstall(store: &Store) -> AppResult<()> {
    let zdir = env::var_os("ZDOTDIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    update_profiles(store, &conflicts::home()?, zdir.as_deref(), false)?;
    // Keep the helper and terminal.sh for already-open shells. Manager::uninstall
    // has switched to normal login, so they safely forward ordinary CLI launches.
    // New terminals no longer load the function. No user files are deleted.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bash_login_selection_preserves_existing_profile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".profile"), "# fixture\n").unwrap();
        let paths = profiles(dir.path(), None);
        assert!(paths.contains(&dir.path().join(".profile")));
        assert!(!paths.contains(&dir.path().join(".bash_profile")));
    }
    #[test]
    fn rc_install_remove_keeps_user_settings_and_private_backup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        platform::private_directory(&root).unwrap();
        let store = Store { root };
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join(".bashrc"), "# user\nexport KEEP=1\n").unwrap();
        update_profiles(&store, &home, None, true).unwrap();
        let first = fs::read_to_string(home.join(".bashrc")).unwrap();
        update_profiles(&store, &home, None, true).unwrap();
        assert_eq!(first, fs::read_to_string(home.join(".bashrc")).unwrap());
        update_profiles(&store, &home, None, false).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(".bashrc")).unwrap(),
            "# user\nexport KEEP=1\n"
        );
    }
}
