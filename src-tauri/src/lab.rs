//! Only compiled with --features synthetic-tests. Never a production bypass.
use crate::{model::*, network::Curl, platform, registry::UserEnvironment, service::Manager};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{ffi::OsString, path::PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fixture {
    kind: String,
    pub root: PathBuf,
    pub home: PathBuf,
    pub local_app_data: PathBuf,
    pub ca: PathBuf,
    pub python: PathBuf,
    pub proxy_a: String,
    pub proxy_b: String,
    pub token_a: String,
    pub token_b: String,
}
pub fn config() -> AppResult<Fixture> {
    let file = PathBuf::from(
        std::env::var_os("CAM_SYNTHETIC_FILE")
            .ok_or("Synthetic build requires a marked lab fixture")?,
    );
    let f: Fixture = platform::read_json(&file)?.ok_or("Missing synthetic fixture")?;
    let root = platform::checked_path(&f.root)?;
    if f.kind != "CAM-SYNTHETIC-ONLY"
        || file.parent() != Some(root.as_path())
        || !root.to_string_lossy().contains("CAMHotSwitchLab")
        || ![&f.home, &f.local_app_data, &f.ca]
            .iter()
            .all(|p| p.starts_with(&root))
        || std::env::var_os("USERPROFILE").map(PathBuf::from) != Some(f.home.clone())
        || std::env::var_os("LOCALAPPDATA").map(PathBuf::from) != Some(f.local_app_data.clone())
        || ![&f.token_a, &f.token_b]
            .iter()
            .all(|t| t.starts_with("sk-ant-oat01-CAM_SYNTHETIC_"))
    {
        return Err("Synthetic fixture isolation check failed".into());
    }
    for p in [&f.proxy_a, &f.proxy_b] {
        crate::network::validate_proxy(p)?;
    }
    Ok(f)
}
pub fn check_proxy(proxy: &str) -> AppResult<()> {
    let f = config()?;
    if proxy != f.proxy_a && proxy != f.proxy_b {
        return Err("Synthetic proxy is not in the fixture".into());
    }
    Ok(())
}
pub fn activate() -> AppResult<()> {
    let f = config()?;
    std::env::set_var("NODE_EXTRA_CA_CERTS", &f.ca);
    std::env::set_var("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    std::env::set_var("DISABLE_AUTOUPDATER", "1");
    Ok(())
}
pub fn control(args: &[OsString]) -> AppResult<Value> {
    let f = config()?;
    let m = Manager::discover()?;
    let arg = |i: usize| {
        args.get(i)
            .map(|v| v.to_string_lossy().into_owned())
            .ok_or("Missing fixture argument".to_owned())
    };
    let id = |name: &str| -> AppResult<Option<String>> {
        if name == "normal" {
            return Ok(None);
        }
        let db = m.store.read()?;
        if let Some(a) = db.ordinary_accounts.iter().find(|a| a.name == name) {
            return Ok(Some(a.id.clone()));
        }
        db.accounts
            .into_iter()
            .find(|a| a.name == name)
            .map(|a| Some(a.id))
            .ok_or("Fixture account missing".into())
    };
    match arg(0)?.as_str() {
        "login" => {
            let label = arg(2)?;
            let (proxy, default_ip) = match label.as_str() {
                "A" => (f.proxy_a, "203.0.113.11"),
                "B" => (f.proxy_b, "203.0.113.77"),
                _ => return Err("unknown lab route".into()),
            };
            let login = crate::ordinary::begin(
                m.clone(),
                arg(1)?,
                proxy,
                arg(3).unwrap_or_else(|_| default_ip.into()),
                args.get(4).map(|a| a.to_string_lossy().into_owned()),
            )?;
            // Keep the same product enrollment thread and Job alive as the Tauri host.
            loop {
                let mut s = crate::ordinary::status(&m.store, &login.login_id)?;
                if s.status != "waiting" {
                    s.login_url = None;
                    return serde_json::to_value(s).map_err(|e| e.to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
        "cancel-login" => {
            crate::ordinary::cancel(&m.store, &arg(1)?)?;
            Ok(json!({"cancelRequested":true}))
        }
        "init" => {
            if !m.store.read()?.accounts.is_empty() {
                return Err("Fixture already initialized".into());
            }
            for (name, token, proxy, ip) in [
                ("A", f.token_a, f.proxy_a, "203.0.113.11"),
                ("B", f.token_b, f.proxy_b, "203.0.113.77"),
            ] {
                m.add(
                    AccountInput {
                        id: None,
                        name: name.into(),
                        plan: "pro".into(),
                        token: Some(token),
                        proxy,
                        expected_ip: ip.into(),
                        note: "synthetic only".into(),
                    },
                    &Curl,
                    &UserEnvironment,
                )?;
            }
            m.switch(&id("A")?.unwrap(), &Curl, &UserEnvironment)?;
            Ok(json!({"initialized":true}))
        }
        "state" => serde_json::to_value(m.get_state(&UserEnvironment)?).map_err(|e| e.to_string()),
        "preview" => serde_json::to_value(crate::sessions::preview(&m, id(&arg(1)?)?)?)
            .map_err(|e| e.to_string()),
        "refresh" => serde_json::to_value(crate::sessions::refresh(&m, arg(1)?)?)
            .map_err(|e| e.to_string()),
        "execute" => {
            let mode =
                serde_json::from_value(json!(arg(2)?)).map_err(|_| "Invalid permission mode")?;
            serde_json::to_value(crate::sessions::execute(&m, arg(1)?, mode)?)
                .map_err(|e| e.to_string())
        }
        "retry" => serde_json::to_value(crate::sessions::retry(&m, arg(1)?, arg(2)?)?)
            .map_err(|e| e.to_string()),
        "query" => serde_json::to_value(crate::native_probe::check(
            &m,
            id(&arg(1)?)?.as_deref(),
            &Curl,
        )?)
        .map_err(|e| e.to_string()),
        "lifecycle" => crate::native_probe::synthetic_lifecycle(),
        _ => Err("Unknown synthetic command".into()),
    }
}
