//! Genesis block creation (port of `casper/genesis/Genesis.scala`).

pub mod contracts;
pub mod dev_tax;
pub mod rgov;
pub mod standard_deploys;

use std::collections::{BTreeMap, BTreeSet};

use rchain_crypto::public_key::PublicKey;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_version::version_at;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, ProcessedDeploy, RholangState, SignedDeployData,
};
use rchain_models::validator::Validator as ModelsValidator;
use rchain_rholang::native_state::{PosGenesis, PosParams};
use rchain_rholang::system_processes::BlockData;
use rchain_shared::base16;
use rchain_shared::refined::NonNegI64;

use crate::block_random_seed::BlockRandomSeed;
use crate::genesis::contracts::{ProofOfStake, Registry, Vault};
use crate::proto_util::unsigned_block_proto;
use crate::runtime_manager::RuntimeManager;
use crate::validator_identity::ValidatorIdentity;
use rchain_shared::refined::{BlockHeight, SeqNum};

/// Genesis parameters (port of `Genesis`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genesis {
    pub sender: PublicKey,
    pub shard_id: String,
    pub block_number: i64,
    pub proof_of_stake: ProofOfStake,
    pub registry: Registry,
    pub vaults: Vec<Vault>,
    /// **The developer allocation's recipient and amount** (Laws 78–79), or `None` for a genesis with
    /// no allocation. Carried here so the ceremony reaches the same `install_genesis` the replay path
    /// does, with the same value.
    pub dev_vesting: Option<(String, NonNegI64)>,
}

/// Build the bonds map (validator pubkey → stake) from the PoS validators (port of `buildBondsMap`).
fn build_bonds_map(proof_of_stake: &ProofOfStake) -> BTreeMap<ModelsValidator, NonNegI64> {
    proof_of_stake
        .validators
        .iter()
        .map(|v| (ModelsValidator::from_slice(v.pk.bytes()), v.stake))
        .collect()
}

/// Decode a base16 validator public key (used for the config's `pos-multi-sig-public-keys`).
fn parse_validator_hex(key: &str) -> Option<ModelsValidator> {
    let bytes = base16::decode(key)?;
    ModelsValidator::try_from(bytes.as_slice()).ok()
}

/// Build the genesis PoS descriptors: the bond pool, the trusted stakeholder set (the genesis
/// validators plus any configured `pos-multi-sig-public-keys`), and the network parameters.
pub fn build_pos_genesis(
    proof_of_stake: &ProofOfStake,
    dev_vesting: Option<(String, NonNegI64)>,
) -> PosGenesis {
    let bonds = build_bonds_map(proof_of_stake);
    let mut trusted: BTreeSet<ModelsValidator> = bonds.keys().copied().collect();
    for key in &proof_of_stake.pos_multi_sig_public_keys {
        if let Some(validator) = parse_validator_hex(key) {
            trusted.insert(validator);
        }
    }
    let params = PosParams {
        minimum_bond: proof_of_stake.minimum_bond,
        maximum_bond: proof_of_stake.maximum_bond,
        epoch_length: i64::from(proof_of_stake.epoch_length),
        quarantine_length: i64::from(proof_of_stake.quarantine_length),
        number_of_active_validators: i64::from(proof_of_stake.number_of_active_validators),
        executor_share: proof_of_stake.executor_share,
        absence_slack: proof_of_stake.absence_slack,
        participation_grace: proof_of_stake.participation_grace,
        min_phlo_price: proof_of_stake.min_phlo_price,
        max_number_of_parents: proof_of_stake.max_number_of_parents,
        dev_tax_vesting_blocks: proof_of_stake.dev_tax_vesting_blocks,
        liveness_absence_threshold: proof_of_stake.liveness_absence_threshold,
        liveness_jail_after_boundaries: proof_of_stake.liveness_jail_after_boundaries,
        liveness_jail_epochs: proof_of_stake.liveness_jail_epochs,
        liveness_vote_window: proof_of_stake.liveness_vote_window,
        liveness_slash_severity: proof_of_stake.liveness_slash_severity,
    };
    PosGenesis {
        bonds,
        trusted,
        params,
        dev_vesting,
    }
}

/// Build the genesis PoS descriptors for one shard from its spec (the same pool/trusted/params that
/// shard's genesis ceremony installs). Used to seed `RuntimeManager` so a genesis-block replay
/// reconstructs the native genesis state. Call only on a genesis-ceremony (standalone) node: it may
/// create the bonds file via `parse_or_generate`.
pub fn pos_genesis_from_config(spec: &crate::conf::ShardSpec) -> Result<PosGenesis, String> {
    let gbd = &spec.genesis_block_data;
    let bonds = crate::bonds_parser::parse_or_generate(
        std::path::Path::new(&gbd.bonds_file),
        spec.autogen_shard_size,
    )?;
    let vaults = crate::vault_parser::parse_if_exists(std::path::Path::new(&gbd.wallets_file))?;
    let dev_vesting = dev_allocation(&vaults, &gbd.dev_tax_address)?;
    Ok(build_pos_genesis(
        &proof_of_stake_from_config(gbd, bonds)?,
        dev_vesting,
    ))
}

/// The genesis descriptors a node needs to *replay* the genesis block: the native PoS state
/// (`compute_genesis`'s `install_genesis`) and the initial REV vault balances (`set_vault_balance`).
///
/// Both are installed as native state **outside** the genesis block's deploys, so neither is
/// recoverable from the block — a node that replays the genesis without them computes a different
/// post-state hash and then refuses the block. That is AUDIT C46: a joining validator, which is
/// exactly the node that indexes the genesis for the first time, used to replay it with
/// `PosGenesis::default()` and no vaults at all.
pub struct GenesisDescriptors {
    pub pos_genesis: PosGenesis,
    pub vaults: Vec<Vault>,
}

/// The developer allocation for a parsed wallets file (Laws 78–79), as the recipient's base58 address
/// and the amount — or `None` when no address is configured.
///
/// **The one place the allocation is derived**, called by both ends of the genesis: the ceremony
/// that *creates* the block ([`crate::engine::node_launch::create_genesis_block`]) and the descriptor
/// reader every node uses to *replay* it ([`genesis_descriptors_from_config`]). One function, so the
/// two cannot disagree about an amount that is consensus state; a second copy is exactly the drift
/// this indirection exists to prevent. `dev_tax` carries the rule, the refusals, and the reason the
/// allocation is not a vault.
pub fn dev_allocation(
    vaults: &[Vault],
    dev_tax_address: &str,
) -> Result<Option<(String, NonNegI64)>, String> {
    Ok(dev_tax::dev_allocation(vaults, dev_tax_address)?
        .map(|(address, amount)| (address.to_base58(), amount)))
}

/// Read the genesis descriptors from the configured genesis files.
///
/// Read on **any** node, not only a bootstrap: a joining validator replays the genesis when it
/// indexes it, and these files are part of the network configuration rather than of the ceremony.
/// `Ok(None)` means the node has no genesis config at all — the one case in which the genesis is
/// not replayable, and the caller must not pretend otherwise.
///
/// `is_ceremony` is the standalone node's privilege: only it may *create* the bonds file. A syncing
/// node reads it strictly, because `parse_or_generate` there would mint a fresh validator set and
/// quietly reconcile it against a different chain's genesis than the one it is syncing.
///
/// The vaults use the tolerant `parse_if_exists`: a node whose configuration names a wallets file
/// that is absent has no vault balances to re-install, which is a state the caller can act on
/// (empty vaults), unlike a bonds file it would have had to invent.
///
/// **The developer allocation is derived from the parsed vaults** (Laws 78–79, [`dev_tax`]):
/// `max(20 000 000 REV, 2% of the file's total)`, credited to `dev_tax_address` — when one is
/// configured — as a **vesting account** rather than a vault, so it is not spendable until it vests.
/// It rides on [`GenesisDescriptors::pos_genesis`], which is the native state both the ceremony and a
/// replaying node install through the one call in `install_genesis`; deriving it rather than reading it
/// is what keeps those two from disagreeing, and a wallets file that repeats an address — or names the
/// recipient — is refused, because the amount has to be the total the chain will hold.
pub fn genesis_descriptors_from_config(
    spec: &crate::conf::ShardSpec,
    is_ceremony: bool,
) -> Result<Option<GenesisDescriptors>, String> {
    let gbd = &spec.genesis_block_data;
    let bonds_path = std::path::Path::new(&gbd.bonds_file);
    if !is_ceremony && !bonds_path.exists() {
        return Ok(None);
    }
    let bonds = if is_ceremony {
        crate::bonds_parser::parse_or_generate(bonds_path, spec.autogen_shard_size)?
    } else {
        crate::bonds_parser::parse(bonds_path)?
    };
    let vaults = crate::vault_parser::parse_if_exists(std::path::Path::new(&gbd.wallets_file))?;
    let dev_vesting = dev_allocation(&vaults, &gbd.dev_tax_address)?;
    Ok(Some(GenesisDescriptors {
        pos_genesis: build_pos_genesis(&proof_of_stake_from_config(gbd, bonds)?, dev_vesting),
        vaults,
    }))
}

/// The configured PoS parameters with an already-parsed validator set.
fn proof_of_stake_from_config(
    gbd: &crate::conf::GenesisBlockData,
    bonds: BTreeMap<PublicKey, NonNegI64>,
) -> Result<ProofOfStake, String> {
    let validators: Vec<contracts::Validator> = bonds
        .into_iter()
        .map(|(pk, stake)| contracts::Validator { pk, stake })
        .collect();
    let pos = ProofOfStake {
        // Refused, not clamped (deferred item 1d): a genesis file that asks for a negative bond
        // bound is asking for a rule this protocol cannot state.
        minimum_bond: NonNegI64::try_from(gbd.bond_minimum)
            .map_err(|e| format!("casper.genesis.bond-minimum must not be negative: {e}"))?,
        maximum_bond: NonNegI64::try_from(gbd.bond_maximum)
            .map_err(|e| format!("casper.genesis.bond-maximum must not be negative: {e}"))?,
        validators,
        epoch_length: gbd.epoch_length,
        quarantine_length: gbd.quarantine_length,
        number_of_active_validators: gbd.number_of_active_validators,
        // Refused, not clamped, like the bond bounds: a share over the whole would pay the producer
        // more than the deploy burned, out of the stake the same vault holds for everyone else.
        executor_share: {
            let share = NonNegI64::try_from(i64::from(gbd.executor_share))
                .map_err(|e| format!("casper.genesis.executor-share must not be negative: {e}"))?;
            if i64::from(share) > 10_000 {
                return Err(format!(
                    "casper.genesis.executor-share is {} basis points, above the whole of what a \
                     deploy burns",
                    i64::from(share)
                ));
            }
            share
        },
        absence_slack: NonNegI64::try_from(i64::from(gbd.absence_slack))
            .map_err(|e| format!("casper.genesis.absence-slack must not be negative: {e}"))?,
        // **A negative grace is refused like its neighbours, and the caller has already applied the
        // "absent means the binary rule" reading** — a config file that omits the key arrives here with
        // the grace set to the slack, not to zero, so this is the value and not a fallback.
        participation_grace: NonNegI64::try_from(i64::from(gbd.participation_grace))
            .map_err(|e| format!("casper.genesis.participation-grace must not be negative: {e}"))?,
        min_phlo_price: NonNegI64::try_from(gbd.min_phlo_price)
            .map_err(|e| format!("casper.min-phlo-price must not be negative: {e}"))?,
        max_number_of_parents: i64::from(gbd.max_number_of_parents),
        // Refused, not clamped, like the bond bounds and the grace. A negative schedule is not a rule
        // this protocol can state — and the `0` an omitted key resolves to is a real rule (no
        // schedule), not a fallback, so the two must not be conflated here.
        dev_tax_vesting_blocks: NonNegI64::try_from(gbd.dev_tax_vesting_blocks).map_err(|e| {
            format!("casper.genesis.dev-tax-vesting-blocks must not be negative: {e}")
        })?,
        pos_multi_sig_public_keys: gbd.pos_multi_sig_public_keys.clone(),
        pos_multi_sig_quorum: gbd.pos_multi_sig_quorum,
        pos_vault_pub_key: gbd.pos_vault_pub_key.clone(),

        liveness_absence_threshold: gbd.liveness_absence_threshold,
        liveness_jail_after_boundaries: i64::from(gbd.liveness_jail_after_boundaries),
        liveness_jail_epochs: i64::from(gbd.liveness_jail_epochs),
        liveness_vote_window: i64::from(gbd.liveness_vote_window),
        liveness_slash_severity: i64::from(gbd.liveness_slash_severity),
    };
    // The liveness config is all-or-nothing; check it at the config boundary where a person wrote it.
    build_pos_genesis(&pos, None).params.validate()?;
    Ok(pos)
}

/// Build the unsigned genesis block from processed deploys (port of
/// `createBlockWithProcessedDeploys`). `bonds` is the *active* validator set.
fn create_block_with_processed_deploys(
    genesis: &Genesis,
    bonds: BTreeMap<ModelsValidator, NonNegI64>,
    pre_state_hash: StateHash,
    post_state_hash: StateHash,
    processed_deploys: Vec<ProcessedDeploy>,
) -> Result<BlockMessage, String> {
    if let Some(failed) = processed_deploys.iter().find(|d| d.is_failed) {
        return Err(format!(
            "Genesis block contains a failed deploy (deployer {:?})",
            failed.deploy.deployer
        ));
    }
    let state = RholangState {
        deploys: processed_deploys,
        system_deploys: Vec::new(),
    };
    let block_number = BlockHeight::try_from(genesis.block_number).map_err(|e| e.to_string())?;
    Ok(unsigned_block_proto(
        version_at(i64::from(block_number)),
        genesis.shard_id.clone(),
        block_number,
        ModelsValidator::from_slice(genesis.sender.bytes()),
        SeqNum::zero(),
        pre_state_hash,
        post_state_hash,
        Vec::new(),
        bonds,
        BTreeSet::new(),
        state,
        // Genesis carries a fixed timestamp (0) so every node agrees; informational only.
        0,
    ))
}

/// The order the blessed deploys depend on: `(dependent, what must already be installed)`.
///
/// Exactly one entry today, and it is the sharp kind: `MakeMint.rho:27` sends
/// `lookup!(\`rho:lang:nonNegativeNumber\`, …)` **during its own deploy** and waits on a reply pattern
/// a `Nil` reply cannot match. An unmatched `for` is not an error, so with the wrong order the deploy
/// *succeeds* while `MakeMint` never registers at all — nothing fails, and `lookup!` on `makeMint`
/// just keeps answering `Nil`. What catches it is the genesis ceremony's completeness check
/// (`missing_genesis_aliases`), and
/// `installing_make_mint_before_its_dependency_is_caught_by_the_genesis_check` pins both halves of
/// that: the silence, and the check that ends it.
///
/// The vendored rgov contracts need no entry: `memberIdGovRev` looks its `directory`/`inbox` imports
/// up **per call**, not at deploy time, so its position in the genesis list is free (all three are
/// genesis content, so a client calling it always finds them). That was worth checking rather than
/// assuming — the first version of this table claimed an order for it, and the negative test refuted
/// the claim.
#[cfg(test)]
const BLESSED_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("make_mint", &["non_negative_number"]),
    // `AuthKey.rho` looks `listOps` up before it can install; `MultiSigRevVault.rho` looks up
    // `authKey` *and* `listOps` (and `revVault`, which is native and always present).
    ("auth_key", &["list_ops"]),
    ("multi_sig_rev_vault", &["auth_key", "list_ops"]),
];

/// The blessed set with its manifest names, in install order.
///
/// The governance block ([`rgov::governance_deploys`]) follows the libraries: its own order is
/// internal (classes → master directory → the extra slots → the `GetMe` feature), because each step
/// resolves what the previous one published. It is **testnet-only** — see the module doc in
/// `rgov.rs` and `spec/GENESIS.md` for what a public network must do instead.
fn blessed_terms_named(
    shard_id: &str,
    ceremony: &ValidatorIdentity,
) -> Result<Vec<(&'static str, SignedDeployData)>, String> {
    let standard: Vec<(&'static str, SignedDeployData)> = vec![
        (
            "list_ops",
            standard_deploys::StandardDeploys::list_ops(shard_id)?,
        ),
        (
            "non_negative_number",
            standard_deploys::StandardDeploys::non_negative_number(shard_id)?,
        ),
        (
            "make_mint",
            standard_deploys::StandardDeploys::make_mint(shard_id)?,
        ),
        // **The multi-signature vault, installed rather than refused** (AUDIT C114's alternative).
        // `auth_key` first: `MultiSigRevVault.rho` looks it up before it can install, and its
        // `deployerAuthKey` is built through it. Both need `revVault` (native, always present) and
        // `authKey`'s own dependency `listOps` (above).
        (
            "auth_key",
            standard_deploys::StandardDeploys::auth_key(shard_id)?,
        ),
        (
            "multi_sig_rev_vault",
            standard_deploys::StandardDeploys::multi_sig_rev_vault(shard_id)?,
        ),
        // **The ERTP object API** (issue #249). It sits after the contracts and before the
        // governance block, and it depends on nothing above it: the issuer ledger is a native
        // system process, present on every chain regardless of what genesis installs.
        ("ertp", standard_deploys::StandardDeploys::ertp(shard_id)?),
    ];
    let rgov = rgov::governance_deploys(shard_id, ceremony)?;
    Ok(standard.into_iter().chain(rgov).collect())
}

/// Copy the **published** entries of the vendored governance contracts onto the constant keys this
/// node chooses (`rgov::contract_uri_for`, `rgov::masterdict_resolve_uri`).
///
/// A class registers with `insertArbitrary` — upstream's shape, which consumers destructure — so its
/// own URI is `blake2b256` of the deploy's RNG state: deterministic on genesis, but moved by
/// inserting any deploy before it. Each class therefore publishes `["<name>", uri]` on
/// `rgov::URI_PUBLISH_CHANNEL`, and this copies the stored entry to the constant key so a client can
/// hardcode it and does not depend on the install order.
///
/// Idempotent, and called after every blessed deploy — the template's read cap is published by the
/// deploy that mints it, which is several deploys after the classes.
pub async fn seed_rgov_aliases(runtime: &RuntimeManager) -> Result<usize, String> {
    let published = runtime
        .runtime()
        .get_data_par(&rgov::publish_channel())
        .await
        .map_err(|e| e.to_string())?;
    let native =
        rchain_rholang::native_state::NativeSystemState::new(runtime.runtime().native_store());
    seed_rgov_aliases_from(&published, &native).await
}

/// The play/replay-independent half of [`seed_rgov_aliases`]: it takes the published datums and the
/// native store, so the replay path (which holds a `ReplayRuntime`, not a `RuntimeManager`) can
/// reproduce the seeding exactly — a native write outside the deploy log must match or the replayed
/// genesis hash diverges (Law 11).
pub async fn seed_rgov_aliases_from(
    published: &[rchain_models::ast::Par],
    native: &rchain_rholang::native_state::NativeSystemState,
) -> Result<usize, String> {
    let mut seeded = 0;
    for datum in published {
        let Some((name, uri)) = rgov::published_uri(datum) else {
            continue;
        };
        let target = match name.as_str() {
            // The dictionary's three facets, which it registers with `insertArbitrary` like a class
            // and then publishes on the same channel. They replace the `readcap`/`grantcap` pair: a
            // client that reached for the read capability reaches the resolve facet instead, and
            // there is no grant capability to reach for.
            "masterdict-resolve" => rgov::masterdict_resolve_uri()?,
            "masterdict-publish" => rgov::masterdict_publish_uri()?,
            "masterdict-root" => rgov::masterdict_root_uri()?,
            _ => rgov::contract_uri_for(&name)?,
        };
        if native
            .registry_lookup(&target)
            .await
            .map_err(|e| e.to_string())?
            .is_some()
        {
            continue;
        }
        let Some(value) = native
            .registry_lookup(&uri)
            .await
            .map_err(|e| e.to_string())?
        else {
            // Published before its registration landed; the next call will see it.
            continue;
        };
        native.registry_insert(&target, &value);
        seeded += 1;
    }
    Ok(seeded)
}

/// Every governance key a fresh chain must resolve, for the ceremony's completeness check: the
/// classes, the master dictionary's three facets, and every seeded shorthand.
///
/// **All three facets are required, and each one's absence is silent.** A chain whose resolve facet
/// never landed does not fail anything: a client resolving it gets `Nil`, calls it, matches no
/// receive, and its read simply does not happen — the diagnostic trap issue #71 records, one layer
/// down. So a missing facet is a genesis defect, and the ceremony is the only place it can be caught
/// before a chain ships.
pub async fn missing_governance_keys(
    native: &rchain_rholang::native_state::NativeSystemState,
) -> Result<Vec<String>, String> {
    let mut missing = Vec::new();
    for name in rgov::RGOV_CORE {
        let uri = rgov::contract_uri_for(name)?;
        if native
            .registry_lookup(&uri)
            .await
            .map_err(|e| e.to_string())?
            .is_none()
        {
            missing.push(uri);
        }
    }
    for uri in [
        rgov::masterdict_resolve_uri()?,
        rgov::masterdict_publish_uri()?,
        rgov::masterdict_root_uri()?,
    ] {
        if native
            .registry_lookup(&uri)
            .await
            .map_err(|e| e.to_string())?
            .is_none()
        {
            missing.push(uri);
        }
    }
    Ok(missing)
}

/// The ordered list of blessed (standard) genesis deploys (port of `defaultBlessedTerms`).
///
/// Rust-first: the registry, PoS and vault *system* contracts are native (`rholang::native_state` +
/// `system_deploy::NativeSystemDeployOp`), so their `.rho`/`.rhox` sources stay a checklist and are
/// **not** installed — installing them would shadow consensus-critical logic with interpreted
/// equivalents. What genesis does install is the small set of interpreted contracts a consumer
/// actually reaches through `rho:registry:lookup`, in dependency order ([`BLESSED_DEPENDENCIES`]),
/// plus the registry aliases that make those lookups resolve ([`seed_registry_aliases`],
/// `spec/GENESIS.md`).
///
/// Before this, a fresh chain's registry was empty, so `lookup!(\`rho:rchain:revVault\`, *ch)`
/// answered `Nil` — and a consumer cannot tell a `Nil` reply from a pattern that never matched.
pub fn default_blessed_terms(
    _proof_of_stake: &ProofOfStake,
    _registry: &Registry,
    _vaults: &[Vault],
    shard_id: &str,
    ceremony: &ValidatorIdentity,
) -> Result<Vec<SignedDeployData>, String> {
    Ok(blessed_terms_named(shard_id, ceremony)?
        .into_iter()
        .map(|(_, deploy)| deploy)
        .collect())
}

/// Seed the genesis registry aliases whose source is now available. Idempotent, so the genesis loop
/// can call it after every blessed deploy and once at the end.
///
/// Native system channels are seeded on the first call. A *contract* alias is seeded as soon as the
/// blessed deploy that registered it has run — the entry is copied from the contract's deterministic
/// `rho:id` onto the shorthand consumers look up. This is called after *every* deploy rather than
/// once at the end because `MakeMint` looks up `rho:lang:nonNegativeNumber` **during** its deploy,
/// so that alias must already exist when MakeMint runs.
pub async fn seed_registry_aliases(
    native: &rchain_rholang::native_state::NativeSystemState,
) -> Result<usize, String> {
    let mut seeded = 0;
    for alias in standard_deploys::GENESIS_ALIASES {
        if native
            .registry_lookup(alias.shorthand)
            .await
            .map_err(|e| e.to_string())?
            .is_some()
        {
            continue;
        }
        let value = match alias.source {
            standard_deploys::GenesisAliasSource::NativeChannel => {
                standard_deploys::native_channel_alias(alias.shorthand).ok_or_else(|| {
                    format!(
                        "genesis alias {} names a native channel that is not a definition",
                        alias.shorthand
                    )
                })?
            }
            standard_deploys::GenesisAliasSource::Contract { private_key_hex } => {
                let uri = standard_deploys::contract_uri(private_key_hex)?;
                match native
                    .registry_lookup(&uri)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    Some(entry) => entry,
                    // Its blessed deploy has not run yet; the next call after it has will seed this.
                    None => continue,
                }
            }
        };
        native.registry_insert(alias.shorthand, &value);
        seeded += 1;
    }
    Ok(seeded)
}

/// Every shorthand the genesis registry must resolve after the blessed deploys have run. Used to
/// fail the genesis loudly if an alias did not get seeded — a missing alias is the silent-`Nil`
/// failure this whole module exists to remove.
pub async fn missing_genesis_aliases(
    native: &rchain_rholang::native_state::NativeSystemState,
) -> Result<Vec<&'static str>, String> {
    let mut missing = Vec::new();
    for alias in standard_deploys::GENESIS_ALIASES {
        if native
            .registry_lookup(alias.shorthand)
            .await
            .map_err(|e| e.to_string())?
            .is_none()
        {
            missing.push(alias.shorthand);
        }
    }
    Ok(missing)
}

/// Create the signed genesis block (port of `Genesis.createGenesisBlock`).
pub async fn create_genesis_block(
    validator: &ValidatorIdentity,
    genesis: &Genesis,
    runtime: &RuntimeManager,
) -> Result<BlockMessage, String> {
    let blessed_terms = default_blessed_terms(
        &genesis.proof_of_stake,
        &genesis.registry,
        &genesis.vaults,
        &genesis.shard_id,
        // The governance bootstrap is signed by the ceremony's own key: the master directory's admin
        // capability must belong to whoever runs genesis, never to a key derivable from the source.
        validator,
    )?;
    let block_data = BlockData {
        block_number: BlockHeight::try_from(genesis.block_number).map_err(|e| e.to_string())?,
        sender: genesis.sender.clone(),
        seq_num: SeqNum::zero(),
        // Genesis carries a fixed timestamp (0) so every node agrees; informational only.
        timestamp: 0,
    };
    let rand = BlockRandomSeed::random_generator_from_shard_id(&genesis.shard_id)
        .map_err(|e| e.to_string())?;
    let pos_genesis = build_pos_genesis(&genesis.proof_of_stake, genesis.dev_vesting.clone());
    let (start_hash, state_hash, processed_results) = runtime
        .compute_genesis(
            &blessed_terms,
            &rand,
            block_data,
            &pos_genesis,
            &genesis.vaults,
        )
        .await?;
    // Surface deploy evaluation errors (the Scala `require` only checks the `isFailed` flag; the
    // underlying errors are otherwise lost, making genesis failures opaque).
    for (i, r) in processed_results.iter().enumerate() {
        if !r.eval_result.errors.is_empty() {
            return Err(format!(
                "Genesis deploy #{i} failed: {:?}",
                r.eval_result.errors
            ));
        }
    }
    // The ceremony is where the full manifest must hold: a chain that starts with a shorthand
    // answering `Nil` gives every consumer a silent no-op, and the place to refuse that is here —
    // not on a client's first lookup days later.
    let native =
        rchain_rholang::native_state::NativeSystemState::new(runtime.runtime().native_store());
    let missing = missing_genesis_aliases(&native).await?;
    if !missing.is_empty() {
        return Err(format!(
            "genesis registry aliases were not seeded: {missing:?} — the chain would answer `Nil` \
             to those lookups"
        ));
    }
    // The governance block too: a class that registered but never published (or a template that
    // never minted its read cap) leaves a key a client hardcodes resolving to `Nil`, which is the
    // silent no-op this whole module exists to refuse.
    let missing_governance = missing_governance_keys(&native).await?;
    if !missing_governance.is_empty() {
        return Err(format!(
            "genesis governance keys were not published: {missing_governance:?} — a client \
             hardcoding them would get `Nil`"
        ));
    }
    let processed_deploys: Vec<ProcessedDeploy> =
        processed_results.into_iter().map(|r| r.deploy).collect();

    let unsigned_block = create_block_with_processed_deploys(
        genesis,
        pos_genesis.active_bonds(),
        start_hash.into(),
        state_hash.into(),
        processed_deploys,
    )?;
    let signed_block = validator
        .sign_block(&unsigned_block)
        .map_err(|e| e.to_string())?;

    // Signing must not change the block hash.
    if unsigned_block.block_hash != signed_block.block_hash {
        return Err("Signed block has different block hash than unsigned".to_string());
    }
    Ok(signed_block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis::contracts::Validator;

    /// The genesis ceremony's identity, fixed so the tests are deterministic.
    fn ceremony_identity() -> ValidatorIdentity {
        use rchain_crypto::private_key::PrivateKey;
        use rchain_crypto::signatures::secp256k1::Secp256k1;
        use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
        let sk = PrivateKey::new(vec![7u8; 32]);
        let public_key = Secp256k1
            .to_public(&sk)
            .unwrap_or_else(|e| panic!("a fixed 32-byte scalar is a valid key: {e}"));
        ValidatorIdentity {
            public_key,
            private_key: sk,
            sig_algorithm: "secp256k1".to_string(),
        }
    }

    fn pos() -> ProofOfStake {
        ProofOfStake {
            minimum_bond: NonNegI64::try_from(1).unwrap(),
            maximum_bond: NonNegI64::try_from(100).unwrap(),
            validators: vec![
                Validator {
                    pk: PublicKey::new(vec![1; 65]),
                    stake: 10.try_into().unwrap(),
                },
                Validator {
                    pk: PublicKey::new(vec![2; 65]),
                    stake: 20.try_into().unwrap(),
                },
            ],
            epoch_length: 0,
            quarantine_length: 0,
            number_of_active_validators: 0,
            executor_share: NonNegI64::try_from(0).unwrap(),
            absence_slack: NonNegI64::try_from(0).unwrap(),
            participation_grace: NonNegI64::try_from(0).unwrap(),
            min_phlo_price: NonNegI64::zero(),
            max_number_of_parents: 0,
            pos_multi_sig_public_keys: vec![],
            pos_multi_sig_quorum: 0,
            pos_vault_pub_key: String::new(),
            dev_tax_vesting_blocks: NonNegI64::zero(),

            liveness_absence_threshold: 0,
            liveness_jail_after_boundaries: 0,
            liveness_jail_epochs: 0,
            liveness_vote_window: 0,
            liveness_slash_severity: 0,
        }
    }

    /// **The fee floor and the parent bound are part of the genesis state**, not only of the node's
    /// config: they reach `PosParams`, whose encoding is a native-state leaf the genesis post-state hash
    /// covers. So two configurations that differ in either one encode different params records, and a
    /// node configured unlike its peers builds a different genesis instead of silently disagreeing with
    /// them about which blocks are valid.
    #[test]
    fn the_validity_bounds_are_committed_in_the_genesis_params() {
        let committed = ProofOfStake {
            min_phlo_price: NonNegI64::try_from(1).unwrap(),
            max_number_of_parents: 255,
            ..pos()
        };
        let params = build_pos_genesis(&committed, None).params;
        assert_eq!(i64::from(params.min_phlo_price), 1);
        assert_eq!(params.max_number_of_parents, 255);

        let other_floor = ProofOfStake {
            min_phlo_price: NonNegI64::try_from(2).unwrap(),
            ..committed.clone()
        };
        let other_width = ProofOfStake {
            max_number_of_parents: 256,
            ..committed.clone()
        };
        for other in [other_floor, other_width] {
            assert_ne!(
                build_pos_genesis(&other, None).params.encode(),
                params.encode(),
                "a different bound is a different genesis"
            );
        }
    }

    #[test]
    fn build_bonds_map_extracts_stakes() {
        let bonds = build_bonds_map(&pos());
        assert_eq!(bonds.len(), 2);
        assert_eq!(i64::from(bonds[&ModelsValidator::from_slice(&[1; 65])]), 10);
        assert_eq!(i64::from(bonds[&ModelsValidator::from_slice(&[2; 65])]), 20);
    }

    /// The blessed list is in dependency order, and every contract the table names is really in the
    /// list. Both halves matter: a dependent that is missing from the list is a typo that would
    /// otherwise pass, and a dependency ordered *after* its dependent is the silent failure
    /// `BLESSED_DEPENDENCIES` documents.
    #[test]
    fn blessed_terms_are_ordered_by_dependency() {
        let ceremony = ceremony_identity();
        let named = blessed_terms_named("root", &ceremony).expect("the blessed list builds");
        let names: Vec<&str> = named.iter().map(|(name, _)| *name).collect();
        let position = |name: &str| {
            names
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("{name} is not in the blessed list: {names:?}"))
        };
        for (dependent, dependencies) in BLESSED_DEPENDENCIES {
            for dependency in *dependencies {
                assert!(
                    position(dependency) < position(dependent),
                    "{dependent} resolves {dependency} during its own deploy, so {dependency} must \
                     be installed first (order: {names:?}) — a violation is silent, not an error"
                );
            }
        }
        // The names the table keys on are the ones the list carries: a rename that broke the table
        // would otherwise leave every assertion above vacuous.
        for (dependent, _) in BLESSED_DEPENDENCIES {
            position(dependent);
        }
        assert_eq!(
            names,
            vec![
                "list_ops",
                "non_negative_number",
                "make_mint",
                // **A genesis change, deliberately** (the multi-signature vault's install): the two
                // contracts sit after their dependencies and before the governance block. Every
                // chain built before this commit has a different genesis post-state, which is the
                // point of asserting the order here rather than discovering it in a fork.
                "auth_key",
                "multi_sig_rev_vault",
                // **A genesis change, deliberately** (issue #249, the ERTP object API): it goes
                // after the contracts that could depend on it and before the governance block.
                "ertp",
                "kudos",
                "inbox",
                "directory",
                "roll",
                "issue",
                "ballot",
                "chat",
                "group",
                // **A genesis change, deliberately** (issue #99): the rooted master dictionary
                // replaces the testnet template and its extra-slots term. It installs before the
                // feature, because the feature publishes into the namespace the dictionary owns.
                "masterDictionary",
                "memberDirectory",
            ],
            "the install order is part of the chain's identity — a change here is a genesis change"
        );
    }

    /// A genesis spec whose two files live in `dir` (they need not exist).
    fn spec_in(dir: &std::path::Path) -> crate::conf::ShardSpec {
        crate::conf::ShardSpec::new(
            "root".to_string(),
            "/".to_string(),
            crate::conf::GenesisBlockData {
                executor_share: 2500,
                absence_slack: 0,
                participation_grace: 0,
                min_phlo_price: 0,
                max_number_of_parents: 0,
                genesis_data_dir: dir.to_path_buf(),
                bonds_file: dir.join("bonds.txt").to_string_lossy().into_owned(),
                wallets_file: dir.join("wallets.txt").to_string_lossy().into_owned(),
                bond_minimum: 1,
                bond_maximum: 100,
                epoch_length: 100,
                quarantine_length: 10,
                genesis_block_number: 0,
                number_of_active_validators: 1,
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
            },
            1,
        )
        .expect("a minimal spec")
    }

    /// **A node with no genesis config cannot replay the genesis, and `None` says so** (AUDIT C46's
    /// other half). The bonds file is the marker: a wallets file alone is not a genesis config, and a
    /// non-ceremony node must not invent the validator set it would need.
    #[test]
    fn a_node_without_a_bonds_file_has_no_genesis_descriptors() {
        let dir = std::env::temp_dir().join(format!("rchain-c46-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let spec = spec_in(&dir);

        assert!(
            genesis_descriptors_from_config(&spec, false)
                .expect("absent files are not an error")
                .is_none(),
            "no files at all: the node has no genesis config"
        );

        std::fs::write(dir.join("wallets.txt"), "").expect("wallets");
        assert!(
            genesis_descriptors_from_config(&spec, false)
                .expect("a stray wallets file is not an error")
                .is_none(),
            "the vaults alone are not a genesis config"
        );

        // A *malformed* bonds file is an error rather than a silent regeneration: a syncing node
        // that minted its own validator set would reconcile against a different chain's genesis.
        std::fs::write(dir.join("bonds.txt"), "not-a-public-key 100\n").expect("bonds");
        assert!(
            genesis_descriptors_from_config(&spec, false).is_err(),
            "an unreadable bonds file must fail loudly, not fall back"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// **With the files present, the descriptors carry the pool the genesis installed** — read
    /// strictly on a non-ceremony node, and read at all, which is what a joining validator's genesis
    /// replay was missing (AUDIT C46).
    #[test]
    fn a_node_with_the_genesis_files_reads_the_pool() {
        let dir = std::env::temp_dir().join(format!("rchain-c46-some-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let spec = spec_in(&dir);
        let pub_hex = base16::encode(ceremony_identity().public_key.bytes());
        std::fs::write(dir.join("bonds.txt"), format!("{pub_hex} 100\n")).expect("bonds");
        // Empty wallets: no vault balances to re-install, which `parse_if_exists` tolerates.
        std::fs::write(dir.join("wallets.txt"), "").expect("wallets");

        let d = genesis_descriptors_from_config(&spec, false)
            .expect("the files are present and well formed")
            .expect("a node with a bonds file has descriptors");
        assert_eq!(
            d.pos_genesis.bonds.len(),
            1,
            "the pool the genesis installed"
        );
        assert!(d.vaults.is_empty(), "no wallets, no balances");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// **The descriptors carry the developer allocation, derived from the parsed total** (Law 78).
    ///
    /// This is the wiring half of the law's Rust witness: `dev_tax.rs`'s unit tests cover the
    /// arithmetic, and this covers the one call that puts it on the replay path — the path a *joining*
    /// validator reads. Delete that call and every other test stays green, which is why it is asserted
    /// here rather than assumed from the unit tests. The last arm is the reason the file is checked:
    /// `set_vault_balance` overwrites, so a wallets file that already names the address would have that
    /// holder's balance *replaced*, not added to.
    #[test]
    fn the_descriptors_carry_the_developer_allocation() {
        fn address(second_byte: u8) -> rchain_rholang::util::rev_address::RevAddress {
            let mut key = vec![0u8; 65];
            key[0] = 0x04;
            key[1] = second_byte;
            rchain_rholang::util::rev_address::RevAddress::from_public_key(
                &rchain_crypto::public_key::PublicKey::new(key),
            )
            .expect("a valid address")
        }

        let dir = std::env::temp_dir().join(format!("rchain-dev-tax-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut spec = spec_in(&dir);
        let pub_hex = base16::encode(ceremony_identity().public_key.bytes());
        std::fs::write(dir.join("bonds.txt"), format!("{pub_hex} 100\n")).expect("bonds");
        let holder = address(0x00);
        // The published allocation's total, in drops: 908 962 714 REV.
        std::fs::write(
            dir.join("wallets.txt"),
            format!("{},90896271400000000\n", holder.to_base58()),
        )
        .expect("wallets");

        let none = genesis_descriptors_from_config(&spec, false)
            .expect("the files are present")
            .expect("descriptors");
        assert_eq!(none.vaults.len(), 1, "no address configured, no allocation");
        assert_eq!(
            none.pos_genesis.dev_vesting, None,
            "and no vesting account either"
        );

        let dev = address(0x01);
        spec.genesis_block_data.dev_tax_address = dev.to_base58();
        spec.genesis_block_data.dev_tax_vesting_blocks = 18_934_560;
        let d = genesis_descriptors_from_config(&spec, false)
            .expect("the files are present")
            .expect("descriptors");
        assert_eq!(
            d.vaults.len(),
            1,
            "the allocation is a *vesting account*, not a second spendable vault"
        );
        let (recipient, amount) = d
            .pos_genesis
            .dev_vesting
            .clone()
            .expect("the developer vesting account is present");
        assert_eq!(recipient, dev.to_base58());
        assert_eq!(
            i64::from(amount),
            2_000_000_000_000_000,
            "2% of the total is under the floor, so the floor is allocated"
        );
        assert_eq!(
            i64::from(d.pos_genesis.params.dev_tax_vesting_blocks),
            18_934_560,
            "and the schedule rides on the params, where the boundary reads it"
        );

        std::fs::write(
            dir.join("wallets.txt"),
            format!(
                "{},90896271400000000\n{},5\n",
                holder.to_base58(),
                dev.to_base58()
            ),
        )
        .expect("wallets");
        assert!(
            genesis_descriptors_from_config(&spec, false).is_err(),
            "a wallets file that already names the address would be overwritten, so it is refused"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
