use crate::{
    conflicts, crypto, hooks,
    model::*,
    network::{self, Transport},
    normal, platform,
    registry::Environment,
    store::Store,
};

#[derive(Clone)]
pub struct Manager {
    pub store: Store,
    pub(crate) integrate: bool,
}
fn retain_extra_windows(usage: &mut UsageCheck, old: Option<&UsageCheck>) {
    if let Some(old) = old {
        usage.overage = old.overage.clone();
        if let Some(buckets) = usage.buckets.as_mut() {
            for extra in old
                .buckets
                .iter()
                .flatten()
                .filter(|b| b.key != "5h" && b.key != "7d")
            {
                buckets.push(extra.clone());
            }
        }
    }
}
impl Manager {
    pub(crate) fn normal_home(&self) -> AppResult<std::path::PathBuf> {
        if self.integrate {
            conflicts::home()
        } else {
            Ok(self.store.root.join("test-home"))
        }
    }
    #[cfg(test)]
    pub(crate) fn isolated(root: std::path::PathBuf) -> Self {
        Self {
            store: Store { root },
            integrate: false,
        }
    }
    pub fn discover() -> AppResult<Self> {
        Ok(Self {
            store: Store::discover()?,
            integrate: true,
        })
    }
    fn begin(&self, env: &impl Environment) -> AppResult<(platform::Lease, Database, CheckStore)> {
        let lease = self.store.lock()?;
        self.store.recover(env)?;
        let db = self.store.read()?;
        #[cfg(windows)]
        let _ = self.finish_login_removal(&db);
        Ok((lease, db, self.store.checks()?))
    }
    #[cfg(windows)]
    fn finish_login_removal(&self, db: &Database) -> AppResult<()> {
        let marker = self.store.root.join("pending-login-removal.json");
        let Some(id): Option<String> = platform::read_json(&marker)? else { return Ok(()) };
        if db.ordinary(&id).is_none() {
            // Only remove the validated enrollment slot after its database transaction committed.
            let dir = crate::ordinary::home(&self.store, &id)?;
            if dir.try_exists().map_err(|_| "无法检查账号专用目录。")? {
                std::fs::remove_dir_all(dir).map_err(|_| "账号卡已移除，专用目录仍被占用，将自动重试清理。")?;
            }
            let mut checks = self.store.checks()?;
            if checks.remove(&id).is_some() { self.store.save_checks(&checks)?; }
        }
        std::fs::remove_file(marker).map_err(|_| "无法清除账号移除记录。".into())
    }
    fn clean(&self, proxy: &str) -> AppResult<()> {
        if self.integrate {
            conflicts::require_clean(proxy, None)?;
        }
        Ok(())
    }
    fn hook(&self) -> AppResult<()> {
        #[cfg(feature = "synthetic-tests")]
        {
            crate::lab::config()?;
            return Ok(());
        }
        #[cfg(not(feature = "synthetic-tests"))]
        {
            if self.integrate {
                hooks::install(&self.store)?;
            }
            Ok(())
        }
    }
    pub fn get_state(&self, env: &impl Environment) -> AppResult<AppState> {
        let (_lease, db, checks) = self.begin(env)?;
        let selected = db.current.as_deref().and_then(|id| db.route(id).ok());
        let mut conflicts = if self.integrate {
            conflicts::scan(selected.map(|(proxy, _)| proxy), None)?
        } else {
            vec![]
        };
        if db.current.is_none() && env.read("CLAUDE_CODE_OAUTH_TOKEN")?.is_some() {
            conflicts.push(Conflict {
                source: "用户手动设置的环境变量".into(),
                key: "CLAUDE_CODE_OAUTH_TOKEN（已保留）".into(),
            });
        }
        let identity = normal::identity(&self.normal_home()?)?;
        let normal_checks = self.store.normal_checks()?;
        let key = identity.cache_key().unwrap_or_else(|| "no-login".into());
        let normal_record = normal_checks.get(&key).cloned().unwrap_or_default();
        // Old Windows records are keyed by organization (or email), not by
        // account UUID + organization. Keep the existing route pin, but do
        // not display another member's quota, including extra model windows.
        #[cfg(windows)]
        let legacy_normal_usage = None;
        #[cfg(not(windows))]
        let legacy_normal_usage = normal_record.last_usage_check;
        let mut state = AppState {
            version: env!("CARGO_PKG_VERSION").into(),
            data_root: self.store.root.display().to_string(),
            default_proxy: DEFAULT_PROXY.into(),
            ordinary_accounts: Vec::new(),
            accounts: db
                .accounts
                .iter()
                .map(|a| {
                    Account::view(
                        a,
                        db.current.as_ref() == Some(&a.id),
                        checks.get(&a.id).unwrap_or(&Checks::default()),
                    )
                })
                .collect(),
            normal: NormalLogin {
                email: identity.email,
                org: identity.org,
                current: db.current.is_none(),
                plan: normal_record
                    .profile
                    .as_ref()
                    .map(|p| p.plan.clone())
                    .unwrap_or(identity.plan),
                subscription_status: normal_record.profile.and_then(|p| p.subscription_status),
                billing_type: identity.billing_type,
                expected_ip: normal_record.expected_ip,
                last_usage_check: legacy_normal_usage,
            },
            conflicts,
        };
        let home = self.normal_home()?;
        #[cfg(windows)]
        if let Ok(proxy) = crate::client::normal_proxy(&home) {
            state.default_proxy = proxy;
        }
        #[cfg(windows)]
        for a in &db.ordinary_accounts {
            let mut view = crate::ordinary::view(
                &self.store,
                &home,
                a,
                db.current.as_ref() == Some(&a.id),
            )?;
            view.last_network_check = checks.get(&a.id).and_then(|v| v.last_network_check.clone());
            state.ordinary_accounts.push(view);
        }
        for account in &mut state.accounts {
            if let Ok(binding) =
                crate::quota::Binding::selected(&self.store, &home, Some(&account.id))
            {
                if let Some(mut usage) = crate::quota::cached(&self.store, &binding)? {
                    retain_extra_windows(&mut usage, account.last_usage_check.as_ref());
                    account.last_usage_check = Some(usage);
                }
            }
        }
        if let Ok(binding) = crate::quota::Binding::selected(&self.store, &home, None) {
            if let Some(mut usage) = crate::quota::cached(&self.store, &binding)? {
                retain_extra_windows(&mut usage, state.normal.last_usage_check.as_ref());
                state.normal.last_usage_check = Some(usage);
            }
        }
        Ok(state)
    }
    fn validated(input: &AccountInput) -> AppResult<(String, String)> {
        if input.name.trim().is_empty() || input.name.chars().count() > 80 {
            return Err("名称不能为空，且不能超过 80 个字。".into());
        }
        if input.note.chars().count() > 2000 {
            return Err("备注不能超过 2000 个字。".into());
        }
        if !["", "pro", "max5", "max20"].contains(&input.plan.as_str()) {
            return Err("请选择有效的套餐标记。".into());
        }
        if input.expected_ip.trim().is_empty() {
            Ok((network::validate_proxy(&input.proxy)?, "".into()))
        } else {
            network::validate_route(&input.proxy, &input.expected_ip)
        }
    }
    fn persist(
        &self,
        env: &impl Environment,
        before: &Database,
        after: Database,
    ) -> AppResult<Database> {
        let changed_current = after.current.as_ref().is_some_and(|id| {
            match (before.account(id), after.account(id)) {
                (Ok(old), Ok(new)) => {
                    old.encrypted_token != new.encrypted_token
                        || old.proxy != new.proxy
                        || old.expected_ip != new.expected_ip
                }
                _ => before.route(id).ok() != after.route(id).ok(),
            }
        });
        if let Some(id) = after.current.as_ref().filter(|_| changed_current) {
            self.clean(after.route(id)?.0)?;
        }
        self.store.select(env, before, after)
    }
    pub fn add(
        &self,
        mut input: AccountInput,
        transport: &impl Transport,
        env: &impl Environment,
    ) -> AppResult<AddResult> {
        if input.id.is_some() {
            return Err("新增账号不能携带已有编号。".into());
        }
        let (proxy, expected_ip) = Self::validated(&input)?;
        let supplied = zeroize::Zeroizing::new(input.token.take().ok_or("请填写 setup token。")?);
        let token = crypto::validate_token(&supplied)?;
        let (_lease, before, mut checks) = self.begin(env)?;
        let network = network::bind_network(transport, &proxy, &expected_ip);
        if !network.ok {
            return Err(network.error.unwrap_or_else(|| "出口检查失败。".into()));
        }
        let expected_ip = network.ip.clone().ok_or("未能绑定账号出口。")?;
        let duplicate = before.accounts.iter().position(|a| {
            crypto::token(&a.encrypted_token).is_ok_and(|saved| saved.as_str() == token.as_str())
        });
        #[cfg(windows)]
        crate::ordinary::check_slot_routes(
            &before,
            &proxy,
            &expected_ip,
            duplicate.map(|i| before.accounts[i].id.as_str()),
        )?;
        if let Some(index) = duplicate {
            if before.accounts[index].expected_ip != expected_ip {
                return Err("该组织已绑定其他出口，不能通过重复添加变更出口。".into());
            }
        }
        if before
            .accounts
            .iter()
            .enumerate()
            .any(|(i, a)| Some(i) != duplicate && a.name == input.name.trim())
        {
            return Err("已有同名账号，请换一个名称。".into());
        }
        let id = duplicate
            .map(|i| before.accounts[i].id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let account = AccountRecord {
            id: id.clone(),
            name: input.name.trim().into(),
            plan: input.plan,
            token_tail: token[token.len() - 6..].into(),
            encrypted_token: crypto::seal(token.as_bytes())?,
            org: duplicate.and_then(|i| before.accounts[i].org.clone()),
            proxy,
            expected_ip,
            note: input.note.trim().into(),
            added_at: duplicate
                .map(|i| before.accounts[i].added_at.clone())
                .unwrap_or_else(now),
        };
        let mut after = before.clone();
        if let Some(index) = duplicate {
            after.accounts[index] = account.clone();
        } else {
            after.accounts.push(account.clone());
        }
        // Invalidate first, so a crash cannot display an old token's quota as new.
        checks.remove(&id);
        self.store.save_checks(&checks)?;
        let after = self.persist(env, &before, after)?;
        let checked = Checks {
            last_network_check: Some(network),
            last_usage_check: None,
        };
        checks.insert(id.clone(), checked.clone());
        self.store.save_checks(&checks)?;
        Ok(AddResult {
            account: Account::view(&account, after.current.as_ref() == Some(&id), &checked),
            replaced: duplicate.is_some(),
        })
    }
    pub fn update(
        &self,
        input: AccountInput,
        transport: &impl Transport,
        env: &impl Environment,
    ) -> AppResult<Account> {
        if input.token.is_some() {
            return Err("请使用更换令牌操作。".into());
        }
        let (proxy, expected_ip) = Self::validated(&input)?;
        let id = input.id.as_deref().ok_or("缺少账号编号。")?;
        let (_lease, before, mut checks) = self.begin(env)?;
        let old = before.account(id)?;
        // Clearing the edit field must not silently rebind an existing account
        // to a changed exit. An explicit old binding remains the trust anchor.
        let expected_ip = if expected_ip.is_empty() {
            if old.expected_ip.is_empty() {
                network::detect_exit(transport, &proxy)?.ip
            } else {
                old.expected_ip.clone()
            }
        } else {
            expected_ip
        };
        #[cfg(windows)]
        crate::ordinary::check_slot_routes(&before, &proxy, &expected_ip, Some(id))?;
        if before
            .accounts
            .iter()
            .any(|a| a.id != id && a.name == input.name.trim())
        {
            return Err("已有同名账号。".into());
        }
        let routing = old.proxy != proxy || old.expected_ip != expected_ip;
        if routing || input.expected_ip.trim().is_empty() {
            let network = network::network(transport, &proxy, &expected_ip);
            if !network.ok {
                return Err(network.error.unwrap_or_else(|| "出口检查失败。".into()));
            }
            if routing {
                checks.remove(id);
                self.store.save_checks(&checks)?;
            }
        }
        let mut after = before.clone();
        let account = after
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("找不到这个账号。")?;
        account.name = input.name.trim().into();
        account.plan = input.plan;
        account.proxy = proxy;
        account.expected_ip = expected_ip;
        account.note = input.note.trim().into();
        let after = self.persist(env, &before, after)?;
        Ok(Account::view(
            after.account(id)?,
            after.current.as_deref() == Some(id),
            checks.get(id).unwrap_or(&Checks::default()),
        ))
    }
    /// Edit the recorded route only after all trace hosts confirm the existing pin.
    /// Paired cards move together under the same storage lease and revision.
    pub fn edit_card(
        &self, input: AccountInput, update_pair: bool, transport: &impl Transport,
        env: &impl Environment,
    ) -> AppResult<()> {
        if input.token.is_some() { return Err("请从更换令牌入口操作。".into()); }
        let (proxy, entered_ip) = Self::validated(&input)?;
        let id = input.id.as_deref().ok_or("缺少账号编号。")?;
        let (_lease, before, mut checks) = self.begin(env)?;
        let (old_proxy, pinned) = before.route(id)?;
        let canonical = |s: &str| s.parse::<std::net::IpAddr>().ok().map(|ip| ip.to_canonical());
        if !pinned.is_empty() && !entered_ip.is_empty() && canonical(pinned) != canonical(&entered_ip) {
            return Err("已绑定的出口 IP 不能更改，新代理必须走这个出口。".into());
        }
        let pin = if pinned.is_empty() { entered_ip.as_str() } else { pinned };
        let routing = proxy != old_proxy || pin != pinned;
        let mut ids = vec![id];
        if routing {
            if pin.is_empty() { return Err("请先核对固定出口 IP。".into()); }
            for card in &before.ordinary_accounts {
                if let Some(source) = card.source_token_id.as_deref() {
                    if source == id || card.id == id && before.account(source).is_ok() {
                        if !update_pair { return Err("请确认同时更新同号两张卡的代理。".into()); }
                        let peer = if source == id { card.id.as_str() } else { source };
                        if canonical(before.route(peer)?.1) != canonical(pin) { return Err("同号两张卡的固定出口记录不一致。".into()); }
                        ids.push(peer);
                    }
                }
            }
            let port = |s: &str| url::Url::parse(s).ok()?.port_or_known_default();
            for other in before.accounts.iter().map(|a| a.id.as_str()).chain(before.ordinary_accounts.iter().map(|a| a.id.as_str())) {
                if !ids.contains(&other) {
                    let (p, ip) = before.route(other)?;
                    if port(p) == port(&proxy) || canonical(pin) == canonical(ip) {
                        return Err("这个端口或出口已绑定其他账号。".into());
                    }
                }
            }
            #[cfg(windows)]
            if self.integrate {
                let home = self.normal_home()?;
                if port(&proxy) == port(&crate::client::normal_proxy(&home)?) && port(&proxy) != port(old_proxy) {
                    let key = normal::identity(&home)?.cache_key();
                    if let Some(key) = key {
                        let pins = self.store.normal_checks()?;
                        if !pins.get(&key).is_some_and(|r| canonical(&r.expected_ip) == canonical(pin)) {
                            return Err("这个代理端口正在供本机账号登录使用。".into());
                        }
                    }
                }
            }
            let check = network::network(transport, &proxy, pin);
            if !check.ok { return Err(check.error.unwrap_or_else(|| "出口核对失败。".into())); }
            for key in &ids { checks.entry((*key).into()).or_default().last_network_check = Some(check.clone()); }
        }
        let name = input.name.trim();
        if before.accounts.iter().any(|a| a.id != id && a.name == name)
            || before.ordinary_accounts.iter().any(|a| a.id != id && a.name == name) {
            let old_name = before.ordinary(id).map(|a| a.name.as_str()).or_else(|| before.account(id).ok().map(|a| a.name.as_str()));
            if old_name != Some(name) { return Err("已有同名账号。".into()); }
        }
        let mut after = before.clone();
        for a in &mut after.accounts {
            if ids.contains(&a.id.as_str()) { a.proxy = proxy.clone(); a.expected_ip = pin.into(); }
            if a.id == id { a.name = name.into(); a.note = input.note.trim().into(); a.plan = input.plan.clone(); }
        }
        for a in &mut after.ordinary_accounts {
            if ids.contains(&a.id.as_str()) { a.proxy = proxy.clone(); a.expected_ip = pin.into(); }
            if a.id == id { a.name = name.into(); a.note = input.note.trim().into(); a.plan = input.plan.clone(); }
        }
        self.persist(env, &before, after)?;
        if routing { self.store.save_checks(&checks)?; }
        Ok(())
    }
    pub fn replace(
        &self,
        id: &str,
        token: &str,
        transport: &impl Transport,
        env: &impl Environment,
    ) -> AppResult<Account> {
        let token = crypto::validate_token(token)?;
        let (_lease, before, mut checks) = self.begin(env)?;
        let old = before.account(id)?;
        if before
            .ordinary_accounts
            .iter()
            .any(|a| a.source_token_id.as_deref() == Some(id))
        {
            return Err("这张令牌卡已与普通登录共用出口，无法核实新令牌的归属，未替换。".into());
        }
        let network = network::network(transport, &old.proxy, &old.expected_ip);
        if !network.ok {
            return Err(network.error.unwrap_or_else(|| "出口检查失败。".into()));
        }
        if before.accounts.iter().any(|a| {
            a.id != id
                && crypto::token(&a.encrypted_token)
                    .is_ok_and(|saved| saved.as_str() == token.as_str())
        }) {
            return Err("新令牌属于列表中的另一个账号，请在对应账号中更换。".into());
        }
        let mut after = before.clone();
        let account = after
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or("找不到这个账号。")?;
        account.encrypted_token = crypto::seal(token.as_bytes())?;
        account.token_tail = token[token.len() - 6..].into();
        account.org = None;
        checks.remove(id);
        self.store.save_checks(&checks)?;
        let after = self.persist(env, &before, after)?;
        let checked = Checks {
            last_network_check: Some(network),
            last_usage_check: None,
        };
        checks.insert(id.into(), checked.clone());
        self.store.save_checks(&checks)?;
        Ok(Account::view(
            after.account(id)?,
            after.current.as_deref() == Some(id),
            &checked,
        ))
    }
    pub fn switch(
        &self,
        id: &str,
        transport: &impl Transport,
        env: &impl Environment,
    ) -> AppResult<SwitchResult> {
        let (_lease, before, mut checks) = self.begin(env)?;
        let (proxy, ip) = before.route(id)?;
        #[cfg(windows)]
        if let Some(a) = before.ordinary(id) {
            crate::ordinary::verify_slot(&self.store, a)?;
        }
        self.clean(proxy)?;
        let network = network::network(transport, proxy, ip);
        checks.entry(id.into()).or_default().last_network_check = Some(network.clone());
        self.store.save_checks(&checks)?;
        if !network.ok {
            return Err(network.error.unwrap_or_else(|| "出口检查失败。".into()));
        }
        self.hook()?;
        let mut after = before.clone();
        after.current = Some(id.into());
        self.store.select(env, &before, after)?;
        Ok(SwitchResult { switched_at: now() })
    }
    pub fn normal(&self, env: &impl Environment) -> AppResult<SwitchResult> {
        let (_lease, before, _) = self.begin(env)?;
        let mut after = before.clone();
        after.current = None;
        self.store.select(env, &before, after)?;
        Ok(SwitchResult { switched_at: now() })
    }
    pub fn delete(&self, id: &str, env: &impl Environment) -> AppResult<DeleteResult> {
        let (_lease, before, mut checks) = self.begin(env)?;
        #[cfg(windows)]
        if before.ordinary(id).is_some() {
            if before.current.as_deref() == Some(id) { return Err("请先切到其他账号，再删除当前账号。".into()); }
            self.finish_login_removal(&before)?;
            crate::ordinary::home(&self.store, id)?;
            platform::write_json(&self.store.root.join("pending-login-removal.json"), &id)?;
            let mut after = before.clone();
            after.ordinary_accounts.retain(|a| a.id != id);
            let committed = self.store.select(env, &before, after)?;
            self.finish_login_removal(&committed)?;
            return Ok(DeleteResult { fell_back_to_normal: false });
        }
        before.account(id)?;
        let fell_back_to_normal = before.current.as_deref() == Some(id);
        let mut after = before.clone();
        after.accounts.retain(|a| a.id != id);
        for a in &mut after.ordinary_accounts {
            if a.source_token_id.as_deref() == Some(id) { a.source_token_id = None; }
        }
        if fell_back_to_normal {
            after.current = None;
        }
        self.store.select(env, &before, after)?;
        checks.remove(id);
        self.store.save_checks(&checks)?;
        Ok(DeleteResult {
            fell_back_to_normal,
        })
    }
    pub fn check_network(
        &self,
        id: &str,
        transport: &impl Transport,
        env: &impl Environment,
    ) -> AppResult<NetworkCheck> {
        let (_lease, db, mut checks) = self.begin(env)?;
        let (proxy, ip) = db.route(id)?;
        let check = network::network(transport, proxy, ip);
        checks.entry(id.into()).or_default().last_network_check = Some(check.clone());
        self.store.save_checks(&checks)?;
        Ok(check)
    }
    #[cfg(test)]
    pub fn check_usage(
        &self,
        id: &str,
        transport: &impl Transport,
        env: &impl Environment,
    ) -> AppResult<UsageCheck> {
        let (_lease, db, mut checks) = self.begin(env)?;
        let a = db.account(id)?;
        if let Some(cached) = checks.get(id).and_then(|c| c.last_usage_check.as_ref()) {
            if chrono::DateTime::parse_from_rfc3339(&cached.checked_at)
                .ok()
                .is_some_and(|t| {
                    let seconds = chrono::Utc::now().signed_duration_since(t).num_seconds();
                    (0..60).contains(&seconds)
                })
            {
                return Ok(cached.clone());
            }
        }
        let network = network::network(transport, &a.proxy, &a.expected_ip);
        let usage = if !network.ok {
            UsageCheck::error(
                "neterr",
                network.error.as_deref().unwrap_or("出口检查失败。"),
            )
        } else {
            let token = crypto::token(&a.encrypted_token)?;
            let probe = network::probe(transport, &a.proxy, &token);
            if probe.usage.ok && a.org.is_some() && probe.org != a.org {
                UsageCheck::error(
                    "invalid",
                    "令牌的组织编号与保存记录不一致，请重新核验令牌。",
                )
            } else {
                probe.usage
            }
        };
        checks.insert(
            id.into(),
            Checks {
                last_network_check: Some(network),
                last_usage_check: Some(usage.clone()),
            },
        );
        self.store.save_checks(&checks)?;
        Ok(usage)
    }
    pub fn detect_exit(&self, proxy: &str, transport: &impl Transport) -> AppResult<DetectedExit> {
        let _lease = self.store.lock()?;
        network::detect_exit(transport, proxy)
    }
    #[cfg(test)]
    pub fn check_normal_usage(
        &self,
        transport: &impl Transport,
        env: &impl Environment,
    ) -> AppResult<UsageCheck> {
        let (_lease, _db, _checks) = self.begin(env)?;
        let home = self.normal_home()?;
        let identity = normal::identity(&home)?;
        let identity_key = identity.cache_key();
        let key = identity_key.clone().unwrap_or_else(|| "no-login".into());
        let mut all = self.store.normal_checks()?;
        let mut record = all.get(&key).cloned().unwrap_or_default();
        let usage = (|| -> AppResult<UsageCheck> {
            if identity_key.is_none() {
                return Ok(UsageCheck::error(
                    "invalid",
                    "未找到普通登录账号信息，请先在 Claude Code 中登录",
                ));
            }
            let credential = match normal::credentials(&home, self.integrate) {
                Ok(credential) => credential,
                Err(error) => return Ok(*error),
            };
            let network = network::bind_network(transport, DEFAULT_PROXY, &record.expected_ip);
            if !network.ok {
                return Ok(UsageCheck::error(
                    "neterr",
                    network.error.as_deref().unwrap_or("出口检查失败"),
                ));
            }
            if record.expected_ip.is_empty() {
                record.expected_ip = network.ip.ok_or("未能绑定普通登录出口。")?;
                all.insert(key.clone(), record.clone());
                // Persist the first verified route before sending a credential.
                self.store.save_normal_checks(&all)?;
            }
            if !credential.fresh_at(chrono::Utc::now().timestamp_millis()) {
                return Ok(UsageCheck::error("expired", normal::EXPIRED));
            }
            if normal::identity(&home)?.cache_key() != identity_key {
                return Ok(UsageCheck::error(
                    "invalid",
                    "普通登录账号已变化，请重新查询",
                ));
            }
            let response = match transport.oauth(
                DEFAULT_PROXY,
                &credential.value,
                network::OAuthEndpoint::Usage,
            ) {
                Ok(response) => response,
                Err(_) => {
                    return Ok(UsageCheck::error(
                        "neterr",
                        "普通登录额度查询失败，请检查本机代理",
                    ))
                }
            };
            if response.status == 401 {
                return Ok(UsageCheck::error("expired", normal::EXPIRED));
            }
            if response.status != 200 {
                return Ok(UsageCheck::error(
                    "error",
                    if response.status == 403 {
                        "普通登录令牌没有读取额度的权限，请核对 Claude Code 登录"
                    } else {
                        "普通登录额度查询暂未成功，请稍后重试"
                    },
                ));
            }
            let usage = match normal::usage(&response.body) {
                Ok(usage) => usage,
                Err(error) => return Ok(UsageCheck::error("error", &error)),
            };
            if !credential.fresh_at(chrono::Utc::now().timestamp_millis()) {
                return Ok(UsageCheck::error("expired", normal::EXPIRED));
            }
            // Profile is metadata, independent of successful quota retrieval.
            // A transient failure preserves the local/last known plan; 401 is
            // always expired and never invokes any token refresh endpoint.
            if let Ok(response) = transport.oauth(
                DEFAULT_PROXY,
                &credential.value,
                network::OAuthEndpoint::Profile,
            ) {
                if response.status == 401 {
                    return Ok(UsageCheck::error("expired", normal::EXPIRED));
                }
                if response.status == 200 {
                    if let Ok(profile) = normal::profile(&response.body) {
                        record.profile = Some(profile);
                    }
                }
            }
            if normal::identity(&home)?.cache_key() != identity_key {
                return Ok(UsageCheck::error(
                    "invalid",
                    "普通登录账号已变化，请重新查询",
                ));
            }
            Ok(usage)
        })()?;
        record.last_usage_check = Some(usage.clone());
        all.insert(key, record);
        self.store.save_normal_checks(&all)?;
        Ok(usage)
    }
    pub fn uninstall(&self, env: &impl Environment) -> AppResult<()> {
        self.normal(env)?;
        let _lease = self.store.lock()?;
        if self.integrate {
            hooks::uninstall(&self.store)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        network::Response,
        registry::{testing::FakeEnvironment, EnvValue},
    };
    use std::{cell::Cell, collections::BTreeMap};
    struct Net {
        calls: Cell<usize>,
        wrong: Cell<bool>,
    }
    impl Transport for Net {
        fn trace(&self, _: &str, _: &str) -> AppResult<String> {
            Ok(format!(
                "ip={}\n",
                if self.wrong.get() {
                    "203.0.113.99"
                } else {
                    "203.0.113.1"
                }
            ))
        }
        fn message(&self, _: &str, _: &str, _: &str) -> AppResult<Response> {
            self.calls.set(self.calls.get() + 1);
            Ok(Response {
                status: 200,
                body: "{}".into(),
                headers: BTreeMap::from([
                    (
                        "anthropic-ratelimit-unified-5h-utilization".into(),
                        "0.42".into(),
                    ),
                    ("anthropic-organization-id".into(), "org-fixture".into()),
                ]),
            })
        }
    }
    fn input(plan: &str) -> AccountInput {
        AccountInput {
            id: None,
            name: "样例".into(),
            plan: plan.into(),
            token: Some("sk-ant-oat01-aaaaaaaaaaaaaaaaaaaa".into()),
            proxy: DEFAULT_PROXY.into(),
            expected_ip: "203.0.113.1".into(),
            note: "备注".into(),
        }
    }
    #[test]
    fn account_lifecycle_dedup_plan_cooldown_switch_delete() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager {
            store: Store {
                root: dir.path().into(),
            },
            integrate: false,
        };
        let env = FakeEnvironment::default();
        let net = Net {
            calls: Cell::new(0),
            wrong: Cell::new(false),
        };
        let first = manager.add(input("max5"), &net, &env).unwrap();
        let second = manager.add(input("max20"), &net, &env).unwrap();
        assert!(second.replaced);
        assert_eq!(first.account.id, second.account.id);
        assert_eq!(second.account.plan, "max20");
        let id = &first.account.id;
        assert_eq!(
            net.calls.get(),
            0,
            "adding/replacing must not send model probes"
        );
        assert!(second.account.last_usage_check.is_none());
        let before = net.calls.get();
        manager.check_usage(id, &net, &env).unwrap();
        assert_eq!(net.calls.get(), before + 1);
        manager.check_usage(id, &net, &env).unwrap();
        assert_eq!(net.calls.get(), before + 1, "legacy mock check is cached");
        manager.switch(id, &net, &env).unwrap();
        assert!(manager.get_state(&env).unwrap().accounts[0].current);
        assert!(env.read(ENV_KEYS[0]).unwrap().is_none());
        assert_eq!(env.writes.get(), 0, "selection must not change global env");
        env.write("HTTP_PROXY", Some(&EnvValue::text("manual")))
            .unwrap();
        let deleted = manager.delete(id, &env).unwrap();
        assert!(deleted.fell_back_to_normal);
        assert!(env.read(ENV_KEYS[0]).unwrap().is_none());
        assert!(env.read("HTTP_PROXY").unwrap() == Some(EnvValue::text("manual")));
        assert!(manager.get_state(&env).unwrap().normal.current);
        assert!(!std::fs::read_to_string(dir.path().join("accounts.json"))
            .unwrap()
            .contains("sk-ant-oat01"));
    }
    #[test]
    fn mismatch_never_probes_or_switches_and_dto_never_contains_token() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager {
            store: Store {
                root: dir.path().into(),
            },
            integrate: false,
        };
        let env = FakeEnvironment::default();
        let net = Net {
            calls: Cell::new(0),
            wrong: Cell::new(true),
        };
        assert!(manager.add(input("pro"), &net, &env).is_err());
        assert_eq!(net.calls.get(), 0);
        net.wrong.set(false);
        let account = manager.add(input(""), &net, &env).unwrap().account;
        let json = serde_json::to_string(&account).unwrap();
        assert!(!json.contains("encrypted"));
        assert!(!json.contains("sk-ant-oat01"));
        assert!(json.contains("tokenTail"));
        net.wrong.set(true);
        assert!(manager.switch(&account.id, &net, &env).is_err());
        assert_eq!(env.writes.get(), 0);
    }
    #[test]
    fn edit_linked_login_offline_and_verify_proxy_changes_for_both_cards() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::isolated(dir.path().into());
        let env = FakeEnvironment::default();
        let net = Net { calls: Cell::new(0), wrong: Cell::new(false) };
        let token = m.add(input("pro"), &net, &env).unwrap().account.id;
        let id = "login-10000000-0000-4000-8000-000000000001";
        let mut db = m.store.read().unwrap();
        db.schema = ORDINARY_SCHEMA;
        db.ordinary_accounts.push(OrdinaryRecord {
            id: id.into(), name: "账号登录".into(), source_token_id: Some(token.clone()),
            note: String::new(), plan: "pro".into(), proxy: DEFAULT_PROXY.into(),
            expected_ip: "203.0.113.1".into(), account_uuid: "sample".into(), org: "org-fixture".into(),
        });
        m.store.save(&db).unwrap();
        let edit = |proxy: &str| {
            let mut data = input("max20");
            data.id = Some(id.into()); data.name = "别名".into();
            data.token = None; data.proxy = proxy.into(); data
        };
        net.wrong.set(true);
        m.edit_card(edit(DEFAULT_PROXY), false, &net, &env).unwrap();
        let proxy = "http://127.0.0.1:31002";
        assert!(m.edit_card(edit(proxy), true, &net, &env).is_err());
        net.wrong.set(false);
        assert!(m.edit_card(edit(proxy), false, &net, &env).is_err());
        assert_eq!(m.store.read().unwrap().route(&token).unwrap().0, DEFAULT_PROXY);
        m.edit_card(edit(proxy), true, &net, &env).unwrap();
        let mut wrong_pin = edit(proxy);
        wrong_pin.expected_ip = "203.0.113.99".into();
        assert!(m.edit_card(wrong_pin, true, &net, &env).is_err());
        let db = m.store.read().unwrap();
        assert_eq!(db.route(id).unwrap(), (proxy, "203.0.113.1"));
        assert_eq!(db.route(&token).unwrap(), db.route(id).unwrap());
        assert_eq!(db.account(&token).unwrap().name, "样例");
        assert_eq!(db.ordinary(id).unwrap().name, "别名");
        assert_eq!(net.calls.get(), 0);
        #[cfg(windows)]
        {
            let slot = crate::ordinary::home(&m.store, id).unwrap();
            std::fs::create_dir_all(&slot).unwrap();
            std::fs::write(slot.join("fixture"), b"test").unwrap();
            // An interrupted removal before database commit preserves the login on restart.
            platform::write_json(&m.store.root.join("pending-login-removal.json"), &id).unwrap();
            m.begin(&env).unwrap();
            assert!(slot.exists());
            m.delete(id, &env).unwrap();
            assert!(!slot.exists());
            assert!(m.store.read().unwrap().account(&token).is_ok());
        }
    }
    #[test]
    fn blank_exit_binds_on_add_but_cannot_rebind_on_edit() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::isolated(dir.path().into());
        let env = FakeEnvironment::default();
        let net = Net {
            calls: Cell::new(0),
            wrong: Cell::new(false),
        };
        let mut create = input("pro");
        create.expected_ip.clear();
        let a = manager.add(create, &net, &env).unwrap().account;
        assert_eq!(a.expected_ip, "203.0.113.1");
        let mut edit = input("max5");
        edit.id = Some(a.id.clone());
        edit.token = None;
        edit.expected_ip.clear();
        let mut bad = AccountInput {
            id: edit.id.clone(),
            name: edit.name.clone(),
            plan: edit.plan.clone(),
            token: None,
            proxy: edit.proxy.clone(),
            expected_ip: "".into(),
            note: "".into(),
        };
        assert_eq!(
            manager.update(edit, &net, &env).unwrap().expected_ip,
            "203.0.113.1"
        );
        net.wrong.set(true);
        bad.proxy = "http://127.0.0.1:7898".into();
        assert!(manager.update(bad, &net, &env).is_err());
        assert_eq!(
            manager
                .store
                .read()
                .unwrap()
                .account(&a.id)
                .unwrap()
                .expected_ip,
            "203.0.113.1"
        );
        assert_eq!(
            net.calls.get(),
            0,
            "adding or editing never sends a model probe"
        );
    }
    struct NormalNet {
        trace_calls: Cell<usize>,
        calls: std::cell::RefCell<Vec<network::OAuthEndpoint>>,
        wrong: Cell<bool>,
        usage_status: Cell<u16>,
        profile_status: Cell<u16>,
    }
    impl NormalNet {
        fn new() -> Self {
            Self {
                trace_calls: Cell::new(0),
                calls: Default::default(),
                wrong: Cell::new(false),
                usage_status: Cell::new(200),
                profile_status: Cell::new(200),
            }
        }
    }
    impl Transport for NormalNet {
        fn trace(&self, proxy: &str, _host: &str) -> AppResult<String> {
            assert_eq!(proxy, DEFAULT_PROXY);
            self.trace_calls.set(self.trace_calls.get() + 1);
            Ok(format!(
                "ip={}\n",
                if self.wrong.get() {
                    "203.0.113.99"
                } else {
                    "203.0.113.1"
                }
            ))
        }
        fn message(&self, _: &str, _: &str, _: &str) -> AppResult<Response> {
            panic!("Ordinary OAuth queries must never call a model");
        }
        fn oauth(
            &self,
            proxy: &str,
            token: &str,
            endpoint: network::OAuthEndpoint,
        ) -> AppResult<Response> {
            assert_eq!(proxy, DEFAULT_PROXY);
            assert!(token == "normal-fixture-access");
            self.calls.borrow_mut().push(endpoint);
            let (status, body) = match endpoint {
                network::OAuthEndpoint::Usage => (
                    self.usage_status.get(),
                    r#"{"five_hour":{"utilization":43.2,"resets_at":null},"seven_day":{"utilization":7}}"#,
                ),
                network::OAuthEndpoint::Profile => (
                    self.profile_status.get(),
                    r#"{"has_claude_pro":true,"organization":{"subscription_status":"canceled"}}"#,
                ),
            };
            Ok(Response {
                status,
                headers: BTreeMap::new(),
                body: if status == 200 {
                    body.into()
                } else {
                    r#"{"error":{"message":"secret-server-diagnostic","refreshToken":"never-copy-this"}}"#.into()
                },
            })
        }
    }
    fn normal_fixture(manager: &Manager, remaining_ms: i64) -> std::path::PathBuf {
        let home = manager.normal_home().unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            serde_json::json!({"oauthAccount":{
            "emailAddress":"fixture@example.test","organizationUuid":"ordinary-org",
            "organizationType":"claude_max","organizationRateLimitTier":"default_claude_max_20x",
            "billingType":"apple_subscription"}})
            .to_string(),
        )
        .unwrap();
        let path = home.join(".claude/.credentials.json");
        std::fs::write(&path,serde_json::json!({"claudeAiOauth":{
            "accessToken":"normal-fixture-access","expiresAt":chrono::Utc::now().timestamp_millis()+remaining_ms,
            "refreshToken":"never-copy-this","scopes":["user:profile"]}}).to_string()).unwrap();
        path
    }
    #[test]
    fn normal_usage_binds_once_profile_overrides_offline_and_credentials_stay_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::isolated(dir.path().into());
        let path = normal_fixture(&manager, 3_600_000);
        let before = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let env = FakeEnvironment::default();
        let net = NormalNet::new();
        assert_eq!(manager.get_state(&env).unwrap().normal.plan, "max20");
        let usage = manager.check_normal_usage(&net, &env).unwrap();
        assert_eq!(usage.buckets.as_ref().unwrap()[0].utilization, 43.2);
        let state = manager.get_state(&env).unwrap().normal;
        assert_eq!(state.plan, "pro");
        assert_eq!(state.subscription_status.as_deref(), Some("canceled"));
        assert_eq!(state.billing_type.as_deref(), Some("apple_subscription"));
        assert_eq!(state.expected_ip, "203.0.113.1");
        assert_eq!(net.calls.borrow().len(), 2);
        #[cfg(windows)]
        assert!(state.last_usage_check.is_none()); // org-only legacy record has no verified owner
        #[cfg(not(windows))]
        assert!(state.last_usage_check.unwrap().ok);
        assert_eq!(before, std::fs::read(&path).unwrap());
        assert_eq!(
            modified,
            std::fs::metadata(&path).unwrap().modified().unwrap()
        );
        let disk = std::fs::read_to_string(dir.path().join("normal-checks.json")).unwrap();
        assert!(!disk.contains("normal-fixture-access"));
        assert!(!disk.contains("never-copy-this"));
        net.wrong.set(true);
        assert_eq!(
            manager.check_normal_usage(&net, &env).unwrap().state,
            "neterr"
        );
        assert_eq!(net.calls.borrow().len(), 2);
        assert_eq!(env.writes.get(), 0);
        assert_eq!(
            manager.get_state(&env).unwrap().normal.expected_ip,
            "203.0.113.1"
        );
        std::fs::write(manager.normal_home().unwrap().join(".claude.json"),
            r#"{"oauthAccount":{"organizationUuid":"a-different-org","organizationType":"claude_pro"}}"#).unwrap();
        let other = manager.get_state(&env).unwrap().normal;
        assert_eq!(other.plan, "pro");
        assert!(other.expected_ip.is_empty());
        assert!(other.last_usage_check.is_none());
        assert!(other.subscription_status.is_none());
    }
    #[test]
    fn normal_expired_and_401_never_refresh_or_return_raw_errors() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::isolated(dir.path().into());
        let path = normal_fixture(&manager, 59_000);
        let env = FakeEnvironment::default();
        let net = NormalNet::new();
        let bytes = std::fs::read(&path).unwrap();
        let expired = manager.check_normal_usage(&net, &env).unwrap();
        assert_eq!(expired.state, "expired");
        assert_eq!(expired.error.as_deref(), Some(normal::EXPIRED));
        assert_eq!(net.trace_calls.get(), 0);
        assert!(net.calls.borrow().is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        normal_fixture(&manager, 3_600_000);
        for endpoint in [
            network::OAuthEndpoint::Usage,
            network::OAuthEndpoint::Profile,
        ] {
            let net = NormalNet::new();
            match endpoint {
                network::OAuthEndpoint::Usage => net.usage_status.set(401),
                network::OAuthEndpoint::Profile => net.profile_status.set(401),
            };
            let bytes = std::fs::read(&path).unwrap();
            let result = manager.check_normal_usage(&net, &env).unwrap();
            assert_eq!(result.state, "expired");
            assert_eq!(result.error.as_deref(), Some(normal::EXPIRED));
            assert_eq!(
                net.calls.borrow().len(),
                if endpoint == network::OAuthEndpoint::Usage {
                    1
                } else {
                    2
                }
            );
            assert!(!serde_json::to_string(&result)
                .unwrap()
                .contains("secret-server"));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        assert_eq!(env.writes.get(), 0);
    }
    #[test]
    #[ignore = "Requires explicit fixed-exit preflight; read-only live OAuth query, temporary cache only"]
    fn live_normal_oauth_read_only() {
        let expected = std::env::var("CAM_LIVE_NORMAL_EXPECTED_IP").expect("请先指定已知固定出口");
        let transport = network::Curl;
        let preflight = network::network(&transport, DEFAULT_PROXY, &expected);
        assert!(preflight.ok, "已知出口核验未通过，未读取凭据或发送认证请求");
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager {
            store: Store {
                root: dir.path().into(),
            },
            integrate: true,
        };
        let environment = crate::registry::UserEnvironment;
        let before = environment.snapshot().unwrap();
        let result = manager
            .check_normal_usage(&transport, &environment)
            .unwrap();
        let state = manager.get_state(&environment).unwrap();
        assert!(
            before == environment.snapshot().unwrap(),
            "环境变量在只读核验期间发生变化"
        );
        assert!(
            result.ok || result.state == "expired",
            "只读 OAuth 查询失败：{}",
            result.error.as_deref().unwrap_or("无详情")
        );
        eprintln!(
            "NORMAL_LIVE state={} plan={} buckets={} profile_cached={}",
            result.state,
            state.normal.plan,
            result.buckets.as_ref().map_or(0, Vec::len),
            manager
                .store
                .normal_checks()
                .unwrap()
                .values()
                .any(|v| v.profile.is_some())
        );
    }
}
