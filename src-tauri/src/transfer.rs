//! One-way setup-token import over stdin for a user-authorized SSH transfer.
//! Never exports tokens or handles normal-login refresh tokens.
use crate::{crypto, model::*, network};
use std::io::Read;
use zeroize::{Zeroize, Zeroizing};

pub fn read_input(reader: impl Read) -> AppResult<AccountInput> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(16_385));
    reader
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取导入数据。")?;
    if bytes.len() > 16_384 {
        return Err("导入数据过大。".into());
    }
    let mut input: AccountInput =
        serde_json::from_slice(&bytes).map_err(|_| "导入数据格式不正确。")?;
    let valid = input.id.is_none()
        && !input.expected_ip.trim().is_empty()
        && network::validate_route(&input.proxy, &input.expected_ip).is_ok()
        && input
            .token
            .as_deref()
            .is_some_and(|token| crypto::validate_token(token).is_ok());
    if !valid {
        if let Some(token) = input.token.as_mut() {
            token.zeroize();
        }
        return Err("只可导入 setup token，且必须带上原来的固定出口和本机代理。".into());
    }
    Ok(input)
}

#[cfg(target_os = "macos")]
pub fn import_stdin(select: bool) -> AppResult<()> {
    use crate::{registry::UserEnvironment, service::Manager};
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return Err("请通过标准输入传入导入数据，不要把令牌写在命令行里。".into());
    }
    let input = read_input(std::io::stdin().lock())?;
    let manager = Manager::discover()?;
    let result = manager.add(input, &network::Curl, &UserEnvironment)?;
    if select {
        manager.switch(&result.account.id, &network::Curl, &UserEnvironment)?;
    }
    // Account is the existing frontend-safe view: no encrypted or raw tokens.
    let state = manager.get_state(&UserEnvironment)?;
    println!(
        "{}",
        serde_json::to_string(&state).map_err(|_| "无法输出导入结果。")?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_requires_setup_token_and_an_existing_exit_binding() {
        let mut payload = serde_json::json!({"name":"fixture","token":"sk-ant-oat01-fixture-fixture-fixture", "proxy":DEFAULT_PROXY,"expectedIp":"203.0.113.1","plan":"pro"});
        let read = |v: &serde_json::Value| read_input(v.to_string().as_bytes());
        assert!(read(&payload).is_ok());
        payload["expectedIp"] = "".into();
        assert!(read(&payload).is_err());
        payload["expectedIp"] = "203.0.113.1".into();
        payload["refresh_token"] = "fixture-refresh-secret".into();
        let error = read(&payload).err().unwrap();
        assert!(!error.contains("fixture-refresh-secret"));
        assert!(read_input(&vec![b'x'; 16_385][..]).is_err());
    }
}
