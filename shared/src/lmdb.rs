//! LMDB-backed key-value store (port of `store/LmdbKeyValueStore.scala` +
//! `store/LmdbStoreManager.scala`).
//!
//! The `KeyValueStore` trait is synchronous, so LMDB transactions run inline; the surrounding
//! `tokio::sync::Mutex` in `SharedStore` serializes access to a store. `LmdbStoreManager` opens a
//! single LMDB environment (file) whose named databases are the `KeyValueStore`s.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use lmdb::{
    Cursor, Database, DatabaseFlags, Environment, Error as LmdbError, RwTransaction, Transaction,
    WriteFlags,
};

use crate::store::KeyValueStore;
use crate::store_manager::{Db, KeyValueStoreManager, LmdbEnvConfig};
use crate::typed_store::SharedStore;

/// An LMDB-backed key-value store (port of `LmdbKeyValueStore`).
pub struct LmdbKeyValueStore {
    env: Arc<Environment>,
    db: Database,
}

impl LmdbKeyValueStore {
    pub fn new(env: Arc<Environment>, db: Database) -> Self {
        LmdbKeyValueStore { env, db }
    }
}

impl KeyValueStore for LmdbKeyValueStore {
    fn get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, String> {
        let txn = self
            .env
            .begin_ro_txn()
            .map_err(|e| format!("LMDB read transaction: {e}"))?;
        let mut result = Vec::with_capacity(keys.len());
        for k in keys {
            match txn.get(self.db, k) {
                Ok(v) => result.push(Some(v.to_vec())),
                Err(LmdbError::NotFound) => result.push(None),
                Err(e) => return Err(format!("LMDB get failed: {e}")),
            }
        }
        txn.commit().map_err(|e| format!("LMDB commit: {e}"))?;
        Ok(result)
    }

    fn put(&mut self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), String> {
        let mut txn = self
            .env
            .begin_rw_txn()
            .map_err(|e| format!("LMDB write transaction: {e}"))?;
        for (k, v) in &pairs {
            txn.put(self.db, k, v, WriteFlags::empty())
                .map_err(|e| format!("LMDB put: {e}"))?;
        }
        txn.commit().map_err(|e| format!("LMDB commit: {e}"))?;
        Ok(())
    }

    fn delete(&mut self, keys: &[Vec<u8>]) -> Result<usize, String> {
        let mut txn = self
            .env
            .begin_rw_txn()
            .map_err(|e| format!("LMDB write transaction: {e}"))?;
        let mut removed = 0;
        for k in keys {
            match txn.del(self.db, k, None) {
                Ok(()) => removed += 1,
                Err(LmdbError::NotFound) => {}
                Err(e) => return Err(format!("LMDB delete failed: {e}")),
            }
        }
        txn.commit().map_err(|e| format!("LMDB commit: {e}"))?;
        Ok(removed)
    }

    fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        let txn = self
            .env
            .begin_ro_txn()
            .map_err(|e| format!("LMDB read transaction: {e}"))?;
        let mut out = Vec::new();
        {
            let mut cursor = txn
                .open_ro_cursor(self.db)
                .map_err(|e| format!("LMDB cursor: {e}"))?;
            for item in cursor.iter() {
                let (k, v) = item.map_err(|e| format!("LMDB iterate: {e}"))?;
                out.push((k.to_vec(), v.to_vec()));
            }
        }
        txn.commit().map_err(|e| format!("LMDB commit: {e}"))?;
        Ok(out)
    }

    /// **Keys only: the values are never copied.** Same read cursor as `entries`, with the value
    /// skipped — the stored-body audit needs the block hashes and must not materialise the bodies.
    fn keys(&self) -> Result<Vec<Vec<u8>>, String> {
        let txn = self
            .env
            .begin_ro_txn()
            .map_err(|e| format!("LMDB read transaction: {e}"))?;
        let mut out = Vec::new();
        {
            let mut cursor = txn
                .open_ro_cursor(self.db)
                .map_err(|e| format!("LMDB cursor: {e}"))?;
            for item in cursor.iter() {
                let (k, _) = item.map_err(|e| format!("LMDB iterate: {e}"))?;
                out.push(k.to_vec());
            }
        }
        txn.commit().map_err(|e| format!("LMDB commit: {e}"))?;
        Ok(out)
    }

    fn num_records(&self) -> usize {
        let Ok(txn) = self.env.begin_ro_txn() else {
            return 0;
        };
        txn.stat(self.db).map(|s| s.entries()).unwrap_or(0)
    }
}

/// A store manager over a single LMDB environment (port of `LmdbStoreManager`).
pub struct LmdbStoreManager {
    env: Arc<Environment>,
}

impl LmdbStoreManager {
    /// Open (creating if absent) an LMDB environment at `dir_path` with the given max size (port of
    /// `LmdbStoreManager.apply`).
    pub fn new(dir_path: &Path, max_env_size: usize) -> Result<Self, String> {
        std::fs::create_dir_all(dir_path).map_err(|e| e.to_string())?;
        let mut builder = Environment::new();
        builder.set_map_size(max_env_size);
        builder.set_max_dbs(20);
        builder.set_max_readers(2048);
        let env = builder.open(dir_path).map_err(|e| e.to_string())?;
        Ok(LmdbStoreManager { env: Arc::new(env) })
    }

    /// The named database's handle, creating it if absent. The one place the two constructors below
    /// get theirs, so the create flags cannot drift.
    fn open_db(&self, name: &str) -> Result<Database, String> {
        self.env
            .create_db(Some(name), DatabaseFlags::empty())
            .map_err(|e| e.to_string())
    }

    /// **Run `f` inside one read-write transaction on this environment** (C350). `f`'s success is the
    /// commit; an error aborts, so nothing it wrote survives.
    pub(crate) fn write_txn<T>(
        &self,
        f: impl FnOnce(&mut RwTransaction) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut txn = self
            .env
            .begin_rw_txn()
            .map_err(|e| format!("LMDB write transaction: {e}"))?;
        let out = f(&mut txn)?;
        txn.commit().map_err(|e| format!("LMDB commit: {e}"))?;
        Ok(out)
    }

    /// Open (creating if absent) a named database and return the raw synchronous store (mirror of
    /// [`KeyValueStoreManager::store`] without the `Arc<Mutex>` wrapper, used by the RSpace
    /// exporter/importer which require `KeyValueStore` directly).
    pub fn store_sync(&self, name: &str) -> Result<Box<dyn KeyValueStore + Send + Sync>, String> {
        let db = self.open_db(name)?;
        Ok(Box::new(LmdbKeyValueStore::new(self.env.clone(), db)))
    }
}

#[async_trait]
impl KeyValueStoreManager for LmdbStoreManager {
    async fn store(&self, name: &str) -> Result<SharedStore, String> {
        let db = self
            .env
            .create_db(Some(name), DatabaseFlags::empty())
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(tokio::sync::Mutex::new(
            Box::new(LmdbKeyValueStore::new(self.env.clone(), db))
                as Box<dyn KeyValueStore + Send + Sync>,
        )))
    }

    async fn shutdown(&self) {
        // The environment is closed when the last `Arc` handle is dropped (lmdb-rkv `Drop`).
    }
}

/// A store manager that distributes databases across multiple LMDB environments (files) (port of
/// `LmdbDirStoreManager`).
///
/// Each `Db` is assigned an `LmdbEnvConfig` naming the environment (file) that holds it; databases
/// sharing an environment name live in the same LMDB file. Environments are opened lazily on first
/// access and cached, keyed by the environment name.
#[derive(Clone)]
pub struct LmdbDirStoreManager {
    dir_path: PathBuf,
    /// Database id → (database, environment config).
    db_mapping: BTreeMap<String, (Db, LmdbEnvConfig)>,
    /// Environment name → lazily-opened store manager.
    managers: Arc<tokio::sync::Mutex<BTreeMap<String, Arc<LmdbStoreManager>>>>,
}

impl LmdbDirStoreManager {
    /// Build a directory store manager (port of `LmdbDirStoreManager.apply`). Environments are not
    /// opened until their first database is requested.
    pub fn new(dir_path: impl AsRef<Path>, db_mapping: Vec<(Db, LmdbEnvConfig)>) -> Self {
        let db_mapping = db_mapping
            .into_iter()
            .map(|(db, cfg)| (db.id.clone(), (db, cfg)))
            .collect();
        LmdbDirStoreManager {
            dir_path: dir_path.as_ref().to_path_buf(),
            db_mapping,
            managers: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
        }
    }

    /// Open (creating if absent) a named database and return the raw synchronous store (mirror of
    /// [`KeyValueStoreManager::store`] without the `Arc<Mutex>` wrapper).
    pub async fn store_sync(
        &self,
        name: &str,
    ) -> Result<Box<dyn KeyValueStore + Send + Sync>, String> {
        let (db, cfg) = self
            .db_mapping
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unknown database: {name}"))?;
        let manager = self.manager_for(&cfg).await?;
        manager.store_sync(&db.name_override.unwrap_or(db.id))
    }

    /// **Write two databases in one transaction** (C350).
    ///
    /// `pairs` go into `db_id`; for each key, `derive` is handed the key and whatever `other_db_id`
    /// currently holds (if anything) **inside the same transaction**, and returns the value to write
    /// there — or `None` to leave it alone. A row that must preserve what it finds (a fault, a first
    /// sighting) therefore needs no second round trip, and the two writes cannot be separated by a
    /// crash.
    ///
    /// **The two databases must name the same environment, and that is a refusal rather than a
    /// fallback.** LMDB transactions are per environment; a caller that moves one of them elsewhere
    /// has silently reintroduced the window this exists to close.
    pub async fn put_across<F>(
        &self,
        db_id: &str,
        pairs: &[(Vec<u8>, Vec<u8>)],
        other_db_id: &str,
        derive: F,
    ) -> Result<(), String>
    where
        F: Fn(&[u8], Option<&[u8]>) -> Result<Option<Vec<u8>>, String>,
    {
        let (db, cfg) = self
            .db_mapping
            .get(db_id)
            .cloned()
            .ok_or_else(|| format!("unknown database: {db_id}"))?;
        let (other_db, other_cfg) = self
            .db_mapping
            .get(other_db_id)
            .cloned()
            .ok_or_else(|| format!("unknown database: {other_db_id}"))?;
        if cfg.name != other_cfg.name {
            return Err(format!(
                "put_across: {db_id} and {other_db_id} live in different environments ({} and {}); \
                 one LMDB transaction cannot span them",
                cfg.name, other_cfg.name
            ));
        }

        let manager = self.manager_for(&cfg).await?;
        let mine = manager.open_db(&db.name_override.unwrap_or(db.id))?;
        let theirs = manager.open_db(&other_db.name_override.unwrap_or(other_db.id))?;

        manager.write_txn(|txn| {
            for (key, value) in pairs {
                txn.put(mine, key, value, WriteFlags::empty())
                    .map_err(|e| format!("LMDB put: {e}"))?;
                let existing = match txn.get(theirs, key) {
                    Ok(bytes) => Some(bytes.to_vec()),
                    Err(LmdbError::NotFound) => None,
                    Err(e) => return Err(format!("LMDB get: {e}")),
                };
                if let Some(next) = derive(key, existing.as_deref())? {
                    txn.put(theirs, key, &next, WriteFlags::empty())
                        .map_err(|e| format!("LMDB put: {e}"))?;
                }
            }
            Ok(())
        })
    }

    /// The environment's manager, opened on first use and cached by environment name.
    async fn manager_for(&self, cfg: &LmdbEnvConfig) -> Result<Arc<LmdbStoreManager>, String> {
        let mut managers = self.managers.lock().await;
        if !managers.contains_key(&cfg.name) {
            let dir = self.dir_path.join(&cfg.name);
            let max_env_size = usize::try_from(cfg.max_env_size).map_err(|e| e.to_string())?;
            let created = LmdbStoreManager::new(&dir, max_env_size)
                .map_err(|e| format!("open LMDB environment {}: {e}", cfg.name))?;
            managers.insert(cfg.name.clone(), Arc::new(created));
        }
        managers
            .get(&cfg.name)
            .cloned()
            .ok_or_else(|| format!("missing manager for {}", cfg.name))
    }
}

#[async_trait]
impl KeyValueStoreManager for LmdbDirStoreManager {
    async fn store(&self, name: &str) -> Result<SharedStore, String> {
        let (db, cfg) = self
            .db_mapping
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unknown database: {name}"))?;
        let manager = self.manager_for(&cfg).await?;
        manager.store(&db.name_override.unwrap_or(db.id)).await
    }

    async fn shutdown(&self) {
        let managers = self.managers.lock().await;
        for manager in managers.values() {
            manager.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rchain-lmdb-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[tokio::test]
    async fn lmdb_store_round_trips() {
        let dir = temp_dir();
        let manager = LmdbStoreManager::new(&dir, 10 * 1024 * 1024).unwrap();
        let store = manager.store("db").await.unwrap();

        {
            let mut kv = store.lock().await;
            kv.put(vec![
                (b"k1".to_vec(), b"v1".to_vec()),
                (b"k2".to_vec(), b"v2".to_vec()),
            ])
            .unwrap();
        }
        {
            let kv = store.lock().await;
            assert_eq!(
                kv.get(&[b"k1".to_vec()]).unwrap(),
                vec![Some(b"v1".to_vec())]
            );
            assert_eq!(kv.get(&[b"missing".to_vec()]).unwrap(), vec![None]);
            assert_eq!(kv.entries().unwrap().len(), 2);
        }
        {
            let mut kv = store.lock().await;
            assert_eq!(kv.delete(&[b"k1".to_vec()]).unwrap(), 1);
            assert_eq!(kv.entries().unwrap().len(), 1);
        }

        manager.shutdown().await;
        drop(manager);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn dir_manager_groups_envs_and_round_trips() {
        let dir = temp_dir();
        let mapping = vec![
            (Db::new("a"), LmdbEnvConfig::new("env1", 1024 * 1024)),
            (Db::new("b"), LmdbEnvConfig::new("env1", 1024 * 1024)),
            (
                Db {
                    id: "c".to_string(),
                    name_override: Some("c-real".to_string()),
                },
                LmdbEnvConfig::new("env2", 1024 * 1024),
            ),
        ];
        let manager = LmdbDirStoreManager::new(&dir, mapping);

        // "a" and "b" share environment "env1" but are distinct databases.
        let store_a = manager.store("a").await.unwrap();
        let store_b = manager.store("b").await.unwrap();
        {
            let mut kv = store_a.lock().await;
            kv.put(vec![(b"k".to_vec(), b"va".to_vec())]).unwrap();
        }
        {
            let mut kv = store_b.lock().await;
            kv.put(vec![(b"k".to_vec(), b"vb".to_vec())]).unwrap();
        }
        {
            let kv = store_a.lock().await;
            assert_eq!(
                kv.get(&[b"k".to_vec()]).unwrap(),
                vec![Some(b"va".to_vec())]
            );
        }
        {
            let kv = store_b.lock().await;
            assert_eq!(
                kv.get(&[b"k".to_vec()]).unwrap(),
                vec![Some(b"vb".to_vec())]
            );
        }

        // The name override is used as the database name.
        let store_c = manager.store("c").await.unwrap();
        {
            let mut kv = store_c.lock().await;
            kv.put(vec![(b"k".to_vec(), b"vc".to_vec())]).unwrap();
        }
        {
            let kv = store_c.lock().await;
            assert_eq!(
                kv.get(&[b"k".to_vec()]).unwrap(),
                vec![Some(b"vc".to_vec())]
            );
        }

        manager.shutdown().await;
        drop(manager);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unknown_database_is_an_error() {
        let dir = temp_dir();
        let manager = LmdbDirStoreManager::new(
            &dir,
            vec![(Db::new("a"), LmdbEnvConfig::new("env1", 1024 * 1024))],
        );
        assert!(manager.store("nope").await.is_err());
    }

    /// **Two databases in one transaction, and a refusal when they cannot share one** (C350).
    ///
    /// The pair is written together; a `derive` that errors aborts the transaction, so *neither*
    /// half survives — which is the property the body/disposition window needed. Moving the second
    /// database to another environment is refused rather than silently two-phase: LMDB transactions
    /// are per environment.
    #[tokio::test]
    async fn put_across_writes_both_or_neither_and_refuses_two_environments() {
        let dir = temp_dir();
        let manager = LmdbDirStoreManager::new(
            &dir,
            vec![
                (Db::new("bodies"), LmdbEnvConfig::new("one", 1024 * 1024)),
                (Db::new("rows"), LmdbEnvConfig::new("one", 1024 * 1024)),
                (Db::new("elsewhere"), LmdbEnvConfig::new("two", 1024 * 1024)),
            ],
        );

        // Both halves land, and the second half's value is computed from what it finds.
        manager
            .put_across(
                "bodies",
                &[(b"k".to_vec(), b"body".to_vec())],
                "rows",
                |_key, _| Ok(Some(b"row".to_vec())),
            )
            .await
            .expect("same environment");
        let bodies = manager.store("bodies").await.expect("open");
        let rows = manager.store("rows").await.expect("open");
        assert_eq!(
            bodies.lock().await.get(&[b"k".to_vec()]).unwrap(),
            vec![Some(b"body".to_vec())]
        );
        assert_eq!(
            rows.lock().await.get(&[b"k".to_vec()]).unwrap(),
            vec![Some(b"row".to_vec())]
        );

        // A `derive` error aborts the whole write: the body must not be there either.
        let err = manager
            .put_across(
                "bodies",
                &[(b"k2".to_vec(), b"body".to_vec())],
                "rows",
                |_key, _| Err("the row could not be encoded".to_string()),
            )
            .await
            .expect_err("a derive error aborts");
        assert!(err.contains("could not be encoded"));
        assert_eq!(
            bodies.lock().await.get(&[b"k2".to_vec()]).unwrap(),
            vec![None],
            "the body must not survive a failed row"
        );

        // And the pair must share an environment.
        let err = manager
            .put_across(
                "bodies",
                &[(b"k".to_vec(), b"body".to_vec())],
                "elsewhere",
                |_, _| Ok(None),
            )
            .await
            .expect_err("two environments cannot share a transaction");
        assert!(err.contains("different environments"), "{err}");

        manager.shutdown().await;
        drop(manager);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
