//! Per-process native model/effort observations. Never infer these from global
//! defaults: /effort max and session-only model choices deliberately don't persist.
use super::{owner::Record, *};
use crate::session_win::{Identity, Process};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    pub model: String,
    pub model_name: String,
    pub effort: Option<String>,
}
impl Settings {
    pub fn validate(&self) -> AppResult<()> {
        if self.model.is_empty() || self.model.len() > 200
            || !self.model.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.:/[]".contains(&c))
            || self.effort.as_deref().is_some_and(|e| !["low", "medium", "high", "xhigh", "max"].contains(&e))
        { return Err("Claude 回传的模型或思考强度无法识别，未猜测恢复设置。".into()); }
        Ok(())
    }
    fn from_status(input: &Value) -> AppResult<Self> {
        let model = input.pointer("/model/id").and_then(Value::as_str)
            .ok_or("等待 Claude Code 回传当前模型和思考强度。")?.to_owned();
        let effort = match input.get("effort") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.get("level").and_then(Value::as_str)
                .ok_or("Claude 思考强度回传不完整。")?.to_owned()),
        };
        let settings = Self {
            model_name: input.pointer("/model/display_name").and_then(Value::as_str)
                .unwrap_or(&model).chars().filter(|c| !c.is_control()).take(100).collect(),
            model, effort,
        };
        settings.validate()?;
        Ok(settings)
    }
    pub fn apply(&self, args: Vec<OsString>) -> AppResult<Vec<OsString>> {
        self.validate()?;
        let mut out = Vec::new();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            let s = a.to_string_lossy();
            if ["--model", "--effort"].contains(&s.as_ref()) { it.next(); continue; }
            if s.starts_with("--model=") || s.starts_with("--effort=") { continue; }
            out.push(a);
        }
        out.extend(["--model".into(), self.model.clone().into()]);
        if let Some(effort) = &self.effort { out.extend(["--effort".into(), effort.clone().into()]); }
        Ok(out)
    }
}
#[derive(Serialize, Deserialize)]
struct Observation {
    owner: Identity,
    native: Identity,
    nonce: String,
    sid: String,
    observed_at: i64,
    settings: Option<Settings>,
    #[serde(default)]
    error: Option<String>,
}
fn path(store: &Store, id: &str) -> AppResult<PathBuf> {
    Ok(directory(store)?.join(uuid(id)?).join("runtime.json"))
}

// Called only after the existing helper has verified native ownership and its
// live registration. Also works with an already-running old owner; that owner
// need not learn a new RPC format, or be restarted to collect this observation.
pub(super) fn observe(store: &Store, r: &Record, native: &Identity, sid: &str, input: &Value, observed_at: i64) -> AppResult<()> {
    if input.get("remote").is_some()
        || !r.native.as_ref().is_some_and(|n| n.same_process(native))
        || !Process::matches(native)
    { return Ok(()); }
    let settings = Settings::from_status(input);
    let file = path(store, &r.id)?;
    let _lock = platform::lock(&file.with_extension("lock"), Duration::from_secs(2))?;
    if let Some(old) = platform::read_json::<Observation>(&file)? {
        if old.native.same_process(native) && old.observed_at > observed_at { return Ok(()); }
    }
    platform::write_json(&file, &Observation {
        owner:r.owner.clone(), native:native.clone(), nonce:r.nonce.clone(),
        sid:uuid(sid)?, observed_at, error:settings.as_ref().err().cloned(), settings:settings.ok(),
    })
}
pub(super) fn read(store: &Store, r: &Record, sid: &str) -> AppResult<Settings> {
    let o: Observation = platform::read_json(&path(store, &r.id)?)?
        .ok_or("等待 Claude Code 回传当前模型和思考强度；收到后会自动更新。")?;
    if !o.owner.same_process(&r.owner) || o.nonce != r.nonce || o.sid != sid
        || !r.native.as_ref().is_some_and(|n| n.same_process(&o.native))
    { return Err("当前进程的模型和思考强度尚未回传；收到后会自动更新。".into()); }
    let settings = o.settings.ok_or_else(|| o.error.unwrap_or_else(|| "当前模型和思考强度无法确认。".into()))?;
    settings.validate()?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_status_preserves_max_and_overrides_old_launch_flags() {
        let s = Settings::from_status(&serde_json::json!({"model":{"id":"claude-opus-5-5","display_name":"Opus 5.5"},"effort":{"level":"max"}})).unwrap();
        let args = ["--model=sonnet", "--effort", "high", "--resume", "sid", "--permission-mode", "plan"].map(OsString::from).to_vec();
        let actual = s.apply(args).unwrap();
        assert_eq!(actual, ["--resume", "sid", "--permission-mode", "plan", "--model", "claude-opus-5-5", "--effort", "max"].map(OsString::from));
        for input in [serde_json::json!({}), serde_json::json!({"model":{"id":"opus\n/quit"}}), serde_json::json!({"model":{"id":"opus"},"effort":{"level":"unknown"}})] {
            assert!(Settings::from_status(&input).is_err());
        }
        assert!(Settings::from_status(&serde_json::json!({"model":{"id":"custom-without-effort"}})).unwrap().effort.is_none());
    }
    #[test]
    fn observations_are_bound_to_process_session_and_owner_not_selected_account() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store { root:temp.path().into() };
        let id = uuid::Uuid::new_v4().to_string();
        let fixture = Process::open(std::process::id(), false).unwrap().identity;
        let sid = uuid::Uuid::new_v4().to_string();
        let r: Record = serde_json::from_value(serde_json::json!({
            "id":id,"nonce":"nonce","owner":fixture,"native":fixture,"cli":"fixture.exe","home":"home","cwd":"cwd","args":[],
            "roots":[sid],"currentSid":sid,"accountName":"Synthetic","binding":null,"phase":"running","exitCode":null,"operation":null,
            "jobs":[],"jobPids":[],"vim":null,"permissionMode":"default","version":"fixture","updatedAt":crate::model::now()
        })).unwrap();
        let file = path(&store,&id).unwrap();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let mut payload=serde_json::json!({"model":{"id":"opus"},"effort":{"level":"max"}});
        observe(&store,&r,&fixture,&sid,&payload,20).unwrap();
        assert_eq!(read(&store,&r,&sid).unwrap().effort.as_deref(),Some("max"));
        payload["effort"]["level"]="high".into();
        observe(&store,&r,&fixture,&sid,&payload,10).unwrap();
        assert_eq!(read(&store,&r,&sid).unwrap().effort.as_deref(),Some("max"),"late callback must not replace max");
        observe(&store,&r,&fixture,&sid,&payload,30).unwrap();
        assert_eq!(read(&store,&r,&sid).unwrap().effort.as_deref(),Some("high"));
        assert!(read(&store,&r,"different-session").is_err());
        for change in 0..3 {
            let mut wrong=r.clone();
            match change { 0=>wrong.native.as_mut().unwrap().born="0".into(), 1=>wrong.owner.born="0".into(), _=>wrong.nonce="other".into() }
            assert!(read(&store,&wrong,&sid).is_err());
        }
        payload["effort"]["level"]="future-unknown-value".into();
        observe(&store,&r,&fixture,&sid,&payload,40).unwrap();
        assert!(read(&store,&r,&sid).is_err(), "a newer invalid observation must invalidate older settings");
    }
}
