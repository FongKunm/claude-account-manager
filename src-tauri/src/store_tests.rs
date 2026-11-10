use super::*;
use crate::registry::{testing::FakeEnvironment, EnvValue};

fn fixture() -> (tempfile::TempDir, Store, Database) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store {
        root: dir.path().to_owned(),
    };
    let mut db = Database::empty();
    for (id, port) in [("one", 19001), ("two", 19002)] {
        db.accounts.push(AccountRecord {
            id: id.into(),
            name: id.into(),
            plan: "pro".into(),
            token_tail: "fixture".into(),
            encrypted_token: crypto::seal(format!("fake-token-{id}").as_bytes()).unwrap(),
            org: None,
            proxy: format!("http://127.0.0.1:{port}"),
            expected_ip: "203.0.113.1".into(),
            note: String::new(),
            added_at: now(),
        });
    }
    store.save(&db).unwrap();
    (dir, store, db)
}

fn legacy(store: &Store, mut db: Database, env: &FakeEnvironment) -> Database {
    db.schema = 2;
    db.current = Some("one".into());
    for key in ENV_KEYS {
        env.write(key, Some(&EnvValue::text(&format!("fake-old-{key}"))))
            .unwrap();
    }
    let owned = env.snapshot().unwrap();
    db.owned_env = Some(registry::seal_json(&owned).unwrap());
    store.save(&db).unwrap();
    env.writes.set(0);
    db
}

#[test]
fn switches_edits_and_normal_never_write_global_environment() {
    let (_dir, store, mut before) = fixture();
    let env = FakeEnvironment::default();
    env.write(ENV_KEYS[0], Some(&EnvValue::text("manual-token")))
        .unwrap();
    env.write("HTTP_PROXY", Some(&EnvValue::text("manual-proxy")))
        .unwrap();
    let original = env.snapshot().unwrap();
    env.writes.set(0);
    for id in [Some("one"), Some("two"), Some("two"), None] {
        let mut after = before.clone();
        after.current = id.map(String::from);
        after.accounts[1].proxy = "http://127.0.0.1:19003".into();
        before = store.select(&env, &before, after).unwrap();
        assert!(env.snapshot().unwrap() == original);
        assert!(before.owned_env.is_none());
        assert_eq!(env.writes.get(), 0);
    }
    assert_eq!(store.read().unwrap().revision, 4);
}

#[test]
fn legacy_migration_removes_only_owned_values_and_is_idempotent() {
    let (dir, store, db) = fixture();
    let env = FakeEnvironment::default();
    let before = legacy(&store, db, &env);
    env.write("HTTP_PROXY", Some(&EnvValue::text("manual-proxy")))
        .unwrap();
    let after = store.select(&env, &before, before.clone()).unwrap();
    assert_eq!(after.schema, DATABASE_SCHEMA);
    assert_eq!(after.current.as_deref(), Some("one"));
    assert!(after.owned_env.is_none());
    assert!(env.read(ENV_KEYS[0]).unwrap().is_none());
    assert!(env.read("HTTPS_PROXY").unwrap().is_none());
    assert!(env.read("HTTP_PROXY").unwrap() == Some(EnvValue::text("manual-proxy")));
    assert!(after.managed_hashes[ENV_KEYS[0]]
        .contains(&EnvValue::text("fake-old-CLAUDE_CODE_OAUTH_TOKEN").fingerprint()));
    let writes = env.writes.get();
    store.select(&env, &after, after.clone()).unwrap();
    assert_eq!(env.writes.get(), writes);
    assert!(!store.journal_path().exists());
    for entry in fs::read_dir(dir.path().join("backups")).unwrap() {
        let bytes = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!bytes.contains("fake-token"));
        assert!(!bytes.contains("fake-old"));
    }
}

#[test]
fn migration_partial_failure_restores_state_and_owned_environment() {
    let (_dir, store, db) = fixture();
    let env = FakeEnvironment::default();
    let before = legacy(&store, db, &env);
    let original = env.snapshot().unwrap();
    env.fail_on.set(Some(2));
    let mut after = before.clone();
    after.current = Some("two".into());
    assert!(store.select(&env, &before, after).is_err());
    assert!(env.snapshot().unwrap() == original);
    assert_eq!(store.read().unwrap().schema, 2);
    assert_eq!(store.read().unwrap().current.as_deref(), Some("one"));
    assert!(!store.journal_path().exists());
}

#[test]
fn old_journal_recovery_and_committed_migration_recovery() {
    let (_dir, store, before) = fixture();
    let env = FakeEnvironment::default();
    let original = env.snapshot().unwrap();
    let want: Snapshot = ENV_KEYS
        .into_iter()
        .map(|k| (k.into(), Some(EnvValue::text("fake-old-value"))))
        .collect();
    // Recover a half-written v2 switch, including its owned registry write.
    store
        .journal(&Journal {
            before: before.clone(),
            after_revision: 1,
            before_env: original.clone(),
            after_env: want.clone(),
        })
        .unwrap();
    env.write(ENV_KEYS[0], want[ENV_KEYS[0]].as_ref()).unwrap();
    store.recover(&env).unwrap();
    assert!(env.snapshot().unwrap() == original);

    let before = legacy(&store, before, &env);
    let old_env = env.snapshot().unwrap();
    let committed = store.select(&env, &before, before.clone()).unwrap();
    let clean_env = env.snapshot().unwrap();
    store
        .journal(&Journal {
            before,
            after_revision: committed.revision,
            before_env: old_env,
            after_env: clean_env.clone(),
        })
        .unwrap();
    store.recover(&env).unwrap();
    assert!(env.snapshot().unwrap() == clean_env);
    assert_eq!(store.read().unwrap().schema, DATABASE_SCHEMA);
}

#[test]
fn recovery_preserves_an_external_writer() {
    let (_dir, store, before) = fixture();
    let env = FakeEnvironment::default();
    let before = legacy(&store, before, &env);
    let old = env.snapshot().unwrap();
    let empty: Snapshot = ENV_KEYS.into_iter().map(|k| (k.into(), None)).collect();
    store
        .journal(&Journal {
            before: before.clone(),
            after_revision: 1,
            before_env: old,
            after_env: empty,
        })
        .unwrap();
    env.write(ENV_KEYS[0], None).unwrap();
    env.write("HTTPS_PROXY", Some(&EnvValue::text("manual-change")))
        .unwrap();
    assert!(store.recover(&env).is_err());
    assert!(env.read("HTTPS_PROXY").unwrap() == Some(EnvValue::text("manual-change")));
    assert!(env.read(ENV_KEYS[0]).unwrap().is_some());
}

#[test]
fn concurrent_switches_commit_whole_selections_without_environment_writes() {
    use std::sync::{Arc, Barrier};
    let (_dir, store, _) = fixture();
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for id in ["one", "two"] {
        let store = store.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let env = FakeEnvironment::default();
            let _lease = store.lock().unwrap();
            store.recover(&env).unwrap();
            let before = store.read().unwrap();
            let mut after = before.clone();
            after.current = Some(id.into());
            store.select(&env, &before, after).unwrap();
            assert_eq!(env.writes.get(), 0);
        }));
    }
    barrier.wait();
    for thread in handles {
        thread.join().unwrap();
    }
    let db = store.read().unwrap();
    assert_eq!(db.revision, 2);
    assert!(["one", "two"].contains(&db.current.unwrap().as_str()));
}
