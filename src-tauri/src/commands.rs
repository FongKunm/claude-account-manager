use crate::{
    export::ExportResult, model::*, network::Curl, platform, registry::UserEnvironment,
    service::Manager,
};
use std::{path::Path, process::Command};

async fn background<T, F>(manager: Manager, job: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(Manager) -> AppResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || job(manager))
        .await
        .map_err(|_| "后台操作中断，请重试。".to_owned())?
}
#[tauri::command]
pub async fn list_managed_windows(manager: tauri::State<'_, Manager>) -> AppResult<Vec<serde_json::Value>> {
    #[cfg(windows)]
    return background(manager.inner().clone(), |m| crate::sessions::list_windows(&m.store)).await;
    #[cfg(not(windows))]
    { let _ = manager; Ok(Vec::new()) }
}

#[tauri::command]
pub async fn focus_managed_window(window_id: String, manager: tauri::State<'_, Manager>) -> AppResult<()> {
    #[cfg(windows)]
    return background(manager.inner().clone(), move |m| crate::sessions::focus_window(&m.store,&window_id)).await;
    #[cfg(not(windows))]
    { let _ = (window_id,manager); Err("这个系统暂不支持定位终端。".into()) }
}

#[tauri::command]
pub async fn get_state(manager: tauri::State<'_, Manager>) -> AppResult<AppState> {
    background(manager.inner().clone(), |m| m.get_state(&UserEnvironment)).await
}
#[tauri::command]
pub async fn begin_normal_login(
    name: String,
    proxy: String,
    expected_ip: String,
    source_token_id: Option<String>,
    note: Option<String>,
    plan: Option<String>,
    manager: tauri::State<'_, Manager>,
) -> AppResult<serde_json::Value> {
    #[cfg(windows)]
    return background(manager.inner().clone(), move |m| {
        serde_json::to_value(crate::ordinary::begin_with_details(
            m,
            name,
            proxy,
            expected_ip,
            source_token_id,
            note.unwrap_or_default(),
            plan.unwrap_or_default(),
        )?)
        .map_err(|_| "无法返回登录编号。".into())
    })
    .await;
    #[cfg(not(windows))]
    Err("此平台暂不支持添加多个普通登录账号。".into())
}
#[tauri::command]
pub async fn get_normal_login(
    login_id: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<serde_json::Value> {
    #[cfg(windows)]
    return background(manager.inner().clone(), move |m| {
        serde_json::to_value(crate::ordinary::status(&m.store, &login_id)?)
            .map_err(|_| "无法返回登录状态。".into())
    })
    .await;
    #[cfg(not(windows))]
    Err("此平台暂不支持添加多个普通登录账号。".into())
}
#[tauri::command]
pub async fn cancel_normal_login(
    login_id: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<()> {
    #[cfg(windows)]
    return background(manager.inner().clone(), move |m| {
        crate::ordinary::cancel(&m.store, &login_id)
    })
    .await;
    #[cfg(not(windows))]
    Err("此平台暂不支持添加多个普通登录账号。".into())
}
#[tauri::command]
pub async fn add_account(
    input: AccountInput,
    manager: tauri::State<'_, Manager>,
) -> AppResult<AddResult> {
    background(manager.inner().clone(), move |m| {
        let _guard = crate::sessions::mutation_guard(&m.store)?;
        m.add(input, &Curl, &UserEnvironment)
    })
    .await
}
#[tauri::command]
pub async fn update_account(
    input: AccountInput,
    manager: tauri::State<'_, Manager>,
) -> AppResult<Account> {
    background(manager.inner().clone(), move |m| {
        let _guard = crate::sessions::mutation_guard(&m.store)?;
        m.update(input, &Curl, &UserEnvironment)
    })
    .await
}
#[tauri::command]
pub async fn edit_card(
    input: AccountInput, update_pair: bool, manager: tauri::State<'_, Manager>,
) -> AppResult<()> {
    background(manager.inner().clone(), move |m| {
        let _guard = crate::sessions::mutation_guard(&m.store)?;
        m.edit_card(input, update_pair, &Curl, &UserEnvironment)
    }).await
}
#[tauri::command]
pub async fn replace_token(
    id: String,
    token: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<Account> {
    let token = zeroize::Zeroizing::new(token);
    background(manager.inner().clone(), move |m| {
        let _guard = crate::sessions::mutation_guard(&m.store)?;
        m.replace(&id, &token, &Curl, &UserEnvironment)
    })
    .await
}
#[tauri::command]
pub async fn delete_account(
    id: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<DeleteResult> {
    background(manager.inner().clone(), move |m| {
        let _guard = crate::sessions::mutation_guard(&m.store)?;
        if m.store.read()?.current.as_deref() == Some(id.as_str()) {
            #[cfg(windows)]
            if {
                let p = crate::sessions::preview(&m, None)?;
                !p.windows.is_empty() || !p.blockers.is_empty()
            } {
                return Err("请先切换到其他账号，待原窗口恢复后再删除。".into());
            }
        }
        m.delete(&id, &UserEnvironment)
    })
    .await
}
#[tauri::command]
pub async fn switch_to(id: String, manager: tauri::State<'_, Manager>) -> AppResult<SwitchResult> {
    background(manager.inner().clone(), move |m| {
        let _guard = crate::sessions::mutation_guard(&m.store)?;
        #[cfg(windows)]
        if {
            let p = crate::sessions::preview(&m, Some(id.clone()))?;
            !p.windows.is_empty() || !p.blockers.is_empty()
        } {
            return Err("请先在切换确认窗口中检查并恢复正在运行的终端。".into());
        }
        m.switch(&id, &Curl, &UserEnvironment)
    })
    .await
}
#[tauri::command]
pub async fn use_normal(manager: tauri::State<'_, Manager>) -> AppResult<SwitchResult> {
    background(manager.inner().clone(), |m| {
        let _guard = crate::sessions::mutation_guard(&m.store)?;
        #[cfg(windows)]
        if {
            let p = crate::sessions::preview(&m, None)?;
            !p.windows.is_empty() || !p.blockers.is_empty()
        } {
            return Err("请先在切换确认窗口中检查并恢复正在运行的终端。".into());
        }
        m.normal(&UserEnvironment)
    })
    .await
}
#[tauri::command]
pub async fn preview_switch(
    id: Option<String>,
    manager: tauri::State<'_, Manager>,
) -> AppResult<crate::sessions::Plan> {
    background(manager.inner().clone(), move |m| {
        crate::sessions::preview(&m, id)
    })
    .await
}
#[tauri::command]
pub async fn refresh_switch(
    plan_id: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<crate::sessions::Plan> {
    background(manager.inner().clone(), move |m| crate::sessions::refresh(&m, plan_id)).await
}
#[tauri::command]
pub async fn execute_switch(
    plan_id: String,
    permission_mode: crate::sessions::Permissions,
    manager: tauri::State<'_, Manager>,
) -> AppResult<crate::sessions::Result> {
    background(manager.inner().clone(), move |m| {
        crate::sessions::execute(&m, plan_id, permission_mode)
    })
    .await
}
#[tauri::command]
pub async fn get_switch_progress(
    plan_id: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<crate::sessions::Progress> {
    background(manager.inner().clone(), move |m| {
        crate::sessions::progress(&m.store, plan_id)
    })
    .await
}
#[tauri::command]
pub async fn retry_restore(
    operation_id: String,
    window_id: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<crate::sessions::Result> {
    background(manager.inner().clone(), move |m| {
        crate::sessions::retry(&m, operation_id, window_id)
    })
    .await
}
#[tauri::command]
pub async fn check_usage(id: String, manager: tauri::State<'_, Manager>) -> AppResult<UsageCheck> {
    background(manager.inner().clone(), move |m| {
        crate::native_probe::check(&m, Some(&id), &Curl)
    })
    .await
}
#[tauri::command]
pub async fn check_normal_usage(manager: tauri::State<'_, Manager>) -> AppResult<UsageCheck> {
    background(manager.inner().clone(), |m| {
        crate::native_probe::check(&m, None, &Curl)
    })
    .await
}
#[tauri::command]
pub async fn detect_exit(
    proxy: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<DetectedExit> {
    background(manager.inner().clone(), move |m| {
        m.detect_exit(&proxy, &Curl)
    })
    .await
}
#[tauri::command]
pub async fn check_network(
    id: String,
    manager: tauri::State<'_, Manager>,
) -> AppResult<NetworkCheck> {
    background(manager.inner().clone(), move |m| {
        m.check_network(&id, &Curl, &UserEnvironment)
    })
    .await
}
#[tauri::command]
pub async fn export_cpa<R: tauri::Runtime>(
    id: Option<String>,
    window: tauri::WebviewWindow<R>,
    manager: tauri::State<'_, Manager>,
) -> AppResult<Option<ExportResult>> {
    background(manager.inner().clone(), move |m| {
        m.export_cpa_with_dialog(id.as_deref(), |directory, name| {
            #[cfg(any(windows, target_os = "macos"))]
            {
                rfd::FileDialog::new()
                    .set_parent(&window)
                    .set_title("导出 CPA JSON（包含完整凭据）")
                    .set_directory(directory)
                    .set_file_name(name)
                    .add_filter("CPA 认证文件", &["json"])
                    .save_file()
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            {
                let _ = (window, directory, name);
                None
            }
        })
    })
    .await
}
fn open_local(path: &Path, exe: &Path) -> AppResult<()> {
    let mut command = Command::new(exe);
    command.arg(path);
    platform::hide(&mut command);
    command
        .spawn()
        .map_err(|_| "无法打开本机文件或目录。".to_owned())?;
    Ok(())
}
#[tauri::command]
pub async fn open_data_folder(manager: tauri::State<'_, Manager>) -> AppResult<()> {
    background(manager.inner().clone(), |m| {
        #[cfg(windows)]
        {
            open_local(&m.store.root, &platform::windows_exe("explorer.exe"))
        }
        #[cfg(not(windows))]
        {
            open_local(&m.store.root, Path::new("/usr/bin/open"))
        }
    })
    .await
}
#[tauri::command]
pub async fn open_help(manager: tauri::State<'_, Manager>) -> AppResult<()> {
    background(manager.inner().clone(), |m| {
        let help = m.store.root.join("使用说明.txt");
        #[cfg(windows)]
        let text = include_str!("../HELP.txt");
        #[cfg(not(windows))]
        let text = include_str!("../HELP-mac.txt");
        std::fs::write(&help, text).map_err(|_| "无法写入使用说明。")?;
        #[cfg(windows)]
        {
            open_local(&help, &platform::system_exe("notepad.exe"))
        }
        #[cfg(not(windows))]
        {
            open_local(&help, Path::new("/usr/bin/open"))
        }
    })
    .await
}
fn app_context<R: tauri::Runtime>() -> tauri::Context<R> {
    // macOS embeds one Info.plist symbol per generate_context! expansion.
    // Reuse the same expansion for the real app and the IPC test runtime.
    tauri::generate_context!()
}
pub fn run_app() {
    #[cfg(debug_assertions)]
    let smoke = std::env::args_os().any(|a| a == "--smoke-test")
        || std::env::var("CAM_SMOKE_TEST").as_deref() == Ok("1");
    #[cfg(not(debug_assertions))]
    let smoke = false;
    let builder = tauri::Builder::default()
        .setup(move |app| {
            use tauri::Manager as _;
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                crate::window_corners::install(&window);
            }
            let manager = Manager::discover().map_err(std::io::Error::other)?;
            let usage_dir = manager.store.root.join("observed-usage");
            let usage_app = app.handle().clone();
            std::thread::spawn(move || {
                use tauri::Emitter;
                let signature = || {
                    let mut files: Vec<_> = std::fs::read_dir(&usage_dir).into_iter().flatten()
                        .filter_map(|entry| entry.ok())
                        .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
                        .filter_map(|entry| Some((entry.file_name(), entry.metadata().ok()?.modified().ok()?)))
                        .collect();
                    files.sort(); files
                };
                let mut previous = signature();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let current = signature();
                    if current != previous {
                        previous = current;
                        if usage_app.emit("usage-updated", ()).is_err() { break; }
                    }
                }
            });
            app.manage(manager);
            if smoke {
                use tauri::Listener;
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
                let handle=app.handle().clone();
                app.listen("cam-smoke-result",move|event|{
                    let passed=serde_json::from_str::<serde_json::Value>(event.payload()).ok()
                        .is_some_and(|v|v.get("passed").and_then(|v|v.as_bool())==Some(true));
                    eprintln!("CAM_SMOKE_{}",if passed {"PASS"} else {"FAIL"});
                    handle.exit(if passed {0} else {3});
                });
                let handle=app.handle().clone();
                std::thread::spawn(move||{
                    std::thread::sleep(std::time::Duration::from_secs(35));
                    eprintln!("CAM_SMOKE_TIMEOUT");
                    handle.exit(4);
                });
            }
            Ok(())
        })
        .on_page_load(move|webview,payload|{
            if smoke && matches!(payload.event(),tauri::webview::PageLoadEvent::Finished) {
                // This debug-only, hidden smoke window invokes the same production IPC
                // and checks React mounted in real WebView2. Only a boolean is emitted.
                let _=webview.eval(r#"
                    setTimeout(async () => {
                      let passed = false;
                      try {
                        const s = await window.__TAURI_INTERNALS__.invoke('get_state');
                        const p = await window.__TAURI_INTERNALS__.invoke('preview_switch', {id:null});
                        const progress = p.supported
                          ? await window.__TAURI_INTERNALS__.invoke('get_switch_progress', {planId:p.planId})
                          : {stage:'unsupported'};
                        passed = typeof s.version === 'string' && Array.isArray(s.accounts)
                          && Object.hasOwn(s.normal, 'org') && document.querySelector('#root')?.childElementCount > 0
                          && typeof p.supported === 'boolean' && Array.isArray(p.windows)
                          && Array.isArray(p.blockers) && typeof progress.stage === 'string'
                          && !document.body.innerText.includes('演示模式');
                      } catch (_) {}
                      await window.__TAURI_INTERNALS__.invoke('plugin:event|emit',
                        {event:'cam-smoke-result',payload:{passed}});
                    }, 1500);
                "#);
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            edit_card,
            list_managed_windows,
            focus_managed_window,
            begin_normal_login,
            get_normal_login,
            cancel_normal_login,
            add_account,
            update_account,
            replace_token,
            delete_account,
            switch_to,
            use_normal,
            preview_switch,
            refresh_switch,
            execute_switch,
            get_switch_progress,
            retry_restore,
            check_usage,
            check_normal_usage,
            detect_exit,
            check_network,
            export_cpa,
            open_data_folder,
            open_help
        ]);
    // Do not log raw errors or request arguments. Startup errors contain no tokens.
    if builder.run(app_context()).is_err() {
        eprintln!("Claude 账号管理器启动失败，请检查程序包和本机数据目录。");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    #[test]
    fn ipc_contract_and_main_window_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let context = app_context();
        let app = tauri::test::mock_builder()
            .manage(Manager::isolated(dir.path().into()))
            .invoke_handler(tauri::generate_handler![
                get_state,
                edit_card,
                list_managed_windows,
                focus_managed_window,
                begin_normal_login,
                get_normal_login,
                cancel_normal_login,
                add_account,
                check_normal_usage,
                detect_exit,
                export_cpa
            ])
            .build(context)
            .unwrap();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let call_from = |window: &tauri::WebviewWindow<tauri::test::MockRuntime>,
                         origin: &str,
                         cmd: &str,
                         body: Value| {
            tauri::test::get_ipc_response(
                window,
                tauri::webview::InvokeRequest {
                    cmd: cmd.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: origin.parse().unwrap(),
                    body: tauri::ipc::InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.into(),
                },
            )
            .map(|body| body.deserialize::<Value>().unwrap())
        };
        let call =
            |window: &tauri::WebviewWindow<tauri::test::MockRuntime>, cmd: &str, body: Value| {
                #[cfg(windows)]
                let origin = "http://tauri.localhost";
                #[cfg(not(windows))]
                let origin = "tauri://localhost";
                call_from(window, origin, cmd, body)
            };
        // get_state performs read-only environment inspection, never writes it.
        let state = call(&main, "get_state", json!({})).unwrap();
        assert_eq!(state["version"], env!("CARGO_PKG_VERSION"));
        assert!(state["accounts"].as_array().unwrap().is_empty());
        assert!(state["ordinaryAccounts"].as_array().unwrap().is_empty());
        #[cfg(windows)]
        {
            // Exercise the actual webview argument names and ACL without starting a login.
            let bad = call(
                &main,
                "begin_normal_login",
                json!({"name":"", "proxy":"http://127.0.0.1:31004", "expectedIp":"203.0.113.84"}),
            )
            .unwrap_err();
            assert!(bad.as_str().unwrap().contains("账号名称"));
            for cmd in ["get_normal_login", "cancel_normal_login"] {
                let bad = call(&main, cmd, json!({"loginId":"fixture-invalid"})).unwrap_err();
                assert!(bad.as_str().unwrap().contains("登录编号无效"));
            }
        }
        assert!(state["normal"].get("org").is_some());
        assert!(state.get("dataRoot").is_some());
        for field in [
            "plan",
            "subscriptionStatus",
            "billingType",
            "expectedIp",
            "lastUsageCheck",
        ] {
            assert!(
                state["normal"].get(field).is_some(),
                "Missing normal field: {field}"
            );
        }
        let normal = call(&main, "check_normal_usage", json!({})).unwrap();
        assert_eq!(normal["state"], "invalid"); // Isolated home has no credentials; no network call.
        assert!(call(
            &main,
            "detect_exit",
            json!({"proxy":"https://example.test:8080"})
        )
        .is_err());
        // A development page gets native privileges. Never trust an old/default
        // Vite origin, including the IPv6 listener belonging to another project.
        #[cfg(dev)]
        assert!(call_from(&main, "http://127.0.0.1:5281", "get_state", json!({})).is_ok());
        for origin in [
            "http://localhost:5173",
            "http://[::1]:5173",
            "http://127.0.0.1:5173",
            "http://127.0.0.1:5282",
        ] {
            assert!(
                call_from(&main, origin, "get_state", json!({})).is_err(),
                "Unexpected IPC access from {origin}"
            );
        }
        let error = call(
            &main,
            "add_account",
            json!({"input":{
                "name":"测试","plan":"max5","token":"invalid-fixture","proxy":DEFAULT_PROXY,
                "expectedIp":"203.0.113.1","note":""
            }}),
        )
        .unwrap_err();
        assert!(error.as_str().unwrap().contains("setup token"));
        assert!(!error.as_str().unwrap().contains("invalid-fixture"));
        let other = tauri::WebviewWindowBuilder::new(&app, "untrusted", Default::default())
            .build()
            .unwrap();
        assert!(call(&other, "get_state", json!({})).is_err());
        assert!(call(&other, "export_cpa", json!({"id": null})).is_err());
        // No ordinary identity exists in this isolated home, so no save dialog
        // opens. The permitted command fails with a sanitized business error.
        let export_error = call(&main, "export_cpa", json!({"id": null})).unwrap_err();
        assert!(export_error
            .as_str()
            .unwrap()
            .contains("未找到普通登录账号"));
        assert!(call_from(
            &main,
            "http://localhost:5173",
            "export_cpa",
            json!({"id": null})
        )
        .is_err());
    }
}
