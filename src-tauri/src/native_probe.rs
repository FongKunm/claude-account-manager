//! Explicit, user-triggered quota query through an unmodified native CLI.
//! The temporary process has only the requested account's access token and route.
use crate::{
    client, crypto,
    model::*,
    network::{self, Transport},
    normal, platform, quota,
    service::Manager,
};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub fn check(
    manager: &Manager,
    id: Option<&str>,
    transport: &impl Transport,
) -> AppResult<UsageCheck> {
    // One short-lived query at a time, including queries for different cards.
    // A rapid series of clicks must never create a queue of native processes.
    let _query = platform::lock(
        &manager.store.root.join("native-query.lock"),
        Duration::from_millis(1),
    )
    .map_err(|_| "已有一次额度查询正在进行，请等它结束后再查。")?;
    let (binding, token, proxy, expected) = {
        let _lock = manager.store.lock()?;
        let home = manager.normal_home()?;
        if id.is_none() && normal::identity(&home)?.account_uuid.is_none() {
            return Ok(UsageCheck::error(
                "invalid",
                "未找到普通登录账号，请先在 Claude Code 中登录",
            ));
        }
        let binding = quota::Binding::selected(&manager.store, &home, id)?;
        if let Some(id) = id {
            let db = manager.store.read()?;
            #[cfg(windows)]
            if let Some(a) = db.ordinary(id) {
                let dir = crate::ordinary::home(&manager.store, id)?;
                let access = match normal::credentials(&dir, false) {
                    Ok(t) => t,
                    Err(e) => return Ok(*e),
                };
                (
                    binding,
                    access.value,
                    a.proxy.clone(),
                    a.expected_ip.clone(),
                )
            } else {
                let a = db.account(id)?;
                (
                    binding,
                    crypto::token(&a.encrypted_token)?,
                    a.proxy.clone(),
                    a.expected_ip.clone(),
                )
            }
            #[cfg(not(windows))]
            {
                let a = db.account(id)?;
                (
                    binding,
                    crypto::token(&a.encrypted_token)?,
                    a.proxy.clone(),
                    a.expected_ip.clone(),
                )
            }
        } else {
            let credentials = match normal::credentials(&home, manager.integrate) {
                Ok(v) => v,
                Err(e) => return Ok(*e),
            };
            let identity = normal::identity(&home)?;
            let expected = identity
                .cache_key()
                .and_then(|k| {
                    manager
                        .store
                        .normal_checks()
                        .ok()?
                        .get(&k)
                        .map(|r| r.expected_ip.clone())
                })
                .unwrap_or_default();
            (
                binding,
                credentials.value,
                client::normal_proxy(&home)?,
                expected,
            )
        }
    };
    let check = network::bind_network(transport, &proxy, &expected);
    if !check.ok {
        return Ok(UsageCheck::error(
            "neterr",
            check.error.as_deref().unwrap_or("出口检查失败。"),
        ));
    }
    if !binding.current(&manager.store) {
        return Err("该账号凭据已经变化，请重新查询。".into());
    }
    if id.is_none() {
        let _lock = manager.store.lock()?;
        if !binding.current(&manager.store) {
            return Err("普通登录账号已变化，请重新查询。".into());
        }
        normal::remember_exit(&manager.store, &binding.home, check.ip.as_deref())?;
    }
    let base = manager.store.root.join("native-probes");
    if !base.is_dir() {
        platform::private_directory(&base)?;
    }
    let temp = tempfile::Builder::new()
        .prefix("query-")
        .tempdir_in(&base)
        .map_err(|_| "无法创建独立查询目录。")?;
    let home = temp.path().join("home");
    let config = home.join(".claude");
    platform::private_directory(&config)?;
    let mut command = Command::new(client::executable()?);
    // Keep only normal OS execution support. Neither the UI process's auth nor B's
    // proxy/config/FD can reach this A-only process. No refresh token is copied.
    command.env_clear();
    #[cfg(feature = "synthetic-tests")]
    {
        crate::lab::check_proxy(&proxy)?;
        let fixture = crate::lab::config()?;
        command.env("NODE_EXTRA_CA_CERTS", fixture.ca);
    }
    for key in [
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATH",
        "PATHEXT",
        "TEMP",
        "TMP",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROCESSOR_ARCHITECTURE",
        "NUMBER_OF_PROCESSORS",
        "CLAUDE_CODE_GIT_BASH_PATH",
    ] {
        if let Some(v) = std::env::var_os(key) {
            command.env(key, v);
        }
    }
    command
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("CLAUDE_CONFIG_DIR", &config)
        .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", &config)
        .env("CLAUDE_CODE_OAUTH_TOKEN", token.as_str())
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("CLAUDE_CODE_DISABLE_BG_EXIT_HANDOFF", "1")
        .env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1")
        .env("DISABLE_AUTOUPDATER", "1");
    let mut route = serde_json::Map::new();
    for key in client::PROXY_KEYS {
        command.env(key, &proxy);
        route.insert(key.into(), proxy.clone().into());
    }
    for key in ["NO_PROXY", "no_proxy"] {
        command.env(key, client::SESSION_NO_PROXY);
        route.insert(key.into(), client::SESSION_NO_PROXY.into());
    }
    command
        .current_dir(temp.path())
        .args([
            "-p",
            "Reply with OK only.",
            "--output-format",
            "stream-json",
            "--verbose",
            "--max-turns",
            "1",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--setting-sources",
            "",
            "--no-session-persistence",
            "--no-chrome",
            "--disable-slash-commands",
            "--system-prompt",
            "Reply with OK only. Do not call tools.",
        ])
        .arg("--settings")
        .arg(serde_json::json!({"env":route}).to_string());
    let buckets = run(command)?;
    if buckets.is_empty() {
        return Ok(UsageCheck::error(
            "error",
            "Claude Code 没有返回额度窗口，保留原记录；未使用其他账号或缓存补数。",
        ));
    }
    if !binding.current(&manager.store) {
        return Err("查询期间账号凭据变化，结果未写入。".into());
    }
    quota::observe(&manager.store, &binding, buckets, "native-probe")?;
    quota::cached(&manager.store, &binding)?.ok_or_else(|| "额度未能保存，请重试。".into())
}
struct QueryProcess {
    child: Child,
    #[cfg(windows)]
    job: crate::session_win::Job,
    cleaned: bool,
}
impl QueryProcess {
    fn spawn(command: &mut Command) -> AppResult<Self> {
        #[cfg(windows)]
        {
            let job = crate::session_win::Job::new()?;
            let (child, _) = job.spawn_hidden(command)?;
            Ok(Self {
                child,
                job,
                cleaned: false,
            })
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::process::CommandExt;
            let child = command
                .process_group(0)
                .spawn()
                .map_err(|_| "无法启动原生 Claude Code 查询。")?;
            Ok(Self {
                child,
                cleaned: false,
            })
        }
    }
    fn cleanup(&mut self) -> AppResult<()> {
        if self.cleaned {
            return Ok(());
        }
        #[cfg(windows)]
        self.job.stop()?;
        #[cfg(not(windows))]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            // Only the new, private process group created for this query.
            unsafe {
                kill(-(self.child.id() as i32), 9);
            }
        }
        let _ = self.child.kill();
        self.child.wait().map_err(|_| "查询进程尚未完成回收。")?;
        self.cleaned = true;
        Ok(())
    }
}
impl Drop for QueryProcess {
    fn drop(&mut self) {
        let _ = self.cleanup();
        // Windows KILL_ON_JOB_CLOSE also covers an early error or app exit.
    }
}
enum Output {
    Windows(Vec<UsageBucket>),
    Complete,
    Invalid,
}
fn run(command: Command) -> AppResult<Vec<UsageBucket>> {
    run_with_timeout(command, Duration::from_secs(90))
}
fn run_with_timeout(mut command: Command, timeout: Duration) -> AppResult<Vec<UsageBucket>> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = QueryProcess::spawn(&mut command)?;
    let stdout = process.child.stdout.take().ok_or("无法读取查询结果。")?;
    let stderr = process.child.stderr.take().ok_or("无法读取查询状态。")?;
    let (send, receive) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut stream = BufReader::new(stdout);
        let mut total = 0usize;
        loop {
            let mut bytes = Vec::new();
            match stream
                .by_ref()
                .take(1_048_577)
                .read_until(b'\n', &mut bytes)
            {
                Ok(0) => break,
                Err(_) => {
                    let _ = send.send(Output::Invalid);
                    break;
                }
                Ok(n) => {
                    total += n;
                    if n > 1_048_576 || total > 8_388_608 {
                        let _ = send.send(Output::Invalid);
                        break;
                    }
                    if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                        let rows = quota::stream_event(&v);
                        if !rows.is_empty() {
                            let _ = send.send(Output::Windows(rows));
                        }
                        if v.get("type").and_then(Value::as_str) == Some("result") {
                            let _ = send.send(Output::Complete);
                        }
                    }
                }
            }
        }
    });
    let errors = thread::spawn(move || {
        let _ = std::io::copy(&mut stderr.take(1_048_576), &mut std::io::sink());
    });
    let until = Instant::now() + timeout;
    let mut rows = Vec::new();
    let outcome = (|| -> AppResult<()> {
        loop {
            while let Ok(found) = receive.try_recv() {
                match found {
                    Output::Windows(v) => rows.extend(v),
                    Output::Complete => return Ok(()),
                    Output::Invalid => {
                        return Err("Claude Code 返回内容异常，本次查询已停止。".into())
                    }
                }
            }
            if process
                .child
                .try_wait()
                .map_err(|_| "无法读取原生查询状态。")?
                .is_some()
            {
                return Ok(());
            }
            if Instant::now() > until {
                return if rows.is_empty() {
                    Err("Claude Code 查询超时，临时进程已停止。".into())
                } else {
                    Ok(())
                };
            }
            thread::sleep(Duration::from_millis(40));
        }
    })();
    // Close the whole private job BEFORE joining pipe readers: a descendant may
    // inherit stdout even after the native CLI has exited. Every return uses this.
    let cleanup = process.cleanup();
    let _ = reader.join();
    let _ = errors.join();
    while let Ok(found) = receive.try_recv() {
        if let Output::Windows(v) = found {
            rows.extend(v);
        }
    }
    cleanup?;
    outcome?;
    Ok(rows)
}

#[cfg(all(windows, feature = "synthetic-tests"))]
pub(crate) fn synthetic_lifecycle() -> AppResult<Value> {
    use crate::session_win::Process;
    use serde_json::json;
    let f = crate::lab::config()?;
    let mut control = Command::new(&f.python);
    control
        .args(["-c", "import time;time.sleep(90)"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut control = QueryProcess::spawn(&mut control)?;
    let mut results = vec![];
    for i in 0..12 {
        let mode = ["done", "exit", "error", "timeout"][i % 4];
        let marker = f.root.join(format!("query-life-{i}.json"));
        let script = r#"
import json,os,subprocess,sys,time
mode,path=sys.argv[1:]
child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(120)'],creationflags=8,close_fds=False)
open(path,'w').write(json.dumps({'parent':os.getpid(),'child':child.pid}))
if mode=='timeout': time.sleep(90)
elif mode=='error': sys.exit(3)
else:
 print(json.dumps({'type':'rate_limit_event','rate_limit_info':{'unifiedWindows':{'five_hour':{'utilization':0.11,'resetsAt':2000000000}}}}),flush=True)
 if mode=='done':
  print(json.dumps({'type':'result'}),flush=True)
  time.sleep(90)
"#;
        let mut command = Command::new(&f.python);
        command.args(["-c", script, mode]).arg(&marker);
        let result = run_with_timeout(command, Duration::from_secs(2));
        let pids: Value = platform::read_json(&marker)?.ok_or("Fixture did not start")?;
        let alive: Vec<_> = ["parent", "child"]
            .iter()
            .filter(|k| {
                Process::open(pids[**k].as_u64().unwrap_or(0) as u32, false)
                    .is_ok_and(|p| p.alive())
            })
            .copied()
            .collect();
        if !alive.is_empty()
            || control
                .child
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
        {
            return Err(format!("Fixture cleanup failed in case {i}"));
        }
        if mode == "timeout" && result.is_ok() {
            return Err("Timeout fixture not rejected".into());
        }
        if ["done", "exit"].contains(&mode) && !result.as_ref().is_ok_and(|r| r.len() == 1) {
            return Err("Quota fixture not captured".into());
        }
        results.push(json!({"case":i,"mode":mode,"remaining":alive,"controlAlive":true}));
    }
    control.cleanup()?;
    Ok(json!({"passed":true,"queries":results}))
}
