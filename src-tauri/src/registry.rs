use crate::{
    crypto,
    model::{AppResult, ENV_KEYS},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvValue {
    pub bytes: Vec<u8>,
    pub kind: u32,
}
impl Drop for EnvValue {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.bytes);
    }
}
pub type Snapshot = BTreeMap<String, Option<EnvValue>>;
impl EnvValue {
    pub fn text(value: &str) -> Self {
        Self {
            kind: 1,
            bytes: value
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect(),
        }
    }
    pub fn string(&self) -> AppResult<Zeroizing<String>> {
        if ![1, 2].contains(&self.kind) || !self.bytes.len().is_multiple_of(2) {
            return Err("环境变量格式不正确。".into());
        }
        let mut wide: Vec<u16> = self
            .bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        while wide.last() == Some(&0) {
            wide.pop();
        }
        String::from_utf16(&wide)
            .map(Zeroizing::new)
            .map_err(|_| "环境变量编码不正确。".into())
    }
    pub fn fingerprint(&self) -> String {
        crypto::fingerprint(&self.bytes)
    }
}
pub trait Environment {
    fn read(&self, key: &str) -> AppResult<Option<EnvValue>>;
    fn write(&self, key: &str, value: Option<&EnvValue>) -> AppResult<()>;
    fn broadcast(&self) {}
    fn snapshot(&self) -> AppResult<Snapshot> {
        ENV_KEYS
            .into_iter()
            .map(|key| Ok((key.into(), self.read(key)?)))
            .collect()
    }
}
pub struct UserEnvironment;
#[cfg(all(windows, not(feature = "synthetic-tests")))]
impl Environment for UserEnvironment {
    fn read(&self, name: &str) -> AppResult<Option<EnvValue>> {
        use winreg::{enums::*, RegKey};
        let key = match RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Environment", KEY_READ)
        {
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("无法读取用户环境变量。".into()),
        };
        match key.get_raw_value(name) {
            Ok(v) => Ok(Some(EnvValue {
                bytes: v.bytes,
                kind: v.vtype as u32,
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("无法读取用户环境变量。".into()),
        }
    }
    fn write(&self, name: &str, value: Option<&EnvValue>) -> AppResult<()> {
        use winreg::{enums::*, RegKey, RegValue};
        if !ENV_KEYS.contains(&name) {
            return Err("不允许修改此环境变量。".into());
        }
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey("Environment")
            .map_err(|_| "无法写入用户环境变量。")?;
        if let Some(v) = value {
            let kind = match v.kind {
                1 => REG_SZ,
                2 => REG_EXPAND_SZ,
                _ => return Err("环境变量格式不支持，未覆盖。".into()),
            };
            key.set_raw_value(
                name,
                &RegValue {
                    bytes: v.bytes.clone(),
                    vtype: kind,
                },
            )
            .map_err(|_| "无法写入用户环境变量。")?;
        } else {
            match key.delete_value(name) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err("无法清理用户环境变量。".into()),
            }
        }
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn RegFlushKey(key: *mut std::ffi::c_void) -> i32;
        }
        // SAFETY: RegKey keeps the HKEY live for the duration of this flush.
        if unsafe { RegFlushKey(key.raw_handle().cast()) } != 0 {
            return Err("无法提交用户环境变量。".into());
        }
        Ok(())
    }
    fn broadcast(&self) {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SendMessageTimeoutW(
                window: *mut std::ffi::c_void,
                message: u32,
                wparam: usize,
                lparam: isize,
                flags: u32,
                timeout: u32,
                result: *mut usize,
            ) -> isize;
        }
        let text: Vec<u16> = "Environment".encode_utf16().chain(Some(0)).collect();
        // SAFETY: HWND_BROADCAST is a Win32 sentinel; the string is valid for this
        // synchronous, bounded call. No input or keystrokes are sent to applications.
        unsafe {
            SendMessageTimeoutW(
                0xffffusize as *mut _,
                0x001a,
                0,
                text.as_ptr() as isize,
                0x0002,
                1500,
                std::ptr::null_mut(),
            );
        }
    }
}
#[cfg(all(windows, feature = "synthetic-tests"))]
impl Environment for UserEnvironment {
    fn read(&self, _: &str) -> AppResult<Option<EnvValue>> {
        crate::lab::config()?;
        Ok(None)
    }
    fn write(&self, _: &str, _: Option<&EnvValue>) -> AppResult<()> {
        Err("Synthetic build forbids global environment writes".into())
    }
}
#[cfg(all(target_os = "macos", not(test)))]
impl Environment for UserEnvironment {
    fn read(&self, name: &str) -> AppResult<Option<EnvValue>> {
        if !ENV_KEYS.contains(&name) {
            return Ok(None);
        }
        let root = crate::store::Store::discover()?.root;
        crate::macos_support::LaunchEnvironment { root }.read(name)
    }
    fn write(&self, name: &str, value: Option<&EnvValue>) -> AppResult<()> {
        let root = crate::store::Store::discover()?.root;
        crate::macos_support::LaunchEnvironment { root }.write(name, value)
    }
}
#[cfg(all(target_os = "macos", test))]
impl Environment for UserEnvironment {
    fn read(&self, _: &str) -> AppResult<Option<EnvValue>> {
        Ok(None)
    }
    fn write(&self, _: &str, _: Option<&EnvValue>) -> AppResult<()> {
        Err("测试禁止写入真实 Mac 启动配置。".into())
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
impl Environment for UserEnvironment {
    fn read(&self, _: &str) -> AppResult<Option<EnvValue>> {
        Err("此版本仅支持 Windows。".into())
    }
    fn write(&self, _: &str, _: Option<&EnvValue>) -> AppResult<()> {
        Err("此版本仅支持 Windows。".into())
    }
}
pub fn owned(db: &crate::model::Database) -> AppResult<Snapshot> {
    match &db.owned_env {
        None => Ok(Snapshot::new()),
        Some(value) => serde_json::from_slice(&crypto::unseal(value)?)
            .map_err(|_| "环境变量归属记录损坏。".into()),
    }
}
pub fn seal_json<T: Serialize>(value: &T) -> AppResult<String> {
    let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(|_| "无法编码加密数据。")?);
    crypto::seal(&bytes)
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::cell::{Cell, RefCell};
    #[derive(Default)]
    pub struct FakeEnvironment {
        pub values: RefCell<Snapshot>,
        pub writes: Cell<usize>,
        pub fail_on: Cell<Option<usize>>,
    }
    impl Environment for FakeEnvironment {
        fn read(&self, key: &str) -> AppResult<Option<EnvValue>> {
            Ok(self.values.borrow().get(key).cloned().flatten())
        }
        fn write(&self, key: &str, value: Option<&EnvValue>) -> AppResult<()> {
            let count = self.writes.get() + 1;
            self.writes.set(count);
            if self.fail_on.get() == Some(count) {
                return Err("模拟注册表写入失败。".into());
            }
            self.values.borrow_mut().insert(key.into(), value.cloned());
            Ok(())
        }
    }
}
