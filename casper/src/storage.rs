//! RNode key-value store layout (port of `storage/RNodeKeyValueStoreManager.scala`).

use std::path::{Path, PathBuf};

use rchain_shared::lmdb::LmdbDirStoreManager;
use rchain_shared::refined::ShardId;
use rchain_shared::store_manager::{Db, LmdbEnvConfig, GB, TB};

/// The file recording which shard owns a data directory, written on first boot and checked on every
/// later one. Because the primary membership keeps the data directory root while every additional
/// shard nests under `shard/…` (see [`shard_data_dir`]), reordering memberships would otherwise
/// silently re-point a shard at another shard's chain. See `node`'s `check_shard_data_dir`.
pub const SHARD_ID_MARKER: &str = "shard-id";

/// **The file recording the format of the store beside it** (W-N11-03), written on first boot by a
/// build that knows about it and checked on every later one.
///
/// The row: `rnode_db_mapping`'s own rule is that an upgraded node "leaves any existing rows on disk
/// and ignores them", and the only data-directory marker was the shard id — so an *older* binary, or
/// an image at a base pinned only by tag, running over a *newer* store was accepted silently, which is
/// the reverse of the advance the upgrade is supposed to be. A store this build opens is stamped with
/// [`STORE_FORMAT_VERSION`]; a store stamped with a **higher** one refuses to open. A lower one is an
/// advance and is accepted, which is the intended direction.
///
/// **What it cannot do, said rather than implied:** a store written by a build that predates this
/// marker carries none, so it is stamped with this build's version and read — an absent stamp is not
/// evidence of anything. Bump `STORE_FORMAT_VERSION` whenever the layout below changes; the number is
/// a promise about the rows, and nothing derives it.
pub const STORE_FORMAT_MARKER: &str = "store-format";

/// The store format this build writes. See [`STORE_FORMAT_MARKER`].
// Version 2 adds the recovery-pending key: a v1 binary would ignore it and
// could enter Running over an interrupted publication. Downgrade must refuse.
pub const STORE_FORMAT_VERSION: u32 = 2;

/// The gateway's durable coordinator-record database ([`crate::gateway::ledger`]). Node-local: it
/// is deliberately **not** part of a shard's content-addressed state, because a coordinator's votes
/// and decision belong to one node and must not enter another shard's state hash.
pub const GATEWAY_TXN_DB: &str = "gateway-txn";

/// **Where a shard records a reset it performed** (F-U5-01's acting half, register C274). A restart
/// reads it to repair a reset that crashed between its two store operations, and to re-enter the
/// catch-up walk at the anchor rather than reconnecting to a deliberately truncated DAG. One literal
/// key (`RESET_MARKER_KEY`, `block-storage`), and no rows at all on a shard that has never reset.
pub const RESET_MARKER_DB: &str = "reset-marker";

/// The data directory of the shard at `index` in the node's membership list.
///
/// The primary membership (index 0) keeps `root` itself, so every existing single-shard deployment
/// finds its data exactly where it left it; additional memberships nest under `shard/` with one path
/// segment per shard-id segment. [`ShardId::segments`] is reused for the path so the layout cannot
/// collide the way a `/`-to-`_` slug would.
pub fn shard_data_dir(root: &Path, index: usize, shard_id: &ShardId) -> PathBuf {
    if index == 0 {
        return root.to_path_buf();
    }
    shard_id
        .segments()
        .into_iter()
        .fold(root.join("shard"), |path, segment| path.join(segment))
}

/// The RNode DB → LMDB environment mapping (port of `rnodeDbMapping`).
///
/// Keys with the same environment name share one LMDB file.
pub fn rnode_db_mapping() -> Vec<(Db, LmdbEnvConfig)> {
    vec![
        // Block storage
        (
            Db::new("blocks"),
            LmdbEnvConfig::new("blockstorage", 1 * TB),
        ),
        // Block metadata storage
        (
            Db::new("block-metadata"),
            LmdbEnvConfig::new("dagstorage", 100 * GB),
        ),
        // `fringe-data` retired with the store itself (Law 66/68; C250's residue, C270): the claim
        // it keyed is per-block metadata now, so there is no DB to open. An upgraded node leaves any
        // existing rows on disk and ignores them.
        (
            Db::new("finalized-store"),
            LmdbEnvConfig::new("dagstorage", 100 * GB),
        ),
        // Deploys from blocks
        (
            Db::new("deploy-index"),
            LmdbEnvConfig::new("dagstorage", 100 * GB),
        ),
        // Deployer key (hashed) → a block it signed in (`GET /api/v1/deployer`)
        (
            Db::new("deployer-index"),
            LmdbEnvConfig::new("dagstorage", 100 * GB),
        ),
        // The node's own record of a reset it performed (one literal key, usually absent)
        (
            Db::new(RESET_MARKER_DB),
            LmdbEnvConfig::new("dagstorage", 100 * GB),
        ),
        // Runtime mergeable store (cache of mergeable channels for block-merge)
        (
            Db::new("mergeable-channel-cache"),
            LmdbEnvConfig::new("dagstorage", 100 * GB),
        ),
        // Runtime native-changes store (cache of each block's native system-contract writes, so a
        // multi-parent merge can re-apply them instead of reverting them; issue #74)
        (
            Db::new("native-changes-cache"),
            LmdbEnvConfig::new("dagstorage", 100 * GB),
        ),
        // Deploys waiting to be added
        (
            Db::new("deploy-pool"),
            LmdbEnvConfig::new("deploypoolstorage", 1 * GB),
        ),
        // Reporting (trace) cache
        (
            Db::new("reporting-cache"),
            LmdbEnvConfig::new("reporting", 10 * TB),
        ),
        // On-chain RSpace (Rholang state); history and roots share one environment.
        (
            Db::new("rspace-history"),
            LmdbEnvConfig::new("rspace/history", 1 * TB),
        ),
        (
            Db::new("rspace-roots"),
            LmdbEnvConfig::new("rspace/history", 1 * TB),
        ),
        (
            Db::new("rspace-cold"),
            LmdbEnvConfig::new("rspace/cold", 1 * TB),
        ),
        // Transaction store
        (
            Db::new("transaction"),
            LmdbEnvConfig::new("transaction", 1 * GB),
        ),
        // Evaluator RSpace (Rholang state)
        (
            Db::new("eval-history"),
            LmdbEnvConfig::new("eval/history", 1 * TB),
        ),
        (
            Db::new("eval-roots"),
            LmdbEnvConfig::new("eval/history", 1 * TB),
        ),
        (
            Db::new("eval-cold"),
            LmdbEnvConfig::new("eval/cold", 1 * TB),
        ),
        // The gateway's durable coordinator records (node-local, never consensus state). Its own
        // environment, so opening it over the primary shard's manager writes one extra directory
        // (`<data-dir>/gateway/`) rather than sharing a shard's.
        (
            Db::new(GATEWAY_TXN_DB),
            LmdbEnvConfig::new("gateway", 1 * GB),
        ),
    ]
}

/// Open the RNode LMDB store manager over `dir_path` (port of `RNodeKeyValueStoreManager.apply`).
///
/// Databases are distributed across LMDB environments (files) per [`rnode_db_mapping`]; the
/// environments are opened lazily on first access.
pub fn rnode_key_value_store_manager(dir_path: &Path) -> LmdbDirStoreManager {
    LmdbDirStoreManager::new(dir_path, rnode_db_mapping())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_shared::store_manager::KeyValueStoreManager;

    /// The primary membership keeps the data-directory root (so existing deployments are
    /// unchanged); each additional shard nests by shard-id segment.
    #[test]
    fn shard_data_dirs_are_the_root_for_the_primary_and_nested_afterwards() {
        let root = Path::new("/var/lib/rnode");
        let primary = ShardId::try_from("/root".to_string()).unwrap();
        let child = ShardId::try_from("/root/child".to_string()).unwrap();
        let grandchild = ShardId::try_from("/root/child/leaf".to_string()).unwrap();

        assert_eq!(shard_data_dir(root, 0, &primary), PathBuf::from(root));
        assert_eq!(
            shard_data_dir(root, 1, &child),
            Path::new("/var/lib/rnode/shard/root/child")
        );
        assert_eq!(
            shard_data_dir(root, 2, &grandchild),
            Path::new("/var/lib/rnode/shard/root/child/leaf")
        );
        // Distinct shard ids never share a directory — even one whose segments could collide under a
        // naive `/`-to-`_` slug.
        assert_ne!(
            shard_data_dir(root, 1, &ShardId::try_from("/a/b".to_string()).unwrap()),
            shard_data_dir(root, 1, &ShardId::try_from("/a_b".to_string()).unwrap())
        );
    }

    /// **The deploy pool is bounded in bytes, not in slots times a wire size** (W-N12-05).
    ///
    /// The row's arithmetic: the pool's bound is `MAX_POOLED_DEPLOYS` deploys and a deploy may be 16 MiB
    /// on the wire, so a count bound is not a byte bound — a caller filling the pool would decide how much
    /// disk the node spends. What bounds it is the environment's own map size, and the test above checks
    /// ids and shared environments and would not notice that map growing by three orders of magnitude.
    ///
    /// Falsified by deriving the cap from the slots (the row's own number, 160 GB, is the arithmetic of a
    /// slot-derived bound) or by dropping it to something the pool cannot hold: the assertion is exact.
    #[test]
    fn the_deploy_pool_is_bounded_in_bytes_not_in_slots() {
        let pool = rnode_db_mapping()
            .iter()
            .find(|(_, cfg)| cfg.name == "deploypoolstorage")
            .map(|(_, cfg)| cfg.max_env_size)
            .expect("the deploy pool has an environment of its own");
        assert_eq!(
            pool,
            1 * GB,
            "the deploy pool is a 1 GB LMDB map — its bound is bytes"
        );
        // The row's slot-derived number: MAX_POOLED_DEPLOYS (10 000) x a 16 MiB wire term, which is
        // ~160 GB. A cap at or above this would be a bound on entries wearing a byte cap's name.
        assert!(
            pool < 10_000 * 16 * 1024 * 1024,
            "the cap must be far below the slot arithmetic the row names, or the pool is bounded by \
             its entry count after all: {pool} bytes"
        );
    }

    #[test]
    fn mapping_has_expected_databases() {
        let mapping = rnode_db_mapping();
        let ids: Vec<&str> = mapping.iter().map(|(db, _)| db.id.as_str()).collect();
        assert!(ids.contains(&"blocks"));
        assert!(ids.contains(&"rspace-history"));
        assert!(ids.contains(&"mergeable-channel-cache"));
        assert!(ids.contains(&"deployer-index"));
        // The gateway's coordinator ledger is node-local and in its own environment, so opening it
        // writes one extra directory rather than sharing a shard's.
        assert!(ids.contains(&GATEWAY_TXN_DB));
        // The reset marker shares the DAG's environment (F-U5-01, C274): it is a fact about this
        // shard's chain, and one literal key does not warrant a file of its own.
        assert!(ids.contains(&RESET_MARKER_DB));
        let marker = mapping
            .iter()
            .find(|(db, _)| db.id == RESET_MARKER_DB)
            .map(|(_, c)| c.name.clone());
        assert_eq!(marker.as_deref(), Some("dagstorage"));
        let gateway = mapping
            .iter()
            .find(|(db, _)| db.id == GATEWAY_TXN_DB)
            .map(|(_, c)| c.name.clone());
        assert_eq!(gateway.as_deref(), Some("gateway"));
        // History and roots share an environment name.
        let history = mapping
            .iter()
            .find(|(db, _)| db.id == "rspace-history")
            .map(|(_, c)| c.name.clone());
        let roots = mapping
            .iter()
            .find(|(db, _)| db.id == "rspace-roots")
            .map(|(_, c)| c.name.clone());
        assert_eq!(history, roots);
    }

    #[tokio::test]
    async fn store_manager_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "rchain-rnode-kvm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let manager = rnode_key_value_store_manager(&dir);
        let store = manager.store("deploy-pool").await.unwrap();
        {
            let mut kv = store.lock().await;
            kv.put(vec![(b"k".to_vec(), b"v".to_vec())]).unwrap();
        }
        {
            let kv = store.lock().await;
            assert_eq!(kv.get(&[b"k".to_vec()]).unwrap(), vec![Some(b"v".to_vec())]);
        }
        manager.shutdown().await;
        drop(manager);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
