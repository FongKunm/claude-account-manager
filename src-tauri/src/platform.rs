use crate::model::AppResult;
use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use wait_timeout::ChildExt;

pub const NO_WINDOW: u32 = 0x08000000;

pub fn hide(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

pub fn windows_exe(name: &str) -> PathBuf {
    PathBuf::from(env::var_os("SystemRoot").unwrap_or_else(|| OsString::from("C:\\Windows")))
        .join(name)
}

pub fn system_exe(name: &str) -> PathBuf {
    windows_exe("System32").join(name)
}

pub fn curl_exe() -> PathBuf {
    #[cfg(all(windows, feature = "synthetic-tests"))]
    {
        // The lab's old Windows curl predates process-local --cacert support.
        // Git curl trusts only the fixture CA passed explicitly by lab::trace.
        return PathBuf::from("C:/Program Files/Git/mingw64/bin/curl.exe");
    }
    #[cfg(all(windows, not(feature = "synthetic-tests")))]
    {
        system_exe("curl.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/usr/bin/curl")
    }
}

pub fn checked_path(path: &Path) -> AppResult<PathBuf> {
    let path = std::path::absolute(path).map_err(|_| "无法解析本机数据路径。".to_owned())?;
    // macOS's system temporary directory uses the OS-owned /var -> /private/var
    // alias. Normalize only that alias; continue rejecting user-created symlinks.
    #[cfg(target_os = "macos")]
    let path = if let Ok(rest) = path.strip_prefix("/var") {
        PathBuf::from("/private/var").join(rest)
    } else {
        path
    };
    for entry in path.ancestors() {
        let metadata = match fs::symlink_metadata(entry) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("无法检查数据路径权限。".into()),
        };
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if metadata.file_type().is_symlink() || reparse {
            return Err(format!(
                "数据路径不能经过符号链接或目录联接：{}",
                entry.display()
            ));
        }
    }
    Ok(path)
}

pub struct Capture {
    pub success: bool,
    pub stdout: Vec<u8>,
}

/// Bounded, concurrent pipe reads. Never return raw stderr to callers or logs.
pub fn capture(command: Command, timeout: Duration) -> AppResult<Capture> {
    capture_with_input(command, timeout, None)
}

pub fn capture_with_input(
    command: Command,
    timeout: Duration,
    input: Option<&[u8]>,
) -> AppResult<Capture> {
    capture_with_started(command, timeout, input, |_| {})
}

pub fn capture_with_started(
    mut command: Command,
    timeout: Duration,
    input: Option<&[u8]>,
    started: impl FnOnce(u32),
) -> AppResult<Capture> {
    hide(&mut command);
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "无法启动所需的本机程序。".to_owned())?;
    started(child.id());
    let stdout = child.stdout.take().ok_or("无法读取程序状态。")?;
    let stderr = child.stderr.take().ok_or("无法读取程序状态。")?;
    let output_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(65_537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let error_reader = thread::spawn(move || {
        let _ = std::io::copy(&mut stderr.take(65_537), &mut std::io::sink());
    });
    if let Some(input) = input {
        let result = child
            .stdin
            .take()
            .ok_or("无法传递请求参数。")?
            .write_all(input);
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("无法传递请求参数。".into());
        }
    }
    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("本机程序响应超时，已停止本次操作。".into());
        }
    };
    let bytes = output_reader
        .join()
        .map_err(|_| "无法读取程序状态。")?
        .map_err(|_| "无法读取程序状态。")?;
    let _ = error_reader.join();
    if bytes.len() > 65_536 {
        return Err("程序返回的状态过大，未继续处理。".into());
    }
    Ok(Capture {
        success: status.success(),
        stdout: bytes,
    })
}

pub fn private_directory(path: &Path) -> AppResult<()> {
    let path = checked_path(path)?;
    fs::create_dir_all(&path).map_err(|_| "无法创建本机私有数据目录。".to_owned())?;
    private_permissions(&path, true)
}

/// Apply an owner-only ACL before writing a plaintext credential export.
pub fn private_file(path: &Path) -> AppResult<()> {
    private_permissions(&checked_path(path)?, false)
}

fn private_permissions(path: &Path, directory: bool) -> AppResult<()> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
                s: *const u16,
                revision: u32,
                descriptor: *mut *mut c_void,
                size: *mut u32,
            ) -> i32;
            fn SetFileSecurityW(path: *const u16, information: u32, descriptor: *mut c_void)
                -> i32;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LocalFree(memory: *mut c_void) -> *mut c_void;
        }
        let mut who = Command::new(system_exe("whoami.exe"));
        who.args(["/user", "/fo", "csv", "/nh"]);
        let result = capture(who, Duration::from_secs(10))?;
        let text = String::from_utf8_lossy(&result.stdout);
        let sid = text.split([',', '"', '\r', '\n', ' ']).find(|s| {
            s.starts_with("S-1-")
                && s.chars()
                    .all(|c| c.is_ascii_digit() || c == '-' || c == 'S')
        });
        let sid = sid
            .filter(|_| result.success)
            .ok_or("无法确定当前 Windows 用户，已停止。")?;
        let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor: *mut c_void = std::ptr::null_mut();
        // SAFETY: UTF-16 strings are NUL terminated; Windows allocates the descriptor,
        // it remains live until SetFileSecurityW returns, then is released by LocalFree.
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err("无法构造私有目录权限。".into());
            }
            let success = SetFileSecurityW(name.as_ptr(), 0x80000004, descriptor) != 0;
            LocalFree(descriptor);
            if !success {
                return Err("无法设置 Windows 私有目录权限，已停止。".into());
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
        .map_err(|_| "无法设置私有目录权限。")?;
    }
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

pub struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub fn lock(path: &Path, timeout: Duration) -> AppResult<Lease> {
    let path = checked_path(path)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| "无法锁定账号数据。".to_owned())?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(Lease(file)),
            Err(_) if Instant::now() >= deadline => {
                return Err("其他操作尚未完成，请稍后重试。".into())
            }
            Err(_) => thread::sleep(Duration::from_millis(40)),
        }
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> AppResult<Option<T>> {
    let path = checked_path(path)?;
    let metadata = match fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(format!("无法读取配置：{}", path.display())),
        Ok(metadata) => metadata,
    };
    if metadata.len() > 4 * 1024 * 1024 {
        return Err("配置文件过大，已停止。".into());
    }
    let bytes = fs::read(&path).map_err(|_| format!("无法读取配置：{}", path.display()))?;
    let data = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    serde_json::from_slice(data)
        .map(Some)
        .map_err(|_| format!("配置损坏或格式不正确，未覆盖原文件：{}", path.display()))
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    let path = checked_path(path)?;
    let parent = path.parent().ok_or("配置路径不正确。")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建配置目录。")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法创建配置临时文件。")?;
    serde_json::to_writer_pretty(&mut temp, value).map_err(|_| "无法保存配置。")?;
    temp.flush()
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "无法写入配置。")?;
    let deadline = std::time::Instant::now() + Duration::from_millis(600);
    loop {
        match temp.persist(&path) {
            Ok(_) => return Ok(()),
            Err(e) => {
                // Windows readers (including Python and antivirus) can briefly
                // omit FILE_SHARE_DELETE. Keep the already-written temp file and
                // retry the same atomic replacement, never truncate the target.
                if cfg!(windows)
                    && matches!(e.error.raw_os_error(), Some(5 | 32 | 33))
                    && std::time::Instant::now() < deadline
                {
                    temp = e.file;
                    thread::sleep(Duration::from_millis(15));
                } else {
                    return Err("无法原子替换配置，原文件保留。".into());
                }
            }
        }
    }
}

#[cfg(all(test, windows))]
#[test]
fn atomic_write_survives_a_brief_non_delete_sharing_reader() {
    use std::os::windows::fs::OpenOptionsExt;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("owner.json");
    fs::write(&path, b"{\"old\":true}").unwrap();
    let reader = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        drop(reader);
    });
    write_json(&path, &serde_json::json!({"new":true})).unwrap();
    release.join().unwrap();
    assert_eq!(
        read_json::<serde_json::Value>(&path).unwrap().unwrap(),
        serde_json::json!({"new":true})
    );
}

pub fn reject_managed_registry() -> AppResult<()> {
    #[cfg(windows)]
    {
        use winreg::{
            enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ},
            RegKey,
        };
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            match RegKey::predef(hive)
                .open_subkey_with_flags("SOFTWARE\\Policies\\ClaudeCode", KEY_READ)
            {
                Ok(key)
                    if key.enum_values().next().is_some() || key.enum_keys().next().is_some() =>
                {
                    return Err("检测到 Claude Code 注册表管理策略，需人工核验后使用。".into())
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err("无法核验 Claude Code 管理策略，已停止。".into()),
            }
        }
    }
    Ok(())
}
