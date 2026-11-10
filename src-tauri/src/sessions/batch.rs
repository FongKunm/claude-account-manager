use super::owner::{self, Record};
use super::*;
use crate::{
    client, conflicts,
    model::now,
    network::{self, Curl},
    registry::UserEnvironment,
    service::Manager,
    session_win::Process,
};
use std::{process::Command, thread};
#[derive(Clone, Serialize, Deserialize)]
struct Item {
    window: Window,
    target: Target,
    owner: Record,
    #[serde(default)]
    runtime: Option<super::runtime::Settings>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    plan: Plan,
    target_id: Option<String>,
    revision: u64,
    items: Vec<Item>,
    permissions: Permissions,
    result: Option<Result>,
    committed_revision: Option<u64>,
    #[serde(default)]
    restore_account: Option<client::RestoreAccount>,
}
fn path(store: &Store, id: &str) -> AppResult<PathBuf> {
    let p = directory(store)?.join("operations");
    if !p.is_dir() {
        platform::private_directory(&p)?;
    }
    Ok(p.join(format!("{}.json", uuid(id)?)))
}
fn save(store: &Store, s: &Saved) -> AppResult<()> {
    platform::write_json(&path(store, &s.plan.plan_id)?, s)
}
fn load(store: &Store, id: &str) -> AppResult<Saved> {
    platform::read_json(&path(store, id)?)?.ok_or_else(|| "切换预览已失效，请重新检查。".into())
}
fn update(
    store: &Store,
    id: &str,
    stage: &str,
    text: &str,
    completed: usize,
    total: usize,
) -> AppResult<()> {
    platform::write_json(
        &path(store, id)?.with_extension("progress.json"),
        &Progress {
            stage: stage.into(),
            text: text.into(),
            completed,
            total,
        },
    )
}
pub fn progress(store: &Store, id: String) -> AppResult<Progress> {
    Ok(
        platform::read_json(&path(store, &id)?.with_extension("progress.json"))?.unwrap_or(
            Progress {
                stage: "ready".into(),
                text: "等待确认".into(),
                completed: 0,
                total: 0,
            },
        ),
    )
}
fn call(store: &Store, id: &str, action: &str, data: Value) -> AppResult<Value> {
    if action == "quit" {
        let r = owner::record(store, id)?;
        if r.process_identity_version >= 1 && r.exit_control_version < 3
            && r.native.as_ref().is_some_and(Process::matches)
        {
            return super::control::call(store, &r, &r.current_sid, "quit");
        }
    }
    let mut value = rpc(store, id, action, data, Duration::from_secs(12))?;
    if action == "inspect" && value["checkedRestore"] == true {
        let r: Record = serde_json::from_value(value["record"].clone())
            .map_err(|_| "窗口状态无效。")?;
        if r.process_identity_version >= 1 && r.exit_control_version < 3
            && r.native.as_ref().is_some_and(Process::matches)
        {
            let old_gate = format!("Claude Code {} 尚未验证自动退出，请先手动退出。", r.version);
            if let Some(blockers) = value["blockers"].as_array_mut() {
                // Re-evaluate only the known legacy readiness checks in the
                // new helper. Keep unrelated/unknown blockers intact.
                blockers.retain(|b| b.as_str().is_none_or(|s| {
                    if r.exit_control_version == 2 {
                        return s != "输入框显示文字，可能是草稿或灰色建议；请定位检查，处理后会自动复查。";
                    }
                    s != old_gate && ![
                    "窗口正在工作或有弹窗，请先回到空闲输入框。",
                    "窗口在 Vim 普通模式，请先切回插入模式。",
                    "终端有草稿、正在输入或界面未就绪，请先处理后重试。",
                    "终端输入区域无法核对，未发送退出命令。",
                ].contains(&s)
                }));
                if blockers.is_empty() {
                    if let Err(error) = super::control::call(store, &r, &r.current_sid, "inspect") {
                        blockers.push(error.into());
                    }
                }
            }
        }
    }
    Ok(value)
}
fn snapshot(manager: &Manager, id: Option<String>) -> AppResult<Saved> {
    let store = &manager.store;
    let db = store.read()?;
    if let Some(id) = id.as_deref() {
        db.route(id)?;
    }
    let mut items = Vec::new();
    let mut blockers = Vec::new();
    let mut window_blockers = std::collections::BTreeMap::<String, Vec<String>>::new();
    let listed: Vec<Window> = owner::list_windows(store)?.into_iter()
        .map(serde_json::from_value).collect::<std::result::Result<_,_>>()
        .map_err(|_| "窗口状态无效。")?;
    let mut legacy = Vec::new();
    for r in owner::all(store)? {
        // A helper can be hot-upgraded, but a running legacy owner still uses
        // path equality in destructive lifecycle operations. Never let that
        // owner mistake a renamed live native process for an exited one.
        if r.process_identity_version == 0 && r.native.as_ref().is_some_and(|n| {
            Process::open(n.pid, false).is_ok_and(|p| p.alive()
                && p.identity.same_process(n)
                && !crate::session_win::same_image(&p.identity.exe, &n.exe))
        }) {
            window_blockers.entry(r.id.clone()).or_default().push("Claude 已自动更新，此旧版终端可继续工作；本轮不自动换号，请结束工作后用新版启动器恢复。".into());
            legacy.push(r);
            continue;
        }
        match call(store, &r.id, "inspect", serde_json::json!({})) {
            Ok(v) => {
                let mut window: Window = serde_json::from_value(v["window"].clone())
                    .map_err(|_| "终端返回的数据无效。")?;
                if let Some(current) = listed.iter().find(|w| w.window_id == r.id) {
                    window.name = current.name.clone();
                    window.can_focus = current.can_focus;
                }
                let reasons = window_blockers.entry(r.id.clone()).or_default();
                if v["checkedRestore"] != true {
                    reasons.push("此终端由旧版启动器打开，请先退出，用新版启动器恢复一次原对话。".into());
                }
                let target: Target =
                    serde_json::from_value(v["target"].clone()).map_err(|_| "恢复目标无效。")?;
                for e in v["blockers"].as_array().unwrap_or(&vec![]) {
                    if let Some(e) = e.as_str() {
                        reasons.push(e.into())
                    }
                }
                let runtime = match super::runtime::read(store, &r, &target.sid) {
                    Ok(settings) => {
                        window.model = Some(settings.model_name.clone());
                        window.effort = settings.effort.clone();
                        Some(settings)
                    }
                    Err(e) => { reasons.push(e); None }
                };
                items.push(Item {
                    window,
                    target,
                    runtime,
                    owner: serde_json::from_value(v["record"].clone())
                        .map_err(|_| "窗口状态无效。")?,
                });
            }
            Err(e) => window_blockers.entry(r.id.clone()).or_default().push(e),
        }
    }
    items.sort_by(|a, b| a.window.window_id.cmp(&b.window.window_id));
    let home = conflicts::home()?;
    let cli = client::executable()?
        .canonicalize()
        .map_err(|_| "无法识别 Claude 程序。")?;
    for row in owner::live_rows(&home)? {
        let represented = items.iter().any(|i| {
            i.target.chain.contains(&row.session_id)
                && crate::session_win::same_image(&i.owner.cli, &cli)
        }) || legacy.iter().any(|r| r.native.as_ref().is_some_and(|n|
            n.pid == row.pid && n.born == row.proc_start && Process::matches(n)));
        if represented {
            continue;
        }
        if owner::is_cli_session(&row, &cli) {
            blockers.push(format!(
                "PID {} 的 Claude 会话不由新版启动器管理，请先在该窗口退出；不会替它结束进程。",
                row.pid
            ));
        }
    }
    let mut ids = std::collections::BTreeSet::new();
    for item in &items {
        if !ids.insert(&item.target.sid) {
            blockers.push("多个窗口正在使用同一个对话，请先保留一个窗口，再切换。".into());
        }
    }
    let mut windows: Vec<_> = items.iter().map(|i| i.window.clone()).collect();
    for window in listed {
        if !windows.iter().any(|w| w.window_id == window.window_id) {
            windows.push(window);
        }
    }
    for window in &mut windows {
        window.blockers = window_blockers.remove(&window.window_id).unwrap_or_default();
    }
    // A window can disappear between enumeration and inspection. Keep its
    // unresolved failure visible; a missing display row is not permission to
    // omit a failed lifecycle check.
    for (id, reasons) in window_blockers {
        blockers.extend(reasons.into_iter().map(|e| format!("终端 {id}：{e}")));
    }
    let global_blockers = blockers.clone();
    for window in &windows {
        blockers.extend(window.blockers.iter().map(|e| format!("{}（{}）：{e}", window.name, window.session_id.chars().take(8).collect::<String>())));
    }
    let plan = Plan {
        plan_id: uuid::Uuid::new_v4().to_string(),
        created_at: now(),
        supported: true,
        // Include blocked windows too. They did not disappear merely because
        // this operation cannot safely restart their old owner implementation.
        windows,
        blockers,
        global_blockers,
    };
    Ok(Saved {
        plan,
        target_id: id,
        revision: db.revision,
        items,
        permissions: Permissions::Preserve,
        result: None,
        committed_revision: None,
        restore_account: None,
    })
}
pub fn preview(manager: &Manager, id: Option<String>) -> AppResult<Plan> {
    let saved = snapshot(manager, id)?;
    save(&manager.store, &saved)?;
    Ok(saved.plan)
}
pub fn refresh(manager: &Manager, id: String) -> AppResult<Plan> {
    // Reuse one preview file. Serialize with execute so a late poll can never
    // overwrite an in-progress/finished operation or replay it as a new plan.
    let _gate = platform::lock(&directory(&manager.store)?.join("operation.lock"), Duration::from_millis(1))?;
    let old = load(&manager.store, &id)?;
    if old.result.is_some() { return Err("这次切换已经执行。".into()); }
    let mut fresh = snapshot(manager, old.target_id)?;
    fresh.plan.plan_id = old.plan.plan_id;
    save(&manager.store, &fresh)?;
    Ok(fresh.plan)
}
fn native(cli: &Path, args: &[&str]) -> AppResult<platform::Capture> {
    let mut command = Command::new(cli);
    command.args(args);
    platform::capture(command, Duration::from_secs(25))
}
fn wait(mut check: impl FnMut() -> AppResult<bool>, until: Instant) -> AppResult<()> {
    loop {
        if check()? {
            return Ok(());
        }
        if Instant::now() > until {
            return Err("会话没有按时退出或恢复，请查看对应终端。".into());
        }
        thread::sleep(Duration::from_millis(80));
    }
}
fn ready(store: &Store, item: &Item, until: Instant) -> AppResult<()> {
    wait(
        || {
            let r = owner::record(store, &item.window.window_id)?;
            if let Some(n) = &r.native {
                let registered = owner::live_rows(&r.home)?.iter().any(|row| {
                    row.pid == n.pid
                        && row.proc_start == n.born
                        && (item.target.empty || row.session_id == item.target.sid)
                        // Native writes its registration before the owner's
                        // next refresh. Do not issue a restore ticket with the
                        // owner's still-empty SID during that short interval.
                        && row.session_id == r.current_sid
                        && !r.current_sid.is_empty()
                });
                if registered {
                    return Ok(true);
                }
                if r.phase == "exited" || r.phase == "closed" {
                    return Err("Claude 启动后已退出，对话尚未恢复。".into());
                }
            }
            Ok(false)
        },
        until,
    )
}
fn restore(store: &Store, saved: &Saved, item: &Item, mode: &Permissions) -> AppResult<()> {
    let account = saved
        .restore_account
        .as_ref()
        .ok_or("没有已核实的原账号和固定出口，该窗口未启动。")?;
    let existing = owner::record(store, &item.window.window_id)?;
    let already_running = existing.native.as_ref().is_some_and(Process::matches);
    if already_running {
        // A previous restore can have opened the conversation successfully but
        // timed out verifying its settings. Retry that same armed process only;
        // never spawn a duplicate or take over another conversation.
        if !existing.owner.same_process(&item.owner.owner)
            || existing.nonce != item.owner.nonce
            || existing.operation.as_deref() != Some(saved.plan.plan_id.as_str())
            || existing.native.as_ref().is_some_and(|n| item.owner.native.as_ref().is_some_and(|old| n.same_process(old)))
            || (!item.target.empty && existing.current_sid != item.target.sid)
        { return Err("此窗口已有其他活跃会话，未重复启动或修改其设置。".into()); }
    } else {
      rpc(
        store,
        &item.window.window_id,
        "resumeChecked",
        serde_json::json!({"target":item.target,"permissionMode":mode,"account":account,
            "runtime":item.runtime,
            "operation":saved.plan.plan_id,"revision":saved.committed_revision.unwrap_or(saved.revision)}),
        Duration::from_secs(60),
      )?;
    }
    ready(store, item, Instant::now() + Duration::from_secs(30))?;
    let settings = item.runtime.as_ref().ok_or("缺少原模型和思考强度，未宣称恢复完成。")?;
    let r = owner::record(store, &item.window.window_id)?;
    if r.resume_state_version == 0 || already_running {
        super::control::restore_runtime(store, &r, &r.current_sid, settings)?;
    }
    wait(|| {
        let r = owner::record(store, &item.window.window_id)?;
        Ok(super::runtime::read(store, &r, &r.current_sid).is_ok_and(|actual| actual.model == settings.model && actual.effort == settings.effort))
    }, Instant::now() + Duration::from_secs(15))
        .map_err(|_| "对话已打开，但原模型或思考强度尚未核对成功，请定位此终端检查。".to_owned())?;
    call(
        store,
        &item.window.window_id,
        "disarm",
        serde_json::json!({}),
    )?;
    Ok(())
}
fn failed(item: &Item, error: String) -> Failed {
    Failed {
        window_id: item.window.window_id.clone(),
        cwd: item.target.cwd.display().to_string(),
        session_id: item.target.sid.clone(),
        error,
    }
}
pub fn execute(manager: &Manager, id: String, permissions: Permissions) -> AppResult<Result> {
    let store = &manager.store;
    let _operation = mutation_guard(store)?;
    let mut saved = load(store, &id)?;
    if saved.result.is_some() {
        return Err("这次切换已经执行，请重新检查窗口。".into());
    }
    let fresh = snapshot(manager, saved.target_id.clone())?;
    if fresh.revision != saved.revision
        || fresh.items.len() != saved.items.len()
        || fresh.items.iter().zip(&saved.items).any(|(a, b)| {
            a.owner.owner != b.owner.owner
                || a.target.sid != b.target.sid
                || a.owner.native != b.owner.native
                || a.runtime != b.runtime
        })
    {
        return Err("窗口、账号或恢复设置有变化，请重新检查。".into());
    }
    if !fresh.plan.blockers.is_empty() {
        return Err(fresh.plan.blockers.join("\n"));
    }
    saved.permissions = permissions;
    saved.items = fresh.items;
    let total = saved.items.len();
    update(store, &id, "checking", "检查目标账号出口", 0, total)?;
    // Check the new route before closing any window. Never send a model probe here.
    if let Some(target) = saved.target_id.as_deref() {
        let db = store.read()?;
        let (proxy, ip) = db.route(target)?;
        if let Some(a) = db.ordinary(target) {
            crate::ordinary::verify_slot(store, a)?;
        }
        conflicts::require_clean(proxy, None)?;
        let check = network::network(&Curl, proxy, ip);
        if !check.ok {
            return Err(check
                .error
                .unwrap_or_else(|| "出口检查失败，原窗口保持不变。".into()));
        }
    } else {
        let home = conflicts::home()?;
        let proxy = client::normal_proxy(&home)?;
        conflicts::require_clean(&proxy, None)?;
        let identity = crate::normal::identity(&home)?;
        let expected = identity
            .cache_key()
            .and_then(|k| {
                store
                    .normal_checks()
                    .ok()?
                    .get(&k)
                    .map(|r| r.expected_ip.clone())
            })
            .unwrap_or_default();
        let check = network::bind_network(&Curl, &proxy, &expected);
        if !check.ok {
            return Err(check
                .error
                .unwrap_or_else(|| "普通登录出口检查失败。".into()));
        }
        let _lock = store.lock()?;
        crate::normal::remember_exit(store, &home, check.ip.as_deref())?;
    }
    let target_account = if total > 0 {
        let _lock = store.lock()?;
        let db = store.read()?;
        if db.revision != saved.revision {
            return Err("账号有变化，请重新检查。".into());
        }
        // A failed rollback must never start an unverified old identity. It is
        // still possible to switch away from an account that can no longer log in.
        saved.restore_account =
            client::RestoreAccount::capture(store, &db, db.current.as_deref()).ok();
        Some(client::RestoreAccount::capture(
            store,
            &db,
            saved.target_id.as_deref(),
        )?)
    } else {
        None
    };
    let mut armed = Vec::new();
    for item in &saved.items {
        if let Err(e) = call(
            store,
            &item.window.window_id,
            "arm",
            serde_json::json!({"operation":id,"checkedRestore":true}),
        ) {
            for window in armed {
                let _ = call(store, window, "disarm", serde_json::json!({}));
            }
            return Err(e);
        }
        armed.push(item.window.window_id.as_str());
    }
    save(store, &saved)?;
    let stop = (|| -> AppResult<()> {
        for (i, item) in saved.items.iter().enumerate() {
            let r = owner::record(store, &item.window.window_id)?;
            if Some(super::runtime::read(store, &r, &item.target.sid)?) != item.runtime {
                return Err(format!("{} 的模型或思考强度刚刚发生变化，请重新检查。", item.window.name));
            }
            update(
                store,
                &id,
                "stopping",
                "正在退出终端、停止后台任务",
                i,
                total,
            )?;
            call(store, &item.window.window_id, "quit", serde_json::json!({}))?;
        }
        for item in &saved.items {
            wait(
                || {
                    let r = owner::record(store, &item.window.window_id)?;
                    if r.native.as_ref().is_some_and(Process::matches) {
                        return Ok(false);
                    }
                    match r.exit_code {
                        Some(0) => Ok(true),
                        Some(_) => Err("Claude 非正常退出，账号未切换，请检查原终端。".into()),
                        None => Ok(false),
                    }
                },
                Instant::now() + Duration::from_secs(25),
            )?;
        }
        let home = conflicts::home()?;
        let rows = owner::live_rows(&home)?;
        for row in rows
            .iter()
            .filter(|r| ["bg", "daemon-worker"].contains(&r.kind.as_str()))
        {
            if !saved
                .items
                .iter()
                .any(|i| i.target.chain.contains(&row.session_id))
            {
                return Err("出现新的未受管后台会话，账号未切换。".into());
            }
            let job = row.job_id.as_deref().unwrap_or(&row.session_id);
            let result = native(&client::executable()?, &["stop", job])?;
            if !result.success {
                return Err("后台会话未能正常停止，账号未切换。".into());
            }
        }
        if !saved.items.is_empty() {
            // All live background SIDs have been checked above. Native on-demand
            // supervisors require the explicit --any confirmation even after
            // their last owned worker has stopped.
            if owner::live_rows(&home)?.iter().any(|r| {
                ["bg", "daemon-worker"].contains(&r.kind.as_str())
                    && !saved
                        .items
                        .iter()
                        .any(|i| i.target.chain.contains(&r.session_id))
            }) {
                return Err("出现未受管后台会话，未停止后台服务或切换账号。".into());
            }
            let result = native(&client::executable()?, &["daemon", "stop", "--any"])?;
            let text = String::from_utf8_lossy(&result.stdout);
            if !result.success && !text.contains("not running") {
                return Err("后台服务尚未停止，账号未切换。".into());
            }
        }
        for item in &saved.items {
            wait(
                || {
                    Ok(!owner::live_rows(&item.owner.home)?
                        .iter()
                        .any(|r| item.target.chain.contains(&r.session_id)))
                },
                Instant::now() + Duration::from_secs(20),
            )?;
            call(
                store,
                &item.window.window_id,
                "clean",
                serde_json::json!({}),
            )?;
        }
        if !saved.items.is_empty() {
            let status = native(&client::executable()?, &["daemon", "status"])?;
            if !String::from_utf8_lossy(&status.stdout).contains("not running") {
                return Err("后台服务仍在运行，账号未切换。".into());
            }
        }
        // Recheck without `agents`, which could restart a stopped daemon.
        for row in owner::live_rows(&home)? {
            if ["bg", "daemon-worker"].contains(&row.kind.as_str())
                || saved
                    .items
                    .iter()
                    .any(|i| i.target.chain.contains(&row.session_id))
            {
                return Err("检测到仍在运行的旧会话，账号未切换。".into());
            }
        }
        for item in &saved.items {
            if !owner::record(store, &item.window.window_id)?
                .job_pids
                .is_empty()
            {
                return Err("窗口还有任务没有结束，账号未切换。".into());
            }
        }
        Ok(())
    })();
    let mut result = Result {
        operation_id: id.clone(),
        switched: false,
        switched_at: None,
        restored: 0,
        failed: vec![],
        errors: vec![],
    };
    if let Err(e) = stop {
        result.errors.push(e);
        update(
            store,
            &id,
            "restoring",
            "未切换账号，恢复已退出的窗口",
            0,
            total,
        )?;
        for item in &saved.items {
            let r = owner::record(store, &item.window.window_id)?;
            if r.native.as_ref().is_none_or(|n| !Process::matches(n)) {
                // Native may have exited but left a tool child. Clean the owned
                // jobs before restoring A, just as on the successful path.
                let recovered = call(
                    store,
                    &item.window.window_id,
                    "clean",
                    serde_json::json!({}),
                )
                .and_then(|_| restore(store, &saved, item, &Permissions::Preserve));
                match recovered {
                    Ok(()) => result.restored += 1,
                    Err(e) => result.failed.push(failed(item, e)),
                }
            } else {
                let _ = call(
                    store,
                    &item.window.window_id,
                    "disarm",
                    serde_json::json!({}),
                );
            }
        }
    } else {
        update(
            store,
            &id,
            "switching",
            "旧会话和任务已停止，正在切换账号",
            0,
            total,
        )?;
        // This uses the existing transactional account selection. It does not write
        // shared /login credentials or alter any Desktop / third-party API process.
        let selected = if let Some(target) = saved.target_id.as_deref() {
            manager.switch(target, &Curl, &UserEnvironment)
        } else {
            manager.normal(&UserEnvironment)
        };
        match selected {
            Err(e) => {
                result.errors.push(e);
                for item in &saved.items {
                    match restore(store, &saved, item, &Permissions::Preserve) {
                        Ok(()) => result.restored += 1,
                        Err(e) => result.failed.push(failed(item, e)),
                    }
                }
            }
            Ok(s) => {
                result.switched = true;
                result.switched_at = Some(s.switched_at);
                saved.committed_revision = Some(store.read()?.revision);
                saved.restore_account = target_account;
                save(store, &saved)?;
                let plan = &saved;
                for group in plan.items.chunks(3) {
                    update(
                        store,
                        &id,
                        "restoring",
                        "正在各窗口分别核对账号和出口、恢复原对话",
                        result.restored + result.failed.len(),
                        total,
                    )?;
                    thread::scope(|scope| {
                        let pending: Vec<_> = group
                            .iter()
                            .map(|item| {
                                let task = thread::Builder::new().spawn_scoped(scope, move || {
                                    restore(store, plan, item, &plan.permissions)
                                });
                                (item, task)
                            })
                            .collect();
                        for (item, task) in pending {
                            let outcome = task
                                .map_err(|_| "无法派发该窗口的恢复任务。".to_owned())
                                .and_then(|task| {
                                    task.join().map_err(|_| "窗口恢复意外中止。".to_owned())?
                                });
                            match outcome {
                                Ok(()) => result.restored += 1,
                                Err(e) => result.failed.push(failed(item, e)),
                            }
                        }
                    });
                }
            }
        }
    }
    saved.result = Some(result.clone());
    save(store, &saved)?;
    update(
        store,
        &id,
        "done",
        if result.failed.is_empty() {
            "操作完成"
        } else {
            "部分窗口需要重试恢复"
        },
        result.restored,
        total,
    )?;
    Ok(result)
}
pub fn retry(manager: &Manager, operation: String, window: String) -> AppResult<Result> {
    let store = &manager.store;
    let _gate = mutation_guard(store)?;
    let mut saved = load(store, &operation)?;
    let mut result = saved.result.clone().ok_or("该切换还没有完成。")?;
    if !result.failed.iter().any(|f| f.window_id == window) {
        return Err("这个窗口没有待重试的恢复操作。".into());
    }
    if let Some(revision) = saved.committed_revision {
        if store.read()?.revision != revision {
            return Err("当前账号已变化，请重新检查窗口后恢复。".into());
        }
    }
    if let Some(previous) = &saved.restore_account {
        // A later retry can use a renewed credential only if the native identity
        // and registered route still match the operation. The owner probes again.
        let _lock = store.lock()?;
        let db = store.read()?;
        if let Ok(current) = client::RestoreAccount::capture(store, &db, db.current.as_deref()) {
            if !previous.same_target(&current) {
                return Err("恢复目标账号或出口已变化，请重新检查。".into());
            }
            saved.restore_account = Some(current);
        }
    }
    let item = saved
        .items
        .iter()
        .find(|i| i.window.window_id == window)
        .ok_or("找不到原窗口。")?;
    let mode = if result.switched {
        saved.permissions.clone()
    } else {
        Permissions::Preserve
    };
    match restore(store, &saved, item, &mode) {
        Ok(()) => {
            result.failed.retain(|f| f.window_id != window);
            result.restored += 1;
        }
        Err(e) => {
            if let Some(f) = result.failed.iter_mut().find(|f| f.window_id == window) {
                f.error = e
            }
        }
    }
    saved.result = Some(result.clone());
    save(store, &saved)?;
    Ok(result)
}
