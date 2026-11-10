//! Rolling upgrade of exit checks without restarting the user's existing owner.
//! No account writes, process termination or desktop-wide input in this helper.
use super::{owner::{self, Record}, *};
use crate::session_win::{self as win, Identity, Process};
use std::{io::Read, process::Command, thread};

#[derive(Serialize, Deserialize)]
struct Ticket {
    owner: Identity,
    native: Identity,
    nonce: String,
    sid: String,
    operation: Option<String>,
    expires_at: i64,
    #[serde(default)]
    runtime: Option<super::runtime::Settings>,
}

fn verify(store: &Store, id: &str, ticket: &Ticket, quit: bool) -> AppResult<Record> {
    verify_state(store, id, ticket, quit, true)
}
fn verify_state(store: &Store, id: &str, ticket: &Ticket, quit: bool, require_idle: bool) -> AppResult<Record> {
    let r = owner::record(store, id)?;
    let fresh = chrono::DateTime::parse_from_rfc3339(&r.updated_at).is_ok_and(|at| {
        (0..=5000).contains(&chrono::Utc::now().signed_duration_since(at).num_milliseconds())
    });
    if ticket.expires_at <= chrono::Utc::now().timestamp_millis()
        || r.process_identity_version < 1 || !fresh || r.phase != "running"
        || r.nonce != ticket.nonce || !r.owner.same_process(&ticket.owner)
        || !r.native.as_ref().is_some_and(|n| n.same_process(&ticket.native))
        || !Process::matches(&ticket.owner) || !Process::matches(&ticket.native)
        || !r.job_pids.contains(&ticket.native.pid)
        || !win::ancestry(ticket.native.pid)?.contains(&ticket.owner.pid)
        || r.operation != ticket.operation
        || (quit && ticket.operation.as_deref().is_none_or(|op| uuid(op).is_err()))
    {
        return Err("终端、任务归属或切换操作已变化，未发送退出命令。".into());
    }
    let row = owner::live_rows(&r.home)?.into_iter().find(|row| {
        row.pid == ticket.native.pid && row.proc_start == ticket.native.born
            && row.session_id == ticket.sid && row.kind == "interactive"
    }).ok_or("前台会话登记尚未就绪。")?;
    if require_idle && (!["idle", "shell"].contains(&row.status.as_str())
        || row.waiting_for.as_ref().is_some_and(|v| !v.is_null())
        || r.vim.as_deref() == Some("NORMAL"))
    {
        return Err("窗口正在工作、有弹窗或处于 Vim 普通模式，请先回到空闲输入框。".into());
    }
    Ok(r)
}

pub(super) fn call(store: &Store, r: &Record, sid: &str, action: &str) -> AppResult<Value> {
    call_with_runtime(store, r, sid, action, None)
}
pub(super) fn restore_runtime(store: &Store, r: &Record, sid: &str, settings: &super::runtime::Settings) -> AppResult<Value> {
    call_with_runtime(store, r, sid, "restoreRuntime", Some(settings.clone()))
}
fn call_with_runtime(store: &Store, r: &Record, sid: &str, action: &str, runtime: Option<super::runtime::Settings>) -> AppResult<Value> {
    let native = r.native.clone().ok_or("窗口没有前台会话。")?;
    let ticket = Ticket {
        owner:r.owner.clone(), native, nonce:r.nonce.clone(), sid:uuid(sid)?,
        operation:r.operation.clone(), expires_at:chrono::Utc::now().timestamp_millis()+45_000, runtime,
    };
    let helper = std::env::current_exe().map_err(|_| "无法定位终端控制程序。")?
        .with_file_name("cam-launcher.exe");
    let mut command = Command::new(helper);
    command.arg("--cam-window-control").arg(&store.root).arg(&r.id).arg(action);
    let bytes = serde_json::to_vec(&ticket).map_err(|_| "无法传递终端控制参数。")?;
    let result = platform::capture_with_started(command, Duration::from_secs(47), Some(&bytes), |pid| {
        if action == "focus" { win::allow_foreground(pid); }
    })?;
    let value: Value = serde_json::from_slice(&result.stdout).map_err(|_| "终端控制程序未返回有效结果。")?;
    if let Some(error) = value["error"].as_str() { return Err(error.into()); }
    if !result.success || value["ok"] != true { return Err("终端控制未完成。".into()); }
    Ok(value)
}

pub fn helper(args: &[OsString]) -> AppResult<Value> {
    if args.len() != 3 { return Err("终端控制参数无效。".into()); }
    let store = Store { root:PathBuf::from(&args[0]) };
    let id = uuid(&args[1].to_string_lossy())?;
    let action = args[2].to_string_lossy();
    if !["inspect", "quit", "restoreRuntime", "focus"].contains(&action.as_ref()) { return Err("未知终端控制操作。".into()); }
    let mut bytes = Vec::new();
    std::io::stdin().take(16_385).read_to_end(&mut bytes).map_err(|_| "无法读取终端控制参数。")?;
    if bytes.len() > 16_384 { return Err("终端控制参数过大。".into()); }
    let ticket: Ticket = serde_json::from_slice(&bytes).map_err(|_| "终端控制参数无效。")?;
    let quit = action == "quit";
    let mutate = quit || action == "restoreRuntime";
    let require_idle = action != "restoreRuntime" && action != "focus";
    verify_state(&store, &id, &ticket, mutate, false)?;
    let _console = win::AttachedConsole::open(&ticket.owner, &ticket.native)?;
    if require_idle && win::screen()?.quota_wait() {
        return Err("Claude 正在等待额度重置并准备自动续跑；请定位后按 Esc 取消等待，再回来切换。".into());
    }
    let r = verify_state(&store, &id, &ticket, mutate, require_idle)?;
    if action == "focus" {
        win::focus_console_for_host(&r.terminal_host)?;
        return Ok(serde_json::json!({"ok":true}));
    }
    if action == "restoreRuntime" { return restore_settings(&store, &id, &ticket); }
    win::check_empty_prompt(&win::screen()?, &r.home)?;
    if !quit { return Ok(serde_json::json!({"ok":true})); }

    verify(&store, &id, &ticket, true)?;
    win::input("/quit")?;
    thread::sleep(Duration::from_millis(180));
    verify(&store, &id, &ticket, true)?;
    win::screen()?.require_command("/quit")?;
    win::input("\r")?;
    // Only the exact native exit dialog may be acknowledged, while the same
    // held foreground and operation still exist. No force-kill on timeout.
    let deadline = Instant::now()+Duration::from_secs(25);
    while Process::matches(&ticket.native) {
        if Instant::now() >= deadline { return Err("Claude 尚未正常退出，账号未切换，请查看原终端。".into()); }
        let r = owner::record(&store, &id)?;
        if !r.owner.same_process(&ticket.owner) || !Process::matches(&ticket.owner)
            || r.operation != ticket.operation
            || !r.native.as_ref().is_some_and(|n| n.same_process(&ticket.native))
        { return Err("退出期间窗口或切换操作已变化。".into()); }
        if win::screen().is_ok_and(|s| s.exit_dialog()) { win::input("\r")?; }
        thread::sleep(Duration::from_millis(60));
    }
    Ok(serde_json::json!({"ok":true}))
}

fn wait_for(store: &Store, id: &str, ticket: &Ticket, confirmation: Option<(&str, &str)>, check: impl Fn(&super::runtime::Settings) -> bool) -> AppResult<()> {
    let until = Instant::now() + Duration::from_secs(12);
    let mut confirmed = false;
    loop {
        // Startup and a completed slash command briefly report "starting" or
        // "busy". Keep checking ownership, but wait for idle before any input.
        let r = verify_state(store, id, ticket, true, false)?;
        if !confirmed && confirmation.is_some_and(|(kind,target)| win::screen().is_ok_and(|s| s.cache_switch_dialog(kind,target))) {
            // Only Claude's exact cache-cost confirmation for the captured
            // original model/effort; never acknowledge a hook permission dialog.
            win::input("\r")?;
            confirmed = true;
        }
        if super::runtime::read(store, &r, &ticket.sid).is_ok_and(|s| check(&s))
            && verify(store, id, ticket, true).is_ok()
            && win::screen().is_ok_and(|s| win::check_empty_prompt(&s, &r.home).is_ok()) { return Ok(()); }
        if Instant::now() >= until { return Err("对话已打开，但模型或思考强度尚未确认；请定位原终端检查。".into()); }
        thread::sleep(Duration::from_millis(80));
    }
}
fn command(store: &Store, id: &str, ticket: &Ticket, text: &str) -> AppResult<()> {
    let r = verify(store, id, ticket, true)?;
    win::check_empty_prompt(&win::screen()?, &r.home)?;
    verify(store, id, ticket, true)?;
    win::input(text)?;
    thread::sleep(Duration::from_millis(180));
    verify(store, id, ticket, true)?;
    win::screen()?.require_command(text)?;
    win::input("\r")
}
fn restore_settings(store: &Store, id: &str, ticket: &Ticket) -> AppResult<Value> {
    let wanted = ticket.runtime.as_ref().ok_or("缺少原模型和思考强度。")?;
    wanted.validate()?;
    wait_for(store,id,ticket,None,|_| true)?;
    let r = verify(store,id,ticket,true)?;
    let current = super::runtime::read(store,&r,&ticket.sid)?;
    if current.model != wanted.model {
        // Direct /model <id> persists a shared default in current Claude Code.
        // Old live owners cannot accept new launch flags: use the native
        // picker's explicit session-only action, and verify the resulting ID.
        command(store,id,ticket,"/model")?;
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            verify_state(store,id,ticket,true,false)?;
            if win::screen().is_ok_and(|s| s.lines.iter().any(|line| line.trim() == "Select model")) { break; }
            if Instant::now() >= until { return Err("未识别到原生模型选择器，未继续输入。".into()); }
            thread::sleep(Duration::from_millis(80));
        }
        for _ in 0..20 {
            verify_state(store,id,ticket,true,false)?;
            let (at,target) = win::screen()?.model_picker(&wanted.model_name)?;
            if at == target { win::input("s")?; break; }
            win::key(if at < target { 0x28 } else { 0x26 })?;
            thread::sleep(Duration::from_millis(150));
        }
        wait_for(store,id,ticket,Some(("model",&wanted.model_name)),|s| s.model == wanted.model)?;
    }
    let r = verify(store,id,ticket,true)?;
    if super::runtime::read(store,&r,&ticket.sid)?.effort != wanted.effort {
        let effort = wanted.effort.as_deref().ok_or("目标模型的思考强度状态与原会话不符。")?;
        if effort == "max" {
            // Native /effort max is explicitly session-only; it never writes a
            // shared default used by other windows or Desktop.
            command(store,id,ticket,"/effort max")?;
        } else {
            // Typing /effort high would save a global per-model default. Use
            // the native slider's explicit session-only action instead.
            command(store,id,ticket,"/effort")?;
            let until = Instant::now() + Duration::from_secs(8);
            loop {
                verify_state(store,id,ticket,true,false)?;
                if win::screen().is_ok_and(|s| s.effort_picker().is_ok()) { break; }
                if Instant::now() >= until { return Err("未识别到原生思考强度选择器，未继续输入。".into()); }
                thread::sleep(Duration::from_millis(80));
            }
            for _ in 0..6 {
                verify_state(store,id,ticket,true,false)?;
                let (levels, at) = win::screen()?.effort_picker()?;
                let wanted = levels.iter().position(|level| level == effort).ok_or("当前模型不支持原思考强度。")?;
                if at == wanted { win::input("s")?; break; }
                win::key(if at < wanted { 0x27 } else { 0x25 })?;
                thread::sleep(Duration::from_millis(150));
            }
        }
    }
    wait_for(store,id,ticket,wanted.effort.as_deref().map(|e| ("effort",e)),|s| s.model == wanted.model && s.effort == wanted.effort)?;
    Ok(serde_json::json!({"ok":true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_requires_live_owned_idle_session_and_exact_armed_operation() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store { root:temp.path().join("store") };
        let home = temp.path().join("home");
        fs::create_dir_all(home.join(".claude/sessions")).unwrap();
        let job = win::Job::new().unwrap();
        let fixture = win::identity_tests::renamed_fixture(&job);
        let native = fixture.identity.clone();
        let id = uuid::Uuid::new_v4().to_string();
        let sid = uuid::Uuid::new_v4().to_string();
        let record = Record {
            id:id.clone(),nonce:uuid::Uuid::new_v4().to_string(),
            owner:Process::open(std::process::id(),false).unwrap().identity,
            cli:native.exe.clone(),home:home.clone(),cwd:temp.path().into(),args:vec![],
            native:Some(native.clone()),roots:vec![sid.clone()],current_sid:sid.clone(),
            account_name:"Synthetic".into(),binding:None,phase:"running".into(),exit_code:None,
            operation:None,jobs:vec![],job_pids:job.pids().unwrap(),vim:None,
            permission_mode:"default".into(),version:"2.1.319 (Claude Code)".into(),
            updated_at:crate::model::now(),terminal_host:"fixture".into(),
            process_identity_version:1,exit_control_version:0,resume_state_version:0,
        };
        let path = store.root.join("sessions-v1").join(&id).join("owner.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let row_path=home.join(format!(".claude/sessions/{}.json",native.pid));
        let row=serde_json::json!({"pid":native.pid,"procStart":native.born,"sessionId":sid,"kind":"interactive","status":"idle"});
        platform::write_json(&row_path,&row).unwrap();
        platform::write_json(&path,&record).unwrap();
        let mut ticket=Ticket { owner:record.owner.clone(),native:native.clone(),nonce:record.nonce.clone(),
            sid:sid.clone(),operation:None,expires_at:chrono::Utc::now().timestamp_millis()+30_000,runtime:None };
        assert!(verify(&store,&id,&ticket,false).is_ok());
        assert!(verify(&store,&id,&ticket,true).is_err());
        for mutation in 0..7 {
            let mut invalid=record.clone();
            match mutation {
                0=>invalid.owner.born="0".into(),
                1=>invalid.native.as_mut().unwrap().born="0".into(),
                2=>invalid.job_pids.clear(),
                3=>invalid.nonce="different".into(),
                4=>invalid.phase="closed".into(),
                5=>invalid.updated_at="2000-01-01T00:00:00Z".into(),
                _=>invalid.process_identity_version=0,
            }
            platform::write_json(&path,&invalid).unwrap();
            assert!(verify(&store,&id,&ticket,false).is_err(),"mutation {mutation}");
        }
        platform::write_json(&path,&record).unwrap();
        for status in ["busy","starting","waiting"] {
            let mut busy=row.clone(); busy["status"]=status.into();
            platform::write_json(&row_path,&busy).unwrap();
            assert!(verify(&store,&id,&ticket,false).is_err());
        }
        platform::write_json(&row_path,&row).unwrap();
        let mut armed=record.clone(); armed.operation=Some(uuid::Uuid::new_v4().to_string());
        platform::write_json(&path,&armed).unwrap();
        assert!(verify(&store,&id,&ticket,true).is_err());
        ticket.operation=armed.operation.clone();
        assert!(verify(&store,&id,&ticket,true).is_ok());
        ticket.expires_at=0;
        assert!(verify(&store,&id,&ticket,true).is_err());
    }
}
