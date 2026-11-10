use super::*;
use std::{collections::BTreeMap, io::Write};

fn options(command: &Command) -> serde_json::Value {
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args[0], "--settings");
    serde_json::from_str(&args[1].to_string_lossy()).unwrap()
}

#[test]
fn snapshot_pins_all_proxy_spellings_and_keeps_token_out_of_arguments() {
    let mut child = Command::new("unused-test-executable");
    let args = vec!["--resume".into(), "fake-session".into()];
    configure_selected(
        &mut child,
        &args,
        "CAM_SYNTHETIC_TOKEN_A",
        "http://127.0.0.1:18001",
    )
    .unwrap();
    let snapshot = options(&child);
    let environment: BTreeMap<_, _> = child.get_envs().collect();
    for key in PROXY_KEYS {
        assert_eq!(snapshot["env"][key], "http://127.0.0.1:18001");
        // Windows has case-insensitive environment names; settings JSON does not.
        assert!(environment
            .iter()
            .any(|(k, v)| k.to_string_lossy().eq_ignore_ascii_case(key)
                && *v == Some(std::ffi::OsStr::new("http://127.0.0.1:18001"))));
    }
    for key in ["NO_PROXY", "no_proxy"] {
        assert_eq!(snapshot["env"][key], SESSION_NO_PROXY);
    }
    assert_eq!(snapshot["env"].as_object().unwrap().len(), 8);
    assert_eq!(
        environment[std::ffi::OsStr::new("CLAUDE_CODE_OAUTH_TOKEN")],
        Some(std::ffi::OsStr::new("CAM_SYNTHETIC_TOKEN_A"))
    );
    assert!(!format!("{:?}", child.get_args().collect::<Vec<_>>()).contains("CAM_SYNTHETIC"));
    let argv: Vec<_> = child.get_args().skip(2).collect();
    assert_eq!(argv, args.iter().map(|s| s.as_os_str()).collect::<Vec<_>>());
}

#[test]
fn rejects_mutable_settings_overrides_and_nonlocal_proxies_before_launch() {
    for args in [
        vec!["--settings", "mutable.json"],
        vec!["--settings={}"],
        vec!["--setting-sources", "user"],
        vec!["--setting-sources=user"],
    ] {
        let mut command = Command::new("unused-test-executable");
        assert!(configure_selected(
            &mut command,
            &args.into_iter().map(Into::into).collect::<Vec<_>>(),
            "CAM_SYNTHETIC",
            "http://127.0.0.1:18001"
        )
        .is_err());
        assert_eq!(command.get_args().len(), 0);
        assert_eq!(command.get_envs().count(), 0);
    }
    for proxy in [
        "",
        "http://example.invalid:1234",
        "http://user:pass@127.0.0.1:1234",
    ] {
        let mut command = Command::new("unused-test-executable");
        assert!(configure_selected(&mut command, &[], "CAM_SYNTHETIC", proxy).is_err());
        assert_eq!(command.get_envs().count(), 0);
    }
}

#[test]
fn normal_arguments_and_parent_environment_are_not_modified_by_preparing_a_child() {
    // Read only relevant process variables; never change this process or user env.
    let keys = [
        "CLAUDE_CODE_OAUTH_TOKEN",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ];
    let before: Vec<_> = keys.iter().map(env::var_os).collect();
    let mut a = Command::new("unused-test-executable");
    let mut b = Command::new("unused-test-executable");
    configure_selected(&mut a, &[], "CAM_SYNTHETIC_A", "http://127.0.0.1:18001").unwrap();
    configure_selected(&mut b, &[], "CAM_SYNTHETIC_B", "http://127.0.0.1:18002").unwrap();
    assert_eq!(options(&a)["env"]["HTTPS_PROXY"], "http://127.0.0.1:18001");
    assert_eq!(options(&b)["env"]["HTTPS_PROXY"], "http://127.0.0.1:18002");
    assert!(keys.iter().map(env::var_os).eq(before));
}

/// Test-only adapter for scripts/test-process-isolation.py. It invokes the
/// production command builder, with an invented token and an isolated HOME.
/// It is absent from release binaries and never discovers a real Store/Keychain.
#[test]
#[ignore = "Local mock-only native CLI integration, invoked by test-process-isolation.py"]
fn native_cli_fixture() {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        cli: PathBuf,
        test_root: PathBuf,
        token: String,
        proxy: String,
        args: Vec<String>,
    }
    let input: Input =
        serde_json::from_str(&env::var("CAM_TEST_LAUNCH_JSON").expect("synthetic fixture input"))
            .unwrap();
    assert!(input.token.starts_with("sk-ant-oat01-CAM_SYNTHETIC_"));
    assert!(input.test_root.join("CAM-SYNTHETIC-ONLY").is_file());
    assert_eq!(PathBuf::from(env::var_os("HOME").unwrap()), input.test_root);
    let target = url::Url::parse(&env::var("ANTHROPIC_BASE_URL").unwrap()).unwrap();
    let local_fixture = match target.host() {
        Some(url::Host::Ipv4(ip)) if ip.is_loopback() || ip.is_private() => {
            // Bind proves the target belongs to this machine. macOS cannot bind
            // another 127/8 address by default, so its own LAN IP is also valid.
            std::net::TcpListener::bind((ip, 0)).is_ok()
        }
        _ => false,
    };
    assert!(local_fixture, "fixture only permits local mock targets");
    assert!(target.port().is_some());
    let mut command = Command::new(input.cli);
    configure_selected(
        &mut command,
        &input.args.into_iter().map(Into::into).collect::<Vec<_>>(),
        &input.token,
        &input.proxy,
    )
    .unwrap();
    command.env_remove("CAM_TEST_LAUNCH_JSON");
    let mut child = command.spawn().unwrap();
    println!(
        "{}",
        serde_json::json!({"type":"cam_test_child", "pid":child.id()})
    );
    std::io::stdout().flush().unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "synthetic CLI exited unsuccessfully");
}
