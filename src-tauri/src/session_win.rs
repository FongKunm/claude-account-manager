//! Windows process ownership. Every termination is scoped to a held Job handle.
//! No global keyboard hooks, process-name termination, or Desktop automation.
use crate::model::AppResult;
use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command},
    ptr::null_mut,
    thread,
    time::{Duration, Instant},
};
type Handle = *mut c_void;
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Ft {
    lo: u32,
    hi: u32,
}
#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}
#[repr(C)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    tid: u32,
    pid: u32,
    base: i32,
    delta: i32,
    flags: u32,
}
#[repr(C)]
struct Basic {
    process_time: i64,
    job_time: i64,
    flags: u32,
    min: usize,
    max: usize,
    active: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
struct Extended {
    basic: Basic,
    io: [u64; 6],
    memory: [usize; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Coord {
    x: i16,
    y: i16,
}
#[repr(C)]
struct Rect {
    left: i16,
    top: i16,
    right: i16,
    bottom: i16,
}
#[repr(C)]
struct ScreenInfo {
    size: Coord,
    cursor: Coord,
    attributes: u16,
    window: Rect,
    max: Coord,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Key {
    down: i32,
    repeat: u16,
    vk: u16,
    scan: u16,
    character: u16,
    control: u32,
}
#[repr(C)]
union InputData {
    key: Key,
    padding: [u32; 4],
}
#[repr(C)]
struct Input {
    kind: u16,
    data: InputData,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CloseHandle(h: Handle) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn GetProcessTimes(h: Handle, c: *mut Ft, e: *mut Ft, k: *mut Ft, u: *mut Ft) -> i32;
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, text: *mut u16, size: *mut u32) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(h: Handle, p: *mut ProcessEntry) -> i32;
    fn Process32NextW(h: Handle, p: *mut ProcessEntry) -> i32;
    fn Thread32First(h: Handle, t: *mut ThreadEntry) -> i32;
    fn Thread32Next(h: Handle, t: *mut ThreadEntry) -> i32;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> Handle;
    fn ResumeThread(h: Handle) -> u32;
    fn CreateJobObjectW(attr: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(h: Handle, class: i32, data: *const c_void, size: u32) -> i32;
    fn QueryInformationJobObject(
        h: Handle,
        class: i32,
        data: *mut c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn IsProcessInJob(process: Handle, job: Handle, result: *mut i32) -> i32;
    fn TerminateJobObject(job: Handle, code: u32) -> i32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        creation: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn GetConsoleScreenBufferInfo(h: Handle, i: *mut ScreenInfo) -> i32;
    fn GetConsoleMode(h: Handle, mode: *mut u32) -> i32;
    fn SetConsoleMode(h: Handle, mode: u32) -> i32;
    fn GetConsoleWindow() -> Handle;
    fn GetConsoleTitleW(text: *mut u16, len: u32) -> u32;
    fn SetConsoleTitleW(text: *const u16) -> i32;
    fn ReadConsoleOutputCharacterW(
        h: Handle,
        text: *mut u16,
        n: u32,
        at: Coord,
        read: *mut u32,
    ) -> i32;
    fn GetNumberOfConsoleInputEvents(h: Handle, n: *mut u32) -> i32;
    fn PeekConsoleInputW(h: Handle, data: *mut Input, n: u32, read: *mut u32) -> i32;
    fn WriteConsoleInputW(h: Handle, data: *const Input, n: u32, written: *mut u32) -> i32;
    fn AttachConsole(pid: u32) -> i32;
    fn FreeConsole() -> i32;
    fn GetConsoleProcessList(pids: *mut u32, count: u32) -> u32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetClassNameW(h: Handle, text: *mut u16, len: i32) -> i32;
    fn IsWindowVisible(h: Handle) -> i32;
    fn IsIconic(h: Handle) -> i32;
    fn ShowWindowAsync(h: Handle, command: i32) -> i32;
    fn SetForegroundWindow(h: Handle) -> i32;
    fn GetForegroundWindow() -> Handle;
    fn AllowSetForegroundWindow(pid: u32) -> i32;
}
pub fn classic_console() -> bool {
    // PseudoConsoleWindow can have WS_VISIBLE set despite having no on-screen
    // surface. Only the actual conhost window can be focused directly.
    let window = unsafe { GetConsoleWindow() };
    let mut class = [0u16; 64];
    let count = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    count > 0
        && String::from_utf16_lossy(&class[..count as usize]) == "ConsoleWindowClass"
        && unsafe { IsWindowVisible(window) } != 0
}
pub fn terminal_host() -> &'static str {
    if classic_console() { "Windows 控制台" }
    else if std::env::var_os("WT_SESSION").is_some() { "Windows Terminal" }
    else { "" }
}

pub fn allow_foreground(pid: u32) { unsafe { AllowSetForegroundWindow(pid); } }

fn console_title() -> Vec<u16> {
    let mut title = vec![0;32768];
    let n = unsafe { GetConsoleTitleW(title.as_mut_ptr(),title.len() as u32) };
    title.truncate(n as usize);
    title.push(0);
    title
}

pub fn focus_console() -> AppResult<()> {
    focus_console_for_host(terminal_host())
}
pub fn focus_console_for_host(host: &str) -> AppResult<()> {
    if classic_console() {
        let h = unsafe { GetConsoleWindow() };
        unsafe {
            if IsIconic(h) != 0 { ShowWindowAsync(h,9); }
            SetForegroundWindow(h);
        }
        // Activation crosses the conhost input queue and completes asynchronously.
        let deadline = Instant::now() + Duration::from_millis(750);
        loop {
            if unsafe { GetForegroundWindow() } == h && unsafe { IsIconic(h) } == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline { break; }
            thread::sleep(Duration::from_millis(25));
        }
        return Err("Windows 未允许这个终端切到前台。".into());
    }
    if host != "Windows Terminal" { return Err("无法识别这个终端的宿主。".into()); }
    // A fresh title belongs only to this console. Tabs with a pinned/custom
    // title cannot present the marker; leave their selection alone.
    let marker = format!("CAM-{}",uuid::Uuid::new_v4());
    let wide: Vec<_> = marker.encode_utf16().chain(Some(0)).collect();
    let previous = console_title();
    if unsafe { SetConsoleTitleW(wide.as_ptr()) } == 0 { return Err("终端不支持标签页定位。".into()); }
    let result = (|| -> AppResult<()> {
        let system = std::env::var_os("SystemRoot").ok_or("找不到系统目录。")?;
        let mut child = Command::new(PathBuf::from(system).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
            .args(["-NoLogo","-NoProfile","-NonInteractive","-Command",include_str!("focus_terminal.ps1")])
            .env_clear().env("SystemRoot", std::env::var_os("SystemRoot").unwrap_or_default())
            .env("TEMP",std::env::temp_dir()).env("TMP",std::env::temp_dir()).env("CAM_FOCUS_TITLE",&marker)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .creation_flags(0x08000000).spawn().map_err(|_|"无法启动终端定位。")?;
        allow_foreground(child.id());
        let deadline = Instant::now()+Duration::from_secs(5);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return if status.success(){Ok(())} else {Err("未能定位到这个标签页；它可能设置了固定标题，或 Windows 阻止了前台切换。".into())};
            }
            if Instant::now() >= deadline { let _=child.kill(); let _=child.wait(); return Err("终端定位超时。".into()); }
            thread::sleep(Duration::from_millis(35));
        }
    })();
    if console_title() == wide { unsafe { SetConsoleTitleW(previous.as_ptr()); } }
    result
}

fn error(action: &str) -> String {
    format!(
        "{action}（Windows {}）。",
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    )
}
struct OwnedHandle(Handle);
impl OwnedHandle {
    fn new(h: Handle) -> AppResult<Self> {
        if h.is_null() || h as isize == -1 {
            Err(error("无法打开进程资源"))
        } else {
            Ok(Self(h))
        }
    }
}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub pid: u32,
    pub born: String,
    pub exe: PathBuf,
}
impl Identity {
    /// Windows can rename a running image during an automatic update. PID and
    /// creation time identify the process; the image path is adoption evidence,
    /// not part of the lifetime identity (nor may PID alone authorize reuse).
    pub fn same_process(&self, other: &Self) -> bool {
        self.pid == other.pid && self.born == other.born
    }
}
pub struct Process {
    handle: OwnedHandle,
    pub identity: Identity,
}
pub fn same_image(a: &Path, b: &Path) -> bool {
    fn normalized(p: &Path) -> String {
        let s = p.to_string_lossy().replace('/', "\\");
        s.strip_prefix("\\\\?\\").unwrap_or(&s).to_ascii_lowercase()
    }
    normalized(a) == normalized(b)
}
/// For blocking account mutations only. A retired image may still use the old
/// account. This filename family must NEVER authorize adopting a new process.
pub fn may_be_cli_image(actual: &Path, cli: &Path) -> bool {
    if same_image(actual, cli) {
        return true;
    }
    match (actual.parent(), cli.parent(), actual.file_name(), cli.file_name()) {
        (Some(a), Some(b), Some(actual), Some(expected)) if same_image(a, b) => {
            let prefix = format!("{}.old.", expected.to_string_lossy().to_ascii_lowercase());
            let actual = actual.to_string_lossy().to_ascii_lowercase();
            actual.starts_with(&prefix) && actual.len() > prefix.len()
        }
        _ => false,
    }
}
impl Process {
    pub fn open(pid: u32, manage: bool) -> AppResult<Self> {
        let h = OwnedHandle::new(unsafe {
            OpenProcess(0x100000 | 0x1000 | if manage { 0x101 } else { 0 }, 0, pid)
        })?;
        let (mut c, mut e, mut k, mut u) =
            (Ft::default(), Ft::default(), Ft::default(), Ft::default());
        let mut name = vec![0u16; 32768];
        let mut len = name.len() as u32;
        if unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) } == 0
            || unsafe { QueryFullProcessImageNameW(h.0, 0, name.as_mut_ptr(), &mut len) } == 0
        {
            return Err(error("无法核对进程身份"));
        }
        Ok(Self {
            handle: h,
            identity: Identity {
                pid,
                born: (((c.hi as u64) << 32) | c.lo as u64).to_string(),
                exe: PathBuf::from(String::from_utf16_lossy(&name[..len as usize])),
            },
        })
    }
    pub fn matches(expected: &Identity) -> bool {
        Self::open(expected.pid, false)
            .is_ok_and(|p| p.identity.same_process(expected) && p.alive())
    }
    pub fn alive(&self) -> bool {
        (unsafe { WaitForSingleObject(self.handle.0, 0) }) == 0x102
    }
}

#[cfg(test)]
pub(crate) mod identity_tests {
    use super::*;
    pub(crate) struct RenamedFixture {
        pub identity: Identity,
        child: Child,
        _directory: tempfile::TempDir,
    }
    impl RenamedFixture {
        pub(crate) fn wait(&mut self) {
            self.child.wait().unwrap();
        }
        pub(crate) fn stop(&mut self) {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
        }
    }
    impl Drop for RenamedFixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    pub(crate) fn renamed_fixture(job: &Job) -> RenamedFixture {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("native.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &original).unwrap();
        let (child, identity) = job.spawn_hidden(Command::new(&original)
            .args(["--exact", "session_win::identity_tests::rename_worker", "--ignored"])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())).unwrap();
        std::fs::rename(&original, directory.path().join("native.exe.old.update")).unwrap();
        RenamedFixture { identity, child, _directory: directory }
    }
    #[test]
    #[ignore = "isolated child process launched by rename regression"]
    fn rename_worker() {
        thread::sleep(Duration::from_secs(60));
    }
    #[test]
    fn running_image_rename_preserves_identity_and_job_but_rejects_pid_reuse() {
        let job = Job::new().unwrap();
        let mut fixture = renamed_fixture(&job);
        let actual = Process::open(fixture.identity.pid, false).unwrap();
        assert!(!same_image(&actual.identity.exe, &fixture.identity.exe));
        assert!(actual.identity.same_process(&fixture.identity));
        assert!(Process::matches(&fixture.identity));
        assert!(job.contains(&actual));
        let mut reused = fixture.identity.clone();
        reused.born = "0".into();
        assert!(!Process::matches(&reused));
        reused = fixture.identity.clone();
        reused.pid = std::process::id();
        assert!(!Process::matches(&reused));
        job.stop().unwrap();
        fixture.wait();
        assert!(!Process::matches(&fixture.identity));
    }
}
pub fn ancestry(pid: u32) -> AppResult<Vec<u32>> {
    let snapshot = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(2, 0) })?;
    let mut row: ProcessEntry = unsafe { zeroed() };
    row.size = size_of::<ProcessEntry>() as u32;
    let mut parents = std::collections::BTreeMap::new();
    let mut ok = unsafe { Process32FirstW(snapshot.0, &mut row) };
    while ok != 0 {
        parents.insert(row.pid, row.parent);
        ok = unsafe { Process32NextW(snapshot.0, &mut row) };
    }
    let mut result = Vec::new();
    let mut at = pid;
    for _ in 0..32 {
        at = *parents.get(&at).unwrap_or(&0);
        if at == 0 || result.contains(&at) {
            break;
        }
        result.push(at);
    }
    Ok(result)
}
pub struct Job {
    handle: OwnedHandle,
}
impl Job {
    pub fn new() -> AppResult<Self> {
        let handle = OwnedHandle::new(unsafe { CreateJobObjectW(null_mut(), std::ptr::null()) })?;
        let mut info: Extended = unsafe { zeroed() };
        info.basic.flags = 0x2000;
        if unsafe {
            SetInformationJobObject(
                handle.0,
                9,
                &info as *const _ as _,
                size_of::<Extended>() as u32,
            )
        } == 0
        {
            return Err(error("无法设置任务清理范围"));
        }
        Ok(Self { handle })
    }
    pub fn assign(&self, p: &Process) -> AppResult<()> {
        if unsafe { AssignProcessToJobObject(self.handle.0, p.handle.0) } == 0 {
            return Err(error("无法纳管任务进程"));
        }
        Ok(())
    }
    pub fn contains(&self, p: &Process) -> bool {
        let mut yes = 0;
        (unsafe { IsProcessInJob(p.handle.0, self.handle.0, &mut yes) }) != 0 && yes != 0
    }
    pub fn pids(&self) -> AppResult<Vec<u32>> {
        // The Windows flexible array is pointer-aligned after two DWORDs.
        for capacity in [256usize, 1024, 4096, 16384] {
            let mut bytes = vec![0usize; 1 + capacity];
            if unsafe {
                QueryInformationJobObject(
                    self.handle.0,
                    3,
                    bytes.as_mut_ptr() as _,
                    (bytes.len() * size_of::<usize>()) as u32,
                    null_mut(),
                )
            } != 0
            {
                let count = unsafe { *(bytes.as_ptr() as *const u32).add(1) } as usize;
                if count <= capacity {
                    return Ok(bytes[1..=count].iter().map(|p| *p as u32).collect());
                }
            }
        }
        Err(error("无法核对任务进程清单"))
    }
    pub fn stop(&self) -> AppResult<()> {
        if !self.pids()?.is_empty() && unsafe { TerminateJobObject(self.handle.0, 1223) } == 0 {
            return Err(error("无法停止剩余任务"));
        }
        let until = Instant::now() + Duration::from_secs(8);
        while !self.pids()?.is_empty() {
            if Instant::now() > until {
                return Err("任务仍在退出，未切换账号。".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
    pub fn spawn(&self, command: &mut Command) -> AppResult<(Child, Identity)> {
        self.spawn_flags(command, 0)
    }
    pub fn spawn_hidden(&self, command: &mut Command) -> AppResult<(Child, Identity)> {
        self.spawn_flags(command, crate::platform::NO_WINDOW)
    }
    fn spawn_flags(&self, command: &mut Command, flags: u32) -> AppResult<(Child, Identity)> {
        let mut child = command
            .creation_flags(4 | flags)
            .spawn()
            .map_err(|_| "无法启动受管 Claude Code。")?;
        let result = (|| {
            let p = Process::open(child.id(), true)?;
            self.assign(&p)?;
            let snapshot = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(4, 0) })?;
            let mut row: ThreadEntry = unsafe { zeroed() };
            row.size = size_of::<ThreadEntry>() as u32;
            let mut ok = unsafe { Thread32First(snapshot.0, &mut row) };
            while ok != 0 {
                if row.pid == child.id() {
                    let h = OwnedHandle::new(unsafe { OpenThread(2, 0, row.tid) })?;
                    if unsafe { ResumeThread(h.0) } == u32::MAX {
                        return Err(error("无法恢复受管进程"));
                    }
                    return Ok(p.identity);
                }
                ok = unsafe { Thread32Next(snapshot.0, &mut row) };
            }
            Err("未找到新进程的启动线程。".into())
        })();
        match result {
            Ok(identity) => Ok((child, identity)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
    }
}
#[derive(Serialize, Deserialize)]
pub struct Screen {
    pub lines: Vec<String>,
    pub cursor: [usize; 2],
    pub pending: u32,
}
fn console(name: &str) -> AppResult<OwnedHandle> {
    let name: Vec<u16> = std::ffi::OsStr::new(name)
        .encode_wide()
        .chain(Some(0))
        .collect();
    OwnedHandle::new(unsafe {
        CreateFileW(name.as_ptr(), 0xC0000000, 3, null_mut(), 3, 0, null_mut())
    })
}
/// Match VT last-column wrapping in the launcher's own terminal. Restore the
/// shell's original mode when the launcher returns. Never touch another window.
pub struct ConsoleMode {
    handle: OwnedHandle,
    previous: u32,
}
impl ConsoleMode {
    pub fn refresh() {
        // Claude can activate a new screen buffer after startup. CONOUT$ refers
        // to that active buffer; the startup handle still refers to the old one.
        if let Ok(handle) = console("CONOUT$") {
            let mut mode = 0;
            if unsafe { GetConsoleMode(handle.0, &mut mode) } != 0 && mode & 0xD != 0xD {
                unsafe {
                    SetConsoleMode(handle.0, mode | 0xD);
                }
            }
        }
    }
    pub fn prepare() -> Option<Self> {
        let handle = console("CONOUT$").ok()?;
        let mut previous = 0;
        if unsafe { GetConsoleMode(handle.0, &mut previous) } == 0 {
            return None;
        }
        if unsafe { SetConsoleMode(handle.0, previous | 0xD) } == 0 {
            return None;
        }
        Some(Self { handle, previous })
    }
}
impl Drop for ConsoleMode {
    fn drop(&mut self) {
        if let Ok(current) = console("CONOUT$") {
            unsafe {
                SetConsoleMode(current.0, self.previous);
            }
        }
        unsafe {
            SetConsoleMode(self.handle.0, self.previous);
        }
    }
}
pub fn screen() -> AppResult<Screen> {
    let output = console("CONOUT$")?;
    let input = console("CONIN$")?;
    let mut info: ScreenInfo = unsafe { zeroed() };
    if unsafe { GetConsoleScreenBufferInfo(output.0, &mut info) } == 0 {
        return Err(error("无法识别终端输入框"));
    }
    if info.size.x < 1 || info.size.x > 2000 || info.window.bottom - info.window.top > 1000 {
        return Err("终端尺寸无法核对。".into());
    }
    let mut lines = Vec::new();
    for y in info.window.top..=info.window.bottom {
        let mut row = vec![0u16; info.size.x as usize];
        let mut n = 0;
        if unsafe {
            ReadConsoleOutputCharacterW(
                output.0,
                row.as_mut_ptr(),
                row.len() as u32,
                Coord { x: 0, y },
                &mut n,
            )
        } == 0
        {
            return Err(error("无法读取终端输入框"));
        }
        lines.push(
            String::from_utf16_lossy(&row[..n as usize])
                .trim_end()
                .to_owned(),
        );
    }
    let mut pending = 0;
    if unsafe { GetNumberOfConsoleInputEvents(input.0, &mut pending) } == 0 {
        return Err(error("无法核对终端输入队列"));
    }
    if pending > 0 && pending <= 4096 {
        let mut events: Vec<Input> = (0..pending).map(|_| unsafe { zeroed() }).collect();
        let mut count = 0;
        if unsafe { PeekConsoleInputW(input.0, events.as_mut_ptr(), pending, &mut count) } == 0 {
            return Err(error("无法查看终端输入队列"));
        }
        // Focus/resize/menu notifications are not draft keystrokes. Peek only:
        // never consume or flush the user's keyboard/mouse input.
        pending = events[..count as usize].iter().filter(|e| ![4,8,16].contains(&e.kind)).count() as u32;
    }
    Ok(Screen {
        lines,
        cursor: [
            info.cursor.x as usize,
            (info.cursor.y - info.window.top) as usize,
        ],
        pending,
    })
}
impl Screen {
    pub fn quota_wait(&self) -> bool {
        let footer = self.lines.get(self.cursor[1]+1..).unwrap_or_default().join(" ").to_ascii_lowercase();
        footer.contains("usage limit reached") && footer.contains("continuing automatically") && footer.contains("esc to cancel")
    }
    pub fn cache_switch_dialog(&self, kind: &str, target: &str) -> bool {
        let title = match kind { "model" => "Switch model?", "effort" => "Change effort level?", _ => return false };
        let Some(start) = self.lines.iter().rposition(|s| s.trim() == title) else { return false; };
        let dialog = &self.lines[start..];
        self.pending == 0
            && dialog.iter().any(|s| s.trim() == "Your next response will be slower and use more tokens")
            && dialog.join(" ").contains("This conversation is cached for the current")
            && dialog.join(" ").contains("full history gets re-read.")
            && !dialog.iter().any(|s| s.contains("PreModelSwitch") || s.contains("hook asked"))
            && dialog.iter().any(|s| s.trim() == format!("❯ 1. Yes, switch to {target}"))
            && dialog.iter().any(|s| s.trim() == "2. No, go back")
    }
    pub fn model_picker(&self, model_name: &str) -> AppResult<(usize, usize)> {
        let Some(start) = self.lines.iter().rposition(|s| s.trim() == "Select model") else {
            return Err("未识别到原生模型选择器。".into());
        };
        let rows = &self.lines[start..];
        if self.pending != 0 || !rows.iter().any(|s| s.contains("s to use this session only") && s.contains("Enter to set as default")) {
            return Err("模型选择器已变化，未继续输入。".into());
        }
        let mut selected = None;
        let mut targets = Vec::new();
        for row in rows {
            let row = row.trim();
            let active = row.starts_with("❯ ");
            let row = row.strip_prefix("❯ ").unwrap_or(row);
            let Some((number, label)) = row.split_once(". ") else { continue; };
            let Ok(number) = number.parse::<usize>() else { continue; };
            if active { selected = Some(number); }
            // The default entry may name the same model. Select only the
            // explicit model entry, then verify the actual ID via statusline.
            if label.starts_with("Default ") { continue; }
            if label.split("  ").map(str::trim).any(|s| s == model_name || s.strip_prefix(model_name).is_some_and(|tail| tail.starts_with(" ·"))) {
                targets.push(number);
            }
        }
        match (selected, targets.as_slice()) {
            (Some(at), [target]) => Ok((at, *target)),
            _ => Err("原模型不在原生选择器中或无法唯一确认，请定位终端检查；未改默认模型。".into()),
        }
    }
    pub fn effort_picker(&self) -> AppResult<(Vec<String>, usize)> {
        if self.pending != 0 || !self.lines.iter().any(|s| s.trim() == "Effort")
            || !self.lines.iter().any(|s| s.contains("s for this session only") && s.contains("to adjust"))
        { return Err("思考强度选择器已变化，未继续输入。".into()); }
        for rows in self.lines.windows(2) {
            let Some(marker) = rows[0].chars().position(|c| c == '▲') else { continue; };
            let labels: Vec<char> = rows[1].chars().collect();
            let mut at = 0;
            let mut levels = Vec::<(String, usize)>::new();
            while at < labels.len() {
                if labels[at].is_ascii_alphabetic() {
                    let start = at;
                    while at < labels.len() && labels[at].is_ascii_alphabetic() { at += 1; }
                    let word: String = labels[start..at].iter().collect();
                    if ["low","medium","high","xhigh","max"].contains(&word.as_str()) { levels.push((word, start + (at-start)/2)); }
                } else { at += 1; }
            }
            if levels.len() >= 3 && levels.first().is_some_and(|v| v.0 == "low") {
                let selected = levels.iter().enumerate().min_by_key(|(_,v)| v.1.abs_diff(marker)).unwrap().0;
                return Ok((levels.into_iter().map(|v| v.0).collect(),selected));
            }
        }
        Err("无法核对思考强度滑块位置。".into())
    }
    fn prompt_row(&self) -> AppResult<(usize, String)> {
        if self.quota_wait() { return Err("Claude 正在等待额度重置并准备自动续跑；请定位后按 Esc 取消等待，再回来切换。".into()); }
        if self.pending != 0 { return Err("终端正在接收输入，请稍候。".into()); }
        let y = self.cursor[1];
        let row = self.lines.get(y).ok_or("终端光标位置未知。")?;
        let above = (0..y)
            .rev()
            .find(|i| self.lines[*i].trim().starts_with('─'));
        let below = (y + 1..self.lines.len()).find(|i| self.lines[*i].trim().starts_with('─'));
        let (Some(a), Some(b)) = (above, below) else {
            return Err("终端输入区域无法核对，未发送退出命令。".into());
        };
        let first = self.lines[a + 1].trim_start_matches(' ');
        if !first.starts_with('❯') { return Err("终端当前不在空闲输入框，请定位检查。".into()); }
        if a + 1 != y || b != y + 1 {
            return Err("输入框包含多行内容，请检查后再切换。".into());
        }
        let indent = row.len() - first.len();
        let tail = first.strip_prefix('❯').unwrap_or_default();
        let text = if tail.is_empty() { "" }
            else { tail.strip_prefix([' ', '\u{a0}']).ok_or("终端当前不在空闲输入框，请定位检查。")? };
        Ok((indent + 2, text.to_owned()))
    }
    /// Confirm the exact ASCII command and caret before Enter. In particular
    /// leading spaces, or a caret before the end, are not our command.
    pub fn require_command(&self, command: &str) -> AppResult<()> {
        let (origin, text) = self.prompt_row()?;
        if text == command && self.cursor[0] == origin + command.len() { Ok(()) }
        else { Err("输入框在准备命令时发生变化，未提交。".into()) }
    }
}

const DRAFT: &str = "输入框有未发送的文字，请处理后再切换。";

/// Claude accepts comments in keybindings.json. Only the first key of a
/// chord can intercept an unmodified Home or End in an idle composer.
fn check_navigation_bindings(home: &Path) -> AppResult<()> {
    let config = home.join(".claude/keybindings.json");
    let source = match std::fs::read_to_string(config) {
        Ok(source) => source,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("无法读取终端快捷键设置，未检查输入框。".into()),
    };
    let mut rest = source.as_str();
    while let Some(at) = rest.find(['"', '/']) {
        rest = &rest[at..];
        if let Some(comment) = rest.strip_prefix("//") {
            rest = comment.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(comment) = rest.strip_prefix("/*") {
            rest = comment.split_once("*/").map_or("", |(_, tail)| tail);
        } else if rest.starts_with('"') {
            let mut strings = serde_json::Deserializer::from_str(rest).into_iter::<String>();
            let key = strings.next().and_then(Result::ok).ok_or("终端快捷键设置无法核对。")?;
            rest = &rest[strings.byte_offset()..];
            if key.split_ascii_whitespace().next().is_some_and(|k| {
                k.eq_ignore_ascii_case("home") || k.eq_ignore_ascii_case("end")
            }) {
                return Err("终端自定义了 Home 或 End 快捷键，无法安全核对输入框。".into());
            }
        } else {
            rest = &rest[1..];
        }
    }
    Ok(())
}

/// ConPTY can report identical text, colours and caret position for a hint
/// and for a real draft after Home. End only navigates the native editor.
/// Probe only a verified idle composer. Never use Right/Tab (accept hint),
/// printable input, Enter, the clipboard, or foreground activation here.
pub fn check_empty_prompt(screen_before: &Screen, home: &Path) -> AppResult<()> {
    let (origin, original) = screen_before.prompt_row()?;
    if screen_before.cursor[0] != origin { return Err(DRAFT.into()); }
    check_navigation_bindings(home)?;
    key(0x23)?; // End
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        thread::sleep(Duration::from_millis(25));
        let current = screen()?;
        let (start, text) = current.prompt_row()?;
        if start != origin || current.cursor[1] != screen_before.cursor[1] || text != original {
            return Err("输入框刚刚发生变化，稍后会自动复查。".into());
        }
        if current.cursor[0] != origin {
            // Restore the original Home position only while the same text and
            // position are still present. An intervening edit belongs to the user.
            let latest = screen()?;
            if latest.cursor == current.cursor && latest.prompt_row()? == (start, text) {
                key(0x24)?; // Home
            }
            return Err(DRAFT.into());
        }
        if Instant::now() >= deadline { return Ok(()); }
    }
}
impl Screen {
    pub fn exit_dialog(&self) -> bool {
        let text = self.lines.join("\n");
        text.contains("Background work is running")
            && text.contains("❯ 1. Exit and stop tasks")
            && text.contains("2. Move to background and exit")
            && self.pending == 0
    }
}
pub fn input(text: &str) -> AppResult<()> {
    let input = console("CONIN$")?;
    let mut records = Vec::new();
    for ch in text.encode_utf16() {
        for down in [1, 0] {
            records.push(Input {
                kind: 1,
                data: InputData {
                    key: Key {
                        down,
                        repeat: 1,
                        vk: if ch == 13 { 13 } else { 0 },
                        scan: 0,
                        character: ch,
                        control: 0,
                    },
                },
            });
        }
    }
    let mut n = 0;
    if unsafe { WriteConsoleInputW(input.0, records.as_ptr(), records.len() as u32, &mut n) } == 0
        || n as usize != records.len()
    {
        return Err(error("终端未接收退出操作"));
    }
    Ok(())
}
/// A single key in the attached, ownership-checked console. Never global input.
pub fn key(vk: u16) -> AppResult<()> {
    let handle = console("CONIN$")?;
    let records: Vec<Input> = [1,0].into_iter().map(|down| Input { kind:1,
        data:InputData { key:Key {down, repeat:1, vk, scan:0, character:0, control:0} } }).collect();
    let mut written = 0;
    if unsafe { WriteConsoleInputW(handle.0,records.as_ptr(),2,&mut written) } == 0 || written != 2 {
        return Err(error("终端未接收按键"));
    }
    Ok(())
}

// Used only by a separate, hidden control helper. The GUI never detaches its
// console, and attachment does not activate a tab or send global keystrokes.
pub struct AttachedConsole;
impl AttachedConsole {
    pub fn open(owner: &Identity, native: &Identity) -> AppResult<Self> {
        if !Process::matches(owner) || !Process::matches(native) {
            return Err("原终端进程已变化，未连接控制台。".into());
        }
        // Some launch hosts attach even CREATE_NO_WINDOW children to an inherited
        // console. This dedicated helper may detach itself before attaching the
        // verified target; it never detaches the owner, native process or GUI.
        unsafe { FreeConsole(); }
        if unsafe { AttachConsole(owner.pid) } == 0 {
            return Err(error("无法连接受管终端"));
        }
        let attached = Self;
        let mut pids = vec![0u32; 4096];
        let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) } as usize;
        if count == 0 || count > pids.len() || !pids[..count].contains(&owner.pid)
            || !pids[..count].contains(&native.pid)
            || !Process::matches(owner) || !Process::matches(native)
        {
            return Err("终端宿主与原会话不匹配，未发送输入。".into());
        }
        Ok(attached)
    }
}
impl Drop for AttachedConsole {
    fn drop(&mut self) { unsafe { FreeConsole(); } }
}

#[cfg(test)]
mod runtime_screen_tests {
    use super::*;
    #[test]
    fn distinguishes_quota_wait_footer_from_history_and_never_discards_visible_input() {
        let mut s = Screen { lines: vec![
            "Usage limit reached · Continuing automatically · Esc to cancel".into(),
            "──────────────────".into(), "❯".into(), "──────────────────".into(),
        ], cursor: [2,2], pending: 0 };
        assert!(!s.quota_wait(), "historical output is not an active wait");
        assert_eq!(s.prompt_row().unwrap(), (2, String::new()));
        s.lines.push("Usage limit reached · Continuing automatically · Esc to cancel".into());
        assert!(s.prompt_row().unwrap_err().contains("等待额度重置"));
        s.lines.pop();
        s.lines[2] = "❯ 继续检查".into();
        assert_eq!(s.prompt_row().unwrap(), (2, "继续检查".to_owned()));
        s.lines[2] = "❯".into();
        s.pending = 1;
        assert!(s.prompt_row().unwrap_err().contains("接收输入"));
    }
    #[test]
    fn submitting_a_switch_command_requires_its_exact_text_end_and_single_line() {
        let mut view = Screen { lines: vec!["────────".into(), "❯\u{a0}/quit".into(), "────────".into()],
            cursor: [7, 1], pending: 0 };
        assert!(view.require_command("/quit").is_ok());
        view.cursor[0] = 2;
        assert!(view.require_command("/quit").is_err());
        view.cursor[0] = 7;
        view.lines[1] = "❯  /quit".into();
        assert!(view.require_command("/quit").is_err());
        view.lines[1] = "❯ /quit".into();
        view.lines.insert(2, String::new());
        assert!(view.require_command("/quit").is_err(), "even a blank continuation can be a draft");
    }
    #[test]
    fn navigation_probe_respects_custom_chords_and_escaped_keys() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".claude");
        std::fs::create_dir(&dir).unwrap();
        let config = dir.join("keybindings.json");
        assert!(check_navigation_bindings(temp.path()).is_ok());
        std::fs::write(&config, r#"{ /* "end" */ "bindings":[{"bindings":{"ctrl+end":"chat:submit"}}] }"#).unwrap();
        assert!(check_navigation_bindings(temp.path()).is_ok());
        for key in ["END", "home x", r"e\u006ed"] {
            std::fs::write(&config, format!(r#"{{"bindings":[{{"bindings":{{"{key}":"chat:submit"}}}}]}}"#)).unwrap();
            assert!(check_navigation_bindings(temp.path()).is_err());
        }
    }
    #[test]
    fn recognizes_only_native_session_only_effort_slider() {
        let mut s = Screen { lines: vec![
            "  Effort".into(),
            "  Faster                         Smarter".into(),
            "  ─────────────────────────────────▲─".into(),
            "  low   medium   high   xhigh      max   Tab to toggle".into(),
            "  ←/→to adjust ·Enter to confirm ·s for this session only ·Esc to cancel".into(),
        ], cursor:[0,5], pending:0 };
        let (levels, at) = s.effort_picker().unwrap();
        assert_eq!(levels, ["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(at, 4);
        s.lines[2]="  ─────────▲─────────────────────────".into();
        assert_eq!(s.effort_picker().unwrap().1,1);
        s.pending=1;
        assert!(s.effort_picker().is_err());
        s.pending=0;
        s.lines[4]="Enter to confirm ·Esc to cancel".into();
        assert!(s.effort_picker().is_err(), "never use a picker lacking session-only save");
    }
    #[test]
    fn cache_confirmation_requires_exact_target_and_never_approves_hook_permissions() {
        let mut s=Screen {lines:vec!["Switch model?".into(),"Your next response will be slower and use more tokens".into(),
            "This conversation is cached for the current model. Switching to Sonnet 5.5 means the full history gets re-read.".into(),
            "❯ 1. Yes, switch to Sonnet 5.5".into(),"2. No, go back".into()],cursor:[0,0],pending:0};
        assert!(s.cache_switch_dialog("model","Sonnet 5.5"));
        assert!(!s.cache_switch_dialog("model","Opus 5.5"));
        s.lines[1]="A PreModelSwitch hook asked you to confirm".into();
        assert!(!s.cache_switch_dialog("model","Sonnet 5.5"));
        s.lines[0]="Change effort level?".into();
        s.lines[1]="Your next response will be slower and use more tokens".into();
        s.lines[3]="❯ 1. Yes, switch to medium".into();
        assert!(s.cache_switch_dialog("effort","medium"));
        assert!(!s.cache_switch_dialog("effort","max"));
        s.pending=1;
        assert!(!s.cache_switch_dialog("effort","medium"));
    }
    #[test]
    fn model_picker_uses_unique_explicit_model_and_session_only_entry() {
        let mut s=Screen {lines:vec!["Select model".into(),
            "  1. Default (recommended)  Sonnet 5.5 · Efficient".into(),
            "  2. Sonnet                 Sonnet 5.5 · Efficient".into(),
            "❯ 4. Opus ✔                Opus 5.5 · Best".into(),
            "Enter to set as default ·s to use this session only ·Esc to cancel".into()],cursor:[0,3],pending:0};
        assert_eq!(s.model_picker("Sonnet 5.5").unwrap(),(4,2));
        assert!(s.model_picker("Sonnet 4.6").is_err());
        s.lines.insert(3,"3. Other  Sonnet 5.5 · duplicate".into());
        assert!(s.model_picker("Sonnet 5.5").is_err());
        s.lines.pop();
        assert!(s.model_picker("Opus 5.5").is_err());
    }
}
