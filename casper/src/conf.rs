//! Casper configuration types (port of `CasperConf.scala`).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use rchain_shared::refined::ShardId;

/// One shard this node is a member of (Law 26), fully resolved: the validated full id plus the
/// per-shard genesis and consensus data.
///
/// The node keeps the `(parent-shard-id, shard-name)` *authoring* form — it is how the hierarchy is
/// expressed and `ShardId::child` is the only constructor for a child id — and resolves it once, at
/// construction, so every consumer downstream reads the same validated `shard_id`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShardSpec {
    /// The validated full id `{parent_shard_id}/{shard_name}`. This is the single value the
    /// proposer, the block receiver's `check_if_of_interest`, the deploy APIs and the RNG seed all
    /// agree on.
    pub shard_id: ShardId,
    /// The bare name. Kept because `Genesis.shard_id` is a `String` (the genesis path needs it) and
    /// because it is the authoring half the operator writes.
    pub shard_name: String,
    /// The parent shard's id (the `/`-separated path this shard nests under); the root's is `/`.
    pub parent_shard_id: String,
    /// This shard's genesis data. A multi-shard node gives each membership its own bonds and
    /// wallets — different validator sets per shard are the point.
    pub genesis_block_data: GenesisBlockData,
    pub autogen_shard_size: i32,
}

impl ShardSpec {
    /// Resolve a spec from its authoring form, validating both halves (Law 26: a non-empty, ASCII
    /// shard id — the parent through `TryFrom`, the name through `ShardId::child`).
    pub fn new(
        shard_name: String,
        parent_shard_id: String,
        genesis_block_data: GenesisBlockData,
        autogen_shard_size: i32,
    ) -> Result<Self, String> {
        // The name is validated by the type, not here: `child` applies the id refinement's own
        // predicate to the name it appends (C78), so the pre-check this used to carry is gone — one
        // predicate in one place, and no drift between the composition and the refinement. The
        // operator-visible message is preserved by the mapping.
        let parent = ShardId::try_from(parent_shard_id.clone())
            .map_err(|e| format!("invalid parent-shard-id '{parent_shard_id}': {e}"))?;
        let shard_id = parent
            .child(&shard_name)
            .map_err(|e| format!("invalid shard-name '{shard_name}': {e}"))?;
        Ok(ShardSpec {
            shard_id,
            shard_name,
            parent_shard_id,
            genesis_block_data,
            autogen_shard_size,
        })
    }
}

/// The shards this node is a member of — **always at least one**, index 0 being the *primary* shard.
///
/// Non-emptiness is carried by the type rather than checked at each use, so [`Self::primary`] is
/// total and every shard-selector-less query has a well-defined default. A single-shard node is the
/// one-element case, and is byte-for-byte today's behaviour.
#[derive(Clone, Debug, PartialEq)]
pub struct ShardMemberships {
    primary: ShardSpec,
    others: Vec<ShardSpec>,
}

impl ShardMemberships {
    /// Build the memberships from the configured specs, rejecting an empty list (a node must be a
    /// member of at least one shard) and duplicate ids (two memberships with one id would share a
    /// state hash and a data directory).
    pub fn new(specs: Vec<ShardSpec>) -> Result<Self, String> {
        let mut iter = specs.into_iter();
        let primary = iter
            .next()
            .ok_or_else(|| "a node must be a member of at least one shard".to_string())?;
        let memberships = ShardMemberships {
            primary,
            others: iter.collect(),
        };
        let mut seen: BTreeSet<&ShardId> = BTreeSet::new();
        for spec in memberships.iter() {
            if !seen.insert(&spec.shard_id) {
                return Err(format!("duplicate shard membership '{}'", spec.shard_id));
            }
        }
        Ok(memberships)
    }

    /// The primary shard: the default target for shard-selector-less queries.
    pub fn primary(&self) -> &ShardSpec {
        &self.primary
    }

    pub fn primary_mut(&mut self) -> &mut ShardSpec {
        &mut self.primary
    }

    /// Every membership, primary first.
    pub fn iter(&self) -> impl Iterator<Item = &ShardSpec> {
        std::iter::once(&self.primary).chain(self.others.iter())
    }

    /// Every membership, primary first, mutably — for a resolved default that has to reach each
    /// shard (the production dev-tax address).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ShardSpec> {
        let ShardMemberships { primary, others } = self;
        std::iter::once(primary).chain(others.iter_mut())
    }

    pub fn len(&self) -> usize {
        1 + self.others.len()
    }

    /// Never empty — a membership set always has at least its primary shard. Present because a
    /// public `len` without `is_empty` trips `clippy::len_without_is_empty`.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The membership with this id, if the node is a member of it.
    pub fn get(&self, shard_id: &ShardId) -> Option<&ShardSpec> {
        self.iter().find(|spec| &spec.shard_id == shard_id)
    }
}

/// Consensus configuration (port of the Scala `CasperConf` case class).
#[derive(Clone, Debug, PartialEq)]
pub struct CasperConf {
    pub validator_public_key: Option<String>,
    pub validator_private_key: Option<String>,
    pub validator_private_key_path: Option<PathBuf>,
    /// The shards this node is a member of (Law 26); index 0 is the primary shard. A single-shard
    /// node is a one-element list.
    pub shards: ShardMemberships,
    pub casper_loop_interval: Duration,
    pub requested_blocks_timeout: Duration,
    pub max_number_of_parents: i32,
    pub fork_choice_stale_threshold: Duration,
    pub fork_choice_check_if_stale_interval: Duration,
    pub synchrony_constraint_threshold: f64,
    pub height_constraint_threshold: i64,
    pub min_phlo_price: i64,
    /// **Broadcast a twin of every block this node creates** (default off). A devnet-only injection,
    /// for producing the one fault no honest node can produce — two distinct signed blocks at one
    /// `(sender, seq_num)` — so the equivocation rules (AUDIT C200) can be exercised between two live
    /// nodes rather than only in one process.
    ///
    /// It is *self-harm* on a real network and it says so: the twin is equally signed, so every peer
    /// records the node as having equivocated, and it is the node's own bond the peers will take.
    pub equivocation_injection: bool,
    /// **A retired devnet instrument** (default `0` = off). It perturbed this node's `fringe-data`
    /// record so its merge answered differently from its peers' — the `fringe_states` ambiguity (C215)
    /// made to bite on every arrival order. That store and that ambiguity retired with the per-block
    /// change (Law 66/68; C250's residue, C270), so the flag no longer perturbs anything; it is kept
    /// because the devnet CLI still parses it, and the node reports it as armed-but-inert rather than
    /// silently doing nothing.
    ///
    /// It still refuses to arm without `--dev-mode` (`check_merge_divergence_injection`), so the guard
    /// is unchanged even though its effect is gone. See
    /// `BlockDagKeyValueStorage::with_merge_divergence_injection` for what it used to stage and why the
    /// divergence rig (`up --validators 4 --fresh`) no longer needs it.
    pub merge_divergence_injection: u8,
    /// **An operator-named block to sync to, instead of a finalised fringe** (default `None`).
    ///
    /// The recovery path a frozen chain needs, and the one #287's design assumes exists: a wiping joiner
    /// asks a peer for a `FinalizedFringe`, and a peer with no fringe answers with **genesis**
    /// (`node_running.rs`), so the joiner syncs to block 0 and can never re-enter sync — the state
    /// C259 records. The block the operator names is the meet the reconciliation tool computed: the
    /// deepest height a strict supermajority of stake agrees on, which is what makes it a safe anchor
    /// rather than a tip someone picked.
    ///
    /// Only consulted when this node's DAG is empty (the boot path that would otherwise request a
    /// fringe). Empty means the ordinary path, unchanged.
    pub sync_anchor: Option<String>,
    /// Operator-pinned genesis block hash for joining an existing network.
    pub expected_genesis_hash: Option<String>,
    /// **Whether this node may reset itself** — the acting half of F-U5-01, off by default (#320).
    ///
    /// When a `Divergence` record's revalidation budget is spent, the node can no longer extend the
    /// chain: it refuses that block and its descendants for the process lifetime, and until this switch
    /// the only way back was an operator deleting the shard data dir. With it on, the node rewinds to
    /// its **last finalised block** — a block a strict supermajority of stake finalised, so nothing the
    /// network committed is lost — resets its runtime to that state and re-enters sync from there.
    ///
    /// **Off by default, and not gated on `--dev-mode`.** The detector, the count and the dry run are the
    /// first half and they shipped dark so an operator could see what a reset *would* act on; the switch
    /// is that operator's consent. Dev mode would be the wrong gate: it is for effects that are
    /// indistinguishable from a fault, and this one is the opposite — it counts, logs, and acts only on a
    /// spent budget. So a stranded production validator can turn it on without becoming a dev node.
    ///
    /// **It needs a bootstrap**, refused at config load if it has none: rewinding leaves the node
    /// deliberately behind its peers, and the blocks above the anchor are re-requested *from a peer*.
    /// A node that cannot ask anybody has nothing to re-sync against.
    pub self_reset: bool,
    /// The effect-scheduler mode (Laws 20–25): `dfs` (default), `gate`, `relaxed`, or
    /// `relaxed-validated`. Parsed via `FromStr` at consumption (`EffectMode`); the block paths
    /// hard-reject `relaxed` (off-chain only) and accept `relaxed-validated` with
    /// sequential-reference validation (Laws 23–25,
    /// `docs/src/formal/scheduling.md`).
    pub effect_mode: String,
}

/// Genesis-block data configuration (port of the Scala `GenesisBlockData` case class).
#[derive(Clone, Debug, PartialEq)]
pub struct GenesisBlockData {
    pub genesis_data_dir: PathBuf,
    pub bonds_file: String,
    pub wallets_file: String,
    pub bond_minimum: i64,
    pub bond_maximum: i64,
    pub epoch_length: i32,
    pub quarantine_length: i32,
    pub genesis_block_number: i64,
    pub number_of_active_validators: i32,
    /// **The producer's share of a deploy's burned phlo, in basis points** (B2, #150). A genesis
    /// parameter because it decides amounts, which are consensus state: two nodes with different
    /// values compute different post-states.
    pub executor_share: i32,
    /// **The lag at which a validator's participation weight reaches zero** (B4, #150), in heights.
    /// `0` is the contract's behaviour: absence costs nothing.
    pub absence_slack: i32,
    /// **How far behind the last finalised fringe a validator's latest message may sit and still be
    /// paid its whole share** (B4, #150), in heights. Between this and `absence_slack` the weight ramps
    /// linearly to zero.
    ///
    /// A genesis parameter because it decides amounts, which are consensus state. **A file that does
    /// not state it gets the binary rule** (`grace == slack`), which is what a chain that predates the
    /// field was running — see [`crate::genesis::contracts::ProofOfStake`], where the rule is applied.
    pub participation_grace: i32,
    /// **The fee floor, committed at genesis** (`PosParams::min_phlo_price`). Not a key of
    /// `genesis-block-data`: it is `casper.min-phlo-price`, copied in when the config is read
    /// (`configuration::hocon`), so the value a node validates with and the value its genesis commits are
    /// one setting and cannot drift apart.
    pub min_phlo_price: i64,
    /// **The parent bound, committed at genesis** (`PosParams::max_number_of_parents`) — likewise
    /// `casper.max-number-of-parents`, copied in when the config is read.
    pub max_number_of_parents: i32,
    pub pos_multi_sig_public_keys: Vec<String>,
    pub pos_multi_sig_quorum: i32,
    pub pos_vault_pub_key: String,
    pub system_contract_pub_key: String,
    /// **The REV address the developer allocation is credited to at genesis** (Law 78), or the empty
    /// string for no allocation. The amount is *derived*, not configured — `max(20 000 000 REV, 2% of
    /// the wallets file's total)`, see [`crate::genesis::dev_tax`] — so only the address is a setting.
    ///
    /// A genesis parameter for the same reason `executor_share` is: it decides an amount, and amounts
    /// are consensus state. A value that is **set** must parse as a REV address, refused at startup
    /// (`check_dev_tax_address`); a value left **empty** is refused only where it matters — at the
    /// branch that creates a genesis, and only outside `--dev-mode`
    /// ([`crate::genesis::dev_tax::refuse_a_production_genesis_without_an_address`]) — because a devnet
    /// creates a genesis too, and a standalone node restarting onto an existing chain creates none.
    ///
    /// **What that does not buy, stated because the prose around this key once claimed it.** The
    /// guarantee is that the *creator* decides, and that a node **replaying** the genesis derives the
    /// same vaults and so the same post-state hash (AUDIT C46). A node that instead **restores** the
    /// chain over LFS does not replay genesis — `insert_genesis` marks it validated as it stands — but
    /// the restore-completion guard enforces its local claim when it has one (bonds + wallets files
    /// present) and trusts the bootstrap when it has none.
    pub dev_tax_address: String,
    /// **How long the developer allocation vests over**, in blocks from genesis (Law 79). `0` is no
    /// schedule at all: the whole allocation is released at the first epoch boundary.
    ///
    /// A genesis parameter for the same reason `executor_share` and `epoch_length` are — it decides
    /// amounts, and amounts are consensus state — and because the *term* is a decision while the
    /// arithmetic is not. The network's block cadence is not a constant (`docs/src/node/scaling.md`
    /// records why), so a "month" is an assumption; the chain enforces a **block count**, and
    /// `defaults.conf` carries the count that the intended term works out to at the stated cadence.
    ///
    /// A config written before this key resolves to `0` — no schedule — which is the rule for a chain
    /// with no allocation, and the only rule a chain that predates the field could have run.
    pub dev_tax_vesting_blocks: i64,
    /// Liveness enforcement's five parameters (RCHIP #55, Law 89). All zero is the shipped
    /// pre-change rule; a network arms the mechanism at genesis.
    pub liveness_absence_threshold: i64,
    pub liveness_jail_after_boundaries: i32,
    pub liveness_jail_epochs: i32,
    pub liveness_vote_window: i32,
    pub liveness_slash_severity: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genesis() -> GenesisBlockData {
        GenesisBlockData {
            genesis_data_dir: PathBuf::from("/genesis"),
            bonds_file: "/genesis/bonds.txt".to_string(),
            wallets_file: "/genesis/wallets.txt".to_string(),
            bond_minimum: 1,
            bond_maximum: 100,
            epoch_length: 10,
            quarantine_length: 10,
            genesis_block_number: 0,
            number_of_active_validators: 10,
            executor_share: 2500,
            absence_slack: 0,
            participation_grace: 0,
            min_phlo_price: 0,
            max_number_of_parents: 0,
            pos_multi_sig_public_keys: Vec::new(),
            pos_multi_sig_quorum: 0,
            pos_vault_pub_key: String::new(),
            system_contract_pub_key: String::new(),
            dev_tax_address: String::new(),
            dev_tax_vesting_blocks: 0,

            liveness_absence_threshold: 0,
            liveness_jail_after_boundaries: 0,
            liveness_jail_epochs: 0,
            liveness_vote_window: 0,
            liveness_slash_severity: 0,
        }
    }

    fn spec(name: &str, parent: &str) -> ShardSpec {
        ShardSpec::new(name.to_string(), parent.to_string(), genesis(), 5).expect("valid spec")
    }

    /// A shard id must be non-empty and ASCII (Law 26), and `ShardId::child` now refuses an illegal
    /// name itself (C78) — this pins the operator-visible message `ShardSpec::new` maps out of that
    /// refusal, and that the parent half is validated too.
    #[test]
    fn a_spec_rejects_an_illegal_name_or_parent() {
        let err = ShardSpec::new(String::new(), "/".to_string(), genesis(), 5)
            .expect_err("an empty name must be rejected");
        assert!(err.contains("invalid shard-name"), "{err}");

        let err = ShardSpec::new("røøt".to_string(), "/".to_string(), genesis(), 5)
            .expect_err("a non-ASCII name must be rejected");
        assert!(err.contains("invalid shard-name"), "{err}");

        let err = ShardSpec::new("root".to_string(), String::new(), genesis(), 5)
            .expect_err("an empty parent must be rejected");
        assert!(err.contains("invalid parent-shard-id"), "{err}");
    }

    /// The full id is the parent path plus the name, with the root's parent `/` collapsing so the
    /// default shard is `/root` rather than `//root`.
    #[test]
    fn a_spec_resolves_its_full_id_from_the_hierarchy() {
        assert_eq!(spec("root", "/").shard_id.to_string(), "/root");
        assert_eq!(spec("child", "/root").shard_id.to_string(), "/root/child");
        assert_eq!(
            spec("leaf", "/root/child").shard_id.to_string(),
            "/root/child/leaf"
        );
    }

    /// A membership set is never empty, and `primary` is total *because* of that — the property the
    /// per-shard assembly and the request router both rely on.
    #[test]
    fn memberships_are_never_empty_and_primary_is_the_first() {
        let memberships = ShardMemberships::new(vec![spec("root", "/")]).expect("one shard");
        assert_eq!(memberships.len(), 1);
        assert!(
            !memberships.is_empty(),
            "a membership set always has a primary"
        );
        assert_eq!(memberships.primary().shard_id.to_string(), "/root");

        let err = ShardMemberships::new(Vec::new())
            .expect_err("a node must be a member of at least one shard");
        assert!(err.contains("at least one shard"), "{err}");
    }

    #[test]
    fn memberships_keep_their_order_and_reject_duplicates() {
        let memberships = ShardMemberships::new(vec![
            spec("root", "/"),
            spec("child", "/root"),
            spec("leaf", "/root/child"),
        ])
        .expect("three shards");
        let ids: Vec<String> = memberships.iter().map(|s| s.shard_id.to_string()).collect();
        assert_eq!(ids, vec!["/root", "/root/child", "/root/child/leaf"]);
        assert_eq!(memberships.primary().shard_id.to_string(), "/root");

        let duplicate = ShardMemberships::new(vec![spec("root", "/"), spec("root", "/")]);
        let err = duplicate.expect_err("two memberships with one id must be rejected");
        assert!(err.contains("duplicate shard membership"), "{err}");
    }

    /// `get` distinguishes a member from a non-member — the lookup the deploy router is built on.
    #[test]
    fn get_finds_a_member_and_reports_a_non_member() {
        let memberships =
            ShardMemberships::new(vec![spec("root", "/"), spec("child", "/root")]).expect("two");
        let root = ShardId::try_from("/root".to_string()).unwrap();
        let elsewhere = ShardId::try_from("/elsewhere".to_string()).unwrap();
        assert_eq!(
            memberships.get(&root).map(|s| s.shard_name.as_str()),
            Some("root")
        );
        assert!(memberships.get(&elsewhere).is_none());
    }

    /// `primary_mut` edits the primary in place — how the test harnesses point a shard at a
    /// temporary genesis file.
    #[test]
    fn primary_mut_updates_the_primary_in_place() {
        let mut memberships = ShardMemberships::new(vec![spec("root", "/")]).expect("one");
        memberships.primary_mut().genesis_block_data.bonds_file = "/tmp/bonds".to_string();
        assert_eq!(
            memberships.primary().genesis_block_data.bonds_file,
            "/tmp/bonds"
        );
    }
}
