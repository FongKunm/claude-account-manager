use super::*;
use crate::{
    client, conflicts,
    model::now,
    quota,
    registry::UserEnvironment,
    session_win::{self as win, Identity, Job, Process},
};
use std::{io::Read, process::Child, thread};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    pub id: String,
    pub nonce: String,
    pub owner: Identity,
    pub cli: PathBuf,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub native: Option<Identity>,
    pub roots: Vec<String>,
    pub current_sid: String,
    pub account_name: String,
    pub binding: Option<quota::Binding>,
    pub phase: String,
    pub exit_code: Option<i32>,
    pub operation: Option<String>,
    pub jobs: Vec<Identity>,
    pub job_pids: Vec<u32>,
    pub vim: Option<String>,
    pub permission_mode: String,
    pub version: String,
    pub updated_at: String,
    #[serde(default)]
    pub terminal_host: String,
    #[serde(default)]
    pub process_identity_version: u32,
    #[serde(default)]
    pub exit_control_version: u32,
    #[serde(default)]
    pub resume_state_version: u32,
}
impl Record {
    fn is_foreground(&self, process: &Identity) -> bool {
        self.native.as_ref().is_some_and(|n| n.same_process(process))
    }
    fn knows_process(&self, process: &Identity) -> bool {
        self.is_foreground(process) || self.jobs.iter().any(|p| p.same_process(process))
    }
    fn is_cli_image(&self, image: &Path) -> bool {
        win::same_image(image, &self.cli)
            || self.native.iter().chain(&self.jobs).any(|known| {
                Process::open(known.pid, false).is_ok_and(|p| p.alive()
                    && p.identity.same_process(known) && win::same_image(image, &p.identity.exe))
            })
    }
}
struct Owner {
    store: Store,
    record: Record,
    child: Option<Child>,
    front: Option<Job>,
    jobs: Vec<(Process, Job)>,
    exit_at: Option<Instant>,
    last_saved: Instant,
}
fn own_path(store: &Store, id: &str) -> AppResult<PathBuf> {
    Ok(directory(store)?.join(uuid(id)?))
}
pub(crate) fn record(store: &Store, id: &str) -> AppResult<Record> {
    platform::read_json(&own_path(store, id)?.join("owner.json"))?
        .ok_or_else(|| "终端记录不存在。".into())
}
pub(crate) fn all(store: &Store) -> AppResult<Vec<Record>> {
    let mut result = Vec::new();
    for e in fs::read_dir(directory(store)?).map_err(|_| "无法读取受管终端。")? {
        let path = e.map_err(|_| "无法读取受管终端。")?.path();
        if path.is_dir() {
            if let Some(r) = platform::read_json::<Record>(&path.join("owner.json"))? {
                if r.phase != "closed" && Process::matches(&r.owner) {
                    result.push(r)
                }
            }
        }
    }
    Ok(result)
}
pub fn list_windows(store: &Store) -> AppResult<Vec<Value>> {
    let mut result = Vec::new();
    for r in all(store)? {
        let front = r.native.as_ref().and_then(|p| foreground_registration(&r.home, p));
        let background = r.jobs.iter().any(Process::matches);
        let status = front.as_ref().map(|row| if row.waiting_for.is_some() { "waiting" } else { row.status.as_str() }).unwrap_or(&r.phase);
        let name = front.as_ref().and_then(|row| row.name.as_deref()).filter(|s| !s.is_empty())
            .or_else(|| r.cwd.file_name().and_then(|s| s.to_str())).unwrap_or("Claude Code");
        let can_focus = front.is_some() && r.phase == "running" && !r.terminal_host.is_empty();
        result.push(serde_json::json!({
            "windowId":r.id, "name":name, "cwd":front.as_ref().map(|s| s.cwd.clone()).filter(|s| !s.is_empty()).unwrap_or_else(||r.cwd.to_string_lossy().into_owned()),
            "sessionId":front.as_ref().map(|s|s.session_id.as_str()).unwrap_or(&r.current_sid), "pid":r.native.as_ref().map(|p|p.pid).unwrap_or(r.owner.pid),
            "kind":if front.is_none() && background {"background"} else {"foreground"},
            "permissionMode":r.permission_mode, "accountName":r.account_name, "hasBackgroundTasks":background || status == "shell",
            "status":status, "host":r.terminal_host, "canFocus":can_focus
        }));
    }
    result.sort_by(|a,b| a["windowId"].as_str().cmp(&b["windowId"].as_str()));
    Ok(result)
}

pub fn focus_window(store: &Store, id: &str) -> AppResult<()> {
    let r = record(store,id)?;
    if r.phase != "running" || r.terminal_host.is_empty() || !Process::matches(&r.owner)
        || !r.native.as_ref().is_some_and(Process::matches) {
        return Err("这个终端已关闭或当前启动器不支持定位。".into());
    }
    if r.operation.is_some() {
        // A partially restored session remains armed for safe retry. Older
        // owners reject focus while armed; a verified helper can locate that
        // exact live console without disarming it or sending keyboard input.
        let _gate = mutation_guard(store)?;
        super::control::call(store,&r,&r.current_sid,"focus")?;
        return Ok(());
    }
    win::allow_foreground(r.owner.pid);
    rpc(store,id,"focus",serde_json::json!({}),Duration::from_secs(8))?;
    Ok(())
}

// Poll only this foreground process. A native registration can be empty while
// Claude rewrites it. A missed observation keeps the last SID for display;
// inspect, quit, cleanup and resume still require a fresh, complete inventory.
fn foreground_registration(home: &Path, native: &Identity) -> Option<NativeSession> {
    let path = home
        .join(".claude/sessions")
        .join(format!("{}.json", native.pid));
    let row: NativeSession = platform::read_json(&path).ok()??;
    (row.pid == native.pid && row.proc_start == native.born && Process::matches(native))
        .then_some(row)
}
fn tool_registration(home: &Path, native: &Identity, sid: &str) -> Option<NativeSession> {
    // Native registrations are rewritten in place. Check only this process for
    // tool execution; a damaged unrelated file still blocks switching inventory.
    for delay in [0, 20, 50, 100, 200] {
        if delay != 0 {
            thread::sleep(Duration::from_millis(delay));
        }
        if let Some(row) = foreground_registration(home, native) {
            return (row.session_id == sid).then_some(row);
        }
    }
    None
}
pub(crate) fn live_rows(home: &Path) -> AppResult<Vec<NativeSession>> {
    let mut live: Vec<_> = rows(home)?
        .into_iter()
        .filter(|r| {
            Process::open(r.pid, false).is_ok_and(|p| p.alive() && p.identity.born == r.proc_start)
        })
        .collect();
    // /background records its PTY host before the interactive worker finishes
    // startup and writes sessions/<pid>.json. Keep that handoff represented
    // during this gap; otherwise the original terminal owner exits too early.
    // Deserialize only identity fields, never socket keys or provider settings.
    #[derive(Deserialize, Default)]
    struct Roster {
        proto: u32,
        #[serde(default)]
        workers: std::collections::BTreeMap<String, Worker>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Worker {
        pid: u32,
        #[serde(deserialize_with = "number_string")]
        proc_start: String,
        session_id: String,
    }
    if let Some(roster) = platform::read_json::<Roster>(&home.join(".claude/daemon/roster.json"))? {
        if roster.proto != 1 {
            return Err("后台登记格式尚未验证，请先手动停止后台会话。".into());
        }
        for (job, worker) in roster.workers {
            if live.iter().any(|r| r.session_id == worker.session_id)
                || uuid(&worker.session_id).is_err()
            {
                continue;
            }
            if Process::open(worker.pid, false)
                .is_ok_and(|p| p.alive() && p.identity.born == worker.proc_start)
            {
                live.push(NativeSession {
                    pid: worker.pid,
                    proc_start: worker.proc_start,
                    session_id: worker.session_id,
                    job_id: Some(job),
                    kind: "bg".into(),
                    status: "starting".into(),
                    ..Default::default()
                });
            }
        }
    }
    Ok(live)
}
// Conservative classification for account transitions, not process adoption.
pub(crate) fn is_cli_session(row: &NativeSession, cli: &Path) -> bool {
    ["bg", "daemon-worker"].contains(&row.kind.as_str())
        || Process::open(row.pid, false)
            .is_ok_and(|p| p.alive() && p.identity.born == row.proc_start
                && win::may_be_cli_image(&p.identity.exe, cli))
}
fn command_quote(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
    )
}
impl Owner {
    fn save(&mut self) -> AppResult<()> {
        self.last_saved = Instant::now();
        let mut pids = Vec::new();
        if let Some(job) = &self.front {
            pids.extend(job.pids()?)
        }
        for (_, job) in &self.jobs {
            pids.extend(job.pids()?)
        }
        pids.sort();
        pids.dedup();
        self.record.job_pids = pids;
        self.record.jobs = self.jobs.iter().map(|(p, _)| p.identity.clone()).collect();
        self.record.updated_at = now();
        platform::write_json(
            &own_path(&self.store, &self.record.id)?.join("owner.json"),
            &self.record,
        )
    }
    fn settings(&self) -> Value {
        let exe = std::env::current_exe().unwrap();
        let base = format!(
            "{} --cam-session-helper {} {} {}",
            command_quote(&exe),
            command_quote(&self.store.root),
            self.record.id,
            self.record.nonce
        );
        serde_json::json!({"statusLine":{"type":"command","command":format!("{base} status")},"hooks":{
         "PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":format!("{base} tool"),"timeout":20}]}],
         "SessionStart":[{"hooks":[{"type":"command","command":format!("{base} start"),"timeout":15}]}],
         "SessionEnd":[{"hooks":[{"type":"command","command":format!("{base} end"),"timeout":8}]}]
        }})
    }
    fn spawn(
        &mut self,
        args: Vec<OsString>,
        cwd: PathBuf,
        restore: Option<(&client::RestoreAccount, u64, i64)>,
    ) -> AppResult<()> {
        if let Some((account, revision, _)) = restore {
            account.check_window(&self.store, revision, &cwd)?;
        }
        let _lock = self.store.lock()?;
        self.store.recover(&UserEnvironment)?;
        let (mut command, binding, name) = client::managed_command(
            &self.store,
            &args,
            self.settings(),
            &cwd,
            restore.map(|(account, revision, _)| (account, revision)),
        )?;
        command.current_dir(&cwd);
        let job = Job::new()?;
        if restore.is_some_and(|(_, _, end)| chrono::Utc::now().timestamp_millis() >= end) {
            return Err("窗口恢复操作已过期，请重试。".into());
        }
        let (child, identity) = job.spawn(&mut command)?;
        self.record.native = Some(identity);
        self.record.binding = binding;
        self.record.account_name = name;
        self.record.cwd = cwd;
        self.record.phase = "running".into();
        self.record.exit_code = None;
        self.record.vim = None;
        self.record.current_sid.clear();
        self.record.roots.clear();
        self.record.args = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        self.record.permission_mode = if args.iter().any(|a| a == "--dangerously-skip-permissions")
        {
            "bypassPermissions".into()
        } else {
            args.windows(2)
                .find(|w| w[0] == "--permission-mode")
                .map(|w| w[1].to_string_lossy().into_owned())
                .unwrap_or_else(|| "default".into())
        };
        self.child = Some(child);
        self.front = Some(job);
        self.exit_at = None;
        self.save()
    }
    fn refresh(&mut self) -> AppResult<()> {
        if let Some(n) = &self.record.native {
            if let Some(r) = foreground_registration(&self.record.home, n) {
                self.record.current_sid = r.session_id.clone();
                if !self.record.roots.contains(&r.session_id) {
                    self.record.roots.push(r.session_id);
                }
                if !r.cwd.is_empty() {
                    self.record.cwd = r.cwd.into();
                }
            }
        }
        if self.record.phase == "running" || self.record.phase == "quitting" {
            if let Some(status) = self
                .child
                .as_mut()
                .map(Child::try_wait)
                .transpose()
                .map_err(|_| "无法读取 Claude 退出状态。")?
                .flatten()
            {
                self.record.phase = "exited".into();
                self.record.exit_code = Some(status.code().unwrap_or(1));
                self.exit_at = Some(Instant::now());
            }
        }
        Ok(())
    }
    fn accept_process(&mut self, source: &Identity, sid: &str, mode: &str) -> AppResult<Process> {
        let sid = uuid(sid)?;
        let process = Process::open(source.pid, true)?;
        let front = self.record.is_foreground(source);
        let owned = (front && self.front.as_ref().is_some_and(|j| j.contains(&process)))
            || self.jobs.iter().any(|(p, j)| p.identity.same_process(source) && j.contains(&process));
        if !process.identity.same_process(source) || !process.alive()
            || (!owned && !self.record.is_cli_image(&process.identity.exe))
        {
            return Err("进程身份核对失败。".into());
        }
        if front && !owned {
            return Err("原生 Claude 已不属于原窗口的受管任务。".into());
        }
        if !front {
            let row = tool_registration(&self.record.home, source, &sid)
                .ok_or("后台会话登记不匹配。")?;
            if row.kind != "bg" && row.kind != "daemon-worker" {
                return Err("此会话不属于原窗口的后台接续。".into());
            }
            let linked = self.record.roots.iter().any(|root| {
                follow(&self.record.home, root, &self.record.cwd)
                    .is_ok_and(|t| t.chain.contains(&sid))
            });
            if !linked {
                return Err("后台会话尚未与原窗口建立接续关系。".into());
            }
        } else if mode != "end" && mode != "start" {
            if tool_registration(&self.record.home, source, &sid).is_none() {
                return Err("会话已变化。".into());
            }
        }
        if front {
            self.record.current_sid = sid.clone();
            if !self.record.roots.contains(&sid) {
                self.record.roots.push(sid);
            }
        }
        Ok(process)
    }
    fn observe(&mut self, data: &Value) -> AppResult<Value> {
        if data.get("nonce").and_then(Value::as_str) != Some(&self.record.nonce) {
            return Err("终端绑定已失效。".into());
        }
        let source: Identity =
            serde_json::from_value(data["process"].clone()).map_err(|_| "缺少进程身份。")?;
        let sid = data["sid"].as_str().ok_or("缺少会话编号。")?;
        let mode = data["mode"].as_str().unwrap_or("");
        let process = self.accept_process(&source, sid, mode)?;
        if self.record.is_foreground(&source) {
            if let Some(permission) = data
                .pointer("/input/permission_mode")
                .and_then(Value::as_str)
                .filter(|p| {
                    [
                        "default",
                        "plan",
                        "acceptEdits",
                        "auto",
                        "dontAsk",
                        "bypassPermissions",
                    ]
                    .contains(p)
                })
            {
                self.record.permission_mode = permission.into();
            }
        }
        if ["tool", "start"].contains(&mode)
            && !self.record.is_foreground(&source)
            && !self
                .jobs
                .iter()
                .any(|(p, j)| p.identity.same_process(&source) && j.contains(&process))
        {
            let job = Job::new()?;
            job.assign(&process)?;
            self.jobs.push((process, job));
        }
        if mode == "status" {
            if self.record.is_foreground(&source) {
                self.record.vim = data
                    .pointer("/input/vim/mode")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            // A carried status command alone is not ownership. Require an already-held Job.
            let p = Process::open(source.pid, false)?;
            if !p.identity.same_process(&source) || !p.alive() {
                return Err("Claude 进程身份已变化。".into());
            }
            let owned = self.front.as_ref().is_some_and(|j| j.contains(&p))
                || self.jobs.iter().any(|(_, j)| j.contains(&p));
            // accept_process verified the PID/birth/image, live registration
            // and continuation chain. A worker must already be in a held Job,
            // and only the new launch context pins its native credential route.
            let verified_worker = self
                .record
                .binding
                .as_ref()
                .is_some_and(|b| b.context_generation.is_some())
                && self
                    .jobs
                    .iter()
                    .any(|(known, j)| known.identity.same_process(&source) && j.contains(&p));
            if owned && (self.record.is_foreground(&source) || verified_worker) {
                if let Some(binding) = &self.record.binding {
                    // Startup can renew credentials before native profile
                    // fetch. Confirm the fresh native identity in this generation.
                    crate::account_context::seal_normal(&self.store, binding)?;
                }
                // A first native profile may arrive after startup renewal. Bind
                // an unidentified window only inside its prepared generation.
                // Already identified windows never adopt a different account.
                if let Some(pending) = self.record.binding.as_ref().filter(|b| {
                    b.account_id.is_none()
                        && b.account_uuid.is_none()
                        && b.context_generation.is_some()
                }) {
                    if let Ok(mut bound) =
                        quota::Binding::selected(&self.store, &self.record.home, None)
                    {
                        bound.context_generation = pending.context_generation.clone();
                        if bound.current(&self.store) {
                            self.record.binding = Some(bound);
                        }
                    }
                }
                if let Some(binding) = &self.record.binding {
                    quota::observe(
                        &self.store,
                        binding,
                        quota::statusline(&data["input"]),
                        "claude-code",
                    )?;
                }
            }
        }
        self.save()?;
        Ok(serde_json::json!({"ok":true,"name":self.record.account_name}))
    }
    fn inspect(&mut self) -> AppResult<Value> {
        self.refresh()?;
        self.save()?;
        let target = if self.record.current_sid.is_empty() {
            return Err("窗口尚未完成启动，请先在终端处理登录或信任提示。".into());
        } else {
            follow(
                &self.record.home,
                &self.record.current_sid,
                &self.record.cwd,
            )?
        };
        let live = live_rows(&self.record.home)?;
        let relevant: Vec<_> = live
            .iter()
            .filter(|r| target.chain.contains(&r.session_id))
            .collect();
        let active_front = self.record.native.as_ref().is_some_and(Process::matches);
        let mut blockers = Vec::<String>::new();
        // Exit is gated by the live registration, prompt, input queue and owned
        // process below. A patch-version change alone is not a capability change.
        if active_front {
            let row = relevant
                .iter()
                .find(|r| Some(r.pid) == self.record.native.as_ref().map(|p| p.pid))
                .ok_or("前台会话登记尚未就绪。")?;
            let waiting = row.waiting_for.as_ref().is_some_and(|v| !v.is_null());
            let idle = ["idle", "shell"].contains(&row.status.as_str());
            let screen = win::screen();
            let quota_wait = screen.as_ref().is_ok_and(|s| s.quota_wait());
            if quota_wait { blockers.push("Claude 正在等待额度重置并准备自动续跑；请定位后按 Esc 取消等待，再回来切换。".into()); }
            else if waiting { blockers.push("终端正在等待确认或显示选择框，请定位处理。".into()); }
            else if !idle { blockers.push("终端正在生成回复或执行工具，结束后会自动重新检查。".into()); }
            if self.record.vim.as_deref() == Some("NORMAL") {
                blockers.push("窗口在 Vim 普通模式，请先切回插入模式。".into())
            }
            if idle && !waiting && !quota_wait && self.record.vim.as_deref() != Some("NORMAL") {
                if let Err(e) = screen.and_then(|s| win::check_empty_prompt(&s, &self.record.home)) { blockers.push(e); }
            }
        } else if relevant.is_empty() && self.record.phase != "exited" {
            blockers.push("无法核对原会话状态。".into())
        }
        let background = relevant
            .iter()
            .any(|r| ["bg", "daemon-worker"].contains(&r.kind.as_str()));
        let w = Window {
            window_id: self.record.id.clone(),
            name: self
                .record
                .cwd
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            cwd: target.cwd.display().to_string(),
            session_id: target.sid.clone(),
            pid: self.record.native.as_ref().map(|n| n.pid).unwrap_or(0),
            kind: if background {
                "background"
            } else {
                "foreground"
            }
            .into(),
            permission_mode: self.record.permission_mode.clone(),
            account_name: self.record.account_name.clone(),
            has_background_tasks: background || relevant.iter().any(|r| r.status == "shell"),
            can_focus: active_front && !self.record.terminal_host.is_empty(),
            blockers: blockers.clone(),
            model: None,
            effort: None,
        };
        Ok(
            serde_json::json!({"window":w,"target":target,"blockers":blockers,"record":self.record,"checkedRestore":true}),
        )
    }
    fn handle(&mut self, action: &str, data: &Value, expires_at: i64) -> AppResult<Value> {
        match action {
            "observe" => self.observe(data),
            "inspect" => self.inspect(),
            "focus" => {
                if self.record.phase != "running" || self.record.operation.is_some()
                    || !self.record.native.as_ref().is_some_and(Process::matches) {
                    return Err("终端正在切换或已关闭。".into());
                }
                win::focus_console()?;
                Ok(serde_json::json!({"ok":true}))
            }
            "arm" => {
                if data["checkedRestore"] != true {
                    return Err("请先重新打开新版换号台，再恢复这些窗口。".into());
                }
                self.record.operation =
                    Some(uuid(data["operation"].as_str().ok_or("缺少操作编号。")?)?);
                self.save()?;
                Ok(serde_json::json!({"ok":true}))
            }
            "disarm" => {
                self.record.operation = None;
                self.save()?;
                Ok(serde_json::json!({"ok":true}))
            }
            "quit" => {
                let check = self.inspect()?;
                if !check["blockers"].as_array().is_some_and(|v| v.is_empty()) {
                    return Err("窗口状态变化，请重新检查。".into());
                }
                if self.record.native.as_ref().is_some_and(Process::matches) {
                    win::input("/quit")?;
                    thread::sleep(Duration::from_millis(180));
                    win::screen()?.require_command("/quit")?;
                    let n = self.record.native.as_ref().unwrap();
                    let row = live_rows(&self.record.home)?
                        .into_iter()
                        .find(|r| r.pid == n.pid && r.proc_start == n.born)
                        .ok_or("窗口已变化。")?;
                    if !["idle", "shell"].contains(&row.status.as_str())
                        || row.waiting_for.is_some()
                    {
                        return Err("窗口出现弹窗，未提交退出命令。".into());
                    }
                    win::input("\r")?;
                    self.record.phase = "quitting".into();
                    self.save()?;
                }
                Ok(serde_json::json!({"ok":true}))
            }
            "clean" => {
                if self.record.native.as_ref().is_some_and(Process::matches)
                    || self.jobs.iter().any(|(p, _)| p.alive())
                {
                    return Err("Claude 会话尚未退出，未清理其任务。".into());
                }
                if let Some(j) = &self.front {
                    j.stop()?;
                }
                for (_, j) in &self.jobs {
                    j.stop()?;
                }
                self.save()?;
                Ok(serde_json::json!({"ok":true,"pids":self.record.job_pids}))
            }
            "resumeChecked" => {
                if data["operation"].as_str().is_none()
                    || data["operation"].as_str() != self.record.operation.as_deref()
                {
                    return Err("窗口已不属于这次恢复操作，请重新检查。".into());
                }
                let account: client::RestoreAccount =
                    serde_json::from_value(data["account"].clone())
                        .map_err(|_| "缺少已核实的恢复账号。")?;
                let revision = data["revision"].as_u64().ok_or("缺少换号版本。")?;
                self.refresh()?;
                self.save()?;
                if !self.record.job_pids.is_empty()
                    || self.record.native.as_ref().is_some_and(Process::matches)
                {
                    return Err("原窗口仍有活跃进程，未重复恢复。".into());
                }
                let t: Target =
                    serde_json::from_value(data["target"].clone()).map_err(|_| "恢复目标无效。")?;
                if live_rows(&self.record.home)?
                    .iter()
                    .any(|r| r.session_id == t.sid)
                {
                    return Err("该对话正在其他窗口运行，未重复恢复。".into());
                }
                let mode: Permissions = serde_json::from_value(data["permissionMode"].clone())
                    .map_err(|_| "权限选项无效。")?;
                let mut args = restored_args(
                    &self
                        .record
                        .args
                        .iter()
                        .map(OsString::from)
                        .collect::<Vec<_>>(),
                    &t.sid,
                    t.empty,
                    &mode,
                );
                if matches!(mode, Permissions::Preserve) {
                    // SessionEnd reports the actual current mode, including a
                    // user changing it with Shift+Tab after initial startup.
                    let actual = if self.record.permission_mode == "bypassPermissions" {
                        Permissions::BypassPermissions
                    } else {
                        Permissions::Default
                    };
                    args = restored_args(&args, &t.sid, t.empty, &actual);
                    let len = args.len();
                    // Replace the explicit default flag, without replaying prompts.
                    for i in 0..len.saturating_sub(1) {
                        if args[i] == "--permission-mode" {
                            args[i + 1] = self.record.permission_mode.clone().into();
                        }
                    }
                }
                self.front = None;
                self.jobs.clear();
                if !data["runtime"].is_null() {
                    let settings: super::runtime::Settings = serde_json::from_value(data["runtime"].clone())
                        .map_err(|_| "原模型和思考强度记录无效。")?;
                    args = settings.apply(args)?;
                }
                self.spawn(args, t.cwd, Some((&account, revision, expires_at)))?;
                Ok(serde_json::json!({"ok":true}))
            }
            _ => Err("不支持的终端操作。".into()),
        }
    }
    fn service_requests(&mut self) -> AppResult<()> {
        let directory = own_path(&self.store, &self.record.id)?;
        for e in fs::read_dir(directory.join("requests")).map_err(|_| "无法读取终端命令。")? {
            let path = e.map_err(|_| "无法读取终端命令。")?.path();
            if path.extension().is_none_or(|s| s != "json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| uuid(s).ok()) else {
                continue;
            };
            // Claim once, before acting. A lost response or failed cleanup must
            // never replay a quit/restore request, and malformed input must not
            // unwind this owner and close its kill-on-close Job handles.
            let claimed = path.with_extension("processing");
            match fs::rename(&path, &claimed) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err("无法领取终端命令，现有会话保持运行。".into()),
            }
            let result = match platform::read_json::<Value>(&claimed) {
                Ok(Some(request)) if request["id"].as_str() == Some(&id) => {
                    match request["expiresAt"].as_i64() {
                        Some(at) if at > chrono::Utc::now().timestamp_millis() => {
                            match self.handle(request["action"].as_str().unwrap_or(""), &request["data"], at) {
                                Ok(value) => value,
                                Err(error) => serde_json::json!({"error":error}),
                            }
                        }
                        _ => serde_json::json!({"error":"终端操作已过期，请重新检查。"}),
                    }
                }
                _ => serde_json::json!({"error":"终端命令记录无效，未执行该操作。"}),
            };
            let response = platform::write_json(&directory.join("responses").join(format!("{id}.json")), &result);
            let _ = fs::remove_file(&claimed);
            response?;
        }
        Ok(())
    }
    fn run(mut self) -> AppResult<i32> {
        let mut previous_error = None;
        loop {
            win::ConsoleMode::refresh();
            let refresh = self.refresh();
            let requests = self.service_requests();
            let saved = if self.last_saved.elapsed() >= Duration::from_millis(250) {
                self.save()
            } else {
                Ok(())
            };
            let error = refresh.err().or_else(|| requests.err()).or_else(|| saved.err());
            if error != previous_error {
                if let Some(error) = &error {
                    eprintln!("换号台管理状态暂时不可用，原会话保持运行：{error}");
                }
                previous_error = error;
            }
            if self.record.phase == "quitting" && win::screen().is_ok_and(|s| s.exit_dialog()) {
                let _ = win::input("\r");
            }
            if self.record.operation.is_none()
                && self.record.phase == "exited"
                && self
                    .exit_at
                    .is_some_and(|t| t.elapsed() > Duration::from_secs(3))
            {
                let Ok(live) = live_rows(&self.record.home) else {
                    // Keep the Job handles until absence of a background handoff is confirmed.
                    thread::sleep(Duration::from_millis(120));
                    continue;
                };
                let backgrounds = live.into_iter().any(|r| {
                    self.record.roots.iter().any(|root| {
                        follow(&self.record.home, root, &self.record.cwd)
                            .is_ok_and(|t| t.chain.contains(&r.session_id))
                    }) && ["bg", "daemon-worker"].contains(&r.kind.as_str())
                });
                if !backgrounds {
                    if let Some(j) = &self.front {
                        j.stop()?;
                    }
                    for (_, j) in &self.jobs {
                        j.stop()?;
                    }
                    self.record.phase = "closed".into();
                    self.save()?;
                    return Ok(self.record.exit_code.unwrap_or(0));
                }
            }
            thread::sleep(Duration::from_millis(45));
        }
    }
}
pub fn launch(args: Vec<OsString>) -> AppResult<i32> {
    let _console_mode = win::ConsoleMode::prepare();
    let store = Store::discover()?;
    let gate = mutation_guard(&store)?;
    let id = uuid::Uuid::new_v4().to_string();
    let root = own_path(&store, &id)?;
    platform::private_directory(&root)?;
    platform::private_directory(&root.join("requests"))?;
    platform::private_directory(&root.join("responses"))?;
    let cli = client::executable()?
        .canonicalize()
        .map_err(|_| "无法核对原生 Claude 路径。")?;
    let mut version = Command::new(&cli);
    version.arg("--version");
    let version = platform::capture(version, Duration::from_secs(10))?;
    let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    let cwd = std::env::current_dir().map_err(|_| "无法读取当前目录。")?;
    let owner = Process::open(std::process::id(), false)?.identity;
    let record = Record {
        id,
        nonce: uuid::Uuid::new_v4().to_string(),
        owner,
        cli,
        home: conflicts::home()?,
        cwd: cwd.clone(),
        args: vec![],
        native: None,
        roots: vec![],
        current_sid: String::new(),
        account_name: String::new(),
        binding: None,
        phase: "starting".into(),
        exit_code: None,
        operation: None,
        jobs: vec![],
        job_pids: vec![],
        vim: None,
        permission_mode: "default".into(),
        version,
        updated_at: now(),
        terminal_host: win::terminal_host().into(),
        process_identity_version: 1,
        exit_control_version: 3,
        resume_state_version: 1,
    };
    let mut owner = Owner {
        store,
        record,
        child: None,
        front: None,
        jobs: vec![],
        exit_at: None,
        last_saved: Instant::now(),
    };
    owner.spawn(args, cwd, None)?;
    drop(gate);
    owner.run()
}
use std::process::Command;
fn helper_process(r: &Record, ancestors: &[u32]) -> AppResult<Process> {
    ancestors.iter().find_map(|pid| {
        Process::open(*pid, false).ok().filter(|p| p.alive()
            && (r.knows_process(&p.identity) || r.is_cli_image(&p.identity.exe)))
    }).ok_or_else(|| "找不到已登记的原生 Claude 父进程。".into())
}

// Existing owners cannot be replaced without ending their Job and terminal.
// During a rolling helper upgrade, an already-owned foreground remains usable
// after an image rename. This is independent positive ownership verification,
// never a fallback on arbitrary RPC failures or a filename-prefix allowlist.
fn legacy_foreground(r: &Record, process: &Process, ancestors: &[u32], sid: &str, mode: &str, input: &Value) -> AppResult<Value> {
    let fresh = chrono::DateTime::parse_from_rfc3339(&r.updated_at).is_ok_and(|updated| {
        let age = chrono::Utc::now().signed_duration_since(updated).num_milliseconds();
        (0..=5000).contains(&age)
    });
    let native_at = ancestors.iter().position(|p| *p == process.identity.pid);
    let owner_at = ancestors.iter().position(|p| *p == r.owner.pid);
    let ancestry_matches = matches!((native_at, owner_at), (Some(n), Some(o)) if n < o);
    if r.process_identity_version != 0 || !["tool", "status"].contains(&mode)
        || !r.is_foreground(&process.identity) || !process.alive()
        || !Process::matches(&r.owner) || !ancestry_matches
        || !r.job_pids.contains(&process.identity.pid) || !fresh
        || r.phase != "running" || r.operation.is_some() || input.get("remote").is_some()
        || !tool_registration(&r.home, &process.identity, sid)
            .is_some_and(|row| row.session_id == sid && row.kind == "interactive")
    {
        return Err("旧版终端的进程、会话或受管关系核对失败；未放行工具。".into());
    }
    Ok(serde_json::json!({"ok":true,"name":r.account_name}))
}

fn helper_observe(store: &Store, r: &Record, nonce: &str, mode: &str, input: &Value, ancestors: &[u32]) -> AppResult<Value> {
    let observed_at = chrono::Utc::now().timestamp_millis();
    if r.nonce != nonce || !Process::matches(&r.owner) {
        return Err("终端启动器已退出或绑定已失效。".into());
    }
    if !["tool", "start", "end", "status"].contains(&mode) {
        return Err("未知的终端回传类型。".into());
    }
    let process = helper_process(r, ancestors)?;
    let sid = uuid(input.get("session_id").and_then(Value::as_str).ok_or("缺少会话编号。")?)?;
    if r.process_identity_version == 0 && r.is_foreground(&process.identity)
        && r.native.as_ref().is_some_and(|n| !win::same_image(&n.exe, &process.identity.exe))
        && ["tool", "status"].contains(&mode)
    {
        let response = legacy_foreground(r, &process, ancestors, &sid, mode, input)?;
        if mode == "status" {
            if let Some(binding) = &r.binding {
                // This foreground has the same verified ownership as the
                // owner's status path. Preserve its original account ticket;
                // never attribute a delayed callback to the selected card.
                crate::account_context::seal_normal(store, binding)?;
                quota::observe(store, binding, quota::statusline(input), "claude-code")?;
            }
        }
        return Ok(response);
    }
    let small = if input.get("remote").is_some() {
        serde_json::json!({"remote":true})
    } else {
        serde_json::json!({"rate_limits":input.get("rate_limits"),"vim":input.get("vim"),"permission_mode":input.get("permission_mode")})
    };
    let response = rpc(store, &r.id, "observe",
        serde_json::json!({"nonce":nonce,"process":process.identity,"sid":sid,"mode":mode,"input":small}),
        Duration::from_secs(10))?;
    if mode == "status" {
        // An optional observation must never stop tools or quota reporting.
        // Missing/invalid observations are explained in switch preview instead.
        let _ = super::runtime::observe(store, r, &process.identity, &sid, input, observed_at);
    }
    Ok(response)
}

pub fn helper(args: &[OsString]) -> AppResult<i32> {
    if args.len() != 4 {
        return Err("状态回传参数无效。".into());
    }
    let store = Store {
        root: platform::checked_path(Path::new(&args[0]))?,
    };
    let window = uuid(&args[1].to_string_lossy())?;
    let nonce = args[2].to_string_lossy();
    let mode = args[3].to_string_lossy();
    let r = record(&store, &window)?;
    let mut input = String::new();
    std::io::stdin()
        .take(2 * 1024 * 1024)
        .read_to_string(&mut input)
        .map_err(|_| "无法读取 Claude 状态。")?;
    let input: Value = serde_json::from_str(&input).map_err(|_| "Claude 状态格式无效。")?;
    let observed = helper_observe(&store, &r, &nonce, &mode, &input,
        &win::ancestry(std::process::id())?);
    #[cfg(feature = "synthetic-tests")]
    if mode == "status" {
        // Synthetic fixture diagnostics only; never account credentials or text.
        let _ = platform::write_json(&own_path(&store, &window)?.join("status-diagnostic.json"),
            &serde_json::json!({"keys":input.as_object().map(|v| v.keys().collect::<Vec<_>>()),
                "model":input.get("model"),"effort":input.get("effort"),
                "error":observed.as_ref().err()}));
    }
    let response = observed?;
    if mode == "status" {
        let name = response["name"].as_str().unwrap_or("Claude");
        let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
        let buckets = quota::statusline(&input);
        let brief = buckets
            .iter()
            .map(|b| {
                format!(
                    "{} {:.0}%",
                    if b.key == "5h" { "5小时" } else { "7天" },
                    b.utilization
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        println!(
            "{name}{}",
            if brief.is_empty() {
                String::new()
            } else {
                format!(" · {brief}")
            }
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_owner() -> (tempfile::TempDir, Owner, win::identity_tests::RenamedFixture) {
        let temp = tempfile::tempdir().unwrap();
        let store = Store { root: temp.path().join("store") };
        let home = temp.path().join("home");
        fs::create_dir_all(home.join(".claude/sessions")).unwrap();
        let job = Job::new().unwrap();
        let fixture = win::identity_tests::renamed_fixture(&job);
        let native = fixture.identity.clone();
        let owner_identity = Process::open(std::process::id(), false).unwrap().identity;
        let sid = uuid::Uuid::new_v4().to_string();
        let id = uuid::Uuid::new_v4().to_string();
        fs::create_dir_all(own_path(&store, &id).unwrap()).unwrap();
        let registration = home.join(format!(".claude/sessions/{}.json", native.pid));
        platform::write_json(&registration, &serde_json::json!({
            "pid":native.pid,"procStart":native.born,"sessionId":sid,"kind":"interactive","status":"idle"
        })).unwrap();
        let r = Record {
            id, nonce: uuid::Uuid::new_v4().to_string(), owner: owner_identity,
            cli: native.exe.clone(), home, cwd: temp.path().into(), args: vec![],
            native: Some(native.clone()), roots: vec![sid.clone()], current_sid: sid.clone(),
            account_name: "Synthetic account".into(), binding: None, phase: "running".into(),
            exit_code: None, operation: None, jobs: vec![], job_pids: job.pids().unwrap(),
            vim: None, permission_mode: "default".into(), version: "fixture".into(),
            updated_at: now(), terminal_host: "fixture".into(), process_identity_version: 0, exit_control_version: 0, resume_state_version: 0,
        };
        let owner = Owner { store, record: r, child: None, front: Some(job), jobs: vec![],
            exit_at: None, last_saved: Instant::now() };
        (temp, owner, fixture)
    }

    #[test]
    fn updated_native_keeps_tools_and_legacy_verification_stays_closed_to_unknowns() {
        let (_temp, mut owner, mut fixture) = isolated_owner();
        let r = owner.record.clone();
        let store = &owner.store;
        let sid = r.current_sid.clone();
        let native = r.native.as_ref().unwrap().clone();
        let registration = r.home.join(format!(".claude/sessions/{}.json", native.pid));
        let mut ancestors = vec![native.pid];
        ancestors.extend(win::ancestry(native.pid).unwrap());
        let input = serde_json::json!({"session_id":sid});
        assert!(helper_observe(&store, &r, &r.nonce, "tool", &input, &ancestors).is_ok());
        assert!(helper_observe(&store, &r, &r.nonce, "status", &input, &ancestors).is_ok());
        assert!(helper_observe(&store, &r, "wrong-nonce", "tool", &input, &ancestors).is_err());
        assert!(helper_observe(&store, &r, &r.nonce, "tool",
            &serde_json::json!({"session_id":uuid::Uuid::new_v4()}), &ancestors).is_err());
        assert!(helper_observe(&store, &r, &r.nonce, "tool", &input, &[native.pid]).is_err());
        assert!(helper_observe(&store, &r, &r.nonce, "tool", &input, &[r.owner.pid]).is_err());
        for mutation in 0..5 {
            let mut invalid = r.clone();
            match mutation {
                0 => invalid.native.as_mut().unwrap().born = "0".into(),
                1 => invalid.job_pids.clear(),
                2 => invalid.updated_at = "2000-01-01T00:00:00Z".into(),
                3 => invalid.operation = Some(uuid::Uuid::new_v4().to_string()),
                _ => invalid.owner.born = "0".into(),
            }
            assert!(helper_observe(&store, &invalid, &r.nonce, "tool", &input, &ancestors).is_err(), "mutation {mutation}");
        }
        let actual = Process::open(native.pid, false).unwrap().identity;
        owner.record.process_identity_version = 1;
        let event = serde_json::json!({"nonce":owner.record.nonce,"process":actual,"sid":sid,"mode":"tool","input":{}});
        assert!(owner.observe(&event).is_ok());
        // Unknown images do not gain ownership merely by using a registered SID.
        assert!(owner.accept_process(&Process::open(std::process::id(), false).unwrap().identity,
            &sid, "tool").is_err());
        // Neither a missing registration nor a stale process incarnation is accepted.
        fs::remove_file(registration).unwrap();
        assert!(owner.observe(&event).is_err());
        owner.front.as_ref().unwrap().stop().unwrap();
        fixture.wait();
        assert!(owner.observe(&event).is_err());
    }

    #[test]
    fn audit_foreground_tool_ignores_unrelated_partial_registration() {
        let (_temp, mut owner, _fixture) = isolated_owner();
        owner.record.process_identity_version = 1;
        let source = owner.record.native.clone().unwrap();
        let sid = owner.record.current_sid.clone();
        fs::write(owner.record.home.join(".claude/sessions/0.json"), "{").unwrap();
        assert!(live_rows(&owner.record.home).is_err());
        assert!(owner.accept_process(&source, &sid, "tool").is_ok(),
            "another terminal's incomplete file must not block this owned foreground");
    }

    #[test]
    fn audit_legacy_tool_retries_its_own_registration_rewrite() {
        let (_temp, owner, _fixture) = isolated_owner();
        let r = &owner.record;
        let native = r.native.as_ref().unwrap();
        let file = r.home.join(format!(".claude/sessions/{}.json", native.pid));
        let contents = fs::read(&file).unwrap();
        fs::write(&file, "").unwrap();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(90));
            fs::write(file, contents).unwrap();
        });
        let mut ancestors = vec![native.pid];
        ancestors.extend(win::ancestry(native.pid).unwrap());
        let result = helper_observe(&owner.store, r, &r.nonce, "tool",
            &serde_json::json!({"session_id":r.current_sid}), &ancestors);
        writer.join().unwrap();
        assert!(result.is_ok(), "a brief native rewrite must recover before denying a tool: {result:?}");
    }

    #[test]
    fn audit_retired_live_cli_still_blocks_account_transition() {
        let (_temp, owner, _fixture) = isolated_owner();
        let row = foreground_registration(&owner.record.home, owner.record.native.as_ref().unwrap()).unwrap();
        assert!(is_cli_session(&row, &owner.record.cli),
            "an auto-updated old CLI can still access its account files");
        assert!(!is_cli_session(&row, Path::new("C:/unrelated/desktop/native.exe")));
    }

    #[test]
    fn audit_malformed_request_does_not_abort_the_job_owner() {
        let (_temp, mut owner, _fixture) = isolated_owner();
        let root = own_path(&owner.store, &owner.record.id).unwrap();
        fs::create_dir_all(root.join("requests")).unwrap();
        fs::create_dir_all(root.join("responses")).unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        fs::write(root.join(format!("requests/{request_id}.json")), "{").unwrap();
        assert!(owner.service_requests().is_ok(), "bad input must not drop the owning Job");
        assert!(Process::matches(owner.record.native.as_ref().unwrap()));
        let id = uuid::Uuid::new_v4().to_string();
        platform::write_json(&root.join(format!("requests/{id}.json")), &serde_json::json!({
            "id":id,"action":"observe","data":{
                "nonce":owner.record.nonce,"process":owner.record.native,
                "sid":owner.record.current_sid,"mode":"tool","input":{}
            },"expiresAt":chrono::Utc::now().timestamp_millis()+5000
        })).unwrap();
        assert!(owner.service_requests().is_ok());
        let response: Value = platform::read_json(&root.join(format!("responses/{id}.json"))).unwrap().unwrap();
        assert_eq!(response["ok"], true);
        assert!(Process::matches(owner.record.native.as_ref().unwrap()));
    }

    #[test]
    fn audit_lost_response_cannot_replay_a_consumed_request() {
        let (_temp, mut owner, _fixture) = isolated_owner();
        let root = own_path(&owner.store, &owner.record.id).unwrap();
        fs::create_dir_all(root.join("requests")).unwrap();
        fs::write(root.join("responses"), "blocked-directory-fixture").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let operation = uuid::Uuid::new_v4().to_string();
        platform::write_json(&root.join(format!("requests/{id}.json")), &serde_json::json!({
            "id":id,"action":"arm","data":{"operation":operation,"checkedRestore":true},
            "expiresAt":chrono::Utc::now().timestamp_millis()+5000
        })).unwrap();
        assert!(owner.service_requests().is_err());
        assert_eq!(owner.record.operation.as_deref(), Some(operation.as_str()));
        owner.record.operation = None;
        fs::remove_file(root.join("responses")).unwrap();
        fs::create_dir_all(root.join("responses")).unwrap();
        owner.service_requests().unwrap();
        assert!(owner.record.operation.is_none(), "the already consumed arm request must not run twice");
        assert!(Process::matches(owner.record.native.as_ref().unwrap()));
    }

    #[test]
    fn audit_expired_and_mismatched_requests_cannot_arm_the_owner() {
        let (_temp, mut owner, _fixture) = isolated_owner();
        let root = own_path(&owner.store, &owner.record.id).unwrap();
        fs::create_dir_all(root.join("requests")).unwrap();
        fs::create_dir_all(root.join("responses")).unwrap();
        for expired in [false, true] {
            let file_id = uuid::Uuid::new_v4().to_string();
            let content_id = if expired { file_id.clone() } else { uuid::Uuid::new_v4().to_string() };
            platform::write_json(&root.join(format!("requests/{file_id}.json")), &serde_json::json!({
                "id":content_id,"action":"arm","data":{"operation":uuid::Uuid::new_v4(),"checkedRestore":true},
                "expiresAt":chrono::Utc::now().timestamp_millis()+if expired {-1} else {5000}
            })).unwrap();
            owner.service_requests().unwrap();
            let response: Value = platform::read_json(&root.join(format!("responses/{file_id}.json"))).unwrap().unwrap();
            assert!(response["error"].is_string());
            assert!(owner.record.operation.is_none());
        }
    }

    #[test]
    fn audit_worker_can_register_after_its_parent_image_is_renamed() {
        let (_temp, mut owner, mut fixture) = isolated_owner();
        owner.record.process_identity_version = 1;
        let native = owner.record.native.as_ref().unwrap();
        let renamed_exe = Process::open(native.pid, false).unwrap().identity.exe;
        let (mut worker, source) = owner.front.as_ref().unwrap().spawn_hidden(
            Command::new(renamed_exe).args(["--exact", "session_win::identity_tests::rename_worker", "--ignored"])
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        ).unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        platform::write_json(&owner.record.home.join(format!(".claude/sessions/{}.json", source.pid)),
            &serde_json::json!({"pid":source.pid,"procStart":source.born,"sessionId":sid,"kind":"bg","status":"idle"})).unwrap();
        let project = owner.record.home.join(".claude/projects/fixture");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(format!("{}.jsonl", owner.record.current_sid)),
            serde_json::json!({"type":"continued-in","continuedInSessionId":sid}).to_string()).unwrap();
        let event = serde_json::json!({"nonce":owner.record.nonce,"process":source,"sid":sid,"mode":"tool","input":{}});
        assert!(helper_process(&owner.record, &[source.pid, owner.record.owner.pid]).is_ok());
        let outcome = owner.observe(&event);
        // Stop only this synthetic child even when asserting a regression failure.
        if outcome.is_err() { worker.kill().unwrap(); worker.wait().unwrap(); }
        assert!(outcome.is_ok(), "a verified continuation from the held Job must remain adoptable: {outcome:?}");
        assert!(owner.record.jobs.iter().any(|p| p.same_process(&source)), "keep the worker identity after the foreground exits");
        fixture.stop();
        assert!(helper_process(&owner.record, &[source.pid, owner.record.owner.pid]).is_ok());
        fs::write(owner.record.home.join(".claude/sessions/0.json"), "{").unwrap();
        assert!(live_rows(&owner.record.home).is_err());
        assert!(owner.observe(&event).is_ok(), "an adopted worker survives foreground exit");
        owner.front.as_ref().unwrap().stop().unwrap();
        worker.wait().unwrap();
    }

    #[test]
    fn audit_retired_filename_alone_never_authorizes_an_unknown_process() {
        let (_temp, mut owner, _fixture) = isolated_owner();
        let fake_image = owner.record.cli.with_file_name("native.exe.old.untrusted");
        fs::copy(std::env::current_exe().unwrap(), &fake_image).unwrap();
        let job = Job::new().unwrap();
        let (mut child, source) = job.spawn_hidden(Command::new(&fake_image)
            .args(["--exact", "session_win::identity_tests::rename_worker", "--ignored"])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())).unwrap();
        assert!(win::may_be_cli_image(&fake_image, &owner.record.cli));
        assert!(!owner.record.is_cli_image(&fake_image));
        assert!(helper_process(&owner.record, &[source.pid]).is_err());
        let sid = owner.record.current_sid.clone();
        assert!(owner.accept_process(&source, &sid, "tool").is_err());
        job.stop().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn audit_legacy_status_publishes_quota_to_its_bound_account_only() {
        let (_temp, mut owner, _fixture) = isolated_owner();
        let mut db = crate::model::Database::empty();
        let account = uuid::Uuid::new_v4().to_string();
        db.accounts.push(crate::model::AccountRecord {
            id:account.clone(),name:"Synthetic quota account".into(),plan:"pro".into(),
            token_tail:"fake".into(),encrypted_token:"synthetic-ciphertext".into(),org:None,
            proxy:"http://127.0.0.1:1".into(),expected_ip:"203.0.113.1".into(),note:String::new(),added_at:now(),
        });
        owner.store.save(&db).unwrap();
        let binding = quota::Binding::selected(&owner.store, &owner.record.home, Some(&account)).unwrap();
        owner.record.binding = Some(binding.clone());
        let native = owner.record.native.as_ref().unwrap();
        let mut ancestors = vec![native.pid];
        ancestors.extend(win::ancestry(native.pid).unwrap());
        let input = serde_json::json!({"session_id":owner.record.current_sid,"rate_limits":{
            "five_hour":{"used_percentage":34,"resets_at":2000000000},
            "seven_day":{"used_percentage":85,"resets_at":2000500000}
        }});
        assert!(helper_observe(&owner.store, &owner.record, "wrong-nonce", "status", &input, &ancestors).is_err());
        assert!(quota::cached(&owner.store, &binding).unwrap().is_none());
        helper_observe(&owner.store, &owner.record, &owner.record.nonce, "tool", &input, &ancestors).unwrap();
        assert!(quota::cached(&owner.store, &binding).unwrap().is_none());
        helper_observe(&owner.store, &owner.record, &owner.record.nonce, "status", &input, &ancestors).unwrap();
        let cached = quota::cached(&owner.store, &binding).unwrap().expect("status must reach the quota cache");
        assert_eq!(cached.source.as_deref(), Some("claude-code"));
        let buckets = cached.buckets.unwrap();
        assert_eq!(buckets.iter().find(|b| b.key == "5h").unwrap().utilization, 34.0);
        assert_eq!(buckets.iter().find(|b| b.key == "7d").unwrap().utilization, 85.0);
        // A delayed callback from replaced credentials must not overwrite quota.
        db.accounts[0].encrypted_token = "different-synthetic-ciphertext".into();
        owner.store.save(&db).unwrap();
        let mut delayed = input.clone();
        delayed["rate_limits"]["five_hour"]["used_percentage"] = 99.into();
        helper_observe(&owner.store, &owner.record, &owner.record.nonce, "status", &delayed, &ancestors).unwrap();
        let cached = quota::cached(&owner.store, &binding).unwrap().unwrap();
        assert_eq!(cached.buckets.unwrap().iter().find(|b| b.key == "5h").unwrap().utilization, 34.0);
    }

    #[test]
    fn foreground_survives_partial_native_rows_but_inventory_still_blocks() {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".claude/sessions");
        fs::create_dir_all(&directory).unwrap();
        let native = Process::open(std::process::id(), false).unwrap().identity;
        let own = directory.join(format!("{}.json", native.pid));
        let partial = directory.join("0.json");
        let valid = serde_json::json!({
            "pid": native.pid, "procStart": native.born, "sessionId": "our-session",
        });
        fs::write(&own, valid.to_string()).unwrap();
        fs::write(&partial, "{").unwrap();
        assert_eq!(
            foreground_registration(home.path(), &native)
                .unwrap()
                .session_id,
            "our-session"
        );
        assert!(live_rows(home.path()).is_err());
        fs::remove_file(partial).unwrap();
        fs::write(&own, "").unwrap();
        assert!(foreground_registration(home.path(), &native).is_none());
        assert!(live_rows(home.path()).is_err());
        fs::write(&own, valid.to_string()).unwrap();
        assert_eq!(live_rows(home.path()).unwrap()[0].session_id, "our-session");
    }
    #[test]
    fn retains_live_pty_before_worker_session_registration() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".claude/daemon")).unwrap();
        fs::create_dir_all(temp.path().join(".claude/sessions")).unwrap();
        let me = Process::open(std::process::id(), false).unwrap().identity;
        let sid = uuid::Uuid::new_v4().to_string();
        platform::write_json(
            &temp.path().join(".claude/daemon/roster.json"),
            &serde_json::json!({"proto":1,"workers":{"test-job":{
                "pid":me.pid,"procStart":me.born,"sessionId":sid,
                "rvAuth":"ignored-fixture-value","dispatch":{"ignored":true}
            }}}),
        )
        .unwrap();
        let pending = live_rows(temp.path()).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].session_id, sid);
        assert_eq!(pending[0].status, "starting");
        platform::write_json(
            &temp.path().join(".claude/sessions/registered.json"),
            &serde_json::json!({"pid":me.pid,"procStart":me.born,"sessionId":sid,"kind":"bg","status":"idle"}),
        ).unwrap();
        let registered = live_rows(temp.path()).unwrap();
        assert_eq!(registered.len(), 1);
        assert_eq!(registered[0].status, "idle");
    }
}
