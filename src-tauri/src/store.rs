use crate::{
    crypto,
    model::*,
    platform,
    registry::{self, Environment, Snapshot},
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, time::Duration};

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    before: Database,
    after_revision: u64,
    before_env: Snapshot,
    after_env: Snapshot,
}

impl Store {
    pub fn discover() -> AppResult<Self> {
        #[cfg(windows)]
        let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("找不到本机数据目录。")?);
        #[cfg(not(windows))]
        let base = crate::conflicts::home()?.join("Library/Application Support");
        let root = platform::checked_path(&base.join("ClaudeAccountManager"))?;
        let folded = root.to_string_lossy().to_lowercase();
        if folded.contains("onedrive") || folded.contains("/mobile documents/") {
            return Err("凭据目录不能放在 OneDrive 中。".into());
        }
        if !root.is_dir() {
            platform::private_directory(&root)?;
        }
        Ok(Self { root })
    }
    pub fn lock(&self) -> AppResult<platform::Lease> {
        // Shared by every app instance and the terminal helper, across processes.
        platform::lock(&self.root.join("manager.lock"), Duration::from_secs(120))
    }
    pub fn read(&self) -> AppResult<Database> {
        let db: Database =
            platform::read_json(&self.root.join("accounts.json"))?.unwrap_or_else(Database::empty);
        if ![2, DATABASE_SCHEMA, ORDINARY_SCHEMA].contains(&db.schema) {
            return Err("账号数据版本不支持，未覆盖原文件。".into());
        }
        if let Some(id) = &db.current {
            db.route(id)?;
        }
        Ok(db)
    }
    pub fn save(&self, db: &Database) -> AppResult<()> {
        platform::write_json(&self.root.join("accounts.json"), db)
    }
    pub fn checks(&self) -> AppResult<CheckStore> {
        Ok(platform::read_json(&self.root.join("checks.json"))?.unwrap_or_default())
    }
    pub fn save_checks(&self, checks: &CheckStore) -> AppResult<()> {
        platform::write_json(&self.root.join("checks.json"), checks)
    }
    pub fn normal_checks(&self) -> AppResult<NormalStore> {
        Ok(platform::read_json(&self.root.join("normal-checks.json"))?.unwrap_or_default())
    }
    pub fn save_normal_checks(&self, checks: &NormalStore) -> AppResult<()> {
        platform::write_json(&self.root.join("normal-checks.json"), checks)
    }
    pub fn backup(&self, label: &str, bytes: &[u8]) -> AppResult<()> {
        let directory = self.root.join("backups");
        if !directory.is_dir() {
            platform::private_directory(&directory)?;
        }
        platform::write_json(
            &directory.join(format!(
                "{label}-{}.{}.json",
                uuid::Uuid::new_v4(),
                crypto::STORAGE_TAG
            )),
            &crypto::seal(bytes)?,
        )
    }
    fn journal(&self, journal: &Journal) -> AppResult<()> {
        platform::write_json(&self.journal_path(), &registry::seal_json(journal)?)
    }
    fn clear_journal(&self) -> AppResult<()> {
        fs::remove_file(self.journal_path())
            .map_err(|_| "切换已记录，但清理事务记录失败；重新打开可恢复。".into())
    }
    fn journal_path(&self) -> PathBuf {
        self.root
            .join(format!("switch-journal.{}.json", crypto::STORAGE_TAG))
    }
    /// Call under the store lock before any operation. A committed state finishes
    /// cleanup; an uncommitted state rolls back only values still owned by the transaction.
    pub fn recover(&self, environment: &impl Environment) -> AppResult<()> {
        let Some(encrypted): Option<String> = platform::read_json(&self.journal_path())? else {
            return Ok(());
        };
        let journal: Journal = serde_json::from_slice(&crypto::unseal(&encrypted)?)
            .map_err(|_| "切换恢复记录损坏，已停止。")?;
        let db = self.read()?;
        if db.revision == journal.after_revision {
            self.clear_journal()?;
            environment.broadcast();
            return Ok(());
        }
        if db.revision != journal.before.revision {
            return Err("账号数据在切换中被修改，需检查恢复记录。".into());
        }
        let mut external = false;
        for key in ENV_KEYS {
            let before = journal.before_env.get(key).cloned().flatten();
            let after = journal.after_env.get(key).cloned().flatten();
            let actual = environment.read(key)?;
            if actual == after {
                if actual != before {
                    environment.write(key, before.as_ref())?;
                }
            } else if actual != before {
                external = true;
            }
        }
        self.save(&journal.before)?;
        self.clear_journal()?;
        environment.broadcast();
        if external {
            Err("切换已回滚；检测到手动修改的环境变量，已保留。".into())
        } else {
            Ok(())
        }
    }
    /// Call under the store lock. Selection lives only in accounts.json. The
    /// only environment writes allowed here remove values still owned by v2.
    /// The old encrypted WAL remains supported for crash-safe v2 migration.
    pub fn select(
        &self,
        environment: &impl Environment,
        before: &Database,
        mut after: Database,
    ) -> AppResult<Database> {
        after.revision = before.revision.checked_add(1).ok_or("状态版本过大。")?;
        after.schema = if after.ordinary_accounts.is_empty() && before.schema < ORDINARY_SCHEMA {
            DATABASE_SCHEMA
        } else {
            ORDINARY_SCHEMA
        };
        after.owned_env = None;
        if let Some(id) = &after.current {
            after.route(id)?;
        }
        if before.schema == 2 {
            self.backup(
                "before-process-isolation",
                &serde_json::to_vec(before).map_err(|_| "无法备份旧版账号状态。")?,
            )?;
        }
        if before.owned_env.is_none() {
            // Normal operation: no token decryption, registry reads/writes or
            // WM_SETTINGCHANGE/launchctl/global shell updates at switch time.
            self.save(&after)?;
            return Ok(after);
        }
        let actual = environment.snapshot()?;
        let owned = registry::owned(before)?;
        let mut desired = actual.clone();
        for key in ENV_KEYS {
            if let Some(Some(value)) = owned.get(key) {
                // Keep fingerprints to scrub stale v2 values in old terminals
                // when launching normal login. Never claim a manual value.
                after
                    .managed_hashes
                    .entry(key.into())
                    .or_default()
                    .insert(value.fingerprint());
                if actual.get(key) == Some(&Some(value.clone())) {
                    desired.insert(key.into(), None);
                }
            }
        }
        if desired == actual {
            self.save(&after)?;
            return Ok(after);
        }
        let journal = Journal {
            before: before.clone(),
            after_revision: after.revision,
            before_env: actual.clone(),
            after_env: desired.clone(),
        };
        self.journal(&journal)?;
        let result = (|| {
            for key in ENV_KEYS {
                let target = desired.get(key).cloned().flatten();
                // Detect another writer instead of silently overwriting its newer value.
                if environment.read(key)? != actual.get(key).cloned().flatten() {
                    return Err("环境变量被其他程序修改，已取消切换。".into());
                }
                if target != actual.get(key).cloned().flatten() {
                    environment.write(key, target.as_ref())?;
                }
            }
            if environment.snapshot()? != desired {
                return Err("环境变量校验失败，已取消切换。".into());
            }
            self.save(&after)
        })();
        if let Err(error) = result {
            if self.recover(environment).is_err() {
                return Err("切换失败，恢复记录已保留；请重新打开管理器完成恢复。".into());
            }
            return Err(error);
        }
        self.clear_journal()?;
        environment.broadcast();
        Ok(after)
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
