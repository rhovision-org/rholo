//! Native system-contract state (registry / PoS / vault) layered over the rspace native store.
//!
//! The blessed rholang contracts (`Registry.rho`, `Pos.rhox`, `RevVault.rho`, …) re-implemented a
//! `TreeHashMap` trie *in interpreted rholang*; any interpreter bug made the registry silently empty.
//! This module replaces that fragile bootstrap with native Rust state: typed maps over the
//! content-addressed `InMemNativeStore`, exposed through the same `rho:*` protocol by native system
//! processes. The Scala contracts remain a *checklist* of required behavior only.
//!
//! # Dynamic validators
//!
//! The PoS state models the full validator lifecycle natively:
//!
//! * **observer** — any key that is not bonded; any node can run as an observer (no key, or a key
//!   that is not an active validator).
//! * **trusted** — admission into the validator *stakeholder group* ([`PosGenesis::trusted`]). Only
//!   a trusted key may bond; a trusted stakeholder confers trust via `rho:rchain:pos`'s `trust`.
//! * **bonded / pool** — the full bond pool ([`pos_bonds_key`]); a bond is within `[minimum,
//!   maximum]`, deducted from the validator's REV vault.
//! * **active** — the consensus validator set ([`pos_active_key`]): a draw of up to
//!   `number_of_active_validators` from the pool (0 = unlimited), redrawn at each epoch boundary
//!   from the seeded rule in `select_active`. Consensus (supermajority, finality fringe, bond
//!   queries) reads this set.
//! * **withdrawing** — a withdrawal **stages** a quarantine request
//!   ([`pos_pending_withdrawers_key`]); the validator stays bonded and active, still earning, until
//!   the boundary after the quarantine escrows the bond ([`pos_withdrawers_key`]), and a later
//!   boundary pays the bond plus its committed rewards.
//! * **removed** — a *justified* `slash` removes the validator from every PoS ledger and sends the
//!   tiered share to the Coop vault. **`untrust` only revokes admission and never confiscates a
//!   bond** (AUDIT C111): an untrusted validator keeps its stake, stays in the active set until the
//!   next boundary re-selects, and can withdraw. Admission is not membership.
//!
//! # The staking vault
//!
//! Every movement of REV in the PoS mechanism is a transfer between three places: a user's vault, the
//! **staking vault** ([`pos_vault_key`], the contract's `posVault`), and the Coop multisig vault. A
//! bond moves the stake in, a refund moves a deploy's unused phlo back out (the phlo it *did* use
//! stays in and funds the rewards), a slashing moves a stake to the Coop vault, and a withdrawal pays
//! a stake plus its committed rewards back out. The vault's balance is therefore the epoch pot's
//! source, and no step of the mechanism mints: the only credits are a bond, a phlo charge, and the
//! genesis install that funds the vault with exactly the initial bond sum.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::public_key::PublicKey;
use rchain_models::ast::Par;
use rchain_models::block_metadata::SlashSeverity;
use rchain_models::validator::Validator;
use rchain_shared::refined::{BlockHeight, NonNegI64};
use rchain_shared::serialize::Serialize;

use rchain_rspace::native_store::{
    InMemNativeStore, NativeStoreAction, PREFIX_ERTP, PREFIX_HTTP, PREFIX_POS, PREFIX_REGISTRY,
    PREFIX_TXN, PREFIX_VAULT, PREFIX_VAULT_AUTH, PREFIX_VAULT_NAME,
};

use crate::pos_liveness::{
    decode_absence, decode_anchor, decode_ballots, decode_validator_set, decode_vote_windows,
    encode_absence, encode_anchor, encode_ballots, encode_validator_set, encode_vote_windows,
    AbsenceRecord, Anchor, BallotKey, VoteWindow,
};
use crate::util::rev_address::RevAddress;

/// A public key / validator key is 65 uncompressed secp256k1 bytes.
const VALIDATOR_LEN: usize = 65;
/// The size of a serialized `(Validator, NonNegI64)` bond entry (65-byte key + 8-byte stake).
const BOND_ENTRY_LEN: usize = VALIDATOR_LEN + 8;
/// The size of a serialized withdrawal entry (65-byte key + 8-byte bond + 8-byte deadline).
const WITHDRAWER_ENTRY_LEN: usize = VALIDATOR_LEN + 8 + 8;
/// The size of a serialized withdrawal *request* entry (65-byte key + 8-byte deadline).
const PENDING_ENTRY_LEN: usize = VALIDATOR_LEN + 8;
/// The size of a serialized trusted-set entry.
const TRUSTED_ENTRY_LEN: usize = VALIDATOR_LEN;
/// The format byte at the head of a `PosParams` record, followed by [`PARAMS_FIELDS`] little-endian
/// `i64`s.
///
/// **Why a format byte, and not one more appended field.** The record used to be positional: the
/// decoder inferred the fields from its length, and an absent trailing field read as zero. That kept old
/// records readable, but two branches that each append a field in a different order produce records of
/// the same length that decode to the wrong values with no error. Now each layout has a number: adding a
/// field means a new format, a record in a format this node does not know is refused rather than read,
/// and two branches that both claim the next number collide on this line at merge time.
///
/// **Format 2 appends the developer allocation's vesting term** (Law 79) at payload index 10, after the
/// committed validity bounds #376 placed at indices 8 and 9. Append-only: indices 0–9 are format 1's
/// fields in format 1's order.
const PARAMS_FORMAT: u8 = 3;
/// The format a record without liveness fields still writes, and the only other versioned format this
/// node reads. A zeroed liveness config **encodes as format 2 byte-for-byte**, so an old chain's
/// params leaf and genesis post-state hash are unchanged by a binary that knows format 3.
const PARAMS_FORMAT_LEGACY: u8 = 2;
/// The number of fields in a format-[`PARAMS_FORMAT`] record.
const PARAMS_FIELDS: usize = 16;
/// The length of a current record: the format byte and [`PARAMS_FIELDS`] fields. It is one more than a
/// multiple of eight, which is what tells it from an unversioned record (always a multiple of eight).
const PARAMS_LEN: usize = 1 + PARAMS_FIELDS * 8;
/// A format-2 record: the format byte and the eleven fields before liveness enforcement.
const PARAMS_LEN_V2: usize = 1 + 11 * 8;
/// The length of a record that carries the participation grace (the eighth field). The grace is the one
/// field whose absence is not zero, so the decoder has to know where it ends, not only where the record
/// does — an eight-field record carries it, a seven-field one does not. Eight fields is also the longest
/// unversioned record any chain wrote: everything after it is versioned.
const PARAMS_LEN_WITH_GRACE: usize = 8 * 8;
/// The length of the *original* `PosParams` — the five fields before the producer's share (B2, #150),
/// the absence knee (B4, #150), the participation grace (B4, #150), the committed validity bounds
/// (#373/#376) and the vesting term (Law 79) were added, in that order. A unversioned record at least
/// this long reads the fields it carries and defaults the rest, which is the pre-change rule for each
/// (see [`decode_params`]).
const PARAMS_LEN_ORIGINAL: usize = 5 * 8;

// --- Leaf keys ---------------------------------------------------------------

/// Leaf key for the PoS bond *pool* (all bonded validators).
pub fn pos_bonds_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:bonds")
}

/// Leaf key for the *active* validator set (what consensus uses).
pub fn pos_active_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:active")
}

/// Leaf key for the trusted validator-stakeholder set (admission gate for bonding).
pub fn pos_trusted_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:trusted")
}

/// Leaf key for pending withdrawers (`validator → quarantine deadline block`).
pub fn pos_withdrawers_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:withdrawers")
}

/// Leaf key for the withdrawal *requests* — `validator → epoch-boundary deadline`
/// (`Pos.rhox`'s `pendingWithdrawers`). A request stays here until the next epoch boundary, where
/// `close_block` moves it into [`pos_withdrawers_key`] and takes the validator out of the pool.
pub fn pos_pending_withdrawers_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:pending_withdrawers")
}

/// Leaf key for rewards earned but not yet paid out — `validator → NonNegI64`
/// (`Pos.rhox`'s `committedRewards`). A validator's entry accumulates across epochs and is paid with
/// its bond when its withdrawal is released.
pub fn pos_committed_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:committed")
}

// **`pos:last_spoke` was the per-validator activity record** — `validator → BlockHeight`, written by a
// block-level system deploy, and the key the absence rule used to read. Retired 2026-10-02 (B4, #150):
// the participation now comes from the last finalised fringe, which every node derives from the DAG, so
// the leaf was a consensus-visible write that nothing read. Its key is not reused — a leaf that came
// back under the same name would be read by nothing and trusted by assumption.

/// Leaf key for liveness enforcement's per-validator absence/jail counters (`pos:liveness`).
///
/// Written only at an advancing epoch boundary and only when the mechanism is armed; a zeroed
/// liveness config never creates it, so an old chain's state is unchanged.
pub fn pos_liveness_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:liveness")
}

/// Leaf key for the liveness anchor: the last boundary at which finality actually advanced.
pub fn pos_liveness_anchor_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:liveness-anchor")
}

/// Leaf key for open slash-vote windows (`validator → window`).
pub fn pos_vote_windows_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:vote-windows")
}

/// Leaf key for ballots in open slash votes (`(target, voter) → in favour`).
pub fn pos_slash_votes_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:slash-votes")
}

/// Leaf key for validators permanently ejected by a passing slash vote (`pos:banished`).
pub fn pos_banished_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:banished")
}

/// Leaf key for the immutable PoS parameters.
pub fn pos_params_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:params")
}

/// Leaf key for the Coop slashing vault (confiscated stake).
pub fn pos_coop_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:coop")
}

/// Leaf key for the PoS *staking vault*: the escrowed bonds, plus the phlo charged from deploys that
/// has not been refunded. This is the contract's `posVault` (`Pos.rhox:161-174`), and it is the pot
/// an epoch distributes — see [`NativeSystemState::close_block`].
pub fn pos_vault_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:vault")
}

/// Leaf key for the developer allocation's vesting state (Laws 78–79) — recipient, total, and how much
/// is still unreleased.
///
/// One leaf rather than three, because the three move together: a boundary that releases REV changes
/// the unreleased amount, and nothing else ever writes here. A separate *account* rather than a vault,
/// because the allocation is not spendable at genesis and the schedule is the only thing that makes it
/// spendable — once released it is an ordinary balance in the recipient's vault, and no other rule in
/// the chain has to know this leaf exists.
pub fn pos_dev_vesting_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:dev-vesting")
}

/// The developer allocation's vesting state, as stored under [`pos_dev_vesting_key`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevVesting {
    /// The recipient, as its base58 address — the same spelling `vault_key` uses, so the account and
    /// the vault it releases into agree on what an address is.
    pub address: String,
    /// The whole allocation, fixed at genesis. The rate is applied to this and to nothing else.
    pub allocation: NonNegI64,
    /// What has not vested yet. `0` once the schedule has run out.
    pub unreleased: NonNegI64,
}

impl DevVesting {
    /// Encode as `<u32 little-endian address length><address bytes><allocation i64><unreleased i64>`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.address.len() + 16);
        out.extend_from_slice(&(self.address.len() as u32).to_le_bytes());
        out.extend_from_slice(self.address.as_bytes());
        out.extend_from_slice(&i64::from(self.allocation).to_le_bytes());
        out.extend_from_slice(&i64::from(self.unreleased).to_le_bytes());
        out
    }

    /// Decode [`DevVesting`]. Refuses a malformed record and a negative either amount — the same rule
    /// the bond bounds and the params fields get, since a corrupted leaf must not become a permissive
    /// reading of the same bytes.
    pub fn decode(bytes: &[u8]) -> Result<DevVesting, String> {
        if bytes.len() < 4 {
            return Err(format!(
                "dev vesting record is {} bytes, too short",
                bytes.len()
            ));
        }
        let len = u32::from_le_bytes(
            bytes[..4]
                .try_into()
                .map_err(|_| "dev vesting address length is malformed".to_string())?,
        ) as usize;
        if bytes.len() != 4 + len + 16 {
            return Err(format!(
                "dev vesting record is {} bytes, expected {}",
                bytes.len(),
                4 + len + 16
            ));
        }
        let address = String::from_utf8(bytes[4..4 + len].to_vec())
            .map_err(|e| format!("the dev vesting address is not UTF-8: {e}"))?;
        let read = |at: usize| -> i64 {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&bytes[at..at + 8]);
            i64::from_le_bytes(arr)
        };
        Ok(DevVesting {
            address,
            allocation: NonNegI64::try_from(read(4 + len))
                .map_err(|e| format!("the dev vesting allocation is not a valid amount: {e}"))?,
            unreleased: NonNegI64::try_from(read(4 + len + 8)).map_err(|e| {
                format!("the dev vesting unreleased balance is not a valid amount: {e}")
            })?,
        })
    }
}

/// How much of `allocation` has vested at `height`, over a schedule of `duration` blocks from genesis
/// (**Law 79**).
///
/// Linear and with no cliff: the vested fraction is `height / duration`, truncated toward the network
/// so the released amount never runs ahead of the schedule. Total at or after `duration`.
///
/// `duration <= 0` means *no schedule*, and the whole allocation is vested from the start — which is
/// what a params record written before the vesting field existed decodes to (see `decode_params`), and
/// the rule for a chain that wants the allocation spendable at its first boundary.
///
/// **Why `Result`.** The product `allocation × height` does not fit an `i64` for a large allocation on
/// a long schedule, so it is taken in `i128`; the quotient provably fits here (`height < duration`
/// implies a strictly sub-`allocation` result), but a `try_from` that cannot fail is still written as
/// one rather than as a cast or an `expect`, both of which this crate audits.
pub fn dev_tax_vested(allocation: i64, duration: i64, height: i64) -> Result<i64, String> {
    if duration <= 0 || height >= duration {
        return Ok(allocation);
    }
    if height <= 0 {
        return Ok(0);
    }
    let vested = (i128::from(allocation) * i128::from(height)) / i128::from(duration);
    i64::try_from(vested).map_err(|_| {
        format!(
            "the vested developer allocation {vested} does not fit a balance, at height {height}"
        )
    })
}

/// Leaf key for the HTTP-result oracle table (`url → (value, captured block)`); RCHIP #54.
pub fn http_records_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"http:records")
}

/// Leaf key for a registry URI (the URI string, hashed).
fn registry_key(uri: &str) -> Blake2b256Hash {
    Blake2b256Hash::create(uri.as_bytes())
}

/// Leaf key for a vault balance (the REV address base58 string, hashed).
pub fn vault_key(address: &str) -> Blake2b256Hash {
    Blake2b256Hash::create(address.as_bytes())
}

/// Leaf key for a cross-shard transaction record (the `txn_id` bytes, hashed).
fn txn_key(id: &[u8]) -> Blake2b256Hash {
    Blake2b256Hash::create(id)
}

/// Leaf key for a **vault handle** (the minted name's bytes, hashed).
///
/// The name's own bytes are the key rather than a hash of the address, because a handle is per-call:
/// the same vault handed out twice is two names, and both must resolve (the oracle mints a fresh
/// purse per `findOrCreate` too — `RevVault.rho:103-140`).
fn vault_name_key(name: &[u8]) -> Blake2b256Hash {
    Blake2b256Hash::create(name)
}

/// The state of a cross-shard two-phase-commit transaction record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxnState {
    Prepared,
    Committed,
    Aborted,
}

impl TxnState {
    fn discriminant(self) -> u8 {
        match self {
            TxnState::Prepared => 0,
            TxnState::Committed => 1,
            TxnState::Aborted => 2,
        }
    }

    fn from_discriminant(b: u8) -> Option<Self> {
        match b {
            0 => Some(TxnState::Prepared),
            1 => Some(TxnState::Committed),
            2 => Some(TxnState::Aborted),
            _ => None,
        }
    }
}

/// A cross-shard 2PC transaction record: the escrowed REV, its source and destination, and the
/// coordinator key authorized to drive commit/abort.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxnRecord {
    pub state: TxnState,
    pub coordinator: PublicKey,
    pub amount: NonNegI64,
    pub from: String,
    pub to: String,
}

/// Canonically encode a transaction record: 1-byte state, 65-byte coordinator key, 8-byte LE amount,
/// then two length-prefixed (u32 LE) REV addresses.
fn encode_txn(rec: &TxnRecord) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + VALIDATOR_LEN + 8 + 8 + rec.from.len() + rec.to.len());
    out.push(rec.state.discriminant());
    out.extend_from_slice(rec.coordinator.bytes());
    out.extend_from_slice(&i64::from(rec.amount).to_le_bytes());
    for addr in [&rec.from, &rec.to] {
        out.extend_from_slice(&(addr.len() as u32).to_le_bytes());
        out.extend_from_slice(addr.as_bytes());
    }
    out
}

/// Read a length-prefixed (u32 LE) string at `off`, advancing it.
fn read_len_prefixed(bytes: &[u8], off: &mut usize) -> Result<String, String> {
    let len_end = off.checked_add(4).ok_or("txn record: offset overflow")?;
    if len_end > bytes.len() {
        return Err("txn record truncated".to_string());
    }
    let len_bytes: [u8; 4] = bytes[*off..len_end]
        .try_into()
        .map_err(|_| "txn record: invalid length prefix".to_string())?;
    let len = usize::try_from(u32::from_le_bytes(len_bytes))
        .map_err(|_| "txn record: length does not fit usize".to_string())?;
    *off = len_end;
    let s_end = off.checked_add(len).ok_or("txn record: length overflow")?;
    if s_end > bytes.len() {
        return Err("txn record truncated".to_string());
    }
    let s = String::from_utf8(bytes[*off..s_end].to_vec())
        .map_err(|_| "txn record: non-UTF8 address".to_string())?;
    *off = s_end;
    Ok(s)
}

/// Decode a transaction record (inverse of [`encode_txn`]).
fn decode_txn(bytes: &[u8]) -> Result<TxnRecord, String> {
    if bytes.len() < 1 + VALIDATOR_LEN + 8 + 4 + 4 {
        return Err("txn record too short".to_string());
    }
    let state = TxnState::from_discriminant(bytes[0])
        .ok_or_else(|| format!("txn record: unknown state {}", bytes[0]))?;
    let coordinator = PublicKey::new(bytes[1..1 + VALIDATOR_LEN].to_vec());
    let amount_bytes: [u8; 8] = bytes[1 + VALIDATOR_LEN..1 + VALIDATOR_LEN + 8]
        .try_into()
        .map_err(|_| "txn record: invalid amount".to_string())?;
    let amount = NonNegI64::try_from(i64::from_le_bytes(amount_bytes))
        .map_err(|_| "txn record: negative amount".to_string())?;
    let mut off = 1 + VALIDATOR_LEN + 8;
    let from = read_len_prefixed(bytes, &mut off)?;
    let to = read_len_prefixed(bytes, &mut off)?;
    if off != bytes.len() {
        return Err("txn record has trailing bytes".to_string());
    }
    Ok(TxnRecord {
        state,
        coordinator,
        amount,
        from,
        to,
    })
}

// --- Canonical encoders ------------------------------------------------------

/// Canonically encode a bonds map (sorted by `Validator`, 65-byte key + little-endian stake).
pub fn encode_bonds(bonds: &BTreeMap<Validator, NonNegI64>) -> Vec<u8> {
    let mut out = Vec::with_capacity(bonds.len() * BOND_ENTRY_LEN);
    for (v, stake) in bonds {
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(&i64::from(*stake).to_le_bytes());
    }
    out
}

/// Decode a bonds map (inverse of [`encode_bonds`]).
pub fn decode_bonds(bytes: &[u8]) -> Result<BTreeMap<Validator, NonNegI64>, String> {
    if bytes.len() % BOND_ENTRY_LEN != 0 {
        return Err(format!(
            "bonds encoding has {} bytes, not a multiple of {BOND_ENTRY_LEN}",
            bytes.len()
        ));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(BOND_ENTRY_LEN) {
        let validator = Validator::from_slice(&chunk[..VALIDATOR_LEN]);
        let stake_bytes: [u8; 8] = chunk[VALIDATOR_LEN..BOND_ENTRY_LEN]
            .try_into()
            .map_err(|_| "bonds encoding: invalid stake length".to_string())?;
        let stake = i64::from_le_bytes(stake_bytes);
        let stake =
            NonNegI64::try_from(stake).map_err(|_| format!("negative bond stake {stake}"))?;
        out.insert(validator, stake);
    }
    Ok(out)
}

// --- HTTP-result oracle (RCHIP #54) -----------------------------------------

/// A recorded HTTP value: the captured body and the block height at which it was captured.
pub type HttpRecord = (String, i64);

/// Canonically encode the HTTP-record table (sorted by URL; length-prefixed strings and a
/// little-endian capture block).
pub fn encode_http_records(records: &BTreeMap<String, HttpRecord>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for (url, (value, block_number)) in records {
        out.extend_from_slice(&(url.len() as u32).to_le_bytes());
        out.extend_from_slice(url.as_bytes());
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(&block_number.to_le_bytes());
    }
    out
}

/// Decode the HTTP-record table (inverse of [`encode_http_records`]).
pub fn decode_http_records(bytes: &[u8]) -> Result<BTreeMap<String, HttpRecord>, String> {
    if bytes.len() < 4 {
        return Err("http records: truncated count".to_string());
    }
    let count_bytes: [u8; 4] = bytes[..4]
        .try_into()
        .map_err(|_| "http records: invalid count".to_string())?;
    let count = usize::try_from(u32::from_le_bytes(count_bytes))
        .map_err(|_| "http records: count does not fit usize".to_string())?;
    let mut out = BTreeMap::new();
    let mut off = 4usize;
    for _ in 0..count {
        let url = read_len_prefixed(bytes, &mut off)?;
        let value = read_len_prefixed(bytes, &mut off)?;
        let bn_end = off.checked_add(8).ok_or("http records: offset overflow")?;
        if bn_end > bytes.len() {
            return Err("http records: truncated block number".to_string());
        }
        let block_bytes: [u8; 8] = bytes[off..bn_end]
            .try_into()
            .map_err(|_| "http records: invalid block number".to_string())?;
        off = bn_end;
        out.insert(url, (value, i64::from_le_bytes(block_bytes)));
    }
    if off != bytes.len() {
        return Err("http records: trailing bytes".to_string());
    }
    Ok(out)
}

/// Canonically encode the trusted stakeholder set (sorted 65-byte keys).
pub fn encode_trusted(trusted: &BTreeSet<Validator>) -> Vec<u8> {
    let mut out = Vec::with_capacity(trusted.len() * TRUSTED_ENTRY_LEN);
    for v in trusted {
        out.extend_from_slice(v.as_bytes());
    }
    out
}

/// Decode the trusted stakeholder set (inverse of [`encode_trusted`]).
pub fn decode_trusted(bytes: &[u8]) -> Result<BTreeSet<Validator>, String> {
    if bytes.len() % TRUSTED_ENTRY_LEN != 0 {
        return Err(format!(
            "trusted encoding has {} bytes, not a multiple of {TRUSTED_ENTRY_LEN}",
            bytes.len()
        ));
    }
    Ok(bytes
        .chunks_exact(TRUSTED_ENTRY_LEN)
        .map(Validator::from_slice)
        .collect())
}

/// Canonically encode pending withdrawers (`validator → epoch-boundary deadline`, sorted, LE
/// deadline).
pub fn encode_pending_withdrawers(pending: &BTreeMap<Validator, i64>) -> Vec<u8> {
    let mut out = Vec::with_capacity(pending.len() * PENDING_ENTRY_LEN);
    for (v, deadline) in pending {
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(&deadline.to_le_bytes());
    }
    out
}

/// Decode pending withdrawers (inverse of [`encode_pending_withdrawers`]).
pub fn decode_pending_withdrawers(bytes: &[u8]) -> Result<BTreeMap<Validator, i64>, String> {
    if bytes.len() % PENDING_ENTRY_LEN != 0 {
        return Err(format!(
            "pending withdrawers encoding has {} bytes, not a multiple of {PENDING_ENTRY_LEN}",
            bytes.len()
        ));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(PENDING_ENTRY_LEN) {
        let validator = Validator::from_slice(&chunk[..VALIDATOR_LEN]);
        let deadline: [u8; 8] = chunk[VALIDATOR_LEN..PENDING_ENTRY_LEN]
            .try_into()
            .map_err(|_| "pending withdrawers encoding: invalid deadline length".to_string())?;
        out.insert(validator, i64::from_le_bytes(deadline));
    }
    Ok(out)
}

/// An escrowed withdrawal (`Pos.rhox`'s `withdrawers` entry): the bond taken out of the pool at the
/// epoch boundary that moved the validator out, and the block at which it may be paid.
///
/// The stored amount is the **bond** alone: `movePendingWithdrawer` sets the entry from
/// `allBonds.get(pk)` (`Pos.rhox:582`), and the reward is added at payment time from the committed
/// map (`removeQuarantinedWithdrawers`, `:604` — `bonds + committedRewards.getOrElse(pk, 0)`). The
/// contract's header comment describes the pair as `(original bond + reward, quantinue length)`
/// (`:189-192`), which is what the payee *receives*; the code at `:582` is what is stored, and the
/// difference is what makes the epoch's last reward reach a validator that has already left the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Withdrawal {
    /// The stake escrowed out of the pool when the validator was moved here.
    pub bond: NonNegI64,
    /// The block at which the claim may be paid (`currentBlockNumber >= deadline`).
    pub deadline: i64,
}

/// Canonically encode withdrawals: 65-byte validator, LE bond, LE deadline.
pub fn encode_withdrawers(withdrawers: &BTreeMap<Validator, Withdrawal>) -> Vec<u8> {
    let mut out = Vec::with_capacity(withdrawers.len() * WITHDRAWER_ENTRY_LEN);
    for (v, w) in withdrawers {
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(&i64::from(w.bond).to_le_bytes());
        out.extend_from_slice(&w.deadline.to_le_bytes());
    }
    out
}

/// Decode withdrawals (inverse of [`encode_withdrawers`]).
pub fn decode_withdrawers(bytes: &[u8]) -> Result<BTreeMap<Validator, Withdrawal>, String> {
    if bytes.len() % WITHDRAWER_ENTRY_LEN != 0 {
        return Err(format!(
            "withdrawers encoding has {} bytes, not a multiple of {WITHDRAWER_ENTRY_LEN}",
            bytes.len()
        ));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(WITHDRAWER_ENTRY_LEN) {
        let validator = Validator::from_slice(&chunk[..VALIDATOR_LEN]);
        let bond: [u8; 8] = chunk[VALIDATOR_LEN..VALIDATOR_LEN + 8]
            .try_into()
            .map_err(|_| "withdrawers encoding: invalid bond length".to_string())?;
        let bond = NonNegI64::try_from(i64::from_le_bytes(bond))
            .map_err(|e| format!("withdrawers encoding: {e}"))?;
        let deadline: [u8; 8] = chunk[VALIDATOR_LEN + 8..WITHDRAWER_ENTRY_LEN]
            .try_into()
            .map_err(|_| "withdrawers encoding: invalid deadline length".to_string())?;
        out.insert(
            validator,
            Withdrawal {
                bond,
                deadline: i64::from_le_bytes(deadline),
            },
        );
    }
    Ok(out)
}

/// The immutable PoS parameters (installed at genesis, network-wide constants).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PosParams {
    /// Minimum accepted bond (stake) per validator. **`NonNegI64`, not `i64`** (the deferred item
    /// 1d): a negative minimum is not a configuration this protocol has a meaning for, and while the
    /// field was signed the only thing standing between a genesis file and a stored `-1` was the
    /// bond check's arithmetic — where a *negative* minimum silently accepts every bond, including
    /// the zero-stake one the check exists to refuse. `decode_params` now refuses one at the wire,
    /// so the invariant holds on every value that reaches this struct.
    pub minimum_bond: NonNegI64,
    /// Maximum accepted bond (stake) per validator. Refined for the same reason, in the direction
    /// that matters at the other end: `i64::MAX` is the "no maximum" value, and a negative maximum
    /// would refuse every bond.
    pub maximum_bond: NonNegI64,
    /// Epoch length in blocks (`<= 1` = every block is an epoch boundary).
    pub epoch_length: i64,
    /// Quarantine length in blocks between a withdrawal request and the refund.
    pub quarantine_length: i64,
    /// Maximum size of the active set, by descending stake (`0` = unlimited).
    pub number_of_active_validators: i64,
    /// **What share of a deploy's burned phlo goes to the block's producer** (B2, #150), in basis
    /// points of the amount the staking vault keeps for that deploy. `0` is the contract's own
    /// behaviour — every burned photon reaches the epoch pot and is split by the drawn set — and it is
    /// what a params record written before this field existed decodes to.
    ///
    /// Refined and bounded above by `10000`: a share over 100 % would pay the producer more than the
    /// deploy burned, out of other validators' stake in the same vault. A genesis parameter rather
    /// than a constant because it decides *amounts*, which are consensus state: two nodes with
    /// different values compute different post-states, and genesis identity is the mechanism that
    /// makes a network agree on one.
    pub executor_share: NonNegI64,
    /// **The lag at which a validator's participation weight reaches zero** (B4, #150), in heights —
    /// the *knee* of the weight. `0` disables the rule, which is `Pos.rhox`'s behaviour — in the
    /// contract absence costs nothing and an absent validator is paid for being drawn.
    ///
    /// The rule it gates only ever *scales down* the reward a boundary is about to commit, so it is
    /// income-only by construction: no bond, no pool entry and no ledger is reachable from it.
    pub absence_slack: NonNegI64,
    /// **How far behind the last finalised fringe a validator's latest message may sit and still be
    /// paid its whole share** (B4, #150), in heights: the flat part of the weight. Between this and
    /// `absence_slack` the weight ramps linearly to zero.
    ///
    /// **Why the weight has two parameters rather than one.** A rule that pays an honest validator *in
    /// full* and reaches zero somewhere else cannot be a function of a single threshold — the ramp would
    /// have to begin at lag zero, which puts a haircut on every live validator, including one that
    /// missed nothing. `participation_grace == absence_slack` reproduces a binary threshold rule
    /// exactly, and **that is also what a params record written before this field decodes to**, so the
    /// pre-change rule is the default rather than a special case a reader has to reconstruct.
    pub participation_grace: NonNegI64,
    /// **The fee floor a block's deploys are held to** (`casper.min-phlo-price`), committed at genesis.
    ///
    /// Before this the floor was a per-node setting that every node also used as a *validity* rule
    /// (`Validate::phlo_price`), so two nodes configured differently disagreed about the same block
    /// with nothing committed to say which was right (C198's three withheld knobs,
    /// `casper/src/block_status.rs`). Committing it binds the value to the genesis: a node whose
    /// setting differs builds a different genesis state. Validation reads these immutable bounds
    /// from a validated parent; startup and LFS restore refuse a config mismatch before Running.
    /// A missing legacy field defaults to `0`; presence is determined by the record layout, so an
    /// explicitly committed zero floor never falls back to config.
    pub min_phlo_price: NonNegI64,
    /// **The widest justification set a block may carry** (`casper.max-number-of-parents`, #153),
    /// committed at genesis for the reason [`Self::min_phlo_price`] is. A non-positive value disables
    /// the bound, exactly as `Validate::justification_count` reads it, and is also what a record
    /// written before this field decodes to.
    pub max_number_of_parents: i64,
    /// **How long the developer allocation vests over**, in blocks from genesis (Law 79). `0` means
    /// there is no schedule: the whole allocation is released at the first epoch boundary.
    ///
    /// A genesis parameter rather than a constant in the rule, because it is a *term of the deal*
    /// rather than arithmetic — and because the network's block cadence is not a constant either
    /// (`docs/src/node/scaling.md` records why a "month" is only ever an assumption here). The
    /// **arithmetic** is in `casper/src/genesis/dev_tax.rs` and stated as Law 79; the term is data.
    ///
    /// A params record written before this field decodes it to `0`, which is the rule for a chain that
    /// has no allocation to vest — and the allocation cannot exist without an address, so a chain that
    /// predates the field has no vesting account to release from.
    pub dev_tax_vesting_blocks: NonNegI64,
    /// **Liveness enforcement's absence threshold** (RCHIP #55, Law 89), in heights: a validator
    /// whose latest finalised message is more than this far behind the quorum-supported tip counts
    /// as absent for the boundary. `0` disables the mechanism (all five liveness fields zero is the
    /// pre-change rule and encodes as format 2).
    pub liveness_absence_threshold: i64,
    /// Consecutive **advancing** boundaries absent before a jail; `0` disables. When enabled it must
    /// be at least two: one missed boundary is not sustained absence.
    pub liveness_jail_after_boundaries: i64,
    /// Advancing boundaries a jail lasts. The bond is untouched; only active-set membership and
    /// income are affected.
    pub liveness_jail_epochs: i64,
    /// Advancing boundaries a slash vote stays open; `0` disables voting.
    pub liveness_vote_window: i64,
    /// The tier a passing vote applies, as `SlashSeverity::to_code` (1 Malicious, 2 Misdemeanour,
    /// 3 HonestMistake); `0` when voting is disabled. A vote is the **only** path from liveness to
    /// a bond.
    pub liveness_slash_severity: i64,
}

impl Default for PosParams {
    /// Permissive parameters: no bond bounds, no quarantine, unlimited active set, and **no producer
    /// share** — which is the contract's own behaviour, so an ad-hoc runtime pays exactly what
    /// `Pos.rhox` pays. Used when no genesis PoS state has been installed (ad-hoc runtimes/tests).
    fn default() -> Self {
        PosParams {
            minimum_bond: NonNegI64::zero(),
            maximum_bond: NonNegI64::saturating(i64::MAX),
            epoch_length: 0,
            quarantine_length: 0,
            number_of_active_validators: 0,
            executor_share: NonNegI64::zero(),
            absence_slack: NonNegI64::zero(),
            participation_grace: NonNegI64::zero(),
            min_phlo_price: NonNegI64::zero(),
            max_number_of_parents: 0,
            dev_tax_vesting_blocks: NonNegI64::zero(),
            liveness_absence_threshold: 0,
            liveness_jail_after_boundaries: 0,
            liveness_jail_epochs: 0,
            liveness_vote_window: 0,
            liveness_slash_severity: 0,
        }
    }
}

impl PosParams {
    /// Encode the parameters (inverse: [`decode_params`]).
    ///
    /// **A zeroed liveness config writes format 2, byte-for-byte.** That is what lets a binary that
    /// knows liveness enforcement replay an old genesis to the same post-state hash: the only
    /// difference a new chain can carry is the five fields, and when they are all zero there is no
    /// difference to encode.
    pub fn encode(&self) -> Vec<u8> {
        let enabled = self.liveness_configured();
        let tag = if enabled {
            PARAMS_FORMAT
        } else {
            PARAMS_FORMAT_LEGACY
        };
        let mut out = Vec::with_capacity(if enabled { PARAMS_LEN } else { PARAMS_LEN_V2 });
        out.push(tag);
        out.extend_from_slice(&i64::from(self.minimum_bond).to_le_bytes());
        out.extend_from_slice(&i64::from(self.maximum_bond).to_le_bytes());
        out.extend_from_slice(&self.epoch_length.to_le_bytes());
        out.extend_from_slice(&self.quarantine_length.to_le_bytes());
        out.extend_from_slice(&self.number_of_active_validators.to_le_bytes());
        out.extend_from_slice(&i64::from(self.executor_share).to_le_bytes());
        out.extend_from_slice(&i64::from(self.absence_slack).to_le_bytes());
        out.extend_from_slice(&i64::from(self.participation_grace).to_le_bytes());
        out.extend_from_slice(&i64::from(self.min_phlo_price).to_le_bytes());
        out.extend_from_slice(&self.max_number_of_parents.to_le_bytes());
        out.extend_from_slice(&i64::from(self.dev_tax_vesting_blocks).to_le_bytes());
        if enabled {
            out.extend_from_slice(&self.liveness_absence_threshold.to_le_bytes());
            out.extend_from_slice(&self.liveness_jail_after_boundaries.to_le_bytes());
            out.extend_from_slice(&self.liveness_jail_epochs.to_le_bytes());
            out.extend_from_slice(&self.liveness_vote_window.to_le_bytes());
            out.extend_from_slice(&self.liveness_slash_severity.to_le_bytes());
        }
        out
    }

    /// Whether any liveness field is set. All-zero is the pre-change rule and the format-2 wire.
    pub fn liveness_configured(&self) -> bool {
        self.liveness_absence_threshold != 0
            || self.liveness_jail_after_boundaries != 0
            || self.liveness_jail_epochs != 0
            || self.liveness_vote_window != 0
            || self.liveness_slash_severity != 0
    }

    /// **A liveness config is either wholly off or coherent.** Refused rather than clamped: a
    /// half-configured chain is a rule nobody decided (a jail with no threshold, a vote with no
    /// severity, a window that can outlive the withdrawal escrow).
    pub fn validate(&self) -> Result<(), String> {
        if !self.liveness_configured() {
            return Ok(());
        }
        if self.liveness_absence_threshold < 1 {
            return Err("liveness absence threshold must be at least 1 when enabled".to_string());
        }
        if self.liveness_jail_after_boundaries < 2 {
            return Err(
                "liveness jail-after must be at least 2 boundaries: one lapse is not sustained                  absence"
                    .to_string(),
            );
        }
        if self.liveness_jail_epochs < 1 {
            return Err("liveness jail-epochs must be at least 1".to_string());
        }
        if self.liveness_vote_window < 0 || self.liveness_vote_window > self.liveness_jail_epochs {
            return Err(format!(
                "liveness vote-window {} must be between 0 and the jail term {}",
                self.liveness_vote_window, self.liveness_jail_epochs
            ));
        }
        if self.liveness_vote_window > 0 {
            if !(1..=3).contains(&self.liveness_slash_severity) {
                return Err(format!(
                    "liveness slash severity {} is not a SlashSeverity code (1..=3)",
                    self.liveness_slash_severity
                ));
            }
            let window_blocks = self
                .liveness_vote_window
                .saturating_mul(epoch_divisor(self));
            if window_blocks > self.quarantine_length {
                return Err(format!(
                    "liveness vote window is {window_blocks} blocks, longer than the withdrawal                      quarantine {}: a bond could be paid before the vote tallies",
                    self.quarantine_length
                ));
            }
        } else if self.liveness_slash_severity != 0 {
            return Err("liveness slash severity is set while the vote window is zero".to_string());
        }
        Ok(())
    }
}

/// Decode [`PosParams`] (inverse of [`PosParams::encode`]).
///
/// **A versioned record** (one byte longer than a multiple of eight) must carry a format this node knows
/// and exactly that format's fields; anything else is refused, never read positionally.
///
/// **An unversioned record that stops short of the current form reads the fields it carries and defaults the rest
/// to zero** — the rule [`SlashSeverity::Unspecified`] uses for a slash that predates the tiers, and
/// for the same reason: each trailing field's zero is exactly the behaviour of a chain that ran before
/// it existed (no producer's share; no absence rule). An unversioned record that is *not* a whole
/// number of fields, is shorter than the original five, or is longer than the eight any unversioned
/// chain wrote, is refused.
///
/// [`SlashSeverity::Unspecified`]: rchain_models::block_metadata::SlashSeverity::Unspecified
pub fn decode_params(record: &[u8]) -> Result<PosParams, String> {
    let bytes = if record.len() % 8 == 1 {
        match (record[0], record.len()) {
            (PARAMS_FORMAT, PARAMS_LEN) | (PARAMS_FORMAT_LEGACY, PARAMS_LEN_V2) => &record[1..],
            (tag, len) => {
                return Err(format!(
                    "params record is format {tag} with {len} bytes; this node reads format                      {PARAMS_FORMAT} ({PARAMS_LEN} bytes) or legacy {PARAMS_FORMAT_LEGACY}                      ({PARAMS_LEN_V2} bytes)"
                ))
            }
        }
    } else if record.len() % 8 != 0
        || record.len() < PARAMS_LEN_ORIGINAL
        || record.len() > PARAMS_LEN_WITH_GRACE
    {
        return Err(format!(
            "params encoding has {} bytes: an unversioned params record is a whole number of \
             eight-byte fields, between {PARAMS_LEN_ORIGINAL} and {PARAMS_LEN_WITH_GRACE}",
            record.len()
        ));
    } else {
        record
    };
    let read = |i: usize| -> i64 {
        // A field the record does not carry is the pre-change rule, not a missing value.
        if (i + 1) * 8 > bytes.len() {
            return 0;
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&bytes[i * 8..i * 8 + 8]);
        i64::from_le_bytes(arr)
    };
    let executor_share = read(5);
    let absence_slack = read(6);
    // **The grace is the one field whose absence does not default to zero, and that is the whole
    // compatibility rule here.** A record written before it exists describes a chain running the
    // *binary* rule, which is `grace == knee`; defaulting to zero would silently turn every armed chain
    // into a ramped one on upgrade — a rule change wearing a missing-field's clothes, and the opposite
    // of what "the record does not carry the field" is supposed to mean.
    let participation_grace = if bytes.len() >= PARAMS_LEN_WITH_GRACE {
        read(7)
    } else {
        absence_slack
    };
    // The vesting term (Law 79), appended after the two committed validity bounds. Unlike the grace,
    // its absence *does* default to zero, and that zero is the right rule for a record written before
    // it: a chain cannot have an allocation to vest without an address, and the address is a genesis
    // input newer than any record that lacks this field.
    let dev_tax_vesting_blocks = read(10);
    if executor_share > 10_000 {
        return Err(format!(
            "executor_share is {executor_share} basis points, above the whole of what was burned"
        ));
    }
    // The two bond bounds are refused rather than clamped (the deferred item 1d): a stored negative
    // is not a value this protocol can mean, and clamping it would silently turn a corrupted or
    // malicious params leaf into "no minimum", which is the permissive reading of the same bytes.
    let params = PosParams {
        minimum_bond: NonNegI64::try_from(read(0))
            .map_err(|e| format!("minimum_bond is not a valid accept threshold: {e}"))?,
        maximum_bond: NonNegI64::try_from(read(1))
            .map_err(|e| format!("maximum_bond is not a valid accept threshold: {e}"))?,
        epoch_length: read(2),
        quarantine_length: read(3),
        number_of_active_validators: read(4),
        executor_share: NonNegI64::try_from(executor_share)
            .map_err(|e| format!("executor_share is not a valid basis-point share: {e}"))?,
        absence_slack: NonNegI64::try_from(absence_slack)
            .map_err(|e| format!("absence_slack is not a valid number of heights: {e}"))?,
        dev_tax_vesting_blocks: NonNegI64::try_from(dev_tax_vesting_blocks)
            .map_err(|e| format!("dev_tax_vesting_blocks is not a valid number of blocks: {e}"))?,
        participation_grace: NonNegI64::try_from(participation_grace)
            .map_err(|e| format!("participation_grace is not a valid number of heights: {e}"))?,
        min_phlo_price: NonNegI64::try_from(read(8))
            .map_err(|e| format!("min_phlo_price is not a valid fee floor: {e}"))?,
        max_number_of_parents: read(9),
        liveness_absence_threshold: read(11),
        liveness_jail_after_boundaries: read(12),
        liveness_jail_epochs: read(13),
        liveness_vote_window: read(14),
        liveness_slash_severity: read(15),
    };
    params.validate()?;
    Ok(params)
}

/// The genesis PoS descriptors: the initial bond pool, the initial trusted stakeholder set, and the
/// network parameters. `active` is *derived* from the pool at install time (see [`select_active`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PosGenesis {
    /// The initial bond pool (validator → stake).
    pub bonds: BTreeMap<Validator, NonNegI64>,
    /// The initial trusted stakeholder set. If empty, the initial validators are trusted.
    pub trusted: BTreeSet<Validator>,
    /// The immutable PoS parameters.
    pub params: PosParams,
    /// **The developer allocation's recipient and amount** (Laws 78–79), or `None` for a genesis with
    /// no allocation.
    ///
    /// Carried here rather than passed beside the vaults because it belongs to the *native genesis
    /// state*: it is written by [`NativeSystemState::install_genesis`], which is the one call both the
    /// ceremony and a replaying node make, so the amount and its recipient cannot be installed on one
    /// path and not the other. The amount lands in the **vesting** account, not in a vault — it is not
    /// spendable until it vests.
    pub dev_vesting: Option<(String, NonNegI64)>,
}

/// What a boundary's liveness prelude changed. The `jailed` set is the one the active-set draw
/// excludes; `slashed` is for the boundary log and for tests. The bond is never in either.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LivenessOutcome {
    pub jailed: BTreeSet<Validator>,
    pub slashed: Vec<Validator>,
}

impl PosGenesis {
    /// The initial active set (top-N of the pool by stake).
    pub fn active_bonds(&self) -> BTreeMap<Validator, NonNegI64> {
        select_active(
            &self.bonds,
            &BTreeMap::<Validator, ()>::new(),
            &self.params,
            &genesis_epoch_seed(),
        )
    }

    /// The initial bond sum — the amount the staking vault is created with (`Pos.rhox:167-174`:
    /// `ListOps!("fold", $$initialBonds$$.toList(), 0, *sumFromPair, …)` and then
    /// `createWithBalance(posDeployerRevAddress, bondSum)` before the transfer into the PoS vault).
    /// A sum that does not fit an `i64` is a genesis that cannot be installed, not a genesis to
    /// clamp.
    pub fn bond_sum(&self) -> Result<NonNegI64, String> {
        let sum: i128 = self.bonds.values().map(|s| i128::from(i64::from(*s))).sum();
        let sum = checked_i64(sum, "genesis bond sum")?;
        NonNegI64::try_from(sum).map_err(|_| format!("genesis bond sum is negative: {sum}"))
    }
}

/// Select the active validator set from the pool: drop zero-stake and withdrawing validators, then
/// **draw** up to `number_of_active_validators` of the rest **in proportion to stake**, without
/// replacement, seeded by `seed` (`0` = unlimited, no draw).
///
/// **Why a draw and not the top N.** The set this returns *is* the finality weight set
/// (`casper/src/multi_parent_casper.rs`'s `Finalizer`), so who is in it decides who can finalise.
/// Ranking by stake made membership a pure function of a value the proposer of the drawing block
/// could steer: the old seed was `hash(shard_id, block_number, sender, pre_state_hash)` computed at
/// the moment of use, so every candidate block was a fresh, free reroll — a proposer could try
/// headers until it drew the set it wanted. See `close_block` step 5 for what anchors the draw now.
///
/// **Why weighted, and not uniform.** The rule was a uniform draw until 2026-10-02, and a uniform
/// draw pays per **key** rather than per stake: the first slot went to a dust validator as readily as
/// to the largest one, so splitting a stake across `k` keys bought about `k` times the expected slots
/// (registered as residual O3 in `spec/RUST-VS-SCALA.md` §3). That is a sybil incentive in the
/// finality weight set *and* an income bug — `epoch_rewards` pays the drawn members, so a staker that
/// split its stake out-earned the same stake held whole, while a single large validator earned less
/// than its share. The weighted draw gives the **first** slot to a validator with probability exactly
/// `stake / total`, which is the property that removes the split incentive, and it needs no
/// participation score, no delegation and no new state: `(pool, seed)` still determines the result.
///
/// **Successive weighting, and the residual that stays.** The draw is sequential — pick by weight,
/// remove, renormalise, repeat — so it is exactly proportional for the first slot and *approximately*
/// so for the later ones (an item already drawn cannot be drawn again, so the tail slightly favours
/// the items that were not picked). Splitting a stake is therefore no longer worth ~`k` times the
/// slots, but it is not provably neutral either, and the honest statement is that O3 is reduced rather
/// than closed. The exact-proportional alternative is a systematic (rotated-interval) scheme, which
/// assigns a fixed share instead of a random one and would collide for any stake above `1/k` of the
/// total; that is a different rule with a different variance, and it is not this one.
///
/// Generic over what the withdrawal map holds, because only its key set matters here and the port has
/// two of them: the staged requests (`validator → deadline`, `pendingWithdrawers`) and the claims
/// (`validator → Withdrawal`, `withdrawers`).
pub fn select_active<V>(
    pool: &BTreeMap<Validator, NonNegI64>,
    withdrawers: &BTreeMap<Validator, V>,
    params: &PosParams,
    seed: &EpochSeed,
) -> BTreeMap<Validator, NonNegI64> {
    select_active_excluding(pool, withdrawers, &BTreeSet::new(), params, seed)
}

/// **The active-set draw minus the jailed validators.** A jailed validator keeps its bond and its
/// place in the pool; it is only not *drawn* while the jail runs. Keeping this as a separate function
/// (rather than changing `select_active`) keeps every genesis/test caller on the pre-change,
/// byte-identical path.
pub fn select_active_excluding<V>(
    pool: &BTreeMap<Validator, NonNegI64>,
    withdrawers: &BTreeMap<Validator, V>,
    jailed: &BTreeSet<Validator>,
    params: &PosParams,
    seed: &EpochSeed,
) -> BTreeMap<Validator, NonNegI64> {
    // Eligible candidates, in the `BTreeMap`'s key order — the canonical container order, which is
    // what makes the draw a function of `(pool, seed, jailed)` and nothing else.
    let candidates: Vec<(&Validator, NonNegI64)> = pool
        .iter()
        .filter(|(v, stake)| {
            i64::from(**stake) > 0 && !withdrawers.contains_key(*v) && !jailed.contains(*v)
        })
        .map(|(v, stake)| (v, *stake))
        .collect();
    // `number_of_active_validators` of `0` means unlimited, and a cap that does not bite is not a
    // selection: return the pool as read rather than drawing. Fewer RNG calls, and it keeps `0`
    // meaning "no selection happens" as the params doc says.
    //
    // The `<= 0` arm is load-bearing and was missing in the first draft, which mapped a zero cap to
    // `cap = 0` and drew *nothing* — so a genesis-installed node came up with an empty active set and
    // no validator could propose. Every genesis test failed at once, which is how it was caught.
    if params.number_of_active_validators <= 0 {
        return candidates
            .into_iter()
            .map(|(v, stake)| (*v, stake))
            .collect();
    }
    let cap = usize::try_from(params.number_of_active_validators).unwrap_or(usize::MAX);
    if cap >= candidates.len() {
        return candidates
            .into_iter()
            .map(|(v, stake)| (*v, stake))
            .collect();
    }
    let mut rand = Blake2b512Random::from_init(&seed.rng_input());
    draw_weighted_without_replacement(&candidates, cap, &mut rand)
        .into_iter()
        .map(|(v, stake)| (*v, stake))
        .collect()
}

fn checked_i64(value: i128, what: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{what} overflow: {value}"))
}

/// `balance + delta` as a `NonNegI64`, with both the `i64` range and the sign checked. The single
/// place the port's balance arithmetic happens, so a balance can neither wrap nor go negative
/// silently. `delta` is an `i64` to keep the debit direction explicit at the call sites.
fn balance_plus(balance: NonNegI64, delta: i64, what: &str) -> Result<NonNegI64, String> {
    let sum = checked_i64(i128::from(i64::from(balance)) + i128::from(delta), what)?;
    NonNegI64::try_from(sum).map_err(|_| format!("{what} would become negative: {sum}"))
}

/// The epoch length the port **divides by**.
///
/// The contract divides by `$$epochLength$$` directly, in both of law 44's places — the boundary test
/// (`Pos.rhox:517`, `blockNumber % $$epochLength$$`) and the withdrawal deadline (`:381`,
/// `blockNumber / $$epochLength$$`). With `epochLength == 0` both fault. The port's default parameters
/// are permissive (`epoch_length: 0`, see [`PosParams::default`]), and the meaning of a zero epoch
/// length is "every block is an epoch boundary" — which is what `epochLength == 1` means to the
/// contract. So the divisor is `max(epoch_length, 1)`, which agrees with the contract for every
/// `epoch_length >= 1` and turns the unrepresentable zero into the one-block epoch it must mean.
fn epoch_divisor(params: &PosParams) -> i64 {
    if params.epoch_length <= 0 {
        1
    } else {
        params.epoch_length
    }
}

/// Is `block_number` an epoch boundary (`Pos.rhox:517`)? At a non-boundary the contract does
/// **nothing at all** — no reward, no activation, no payment.
fn is_epoch_boundary(params: &PosParams, block_number: i64) -> bool {
    block_number % epoch_divisor(params) == 0
}

/// The epoch's distributable pot: the staking vault less every outstanding claim on it — the bonded
/// pool, the escrowed withdrawals, the rewards already committed but not yet paid, **and the
/// delegators' accrued rewards** (`Pos.rhox:249`: `posBalance - totalBond - totalWithdraw -
/// totalCommittedRewards`, plus law 57's `pos:delegated_rewards`).
///
/// **Floored at zero.** The Scala computes this in `Long`, so a vault that cannot cover the
/// outstanding claims yields a *negative* pot and therefore negative rewards; this port's balances are
/// `NonNegI64` and a negative reward is not a value it will model. Through the protocol the floor is
/// unreachable — every debit is bounded by the credit that funded it, so the vault always covers its
/// claims — which is why this is a defensive floor and not a silent clamp, and why
/// `a_drafted_vault_floors_the_pot_instead_of_wrapping` has to build the diverged state by hand.
///
/// One consequence of the formula, and not a bug in it: the dust an epoch leaves un-distributed is
/// **not lost**. It stays in the pot, so the next epoch distributes it along with its own phlo.
fn epoch_pot(
    vault: NonNegI64,
    bonds: &BTreeMap<Validator, NonNegI64>,
    withdrawers: &BTreeMap<Validator, Withdrawal>,
    committed: &BTreeMap<Validator, NonNegI64>,
    delegated_rewards: &BTreeMap<DelegationKey, NonNegI64>,
) -> Result<i64, String> {
    // **Four claim ledgers, not three** (law 57, #193). A delegator's accrued reward is a claim against
    // the staking vault that `committed` does not carry — it is held apart from the operator's entry on
    // purpose, so the operator cannot be paid it — and leaving it out of this sum would distribute the
    // same REV twice: once to the delegator at its claim's payout, and once to the drawn set through a
    // pot that never had it subtracted. Absent (or empty) contributes zero, so a chain with no
    // delegations computes exactly the number it computed before this parameter existed.
    let claims: i128 = bonds
        .values()
        .map(|s| i128::from(i64::from(*s)))
        .sum::<i128>()
        + withdrawers
            .values()
            .map(|w| i128::from(i64::from(w.bond)))
            .sum::<i128>()
        + committed
            .values()
            .map(|s| i128::from(i64::from(*s)))
            .sum::<i128>()
        + delegated_rewards
            .values()
            .map(|s| i128::from(i64::from(*s)))
            .sum::<i128>();
    checked_i64((i128::from(i64::from(vault)) - claims).max(0), "epoch pot")
}

/// One active validator's share of the pot (`Pos.rhox:249`):
///
/// ```text
/// pot * (bond / minimumBond) / (activeBonds / minimumBond)
/// ```
///
/// two integer divisions, which is why the shares do not add up to the pot: see
/// `Rchain.sum_rewards_le_pot` in `spec/Rchain/Pos.lean`, whose statement is the inequality and whose
/// `the_dust_is_real` is the case where it is strict.
///
/// **Zero where the contract's formula is undefined.** `minimumBond == 0`, or a normaliser of zero
/// (`activeBonds < minimumBond`), makes the Scala divide by zero — which faults the `closeBlock`
/// deploy rather than producing a value. The port's parameters are permissive by default
/// (`minimum_bond: NonNegI64::try_from(0).unwrap()`), so a fault is not a rule it can copy, and zero is the only value that leaves
/// the epoch total. That is the same case the Lean model leaves as a hypothesis
/// (`hD : 0 < activeBonds / minimumBond`): the model states the theorem for the defined case, the port
/// pays nothing in the undefined one. `an_epoch_with_a_zero_normaliser_pays_nothing` pins it.
/// **The participation weight** in basis points, mirroring `spec/Rchain/Pos.lean`'s
/// `participationWeight`: the whole share inside `grace` heights of the boundary, ramping linearly to
/// `0` at `knee`, and `0` past it. `None` is a validator the fringe does not contain — as far behind as
/// a validator can be, so `0` rather than a default.
///
/// `knee <= grace` is not a shape this protocol means; the first branch has already answered it, and the
/// guard on the last arm is there so a `knee` below the grace degrades to the binary rule rather than
/// dividing by a non-positive number.
fn participation_weight(behind: Option<i64>, grace: i64, knee: i64) -> i64 {
    let Some(behind) = behind else {
        return 0;
    };
    if behind <= grace {
        10_000
    } else if knee <= behind || knee <= grace {
        0
    } else {
        10_000 * (knee - behind) / (knee - grace)
    }
}

/// Scale a committed reward by a basis-point weight, mirroring `absenceAdjusted`.
///
/// `i128` because the product of two `i64`-bounded values is not an `i64`, and a wrapped product here
/// would pay a validator a *different* amount than the weight names, silently, on a consensus path.
fn scaled_by_weight(reward: NonNegI64, weight: i64) -> NonNegI64 {
    let product = i128::from(i64::from(reward)) * i128::from(weight);
    NonNegI64::saturating(i64::try_from(product / 10_000).unwrap_or(i64::MAX))
}

/// **The participation rule** (B4, #150): scale each drawn validator's epoch reward by its
/// participation weight.
///
/// The participation comes from the **last finalised fringe** — each validator's latest *message*
/// height, derived by the caller from the DAG — rather than from a `pos:last_spoke` entry read out of
/// this block's pre-state. The pre-state is a function of the proposer's justification set and nothing
/// requires a block to justify everything it has seen, so the old read could be *aimed* at a chosen
/// rival; the fringe is the `>2/3`-agreed object, so a stale fringe moves every validator's reading back
/// together and singles nobody out. It also measures the right event: an attestation block carries no
/// system deploys and so recorded nothing, while an attestation *is* a message.
///
/// **Income only, and that is structural rather than promised.** The whole effect is to multiply the
/// rewards a boundary is about to commit, so no bond, no pool entry and no ledger is reachable from it —
/// an absent validator is not slashed, not evicted, and not deactivated, it is simply paid less for a
/// boundary at which it did not act. The withheld fraction stays in the staking vault (nothing debits
/// it), so a later epoch distributes it and the epoch's conservation is untouched by construction. That
/// is the Rust half of the model's `weighted_rewards_le_pot`.
///
/// **And it is fully recoverable.** The weight is a function of how far behind the *last finalised
/// fringe* a validator's latest message sits, and one message puts it back inside the grace — so a
/// validator that was away is made whole at its next boundary, with no memory of the lapse.
///
/// `knee == 0` disables the rule entirely, which is `Pos.rhox`'s behaviour: in the contract absence
/// costs nothing and an absent validator is paid for being drawn. `grace == knee` is a binary threshold
/// rule; `grace < knee` is the graded one.
/// The tier a stored liveness vote applies. `0` (voting disabled) has no tier; the genesis
/// validator refuses a voting window without one.
fn liveness_slash_severity(code: i64) -> Option<SlashSeverity> {
    match code {
        1 => Some(SlashSeverity::Malicious),
        2 => Some(SlashSeverity::Misdemeanour),
        3 => Some(SlashSeverity::HonestMistake),
        _ => None,
    }
}

pub(crate) fn apply_weight(
    rewards: BTreeMap<Validator, NonNegI64>,
    participation: &BTreeMap<Validator, BlockHeight>,
    boundary: i64,
    grace: i64,
    knee: i64,
    jailed: &BTreeSet<Validator>,
) -> BTreeMap<Validator, NonNegI64> {
    if knee <= 0 && jailed.is_empty() {
        return rewards;
    }
    rewards
        .into_iter()
        .map(|(validator, reward)| {
            // **A jailed validator earns nothing for the epoch that just ended.** The withheld
            // share stays in the staking vault (the sum of rewards is below the pot), exactly as the
            // absence rule's withheld share does; it is not redistributed to the other validators.
            if jailed.contains(&validator) {
                return (validator, NonNegI64::zero());
            }
            if knee <= 0 {
                return (validator, reward);
            }
            let behind = participation
                .get(&validator)
                .map(|h| boundary.saturating_sub(i64::from(*h)));
            (
                validator,
                scaled_by_weight(reward, participation_weight(behind, grace, knee)),
            )
        })
        .collect()
}

pub(crate) fn epoch_reward(
    pot: i64,
    minimum_bond: i64,
    active_bonds: i64,
    bond: i64,
) -> Result<i64, String> {
    if minimum_bond <= 0 {
        return Ok(0);
    }
    let normaliser = active_bonds / minimum_bond;
    if normaliser <= 0 {
        return Ok(0);
    }
    // The product is exact in `i128` rather than wrapped as the Scala's `Long` multiplication would
    // be, and the quotient is checked rather than clamped: `active_bonds / minimum_bond` is at least
    // `bond / minimum_bond` for an active validator, so the model's lemma bounds the quotient by the
    // pot — the check cannot fire, and if the reasoning is wrong it is an error rather than a silent
    // clamp.
    let scaled = i128::from(pot) * i128::from(bond / minimum_bond);
    checked_i64(scaled / i128::from(normaliser), "epoch reward")
}

/// The typed native system state, wrapping the shared byte-oriented [`InMemNativeStore`].
#[derive(Clone)]
pub struct NativeSystemState {
    store: Arc<InMemNativeStore>,
}

impl NativeSystemState {
    pub fn new(store: Arc<InMemNativeStore>) -> Self {
        NativeSystemState { store }
    }

    // --- PoS: read/write primitives --------------------------------------

    async fn read_bonds(
        &self,
        key: Blake2b256Hash,
    ) -> Result<BTreeMap<Validator, NonNegI64>, String> {
        match self.store.get(PREFIX_POS, &key).await? {
            Some(bytes) => decode_bonds(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Read the full bond *pool* (all bonded validators).
    pub async fn bonds(&self) -> Result<BTreeMap<Validator, NonNegI64>, String> {
        self.read_bonds(pos_bonds_key()).await
    }

    /// Write the bond pool (appends a native `Put` action).
    pub fn set_bonds(&self, bonds: &BTreeMap<Validator, NonNegI64>) {
        self.store
            .put(PREFIX_POS, pos_bonds_key(), encode_bonds(bonds));
    }

    // --- HTTP-result oracle (RCHIP #54) -----------------------------------------------------

    /// Read the whole HTTP-record table (`url → (value, captured block)`).
    pub async fn http_records(&self) -> Result<BTreeMap<String, HttpRecord>, String> {
        match self.store.get(PREFIX_HTTP, &http_records_key()).await? {
            Some(bytes) => decode_http_records(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Read a recorded HTTP value (`None` if the URL was never captured).
    pub async fn http_record(&self, url: &str) -> Result<Option<HttpRecord>, String> {
        Ok(self.http_records().await?.get(url).cloned())
    }

    /// Capture `value` for `url` — **first writer wins**. Returns `true` if this call recorded the
    /// value, `false` if a record already existed (which is left untouched). Recording is a pure
    /// function of the deploy, so replay is deterministic; later deploys compare against the record
    /// rather than re-fetching, which is what makes an HTTP value usable under consensus.
    pub async fn record_http(
        &self,
        url: &str,
        value: &str,
        block_number: i64,
    ) -> Result<bool, String> {
        let mut records = self.http_records().await?;
        if records.contains_key(url) {
            return Ok(false);
        }
        records.insert(url.to_string(), (value.to_string(), block_number));
        self.store.put(
            PREFIX_HTTP,
            http_records_key(),
            encode_http_records(&records),
        );
        Ok(true)
    }

    /// Read the *active* validator bond map (the consensus set).
    pub async fn active(&self) -> Result<BTreeMap<Validator, NonNegI64>, String> {
        self.read_bonds(pos_active_key()).await
    }

    /// Write the active validator bond map.
    pub fn set_active(&self, active: &BTreeMap<Validator, NonNegI64>) {
        self.store
            .put(PREFIX_POS, pos_active_key(), encode_bonds(active));
    }

    /// The active-validator set (the consensus validator set).
    pub async fn active_validators(&self) -> Result<BTreeSet<Validator>, String> {
        Ok(self.active().await?.into_keys().collect())
    }

    /// Read the liveness counters/jails. Absent reads as empty (the mechanism is off).
    pub async fn liveness(&self) -> Result<BTreeMap<Validator, AbsenceRecord>, String> {
        match self.store.get(PREFIX_POS, &pos_liveness_key()).await? {
            Some(bytes) => decode_absence(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Write the liveness counters/jails; an empty map clears the leaf.
    pub async fn set_liveness(
        &self,
        records: &BTreeMap<Validator, AbsenceRecord>,
    ) -> Result<(), String> {
        let value = (!records.is_empty()).then(|| encode_absence(records));
        self.store_optional_pos_leaf(pos_liveness_key(), value)
            .await
    }

    /// The last boundary at which finality advanced, or `None` before the first.
    pub async fn liveness_anchor(&self) -> Result<Option<Anchor>, String> {
        match self
            .store
            .get(PREFIX_POS, &pos_liveness_anchor_key())
            .await?
        {
            Some(bytes) => decode_anchor(&bytes).map(Some),
            None => Ok(None),
        }
    }

    pub fn set_liveness_anchor(&self, anchor: &Anchor) {
        self.store
            .put(PREFIX_POS, pos_liveness_anchor_key(), encode_anchor(anchor));
    }

    /// The open slash-vote windows.
    pub async fn vote_windows(&self) -> Result<BTreeMap<Validator, VoteWindow>, String> {
        match self.store.get(PREFIX_POS, &pos_vote_windows_key()).await? {
            Some(bytes) => decode_vote_windows(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    pub async fn set_vote_windows(
        &self,
        windows: &BTreeMap<Validator, VoteWindow>,
    ) -> Result<(), String> {
        let value = (!windows.is_empty()).then(|| encode_vote_windows(windows));
        self.store_optional_pos_leaf(pos_vote_windows_key(), value)
            .await
    }

    /// The ballots in the open votes.
    pub async fn slash_votes(&self) -> Result<BTreeMap<BallotKey, bool>, String> {
        match self.store.get(PREFIX_POS, &pos_slash_votes_key()).await? {
            Some(bytes) => decode_ballots(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    pub async fn set_slash_votes(&self, ballots: &BTreeMap<BallotKey, bool>) -> Result<(), String> {
        let value = (!ballots.is_empty()).then(|| encode_ballots(ballots));
        self.store_optional_pos_leaf(pos_slash_votes_key(), value)
            .await
    }

    /// The permanently banished validators (a passing vote is the only writer).
    pub async fn banished(&self) -> Result<BTreeSet<Validator>, String> {
        match self.store.get(PREFIX_POS, &pos_banished_key()).await? {
            Some(bytes) => decode_validator_set(&bytes),
            None => Ok(BTreeSet::new()),
        }
    }

    pub async fn set_banished(&self, banished: &BTreeSet<Validator>) -> Result<(), String> {
        let value = (!banished.is_empty()).then(|| encode_validator_set(banished));
        self.store_optional_pos_leaf(pos_banished_key(), value)
            .await
    }

    /// Read the trusted stakeholder set.
    pub async fn trusted(&self) -> Result<BTreeSet<Validator>, String> {
        match self.store.get(PREFIX_POS, &pos_trusted_key()).await? {
            Some(bytes) => decode_trusted(&bytes),
            None => Ok(BTreeSet::new()),
        }
    }

    /// Write the trusted stakeholder set.
    pub fn set_trusted(&self, trusted: &BTreeSet<Validator>) {
        self.store
            .put(PREFIX_POS, pos_trusted_key(), encode_trusted(trusted));
    }

    /// Read the pending withdrawers (`validator → quarantine deadline`).
    pub async fn withdrawers(&self) -> Result<BTreeMap<Validator, Withdrawal>, String> {
        match self.store.get(PREFIX_POS, &pos_withdrawers_key()).await? {
            Some(bytes) => decode_withdrawers(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Write the pending withdrawers.
    pub fn set_withdrawers(&self, withdrawers: &BTreeMap<Validator, Withdrawal>) {
        self.store.put(
            PREFIX_POS,
            pos_withdrawers_key(),
            encode_withdrawers(withdrawers),
        );
    }

    /// Read the withdrawal *requests* (`validator → epoch-boundary deadline`; the contract's
    /// `pendingWithdrawers`).
    pub async fn pending_withdrawers(&self) -> Result<BTreeMap<Validator, i64>, String> {
        match self
            .store
            .get(PREFIX_POS, &pos_pending_withdrawers_key())
            .await?
        {
            Some(bytes) => decode_pending_withdrawers(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Write the withdrawal requests.
    pub fn set_pending_withdrawers(&self, pending: &BTreeMap<Validator, i64>) {
        self.store.put(
            PREFIX_POS,
            pos_pending_withdrawers_key(),
            encode_pending_withdrawers(pending),
        );
    }

    /// Read the committed rewards (`validator → NonNegI64`).
    pub async fn committed_rewards(&self) -> Result<BTreeMap<Validator, NonNegI64>, String> {
        match self.store.get(PREFIX_POS, &pos_committed_key()).await? {
            Some(bytes) => decode_bonds(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Write the committed rewards (the same canonical encoding as the bond maps).
    pub fn set_committed_rewards(&self, committed: &BTreeMap<Validator, NonNegI64>) {
        self.store
            .put(PREFIX_POS, pos_committed_key(), encode_bonds(committed));
    }

    /// Read the immutable PoS parameters (permissive defaults if absent).
    pub async fn params(&self) -> Result<PosParams, String> {
        match self.store.get(PREFIX_POS, &pos_params_key()).await? {
            Some(bytes) => decode_params(&bytes),
            None => Ok(PosParams::default()),
        }
    }

    /// Write the PoS parameters.
    pub fn set_params(&self, params: &PosParams) {
        self.store
            .put(PREFIX_POS, pos_params_key(), params.encode());
    }

    /// Read the Coop slashing-vault balance.
    pub async fn coop_balance(&self) -> Result<NonNegI64, String> {
        self.read_balance(PREFIX_POS, &pos_coop_key(), "coop balance")
            .await
    }

    /// Write the Coop slashing-vault balance.
    pub fn set_coop_balance(&self, balance: NonNegI64) {
        self.write_balance(PREFIX_POS, &pos_coop_key(), balance);
    }

    /// Read the PoS staking-vault balance (the epoch pot's source; `Pos.rhox:203`'s
    /// `posVault!("balance", …)`).
    pub async fn pos_vault_balance(&self) -> Result<NonNegI64, String> {
        self.read_balance(PREFIX_POS, &pos_vault_key(), "pos vault balance")
            .await
    }

    /// Write the PoS staking-vault balance.
    pub fn set_pos_vault_balance(&self, balance: NonNegI64) {
        self.write_balance(PREFIX_POS, &pos_vault_key(), balance);
    }

    /// Read the developer allocation's vesting state, if this chain has one.
    pub async fn dev_vesting(&self) -> Result<Option<DevVesting>, String> {
        match self.store.get(PREFIX_POS, &pos_dev_vesting_key()).await? {
            None => Ok(None),
            Some(bytes) => DevVesting::decode(&bytes).map(Some),
        }
    }

    /// Write the developer allocation's vesting state. `None` clears it — a tombstone folds to
    /// "absent", so a genesis with no allocation ends up with no leaf rather than one holding an empty
    /// value.
    pub fn set_dev_vesting(&self, vesting: Option<&DevVesting>) {
        match vesting {
            Some(v) => self
                .store
                .put(PREFIX_POS, pos_dev_vesting_key(), v.encode()),
            None => self.store.delete(PREFIX_POS, &pos_dev_vesting_key()),
        }
    }

    /// **The total REV this node's state holds, over every address it can see** (AUDIT C109).
    ///
    /// This is the invariant whose absence let the negative-`phlo_limit` mint live: the existing
    /// `total_rev` is a *test helper* that sums a caller-supplied list of addresses plus the two named
    /// vaults, so a credit to an address nobody listed is invisible to it. A conservation check that
    /// only sees what the caller thought to name cannot see a mint into a name nobody thought of —
    /// which is exactly what a mint looks like.
    ///
    /// It sums the Coop vault, the staking vault, and **every leaf under `PREFIX_VAULT`** — so a
    /// balance created at an address the test never mentions is counted, and a charge that credits one
    /// vault without debiting another shows up as a change in the total rather than as two numbers
    /// that quietly disagree.
    ///
    /// **Errors when the store has a base history**, because then it cannot see the whole state:
    /// `NativeHistoryReader` exposes a keyed read and no iteration, so after a checkpoint the trie
    /// holds balances this enumeration does not reach. Returning a partial total would be the same
    /// class of defect as the bug it exists to catch — an instrument reporting a number it cannot
    /// support — so it refuses instead. The conservation tests build a store that never checkpoints,
    /// where the overlay is the whole state and the total is complete.
    pub async fn total_value(&self) -> Result<i64, String> {
        if self.store.has_base_history() {
            return Err(
                "total_value: this store has a base history, whose balances cannot be enumerated \
                 (NativeHistoryReader is keyed-read only), so a total over it would silently omit \
                 part of the state"
                    .to_string(),
            );
        }
        let mut total: i64 = i64::from(self.coop_balance().await?)
            .checked_add(i64::from(self.pos_vault_balance().await?))
            .ok_or_else(|| "total_value: overflow summing the named vaults".to_string())?;
        // **The developer allocation is native value too.** It sits under `PREFIX_POS` rather than
        // `PREFIX_VAULT` until it vests, so a total that only walks the vault prefixes drops it — and
        // then the release looks like a mint. Count what is still unreleased; the released part is
        // already in the recipient's vault.
        if let Some(vesting) = self.dev_vesting().await? {
            total = total
                .checked_add(i64::from(vesting.unreleased))
                .ok_or_else(|| {
                    "total_value: overflow summing the developer allocation".to_string()
                })?;
        }
        for (key, value) in self.store.live_entries(PREFIX_VAULT) {
            let arr: [u8; 8] = value.as_slice().try_into().map_err(|_| {
                format!(
                    "total_value: vault leaf {} is {} bytes, expected 8",
                    key.to_hex(),
                    value.len()
                )
            })?;
            let balance = NonNegI64::try_from(i64::from_le_bytes(arr))
                .map_err(|_| format!("total_value: vault leaf {} is negative", key.to_hex()))?;
            total = total
                .checked_add(i64::from(balance))
                .ok_or_else(|| "total_value: overflow summing vault balances".to_string())?;
        }
        Ok(total)
    }

    /// Read a leaf holding a single little-endian `NonNegI64` — a balance. An absent leaf is zero,
    /// which is how the store distinguishes "never written" from "written as zero".
    async fn read_balance(
        &self,
        prefix: u8,
        key: &Blake2b256Hash,
        what: &str,
    ) -> Result<NonNegI64, String> {
        match self.store.get(prefix, key).await? {
            Some(bytes) => {
                let arr: [u8; 8] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| format!("{what} is {} bytes, expected 8", bytes.len()))?;
                NonNegI64::try_from(i64::from_le_bytes(arr))
                    .map_err(|_| format!("{what} is negative"))
            }
            None => Ok(NonNegI64::zero()),
        }
    }

    /// Write a leaf holding a single little-endian `NonNegI64`.
    fn write_balance(&self, prefix: u8, key: &Blake2b256Hash, balance: NonNegI64) {
        self.store
            .put(prefix, *key, i64::from(balance).to_le_bytes().to_vec());
    }

    /// Credit `amount` to the staking vault (a bond's stake, or a deploy's phlo charge).
    ///
    /// `amount: NonNegI64` rather than the `if amount <= 0 { return Ok(()) }` guard this used to
    /// carry: a silent no-op on an out-of-range amount is how the negative-`phlo_limit` mint stayed
    /// invisible (AUDIT C109) — the charge credited the deployer's vault and this function quietly
    /// declined to move the staking vault, so the two halves of one transfer disagreed and nothing
    /// failed. The type now refuses the value instead of absorbing it.
    pub async fn credit_pos_vault(&self, amount: NonNegI64) -> Result<(), String> {
        if amount == NonNegI64::zero() {
            return Ok(());
        }
        let balance = self.pos_vault_balance().await?;
        self.set_pos_vault_balance(balance_plus(
            balance,
            i64::from(amount),
            "pos vault credit",
        )?);
        Ok(())
    }

    /// Debit `amount` from the staking vault (a withdrawal's bond + rewards, a refund, a slashing).
    ///
    /// **A short vault is a platform error, not a user error.** Every debit is a transfer whose
    /// source is guaranteed by the accounting: a refund is bounded by the pre-charge that funded it,
    /// a withdrawal by the bond it escrowed, a slash by the stake it confiscated. So a vault that
    /// cannot cover the transfer means the ledger has already diverged, and failing the deploy
    /// loudly is the only honest response — the Scala's `payWithdrawer` has a
    /// `// FIXME fix transfer in failure case` here and removes the withdrawer from the maps even
    /// when the transfer failed, which loses the bond. This port refuses instead.
    pub async fn debit_pos_vault(&self, amount: NonNegI64) -> Result<(), String> {
        if amount == NonNegI64::zero() {
            return Ok(());
        }
        let balance = self.pos_vault_balance().await?;
        self.set_pos_vault_balance(balance_plus(
            balance,
            -i64::from(amount),
            "pos vault debit",
        )?);
        Ok(())
    }

    /// Install the genesis PoS state: the pool, the trusted set, the parameters, the derived active
    /// set, empty withdrawal/commitment maps, an empty Coop vault, and a staking vault holding
    /// exactly the initial bond sum. This is the deterministic entry point shared by genesis creation
    /// and genesis replay.
    ///
    /// The vault is funded with the bond sum and nothing more, so the *pot* an epoch distributes
    /// (`vault − bonded − withdrawers − committed rewards`) is **zero** at genesis: the rewards an
    /// epoch pays come from the phlo of the deploys since the last boundary, not from the bonds.
    ///
    /// The genesis active set is taken directly from the pool (`Pos.rhox:178`,
    /// `pickActiveValidators!($$initialBonds$$.toList(), *initialActiveCh)`), because genesis *is* a
    /// boundary.
    pub fn install_genesis(&self, genesis: &PosGenesis) -> Result<(), String> {
        // Computed before anything is written, so a genesis whose bond sum does not fit an `i64`
        // leaves the store untouched rather than half-installed.
        let bond_sum = genesis.bond_sum()?;
        let trusted: BTreeSet<Validator> = if genesis.trusted.is_empty() {
            genesis.bonds.keys().copied().collect()
        } else {
            genesis.trusted.clone()
        };
        let withdrawers: BTreeMap<Validator, Withdrawal> = BTreeMap::new();
        let active = select_active(
            &genesis.bonds,
            &withdrawers,
            &genesis.params,
            &genesis_epoch_seed(),
        );
        self.set_bonds(&genesis.bonds);
        self.set_active(&active);
        self.set_trusted(&trusted);
        self.set_params(&genesis.params);
        self.set_withdrawers(&withdrawers);
        self.set_pending_withdrawers(&BTreeMap::new());
        self.set_committed_rewards(&BTreeMap::new());
        self.set_coop_balance(NonNegI64::zero());
        self.set_pos_vault_balance(bond_sum);
        // **The developer allocation's recipient and its *unreleased* balance** (Laws 78–79). Nothing
        // is credited to its vault here: the allocation is not spendable at genesis, and the release
        // is the schedule's job at each epoch boundary. Installed in this call rather than beside the
        // vaults so that the ceremony and a replaying node cannot install it on one path and not the
        // other.
        match &genesis.dev_vesting {
            Some((address, amount)) => self.set_dev_vesting(Some(&DevVesting {
                address: address.clone(),
                allocation: *amount,
                unreleased: *amount,
            })),
            None => self.set_dev_vesting(None),
        }
        Ok(())
    }

    // --- PoS: validator lifecycle ----------------------------------------

    /// Bond `amount` stake for `validator` (port of the PoS `bond`, `Pos.rhox:330-362`).
    ///
    /// The stake moves out of the validator's vault into the staking vault and into the pool. The
    /// validator becomes **active at the next epoch boundary**, not here: the contract's `bond` only
    /// writes `allBonds` (`:355`), and `activeValidators` is recomputed by `pickActiveValidators`
    /// inside `closeBlock` (`:546`). With the permissive default parameters that is the same block's
    /// `close_block`, so a bonded validator is active by the block's post-state; with an epoch length
    /// greater than one it waits, exactly as the contract does.
    ///
    /// Rejected when the validator is already bonded/active, is not in the trusted stakeholder set,
    /// the amount is outside `[minimum_bond, maximum_bond]`, or the validator's REV vault cannot
    /// cover the stake.
    pub async fn bond(
        &self,
        validator: &Validator,
        amount: NonNegI64,
        block_number: i64,
    ) -> Result<Result<(), String>, String> {
        if self.banished().await?.contains(validator) {
            return Ok(Err(
                "this validator was permanently ejected by a passing liveness vote".to_string(),
            ));
        }
        let mut pool = self.bonds().await?;
        let active = self.active().await?;
        if pool.contains_key(validator) || active.contains_key(validator) {
            return Ok(Err("Public key is already bonded.".to_string()));
        }
        let trusted = self.trusted().await?;
        if !trusted.contains(validator) {
            return Ok(Err(
                "Validator is not trusted: observer admission is required before bonding."
                    .to_string(),
            ));
        }
        let params = self.params().await?;
        let stake = i64::from(amount);
        if stake < i64::from(params.minimum_bond) {
            return Ok(Err(format!(
                "Bond is less than minimum ({} < {}).",
                stake,
                i64::from(params.minimum_bond)
            )));
        }
        if stake > i64::from(params.maximum_bond) {
            return Ok(Err(format!(
                "Bond is greater than maximum ({} > {}).",
                stake,
                i64::from(params.maximum_bond)
            )));
        }
        let address = self.vault_address(validator)?;
        let balance = match self.vault_balance(&address).await? {
            Some(b) => b,
            None => NonNegI64::zero(),
        };
        if i64::from(balance) < stake {
            return Ok(Err(format!(
                "insufficient funds to bond {} (have {})",
                stake,
                i64::from(balance)
            )));
        }
        let new_balance =
            NonNegI64::try_from(i64::from(balance) - stake).map_err(|e| format!("bond: {e}"))?;
        self.set_vault_balance(&address, new_balance);
        // The stake moves from the validator's vault into the staking vault (`Pos.rhox:392-410`'s
        // `deposit!(deployerId, amount, posVaultAddr)`). Crediting the destination is what makes the
        // later reward and refund transfers possible at all: without it the vault would hold nothing
        // to pay out of, and the epoch would distribute a pot that does not exist.
        self.credit_pos_vault(NonNegI64::try_from(stake).map_err(|e| format!("bond: {e}"))?)
            .await?;
        pool.insert(*validator, amount);
        // The pool only. Activation is the epoch boundary's (`close_block`'s
        // `pickActiveValidators`, `Pos.rhox:546`), which is why this does not touch `pos:active`:
        // promoting a validator mid-epoch would change the consensus set inside an epoch, which is
        // the rule law 44 is about.
        self.set_bonds(&pool);
        let _ = block_number; // the contract's `bond` reads no block data at all
        Ok(Ok(()))
    }

    /// Request withdrawal of a validator's bond (port of the PoS `withdraw`, `Pos.rhox:363-387`).
    ///
    /// The request only **stages** the withdrawal. The validator stays bonded and stays in the active
    /// set, earning, until the next epoch boundary — the contract's `withdraw` writes
    /// `pendingWithdrawers` and nothing else. The deadline it records is
    /// `quarantineLength + epochLength * (1 + blockNumber / epochLength)` (`:381`): the end of the
    /// epoch *after* this one, plus the quarantine, so the money is payable at the first boundary at
    /// which the quarantine has elapsed.
    ///
    /// A repeat request re-stages with a fresh deadline, which is what the contract's unconditional
    /// `.set` does (`:378-381`). Before this, the port deactivated the validator immediately and
    /// refused a second request: neither is the contract's rule — the first is the deviation law 47
    /// is about, and the second refused a request the contract accepts.
    pub async fn withdraw(
        &self,
        validator: &Validator,
        block_number: i64,
    ) -> Result<Result<(), String>, String> {
        let pool = self.bonds().await?;
        if !pool.contains_key(validator) {
            return Ok(Err("User is not bonded".to_string()));
        }
        // **A validator with outstanding delegations may not withdraw** (law 57, #193). `withdraw`
        // stages the whole of a key's bond, and with delegations that whole is not the operator's: at
        // the boundary the aggregate would move into an escrow the delegator has no claim on, and a
        // slash reaching that escrow could not tell whose money it was. This is one half of the
        // invariant that keeps one meaning per ledger; `delegate` refusing while a withdrawal is
        // pending is the other. The operator resolves it by waiting for its delegators to leave, which
        // is a boundary each — not by anything the protocol lets it do to them.
        let ledger = self.delegations().await?;
        if ledger.keys().any(|key| key.operator == *validator) {
            return Ok(Err(
                "A validator with outstanding delegations cannot withdraw: its bond is not all its \
                 own, and the staged claim would hold stake it has no title to."
                    .to_string(),
            ));
        }
        let params = self.params().await?;
        // The **normalised** divisor, not the raw field (AUDIT C135): `epoch_divisor` is what
        // `is_epoch_boundary` divides by, and `epoch_length <= 0` means a one-block epoch, so
        // multiplying the raw field collapsed the quarantine to the absolute constant
        // `quarantine_length` and dropped the epoch offset entirely — a rule applied at one of two
        // sibling epoch sites, which is C109's tell.
        let deadline = checked_i64(
            i128::from(params.quarantine_length)
                + i128::from(epoch_divisor(&params))
                    * (1 + i128::from(block_number) / i128::from(epoch_divisor(&params))),
            "withdraw deadline",
        )?;
        let mut pending = self.pending_withdrawers().await?;
        pending.insert(*validator, deadline);
        self.set_pending_withdrawers(&pending);
        Ok(Ok(()))
    }

    /// Close a block — the **epoch transition**, which happens only at an epoch boundary
    /// (`Pos.rhox:517`: if `blockNumber % $$epochLength$$ != 0`, nothing happens at all).
    ///
    /// At a boundary the contract runs one sequence, and the order carries the meaning
    /// (`Pos.rhox:528-551`):
    ///
    /// 1. **reward** — every pooled validator's share of the pot, computed from the state *as it
    ///    stands*, is added to the committed map (`getCurrentEpochRewards`, `:241-256`, then
    ///    `commitCurrentEpochRewards`, `:568-576`). A validator that is not in the active set gets an
    ///    entry of zero, so the committed map has a key for every pooled validator.
    /// 2. **move** — each staged withdrawal becomes a claim: `withdrawers[pk] = (pool[pk], deadline)`
    ///    and the validator leaves the pool (`movePendingWithdrawer`, `:577-587`). This is what a
    ///    request staged during the epoch waited for, and because step 1 ran first, the epoch it
    ///    spent its last blocks in still paid it.
    /// 3. **pay** — every claim whose deadline has passed is paid `bond + committed[pk]` out of the
    ///    staking vault, and both entries are removed (`removeQuarantinedWithdrawers`, `:592-621`).
    /// 4. **re-select** the active set from what is left of the pool (`pickActiveValidators`, `:546`)
    ///    — the only place a bonded validator becomes active, and the only place a validator that was
    ///    below the active cap can be promoted into it.
    ///
    /// Rewards are computed before steps 2–4 change anything, so the sequence is **not idempotent**:
    /// a second call at the same height would distribute the pot again (which is the previous epoch's
    /// dust, since the committed claims now cover the rest). The system deploy calls it once per
    /// block.
    /// `fringe_state_hash` is the state hash of the **last finalised fringe** as of this block, and the
    /// close deploy carries it only to be folded into the seed the *next* boundary draws with (step 5).
    /// It is deliberately neither the deploy's `rand` — a function of the block being proposed, so a
    /// free reroll — nor the block's pre-state, which the proposer influences through its justification
    /// set. See `select_active` for the rule and step 5 for why the fringe is the value that closes it.
    pub async fn close_block(
        &self,
        block_number: i64,
        fringe_state_hash: Blake2b256Hash,
        participation: &BTreeMap<Validator, BlockHeight>,
    ) -> Result<Result<(), String>, String> {
        let params = self.params().await?;
        let boundary = is_epoch_boundary(&params, block_number);
        // The epoch decision, logged where it is made. Everything about validator membership that is not
        // immediate - activation, rewards, withdrawal processing - happens here and nowhere else, and until
        // this line existed the only way to tell whether it ran was to infer it from a changed active set.
        eprintln!(
            "[pos] close_block {} boundary={} epoch_length={} max_active={} bond=[{}, {}]",
            block_number,
            boundary,
            params.epoch_length,
            params.number_of_active_validators,
            i64::from(params.minimum_bond),
            i64::from(params.maximum_bond)
        );
        if !boundary {
            // `Pos.rhox:519`: "Epoch change does not occur." Nothing is written — no reward, no
            // activation, no payment. The active set is not even recomputed, which is faithful: the
            // contract's membership changes take effect at boundaries, and every change (bond's
            // insertion, slash's removal) is applied to the *pool* immediately where the contract
            // applies it.
            return Ok(Ok(()));
        }

        let active_for_epoch = self.active().await?;
        let liveness = self
            .apply_liveness_prelude(block_number, fringe_state_hash, participation, &params)
            .await?;
        // Read the ledgers **after** the prelude: a passing vote has already run `slash`, and these
        // are the post-slash maps the rest of the boundary must use.
        let mut pool = self.bonds().await?;
        let mut withdrawers = self.withdrawers().await?;
        let mut pending = self.pending_withdrawers().await?;
        let mut committed = self.committed_rewards().await?;
        // The four delegation ledgers (law 57, #193). Read on every boundary and written only when they
        // change — the setters clear an empty leaf rather than writing an empty value, so a chain that
        // has never delegated leaves all four **absent** and writes nothing here at all.
        let mut delegations = self.delegations().await?;
        let mut pending_delegations = self.pending_delegations().await?;
        let mut delegation_claims = self.delegation_claims().await?;
        let mut delegated_rewards = self.delegated_rewards().await?;

        // 1. The epoch's rewards, from the state as it stands — and then the **participation rule**
        // (B4, #150), which can only ever *scale down* what this boundary is about to commit. The
        // withheld fraction stays in the staking vault and is distributed by a later epoch: nothing is
        // minted, nothing is burned, and no bond is reachable from here.
        // **The participation comes from the fringe, not from this block's own pre-state** (B4, #150).
        // The pre-state is a function of the justification set and nothing requires a block to justify
        // everything it has seen, so a rule reading `pos:last_spoke` out of it can be *aimed* at a
        // chosen rival — the proposer is never its own victim, since its own record is written earlier
        // in the same block. The fringe is the >2/3-agreed object, so a stale fringe moves every
        // validator's reading back together and singles nobody out. This is the same move step 5 below
        // makes for the epoch seed, and for the same reason.
        let rewards = apply_weight(
            self.epoch_rewards(
                &active_for_epoch,
                &pool,
                &withdrawers,
                &committed,
                &delegated_rewards,
                &params,
            )
            .await?,
            participation,
            block_number,
            i64::from(params.participation_grace),
            i64::from(params.absence_slack),
            &liveness.jailed,
        );
        // **The split** (law 57, #193). Each drawn validator's share is divided across its own stake
        // and its delegations; the delegators' parts are credited to `pos:delegated_rewards` and the
        // operator keeps the remainder. The split is **exact** — `Σ shares + the operator's remainder
        // == the reward` — so a key with delegators commits the same total it would have committed
        // alone, which is what keeps law 46 (`sum_rewards_le_pot`) a statement about the reward rather
        // than about the split.
        for (validator, reward) in &rewards {
            let held = delegations_of(&delegations, validator);
            if held.is_empty() {
                // **The dormancy path, kept as its own branch rather than folded into the general
                // one.** With no delegations the split is the identity, and taking this branch is what
                // writes exactly the `committed` entry the pre-#193 port wrote: no zero-valued
                // delegated-reward entries, no division, and nothing a reader has to convince
                // themselves is a no-op.
                let carried = committed
                    .get(validator)
                    .copied()
                    .unwrap_or(NonNegI64::zero());
                committed.insert(
                    *validator,
                    balance_plus(carried, i64::from(*reward), "committed reward")?,
                );
                continue;
            }
            // The operator's own stake is the aggregate **less** what the ledger says is delegated to
            // it — the ledger is the attribution, the pool is the total. A pool entry smaller than its
            // own ledger is a state this mechanism cannot repair, so it is refused rather than paid
            // out of a negative stake.
            let aggregate = pool.get(validator).copied().unwrap_or(NonNegI64::zero());
            let own = checked_i64(
                i128::from(i64::from(aggregate))
                    - i128::from(delegated_total(&delegations, validator)?),
                "the operator's own stake",
            )?;
            if own < 0 {
                return Err(format!(
                    "the aggregate bond {} is smaller than the delegations to it — the ledger and the \
                     pool disagree",
                    i64::from(aggregate)
                ));
            }
            let amounts: Vec<i64> = held.iter().map(|(_, amount)| *amount).collect();
            let (shares, operator_share) = split_reward(i64::from(*reward), own, &amounts)?;
            for ((key, _), share) in held.iter().zip(shares.iter()) {
                let accrued = delegated_rewards
                    .get(key)
                    .copied()
                    .unwrap_or(NonNegI64::zero());
                delegated_rewards.insert(*key, balance_plus(accrued, *share, "delegated reward")?);
            }
            let carried = committed
                .get(validator)
                .copied()
                .unwrap_or(NonNegI64::zero());
            committed.insert(
                *validator,
                balance_plus(carried, operator_share, "committed reward")?,
            );
        }

        // **The measurement a network needs before it can arm the rule** (B4, #150). The weight is a
        // function of how far behind the last finalised fringe each *drawn* validator's latest message
        // sits, and nothing in this port can say what that distribution is on a network's own chain —
        // its block rate and its active-set size decide it, and both are the network's. So a boundary
        // reports it: one line, the drawn set's lags in order, which is exactly what `absence_slack`
        // (the knee) and `participation_grace` (the flat part below it) have to be chosen against.
        //
        // **Logged whether or not the rule is armed**, because the point is to measure *before*
        // choosing a value — and a value chosen without it is a guess about someone else's chain.
        {
            let mut lags: Vec<i64> = rewards
                .keys()
                .filter_map(|v| participation.get(v))
                .map(|h| block_number.saturating_sub(i64::from(*h)))
                .collect();
            lags.sort_unstable();
            let never_spoke = rewards.len() - lags.len();
            if !lags.is_empty() {
                eprintln!(
                    "[pos] participation lag at boundary {}: drawn={} never_spoke={} min={} median={} \
                     p90={} max={} (heights behind the last finalised fringe)",
                    block_number,
                    rewards.len(),
                    never_spoke,
                    lags[0],
                    lags[lags.len() / 2],
                    lags[lags.len() * 9 / 10],
                    lags[lags.len() - 1],
                );
            }
        }

        // 2. Staged withdrawals become claims against the vault.
        for (validator, deadline) in std::mem::take(&mut pending) {
            if let Some(bond) = pool.remove(&validator) {
                withdrawers.insert(validator, Withdrawal { bond, deadline });
            }
            // A pending validator that is no longer in the pool — slashed, or untrusted — has no
            // stake to claim, and its entry is dropped with it. The contract zeroes a slashed
            // validator's bond but leaves it in `pendingWithdrawers` (`Pos.rhox:491`), so at the next
            // boundary it lands in `withdrawers` with a zero amount and is never paid; dropping it
            // here is the same payable outcome without the permanent tombstone. Registered in
            // `spec/audit/passes.md` §6.
        }

        // 2b. The same move for staged **undelegations** (law 57, #193), and it comes after step 1 for
        // the same reason step 2 does: the epoch the principal spent its last blocks in has already
        // paid it by now. The principal leaves the **operator's aggregate** pool entry — not a ledger
        // of its own — and is escrowed under the delegator's name with the request's deadline, so from
        // here it is a claim against the vault with an owner who is not the operator.
        // **The move reports itself**, for the reason the participation lag does: what a boundary does
        // with a staged undelegation is consensus state, and when it does nothing there is no other
        // way to tell "no request was staged" from "one was staged and dropped here". One line, and
        // only when there is a request to report.
        if !pending_delegations.is_empty() {
            eprintln!(
                "[pos] boundary {block_number}: {} staged undelegation(s) to move out of the pool",
                pending_delegations.len()
            );
        }
        for (key, deadline) in std::mem::take(&mut pending_delegations) {
            let Some(amount) = delegations.remove(&key) else {
                // The delegation is gone — the operator was slashed, which clears its whole ledger
                // (law 57's `a_slash_clears_the_delegations`) — so there is nothing to escrow. Dropping
                // the request is the same payable outcome as the zero-value claim step 2's comment
                // describes, without the tombstone.
                eprintln!(
                    "[pos] boundary {block_number}: a staged undelegation has no ledger entry — dropped"
                );
                continue;
            };
            let Some(aggregate) = pool.get(&key.operator).copied() else {
                eprintln!(
                    "[pos] boundary {block_number}: a staged undelegation's operator is not in the pool \
                     — dropped, and the ledger entry went with it"
                );
                continue;
            };
            let Some(remaining) = i64::from(aggregate).checked_sub(i64::from(amount)) else {
                // Unreachable while the invariant "the aggregate contains every delegation to it"
                // holds. If it ever did not, refunding the ledger's amount out of a smaller pool entry
                // would mint the difference, so the request is dropped and the stake stays where the
                // pool says it is.
                continue;
            };
            let remaining =
                NonNegI64::try_from(remaining).map_err(|e| format!("undelegation move: {e}"))?;
            // A pool entry that reaches zero is **removed**, mirroring step 2's filter: an operator
            // whose own stake was zero and whose last delegation has left is no longer a pooled
            // validator, and leaving a zero entry behind would refuse its next `bond` as "already
            // bonded" while `select_active` skipped it for having no stake.
            if remaining == NonNegI64::zero() {
                pool.remove(&key.operator);
            } else {
                pool.insert(key.operator, remaining);
            }
            delegation_claims.insert(key, DelegationClaim { amount, deadline });
            eprintln!(
                "[pos] boundary {block_number}: undelegation escrowed — {} out of the operator's pool \
                 entry, which now reads {}",
                i64::from(amount),
                i64::from(remaining)
            );
        }

        // 3. Pay the claims whose quarantine has elapsed.
        let due: Vec<Validator> = withdrawers
            .iter()
            .filter(|(_, w)| w.deadline <= block_number)
            .map(|(v, _)| *v)
            .collect();
        for validator in due {
            let Some(claim) = withdrawers.remove(&validator) else {
                continue;
            };
            let reward = committed.remove(&validator).unwrap_or(NonNegI64::zero());
            let payable = balance_plus(claim.bond, i64::from(reward), "withdrawal payment")?;
            self.debit_pos_vault(payable).await?;
            let address = self.vault_address(&validator)?;
            let balance = self
                .vault_balance(&address)
                .await?
                .unwrap_or(NonNegI64::zero());
            self.set_vault_balance(
                &address,
                balance_plus(balance, i64::from(payable), "refund")?,
            );
        }

        // 3b. Pay the **delegation** claims whose quarantine has elapsed (law 57, #193) — the same
        // three-stage shape as step 3, one ledger over: the claim pays `principal + accrued rewards`
        // and both entries are removed. **The accrued reward is read from `pos:delegated_rewards` and
        // not from `committed`**, which is the whole point of holding it apart: `committed[operator]`
        // is money the operator is paid, so a delegator's share left there would be the operator's to
        // keep. The destination is the **delegator's own** vault, derived from its own key.
        let due_delegations: Vec<DelegationKey> = delegation_claims
            .iter()
            .filter(|(_, claim)| claim.deadline <= block_number)
            .map(|(key, _)| *key)
            .collect();
        for key in due_delegations {
            let Some(claim) = delegation_claims.remove(&key) else {
                continue;
            };
            let accrued = delegated_rewards.remove(&key).unwrap_or(NonNegI64::zero());
            let payable = balance_plus(claim.amount, i64::from(accrued), "delegation payment")?;
            self.debit_pos_vault(payable).await?;
            let address = self.vault_address(&key.delegator)?;
            let balance = self
                .vault_balance(&address)
                .await?
                .unwrap_or(NonNegI64::zero());
            self.set_vault_balance(
                &address,
                balance_plus(balance, i64::from(payable), "delegation refund")?,
            );
            // The other half of the pair with the escrow line above: a claim that is paid is a
            // consensus state change like any other, and the only way to observe it from outside is
            // to be told — a balance read is a deploy by the account being read, so it carries its own
            // cost and cannot resolve a return of this size on a live chain.
            eprintln!(
                "[pos] boundary {block_number}: undelegation paid — {} to the delegator's own vault \
                 ({} principal + {} accrued)",
                i64::from(payable),
                i64::from(claim.amount),
                i64::from(accrued)
            );
        }

        // 4. The active set for the epoch that starts now.
        //
        // The seed was written one boundary *ago* — at `B_{k-1}`, from `B_{k-1}`'s pre-state — so the
        // block drawing here is not the block whose state chose the entropy. That separation is the
        // whole design; see `select_active` and `docs/src/node/security-audit.md` §8. A state with no
        // seed yet is a genesis-installed one before its first boundary, and falls back to the
        // genesis constant.
        let seed = self.epoch_seed().await?.unwrap_or_else(genesis_epoch_seed);
        let active = select_active_excluding(&pool, &withdrawers, &liveness.jailed, &params, &seed);
        self.set_bonds(&pool);
        self.set_withdrawers(&withdrawers);
        self.set_pending_withdrawers(&pending);
        self.set_committed_rewards(&committed);
        self.set_active(&active);
        // The four delegation ledgers, written through the setters that **clear an empty leaf** rather
        // than writing an empty value: on a chain that has never delegated these are no-ops that touch
        // no key, which is what keeps this boundary's post-state byte-identical to the pre-#193 one.
        self.set_delegations(&delegations).await?;
        self.set_pending_delegations(&pending_delegations).await?;
        self.set_delegation_claims(&delegation_claims).await?;
        self.set_delegated_rewards(&delegated_rewards).await?;

        // 5. The seed the **next** boundary will draw with.
        //
        // **The entropy is the last finalised fringe's state hash — not this block's pre-state.** The
        // pre-state is a function of the justification set, and a proposer has some say in its own
        // justification set: the set is *derived* from the DAG (`get_pre_state_for_new_block` reads
        // `latest_msgs`), so a pre-state cannot be invented, but nothing requires a block to justify
        // everything it has seen — `validate::check_justification_regression` forbids going
        // *backwards* on the messages a block does carry, not omitting them. With the pre-state as an
        // input, a seed-writer therefore enumerates the candidates its own omissions induce, computes
        // the seed and the drawn set for each, and publishes the one it likes: a search, evaluated
        // offline, invisible in the result. The first version of this step used the pre-state and had
        // exactly that hole.
        //
        // The fringe state hash closes it because it is the **>2/3-agreed** object: it is computed from
        // the parents' seen-sets (`message_map::latest_fringe` plus `MergeScope::merge`), so a lone
        // proposer does not move it — omitting a message that has just arrived normally leaves the
        // fringe exactly where it was, which makes the seed *constant* across the candidates the writer
        // can actually choose between. **The residual, stated rather than implied:** the steering space
        // is now the number of *distinct fringes* those candidates induce — normally one, occasionally
        // a handful, if a proposer is willing to present a stale fringe. That is a reduction from "one
        // per subset" to "one per reachable fringe", not a proof of closure, and it is registered with
        // the rest of the rule's residuals in `spec/RUST-VS-SCALA.md` §3 item 12.
        //
        // **Nothing is published and nothing is verified**, which is the other half of the design: the
        // value is derived by each node from its own DAG (`replay_block`'s caller passes
        // `pre_state.fringe_state`; a replay that has no DAG reads the block's own metadata record), so
        // play and replay agree *by construction* rather than by a claim that has to be checked. The
        // block's header does not carry it, because a field the validator recomputes anyway is a second
        // derivation waiting to fall out of step.
        //
        // **The lag is two boundaries, and that is a property rather than a shortfall.** The seed read
        // at `B_k` was written at `B_{k-1}`, so the entropy behind epoch `k`'s draw is the fringe as of
        // `B_{k-1}`. A one-boundary lag is not available: the only values a writer cannot choose are
        // the ones finality has already fixed, and by the next boundary those have moved on. Older
        // entropy costs nothing — it only has to be *fixed* before the drawing proposer acts.
        //
        // Freshness is self-guaranteeing: the previous seed is an input to this one, so consecutive
        // seeds differ even on a chain whose fringe has not moved, and the seed sequence is a hash chain
        // over the fringes — no epoch's entropy can be swapped without changing every seed after it.
        let next_epoch = block_number / epoch_divisor(&params) + 1;
        self.set_epoch_seed(&EpochSeed {
            epoch: next_epoch,
            anchors: vec![previous_seed_anchor(&seed), fringe_state_hash],
        });

        // 6. **The developer allocation's release** (Laws 78–79).
        //
        // The allocation is credited to a *vesting* account at genesis and is not spendable; it vests
        // linearly in blocks from genesis, and each boundary moves whatever has vested but not yet been
        // released into the recipient's own vault. Once there it is an ordinary balance, and no other
        // rule in this chain has to know the account exists.
        //
        // **Deliberately last and deliberately independent.** It debits the vesting account and credits
        // a *user* vault, so it can reach neither the staking vault nor the pot; running it after the
        // reward steps means it cannot change what any of them computes, whatever the schedule says.
        //
        // **Granular to the epoch, not the block**, and that is a consequence of the design rather than
        // a choice: native state is written at boundaries (the `if !boundary` return above), so a chain
        // with a long epoch releases in steps. Stated here because a reader comparing the schedule to a
        // balance needs to know which clock the balance follows.
        self.release_dev_tax(block_number, &params).await?;
        Ok(Ok(()))
    }

    /// Release the vested part of the developer allocation into its recipient's vault (Law 79).
    ///
    /// Moves `vested(block_number) - released_so_far`, where `released_so_far = allocation -
    /// unreleased` — the account's own two numbers are the whole of its state, so there is no separate
    /// "last released" record to keep in step. A second call at the same height therefore moves
    /// nothing, and the released amount is monotone because both `vested` and the account are.
    ///
    /// **A chain with no allocation returns having written nothing**, which is what keeps an
    /// allocation-free chain's boundary byte-identical to the pre-Law-79 one.
    async fn release_dev_tax(&self, block_number: i64, params: &PosParams) -> Result<(), String> {
        let Some(vesting) = self.dev_vesting().await? else {
            return Ok(());
        };
        if vesting.unreleased == NonNegI64::zero() {
            return Ok(());
        }
        let allocation = i64::from(vesting.allocation);
        let unreleased = i64::from(vesting.unreleased);
        let released_so_far = allocation - unreleased;
        let vested = dev_tax_vested(
            allocation,
            i64::from(params.dev_tax_vesting_blocks),
            block_number,
        )?;
        // Nothing new has vested since the last boundary. `vested` is monotone in the height and the
        // account only moves forward, so this is the ordinary case on a chain whose epoch is shorter
        // than the granularity the schedule resolves to.
        if vested <= released_so_far {
            return Ok(());
        }
        let release = vested - released_so_far;
        let address = vesting.address.clone();
        let balance = self
            .vault_balance(&address)
            .await?
            .unwrap_or(NonNegI64::zero());
        self.set_vault_balance(
            &address,
            balance_plus(balance, release, "dev allocation release")?,
        );
        self.set_dev_vesting(Some(&DevVesting {
            address,
            allocation: vesting.allocation,
            unreleased: NonNegI64::try_from(unreleased - release).map_err(|e| e.to_string())?,
        }));
        eprintln!(
            "[pos] boundary {block_number}: dev allocation released — {release} ({vested} of \
             {allocation} vested)"
        );
        Ok(())
    }

    /// Step 1 of the epoch sequence: every pooled validator's share of the pot, zero for the ones
    /// outside the active set (`getCurrentEpochRewards`, `Pos.rhox:241-256`).
    ///
    /// The active set it reads is the **stored** one, not a recomputation — which is what makes a
    /// validator that bonded during this epoch earn nothing yet (it is not in the active set until
    /// step 4) and a validator whose withdrawal was staged this epoch earn its last reward here.
    async fn epoch_rewards(
        &self,
        active: &BTreeMap<Validator, NonNegI64>,
        pool: &BTreeMap<Validator, NonNegI64>,
        withdrawers: &BTreeMap<Validator, Withdrawal>,
        committed: &BTreeMap<Validator, NonNegI64>,
        delegated_rewards: &BTreeMap<DelegationKey, NonNegI64>,
        params: &PosParams,
    ) -> Result<BTreeMap<Validator, NonNegI64>, String> {
        // **The per-validator share is computed on the aggregate**, which is what makes a delegation
        // count in the draw and in the pot's division alike; the *split* of that share across the
        // key's owner and its delegators is the caller's step, not this one. Delegated stake therefore
        // enters the numerator and the denominator through `pool` alone — there is no second term here.
        let pot = epoch_pot(
            self.pos_vault_balance().await?,
            pool,
            withdrawers,
            committed,
            delegated_rewards,
        )?;
        let active_bonds: i128 = pool
            .iter()
            .filter(|(v, _)| active.contains_key(*v))
            .map(|(_, stake)| i128::from(i64::from(*stake)))
            .sum();
        let active_bonds = checked_i64(active_bonds, "active bonds")?;
        let mut rewards = BTreeMap::new();
        for (validator, stake) in pool {
            let reward = if active.contains_key(validator) {
                epoch_reward(
                    pot,
                    i64::from(params.minimum_bond),
                    active_bonds,
                    i64::from(*stake),
                )?
            } else {
                0
            };
            rewards.insert(
                *validator,
                NonNegI64::try_from(reward).map_err(|e| e.to_string())?,
            );
        }
        Ok(rewards)
    }

    /// **The liveness prelude**: tally open votes, update absence counters, impose/expire jails and
    /// open vote windows, at one advancing boundary. It writes only when the mechanism is armed; a
    /// disabled chain returns without touching a leaf.
    ///
    /// It runs **before** the normal boundary steps read the pool and ledgers, because a passing
    /// vote calls [`Self::slash`] and a later read of stale maps would resurrect the offender.
    /// `participation` is the receiver-derived finalised-fringe map; `fringe_state_hash` is the same
    /// boundary's finalised fringe. No wall-clock or proposer-chosen input enters the rule.
    async fn apply_liveness_prelude(
        &self,
        block_number: i64,
        fringe_state_hash: Blake2b256Hash,
        participation: &BTreeMap<Validator, BlockHeight>,
        params: &PosParams,
    ) -> Result<LivenessOutcome, String> {
        let mut outcome = LivenessOutcome::default();
        if params.liveness_jail_after_boundaries == 0 {
            return Ok(outcome);
        }
        if participation.is_empty() {
            // No finalised evidence at this boundary. Do not jail everyone on an empty/young fringe.
            return Ok(outcome);
        }
        let mut active = self.active().await?;
        let tip = crate::pos_liveness::quorum_tip(&active, participation);
        if tip == 0 {
            return Ok(outcome);
        }
        let Some(anchor) = self.liveness_anchor().await? else {
            // The first armed boundary establishes the comparison point; it judges nobody.
            self.set_liveness_anchor(&Anchor {
                fringe_state_hash: *fringe_state_hash.as_bytes(),
                quorum_tip: tip,
            });
            return Ok(outcome);
        };
        if tip <= anchor.quorum_tip {
            // A changed fringe with an unchanged quorum tip is not progress: the clocks freeze.
            eprintln!(
                "[pos] boundary {block_number}: liveness clocks frozen (quorum tip {tip} did not \
                 advance past {})",
                anchor.quorum_tip
            );
            return Ok(outcome);
        }

        let mut pool = self.bonds().await?;
        let mut windows = self.vote_windows().await?;
        let mut ballots = self.slash_votes().await?;
        let mut banished = self.banished().await?;

        // 1. Tally the windows that were already open. The denominator is the active stake at this
        // boundary; a validator that left the pool drops its window without a slash.
        let total_active: i128 = active
            .values()
            .map(|stake| i128::from(i64::from(*stake)))
            .sum();
        let mut passing = Vec::new();
        for target in windows.keys().copied().collect::<Vec<_>>() {
            if !pool.contains_key(&target) {
                continue;
            }
            // Ballots are weighted by the epoch's active snapshot. Delegation changes
            // the bonded pool immediately, but the active weights only at the next
            // boundary. Reading the mutable pool here lets a single active voter
            // inflate its voting weight mid-epoch and confiscate a jailed peer's bond.
            let for_stake: i128 = ballots
                .iter()
                .filter(|(key, in_favor)| {
                    key.target == target && **in_favor && active.contains_key(&key.voter)
                })
                .map(|(key, _)| {
                    i128::from(i64::from(
                        active.get(&key.voter).copied().unwrap_or(NonNegI64::zero()),
                    ))
                })
                .sum();
            if crate::pos_liveness::passes_vote(for_stake, total_active) {
                passing.push(target);
            }
        }
        if let Some(severity) = liveness_slash_severity(params.liveness_slash_severity) {
            for target in passing {
                match self.slash(&target, severity).await? {
                    Ok(()) => {
                        banished.insert(target);
                        windows.remove(&target);
                        ballots.retain(|key, _| key.target != target);
                        outcome.slashed.push(target);
                        eprintln!(
                            "[pos] boundary {block_number}: slash vote passed for {target:?}"
                        );
                    }
                    Err(e) => eprintln!(
                        "[pos] boundary {block_number}: vote target {target:?} could not be \
                         slashed: {e}"
                    ),
                }
            }
        }
        if !outcome.slashed.is_empty() {
            // `slash` rewrote the ledgers; read the post-slash state before counting.
            active = self.active().await?;
            pool = self.bonds().await?;
        }
        for target in windows.keys().copied().collect::<Vec<_>>() {
            if !pool.contains_key(&target) {
                windows.remove(&target);
                ballots.retain(|key, _| key.target != target);
            }
        }

        // 2. Count presence for the validators that served this epoch. A validator not in the old
        // active set is not asked to have spoken; a jailed validator is handled by expiry below.
        let threshold = params.liveness_absence_threshold;
        let jail_after = params.liveness_jail_after_boundaries;
        let jail_epochs = params.liveness_jail_epochs;
        let mut records = self.liveness().await?;
        for validator in active.keys() {
            let absent = crate::pos_liveness::is_absent(participation, tip, threshold, validator);
            let record = records.entry(*validator).or_default();
            if absent {
                record.absent_boundaries = record.absent_boundaries.saturating_add(1);
            } else {
                record.absent_boundaries = 0;
            }
            if !record.is_jailed() && record.absent_boundaries >= jail_after {
                record.absent_boundaries = 0;
                record.jail_remaining = jail_epochs;
                outcome.jailed.insert(*validator);
                if params.liveness_vote_window > 0 {
                    windows.insert(
                        *validator,
                        VoteWindow {
                            opened_at: block_number,
                            remaining: params.liveness_vote_window,
                        },
                    );
                }
                eprintln!(
                    "[pos] boundary {block_number}: jailed {validator:?} for {jail_epochs} \
                     boundaries (sustained absence)"
                );
            }
        }

        // 3. Expire or renew the jails that were already running. One message releases a jail;
        // continued silence renews it. A jail imposed in step 2 is not decremented on its own
        // boundary.
        let expiring: Vec<Validator> = records
            .iter()
            .filter(|(validator, record)| {
                record.is_jailed() && !outcome.jailed.contains(*validator)
            })
            .map(|(validator, _)| *validator)
            .collect();
        for validator in expiring {
            let Some(record) = records.get_mut(&validator) else {
                continue;
            };
            record.jail_remaining = record.jail_remaining.saturating_sub(1);
            if record.jail_remaining > 0 {
                continue;
            }
            let absent = crate::pos_liveness::is_absent(participation, tip, threshold, &validator);
            if absent {
                record.jail_remaining = jail_epochs;
                if params.liveness_vote_window > 0 && !windows.contains_key(&validator) {
                    windows.insert(
                        validator,
                        VoteWindow {
                            opened_at: block_number,
                            remaining: params.liveness_vote_window,
                        },
                    );
                }
                eprintln!(
                    "[pos] boundary {block_number}: renewed the jail on {validator:?} — still absent"
                );
            } else {
                records.remove(&validator);
                windows.remove(&validator);
                ballots.retain(|key, _| key.target != validator);
                eprintln!("[pos] boundary {block_number}: released {validator:?} — it spoke again");
            }
        }

        // 4. Age the windows that were not opened on this boundary and close the ones with no
        // target or no time left.
        for validator in windows.keys().copied().collect::<Vec<_>>() {
            let Some(window) = windows.get_mut(&validator) else {
                continue;
            };
            if window.opened_at != block_number {
                window.remaining = window.remaining.saturating_sub(1);
            }
            let target_jailed = records
                .get(&validator)
                .is_some_and(AbsenceRecord::is_jailed);
            if window.remaining == 0 || !target_jailed || !pool.contains_key(&validator) {
                windows.remove(&validator);
                ballots.retain(|key, _| key.target != validator);
            }
        }

        // 5. A record is only meaningful for a validator that served this epoch or is still jailed.
        records.retain(|validator, record| {
            pool.contains_key(validator)
                && (record.is_jailed()
                    || (active.contains_key(validator) && record.absent_boundaries > 0))
        });
        outcome.jailed = records
            .iter()
            .filter(|(_, record)| record.is_jailed())
            .map(|(validator, _)| *validator)
            .collect();
        self.set_liveness(&records).await?;
        self.set_vote_windows(&windows).await?;
        self.set_slash_votes(&ballots).await?;
        self.set_banished(&banished).await?;
        self.set_liveness_anchor(&Anchor {
            fringe_state_hash: *fringe_state_hash.as_bytes(),
            quorum_tip: tip,
        });
        Ok(outcome)
    }

    /// Remove a validator and confiscate its stake to the Coop slashing vault (port of the PoS
    /// `slash` behavior). A pending withdrawal is cancelled (the stake is forfeited) and the
    /// validator's **accrued** rewards are dropped with it (AUDIT C197).
    ///
    /// Removal is **immediate** in the contract, unlike bonding: the slashing contract deletes the
    /// validator from `activeValidators` and zeroes its bond in the same state update as the transfer
    /// (`Pos.rhox:486-495`), so law 44's epoch gate does not apply to it. What the contract does not
    /// do is take the validator out of `pendingWithdrawers`; the port does, with the same payable
    /// outcome (see the note in `close_block`).
    ///
    /// **The accrued rewards go too, and leaving them behind was a deviation** (C197). `Pos.rhox`'s
    /// state update deletes the offender's `committedRewards` entry in the same write as the bond
    /// zeroing (`committedRewards: … .delete(slashedValidator)`). This port did not, and the entry was
    /// then unreachable in both directions: **never paid**, because `close_block` pays pool members
    /// and claims in `withdrawers` and the slashed validator is out of both, and **never removed**,
    /// because the only `committed.remove` in this file is the claim payment. Since `epoch_pot`
    /// subtracts *every* `committed` entry, the stranded balance went on shrinking the pot for every
    /// other validator for ever.
    ///
    /// **What is confiscated is a *share* of everything the validator holds, and the tier decides how
    /// much** (AUDIT C199). Three holdings are at risk, and all three are in the staking vault: the
    /// pool **bond**, any **escrowed claim** (a staged withdrawal whose bond has already left the pool),
    /// and the **accrued rewards**. The tier takes `basis_points` of their sum to the Coop vault and
    /// **returns the remainder to the validator's own vault**, so the loss is exactly the tier — which
    /// is the property an operator needs before bonding, and the reason a rule that always took
    /// everything made staking a bad deal whatever the reward was.
    ///
    /// Both sides of that are vault-to-vault transfers, so nothing is minted or burned, and
    /// `confiscated + returned == at_risk` exactly: the floor division's remainder is *returned*, not
    /// kept. `SlashSeverity::Unspecified` — a slash recorded before the tiers existed — takes the whole
    /// sum, which is what it would have done.
    pub async fn slash(
        &self,
        validator: &Validator,
        severity: SlashSeverity,
    ) -> Result<Result<(), String>, String> {
        let mut pool = self.bonds().await?;
        let mut active = self.active().await?;
        let mut withdrawers = self.withdrawers().await?;
        let mut pending = self.pending_withdrawers().await?;
        let mut committed = self.committed_rewards().await?;
        let mut delegations = self.delegations().await?;
        let mut pending_delegations = self.pending_delegations().await?;
        let mut delegation_claims = self.delegation_claims().await?;
        let mut delegated_rewards = self.delegated_rewards().await?;

        let bond = pool.remove(validator).map(i64::from).unwrap_or(0);
        active.remove(validator);
        let escrowed = withdrawers
            .remove(validator)
            .map(|claim| i64::from(claim.bond))
            .unwrap_or(0);
        pending.remove(validator);
        let accrued = committed.remove(validator).map(i64::from).unwrap_or(0);

        // **The delegated holdings, collected per delegator** (law 57, #193) — because a delegator is
        // the payee, and the same delegator may hold a principal, an escrow and an accrual on one key.
        //
        // Two different sums come out of this, and keeping them straight is the crux: `delegator_risk`
        // is the **weight** each delegator's share is proportional to, while what the slash *takes from
        // the staking vault* is `at_risk`. A principal appears in both — it is inside the aggregate
        // `bond` **and** it is the delegator's largest at-risk holding — so it is counted once by
        // subtracting `delegated_principal` out of the offender's own part below rather than by leaving
        // it out of either. The model's `atRisk` reads the same way, and for the same reason.
        let mut delegator_risk: BTreeMap<Validator, i64> = BTreeMap::new();
        // Read the principals **before** the ledger is drained below — they are needed twice, once as
        // weights for the fan-out and once to net out of the aggregate bond, and the second use has to
        // happen while the ledger still holds them.
        let delegated_principal = delegated_total(&delegations, validator)?;
        for key in operator_keys(&delegations, validator) {
            let amount = delegations.remove(&key).map(i64::from).unwrap_or(0);
            add_delegator_risk(&mut delegator_risk, key.delegator, amount)?;
        }
        for key in operator_keys(&delegation_claims, validator) {
            let claim = delegation_claims
                .remove(&key)
                .map(|claim| i64::from(claim.amount))
                .unwrap_or(0);
            let reward = delegated_rewards.remove(&key).map(i64::from).unwrap_or(0);
            add_delegator_risk(&mut delegator_risk, key.delegator, claim)?;
            add_delegator_risk(&mut delegator_risk, key.delegator, reward)?;
        }
        // An accrual with no claim: the ordinary case for a *live* delegation, whose principal is
        // still in the pool. An accrual and a claim go together at a payout, so this loop finds
        // nothing for a pair the one above has already cleared.
        for key in operator_keys(&delegated_rewards, validator) {
            let reward = delegated_rewards.remove(&key).map(i64::from).unwrap_or(0);
            add_delegator_risk(&mut delegator_risk, key.delegator, reward)?;
        }
        // A staged request dies with the delegation it names: the ledger entry it would have escrowed
        // is gone, so letting it outlive the operator could only produce a claim of zero.
        for key in operator_keys(&pending_delegations, validator) {
            pending_delegations.remove(&key);
        }
        let delegators: Vec<Validator> = delegator_risk.keys().copied().collect();
        let delegated_risk: Vec<i64> = delegator_risk.values().copied().collect();

        // **What the offender's own key was holding** — and the subtraction is the whole subtlety of
        // this function. `bond` is the **aggregate**, so the delegators' principals are already inside
        // it; adding them again (they are also in `delegator_risk`, as the weights the fan-out needs)
        // would overstate `at_risk` by exactly the delegated stake, and a tier applied to an overstated
        // sum confiscates more than the offender's key holds — a slash that mints.
        let own_stake = checked_i64(
            i128::from(bond) - i128::from(delegated_principal),
            "the offender's own stake",
        )?;
        if own_stake < 0 {
            return Err(format!(
                "the aggregate bond {bond} is smaller than the delegations to it ({delegated_principal}) \
                 — the ledger and the pool disagree"
            ));
        }
        let operator_part = checked_i64(
            i128::from(own_stake) + i128::from(escrowed) + i128::from(accrued),
            "the offender's own holdings",
        )?;
        let at_risk = checked_i64(
            i128::from(operator_part) + delegated_risk.iter().map(|w| i128::from(*w)).sum::<i128>(),
            "slashed holdings",
        )?;
        let confiscated =
            i64::try_from(i128::from(at_risk) * i128::from(severity.basis_points()) / 10_000)
                .map_err(|e| format!("slash share: {e}"))?;
        let returned = at_risk - confiscated;

        // The confiscated share leaves the staking vault for the Coop multisig vault
        // (`Pos.rhox:470-482`: `posVault!("transfer", coopMultiVaultAddr, valBond, posAuthKey)`).
        // Debiting the source is what makes this a *transfer*: crediting the Coop vault on its own —
        // which is what this did before the staking vault existed — mints the slashed stake from
        // nothing.
        if confiscated > 0 {
            self.debit_pos_vault(NonNegI64::try_from(confiscated).map_err(|e| e.to_string())?)
                .await?;
            let coop = self.coop_balance().await?;
            self.set_coop_balance(balance_plus(coop, confiscated, "slash coop")?);
        }
        // …and the rest is **fanned out** (law 57, #193): the offender's own part back to its own
        // vault, and each delegator's part to **its own** vault. Never the whole of it to the
        // offender's — which is the single most likely way to get this wrong, because before
        // delegations existed `returned` was one payment to one address, and a delegator's principal
        // is inside the aggregate `bond` this function just removed.
        //
        // The division is the same `pro_rata` the epoch split uses, with `operator_part` as the base
        // that is divided *by* but issued no share: each delegator is paid its proportion of what it
        // had at risk and the offender keeps the integer-division remainder. The payments therefore sum
        // to `returned` exactly, which is what keeps this a transfer — `confiscated + Σ payments ==
        // at_risk == what the staking vault released`.
        if returned > 0 {
            let shares = pro_rata(returned, operator_part, &delegated_risk)?;
            self.debit_pos_vault(NonNegI64::try_from(returned).map_err(|e| e.to_string())?)
                .await?;
            let paid: i128 = shares.iter().map(|s| i128::from(*s)).sum();
            let operator_share = checked_i64(
                i128::from(returned) - paid,
                "the offender's share of its own slash",
            )?;
            if operator_share > 0 {
                let address = self.vault_address(validator)?;
                let balance = self
                    .vault_balance(&address)
                    .await?
                    .unwrap_or(NonNegI64::zero());
                self.set_vault_balance(
                    &address,
                    balance_plus(balance, operator_share, "slash return")?,
                );
            }
            for (delegator, share) in delegators.iter().zip(shares.iter()) {
                if *share <= 0 {
                    continue;
                }
                let address = self.vault_address(delegator)?;
                let balance = self
                    .vault_balance(&address)
                    .await?
                    .unwrap_or(NonNegI64::zero());
                self.set_vault_balance(
                    &address,
                    balance_plus(balance, *share, "delegated slash return")?,
                );
            }
        }

        self.set_bonds(&pool);
        self.set_active(&active);
        self.set_withdrawers(&withdrawers);
        self.set_committed_rewards(&committed);
        // The offender leaves every delegation ledger, not only the pool: `a_slash_clears_the_delegations`
        // is the model's statement of it, and the setters clear an empty leaf rather than writing an
        // empty value, so a slashed operator on a chain with no other delegations leaves all four
        // leaves exactly as absent as they were.
        self.set_delegations(&delegations).await?;
        self.set_pending_delegations(&pending_delegations).await?;
        self.set_delegation_claims(&delegation_claims).await?;
        self.set_delegated_rewards(&delegated_rewards).await?;
        // The pending-withdrawal entry this removes is in `pending`, and a slash that leaves it behind
        // hands it to whatever occupies this validator key next: a later accepted bond is then moved into
        // a claim at the epoch boundary instead of joining the active set. Found by the PoS review, which
        // measured the stale entry surviving slash in 9/9 configurations.
        self.set_pending_withdrawers(&pending);
        // **A removed validator has no liveness state.** Clear its counters, its open slash vote and
        // any ballots it cast; `pos:banished` is the vote path's own record and is deliberately left.
        let mut records = self.liveness().await?;
        if records.remove(validator).is_some() {
            self.set_liveness(&records).await?;
        }
        let mut windows = self.vote_windows().await?;
        if windows.remove(validator).is_some() {
            self.set_vote_windows(&windows).await?;
        }
        let mut ballots = self.slash_votes().await?;
        let before = ballots.len();
        ballots.retain(|key, _| key.target != *validator && key.voter != *validator);
        if ballots.len() != before {
            self.set_slash_votes(&ballots).await?;
        }
        Ok(Ok(()))
    }

    /// Admit `target` into the trusted stakeholder set. Only an existing trusted stakeholder
    /// (`caller`) may confer trust. This is the observer → validator admission step.
    pub async fn trust(
        &self,
        caller: &Validator,
        target: &Validator,
    ) -> Result<Result<(), String>, String> {
        let mut trusted = self.trusted().await?;
        if !trusted.contains(caller) {
            return Ok(Err(
                "Only a trusted stakeholder can admit validators.".to_string()
            ));
        }
        trusted.insert(*target);
        self.set_trusted(&trusted);
        Ok(Ok(()))
    }

    /// Revoke `target`'s trust. Only an existing trusted stakeholder may revoke, and a stakeholder
    /// cannot revoke itself.
    ///
    /// **It does not confiscate the target's bond, and that is the fix rather than an omission**
    /// (AUDIT C111). This used to call `slash(target)` whenever the target was bonded, so any single
    /// trusted stakeholder could — with no evidence, no second signature and no delay — move any other
    /// validator's entire stake into the Coop vault. One signature, one deploy, and a peer's bond is
    /// gone; the only gate was "is the caller trusted", which the caller already was by definition.
    ///
    /// Trust is an admission list; a bond is property. Severing the first should not transfer the
    /// second, and confiscation belongs where it is **justified**: a block whose own justifications
    /// hold that validator responsible for an attributable failure, which is now checked on every
    /// validator rather than taken from the proposer (`casper::validate::slashable_senders`,
    /// AUDIT C110). An untrusted-but-bonded validator keeps its stake and can withdraw it, which is
    /// what makes revocation a governance act rather than a robbery. Quorum and delay were the other
    /// candidates; both still end with one actor deciding another's property, just more slowly.
    ///
    /// The `let _ = self.slash(target).await?` this replaces discarded the inner `Result` as well, so a
    /// slash that *failed* — a staking vault that could not cover the transfer — was silently ignored
    /// while the caller was told the revocation had succeeded. Removing the call removes that hazard
    /// with it rather than leaving a second one to fix.
    pub async fn untrust(
        &self,
        caller: &Validator,
        target: &Validator,
    ) -> Result<Result<(), String>, String> {
        let mut trusted = self.trusted().await?;
        if !trusted.contains(caller) {
            return Ok(Err(
                "Only a trusted stakeholder can revoke validators.".to_string()
            ));
        }
        if caller == target {
            return Ok(Err("A stakeholder cannot revoke its own trust.".to_string()));
        }
        trusted.remove(target);
        self.set_trusted(&trusted);
        Ok(Ok(()))
    }

    /// **Cast a ballot in an open slash vote** (RCHIP #55, Law 89). A vote is open only for a
    /// validator the automatic jail has already removed from the active set; an active validator
    /// with its bond intact cannot be targeted. The caller must be in the active set; its weight is
    /// its pool bond, read at the tally.
    pub async fn cast_vote(
        &self,
        voter: &Validator,
        target: &Validator,
        in_favor: bool,
    ) -> Result<Result<(), String>, String> {
        let params = self.params().await?;
        if params.liveness_jail_after_boundaries == 0 || params.liveness_vote_window == 0 {
            return Ok(Err(
                "liveness voting is not enabled on this chain".to_string()
            ));
        }
        if self.banished().await?.contains(voter) {
            return Ok(Err("a banished validator cannot vote".to_string()));
        }
        if !self.active().await?.contains_key(voter) {
            return Ok(Err("only an active validator can vote".to_string()));
        }
        if !self.bonds().await?.contains_key(target) {
            return Ok(Err("the vote target is not a bonded validator".to_string()));
        }
        let jailed = self
            .liveness()
            .await?
            .get(target)
            .is_some_and(AbsenceRecord::is_jailed);
        if !jailed {
            return Ok(Err(
                "the vote target is not jailed; a vote cannot start against an active validator"
                    .to_string(),
            ));
        }
        if !self.vote_windows().await?.contains_key(target) {
            return Ok(Err("no vote window is open for the target".to_string()));
        }
        let mut ballots = self.slash_votes().await?;
        ballots.insert(
            BallotKey {
                target: *target,
                voter: *voter,
            },
            in_favor,
        );
        self.set_slash_votes(&ballots).await?;
        Ok(Ok(()))
    }

    /// Withdraw a ballot. Absent is `Ok`: a retraction that finds nothing has already had the effect
    /// it asked for.
    pub async fn retract_vote(
        &self,
        voter: &Validator,
        target: &Validator,
    ) -> Result<Result<(), String>, String> {
        let mut ballots = self.slash_votes().await?;
        ballots.remove(&BallotKey {
            target: *target,
            voter: *voter,
        });
        self.set_slash_votes(&ballots).await?;
        Ok(Ok(()))
    }

    // --- Vault ------------------------------------------------------------

    /// Read a vault balance (address is the REV base58 string).
    pub async fn vault_balance(&self, address: &str) -> Result<Option<NonNegI64>, String> {
        match self.store.get(PREFIX_VAULT, &vault_key(address)).await? {
            Some(bytes) => {
                let arr: [u8; 8] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| format!("vault balance for {address} is {} bytes", bytes.len()))?;
                NonNegI64::try_from(i64::from_le_bytes(arr))
                    .map(Some)
                    .map_err(|_| format!("vault balance for {address} is negative"))
            }
            None => Ok(None),
        }
    }

    /// Write a vault balance.
    pub fn set_vault_balance(&self, address: &str, balance: NonNegI64) {
        self.store.put(
            PREFIX_VAULT,
            vault_key(address),
            i64::from(balance).to_le_bytes().to_vec(),
        );
    }

    /// Ensure a vault exists for `address`, creating it with a zero balance if absent (port of the
    /// RevVault `findOrCreate` behavior, simplified: the vault is keyed by REV address, so the
    /// unforgeable-name capability is not modeled — `spec/RUST-FIRST.md`'s B2 decision says what that
    /// costs (delegation) and what it does not (the spend rule, which the caller's `deployerId`
    /// carries), and `spec/audit/passes.md` §6 holds it as a registered deviation).
    pub async fn find_or_create_vault(&self, address: &str) -> Result<(), String> {
        if self.vault_balance(address).await?.is_none() {
            self.set_vault_balance(address, NonNegI64::zero());
        }
        Ok(())
    }

    /// The address a **minted handle** opens, or `None` if this name was never handed out.
    ///
    /// `None` is the interesting answer — it is what refuses a caller who presents a name nobody
    /// minted, which is the whole authority check the capability rests on. The bytes are the base58
    /// address, stored as-is; a value that is not valid UTF-8 is treated as absent rather than as an
    /// error, because a corrupted leaf and a forged name are the same answer here (refuse).
    pub async fn vault_name_address(&self, name: &[u8]) -> Result<Option<String>, String> {
        match self
            .store
            .get(PREFIX_VAULT_NAME, &vault_name_key(name))
            .await?
        {
            Some(bytes) => Ok(String::from_utf8(bytes).ok()),
            None => Ok(None),
        }
    }

    /// The address a **minted authority** opens, or `None` if this name was never made an authority.
    ///
    /// This is the spend check's other half: `vault_name_address` answers "which vault does this name
    /// open", this answers "may the holder of this name spend from it". `findOrCreate` writes the
    /// first and never the second, so a handle it mints for someone else's address is not an
    /// authority over that vault.
    pub async fn vault_authority_address(&self, name: &[u8]) -> Result<Option<String>, String> {
        match self
            .store
            .get(PREFIX_VAULT_AUTH, &vault_name_key(name))
            .await?
        {
            Some(bytes) => Ok(String::from_utf8(bytes).ok()),
            None => Ok(None),
        }
    }

    /// Record that `name` **authorises** spends from the vault at `address` — what
    /// `unforgeableAuthKey` writes.
    pub fn set_vault_authority(&self, name: &[u8], address: &str) {
        self.store.put(
            PREFIX_VAULT_AUTH,
            vault_name_key(name),
            address.as_bytes().to_vec(),
        );
    }

    /// Record that `name` opens the vault at `address` — what `findOrCreate` writes when it mints a
    /// handle. Recorded in native state (not in the handler's closure) so the handle still resolves
    /// in a later block and on the replay path, both of which construct a fresh dispatcher.
    pub fn set_vault_name(&self, name: &[u8], address: &str) {
        self.store.put(
            PREFIX_VAULT_NAME,
            vault_name_key(name),
            address.as_bytes().to_vec(),
        );
    }

    /// Move `amount` from one vault to another: the spend rule, in one place.
    ///
    /// **The two callers are two shapes of the same authority** — the classic arm, where `from` comes
    /// from the caller's `deployerId`, and the capability handle, where it comes from the name the
    /// caller presented. They must not be able to disagree about what a legal spend is, so the
    /// movement lives here rather than in each arm.
    ///
    /// The return is deliberately two-layered: `Ok(Err(reason))` is a refusal the *caller* caused and
    /// should see (`(false, reason)` — insufficient balance, a sum that would overflow); `Err(e)` is
    /// the store failing, which is not their fault and must not be dressed as a verdict about their
    /// transfer.
    ///
    /// The self-transfer arm is a no-op **after** the balance check, preserving the arm's existing
    /// behaviour: in the oracle the purse split/deposit nets to zero, but an amount above the balance
    /// must still fail, and without the guard the two writes below would target one leaf.
    pub async fn transfer_vault(
        &self,
        from: &str,
        to: &str,
        amount: NonNegI64,
    ) -> Result<Result<(), String>, String> {
        let from_balance = self.vault_balance(from).await?.unwrap_or(NonNegI64::zero());
        if i64::from(from_balance) < i64::from(amount) {
            return Ok(Err("transfer: insufficient balance".to_string()));
        }
        if from == to {
            return Ok(Ok(()));
        }
        let to_balance = self.vault_balance(to).await?.unwrap_or(NonNegI64::zero());
        let new_from = NonNegI64::try_from(i64::from(from_balance) - i64::from(amount))
            .map_err(|e| e.to_string())?;
        // Accumulate in checked i64 so `to_balance + amount` cannot overflow (both are non-negative,
        // so only an i64::MAX-exceeding sum overflows).
        let new_to_i64 = i64::from(to_balance)
            .checked_add(i64::from(amount))
            .ok_or_else(|| "transfer: destination balance overflow".to_string())?;
        let new_to = NonNegI64::try_from(new_to_i64).map_err(|e| e.to_string())?;
        self.set_vault_balance(from, new_from);
        self.set_vault_balance(to, new_to);
        Ok(Ok(()))
    }

    // --- Cross-shard 2PC transactions ----------------------------------------

    /// Read a cross-shard transaction record, if present.
    pub async fn txn(&self, id: &[u8]) -> Result<Option<TxnRecord>, String> {
        match self.store.get(PREFIX_TXN, &txn_key(id)).await? {
            Some(bytes) => decode_txn(&bytes).map(Some),
            None => Ok(None),
        }
    }

    /// Write a cross-shard transaction record.
    fn set_txn(&self, id: &[u8], rec: &TxnRecord) {
        self.store.put(PREFIX_TXN, txn_key(id), encode_txn(rec));
    }

    /// Escrow `amount` REV from `from`'s vault and record a `Prepared` transaction. Idempotent under
    /// the transaction id (Law 28): a duplicate `prepare` returns the existing state unchanged.
    pub async fn txn_prepare(
        &self,
        id: &[u8],
        coordinator: &PublicKey,
        amount: NonNegI64,
        from: &str,
        to: &str,
    ) -> Result<TxnState, String> {
        if let Some(existing) = self.txn(id).await? {
            return Ok(existing.state);
        }
        let from_balance = self.vault_balance(from).await?.unwrap_or(NonNegI64::zero());
        if i64::from(from_balance) < i64::from(amount) {
            return Err("txn prepare: insufficient balance".to_string());
        }
        let new_from = NonNegI64::try_from(i64::from(from_balance) - i64::from(amount))
            .map_err(|e| e.to_string())?;
        self.set_vault_balance(from, new_from);
        self.set_txn(
            id,
            &TxnRecord {
                state: TxnState::Prepared,
                coordinator: coordinator.clone(),
                amount,
                from: from.to_string(),
                to: to.to_string(),
            },
        );
        Ok(TxnState::Prepared)
    }

    /// Commit a prepared transaction: transfer the escrow to `to`. Idempotent (a committed record is
    /// returned unchanged); committing an aborted transaction is an error.
    pub async fn txn_commit(&self, id: &[u8]) -> Result<TxnState, String> {
        let Some(rec) = self.txn(id).await? else {
            return Err("txn commit: unknown transaction".to_string());
        };
        match rec.state {
            TxnState::Committed => return Ok(TxnState::Committed),
            TxnState::Aborted => return Err("txn commit: already aborted".to_string()),
            TxnState::Prepared => {}
        }
        let to_balance = self
            .vault_balance(&rec.to)
            .await?
            .unwrap_or(NonNegI64::zero());
        let new_to = NonNegI64::try_from(
            i64::from(to_balance)
                .checked_add(i64::from(rec.amount))
                .ok_or("txn commit: destination balance overflow")?,
        )
        .map_err(|e| e.to_string())?;
        self.set_vault_balance(&rec.to, new_to);
        self.set_txn(
            id,
            &TxnRecord {
                state: TxnState::Committed,
                ..rec.clone()
            },
        );
        Ok(TxnState::Committed)
    }

    /// Abort a prepared transaction: return the escrow to `from`. Idempotent (an aborted record is
    /// returned unchanged); aborting a committed transaction is an error.
    pub async fn txn_abort(&self, id: &[u8]) -> Result<TxnState, String> {
        let Some(rec) = self.txn(id).await? else {
            return Err("txn abort: unknown transaction".to_string());
        };
        match rec.state {
            TxnState::Aborted => return Ok(TxnState::Aborted),
            TxnState::Committed => return Err("txn abort: already committed".to_string()),
            TxnState::Prepared => {}
        }
        let from_balance = self
            .vault_balance(&rec.from)
            .await?
            .unwrap_or(NonNegI64::zero());
        let new_from = NonNegI64::try_from(
            i64::from(from_balance)
                .checked_add(i64::from(rec.amount))
                .ok_or("txn abort: source balance overflow")?,
        )
        .map_err(|e| e.to_string())?;
        self.set_vault_balance(&rec.from, new_from);
        self.set_txn(
            id,
            &TxnRecord {
                state: TxnState::Aborted,
                ..rec.clone()
            },
        );
        Ok(TxnState::Aborted)
    }

    // --- Registry ------------------------------------------------------------

    /// Look up a registered `Par` by URI.
    pub async fn registry_lookup(&self, uri: &str) -> Result<Option<Par>, String> {
        match self.store.get(PREFIX_REGISTRY, &registry_key(uri)).await? {
            Some(bytes) => Ok(Some(<Par as Serialize<Par>>::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Register a `Par` under `uri`.
    pub fn registry_insert(&self, uri: &str, value: &Par) {
        self.store.put(
            PREFIX_REGISTRY,
            registry_key(uri),
            <Par as Serialize<Par>>::encode(value),
        );
    }

    // --- System-deploy operations (native) -------------------------------

    /// Charge `amount` to the deployer's REV vault (port of the PoS `chargeDeploy` behavior). The
    /// outer `Result` is a platform failure; the inner is the `(Bool, Either)` user result.
    ///
    /// **`amount: NonNegI64` is the load-bearing half of AUDIT C109.** This used to take `i64` and
    /// guard `amount == 0`, so a negative amount walked past every check: the insufficient-funds test
    /// (`balance < amount`) is false for any `NonNegI64` balance against a negative `amount`, and the
    /// subtraction below (`balance - amount`) *adds*, so `NonNegI64::try_from` succeeded and the
    /// deployer's vault was **credited**. `credit_pos_vault` then no-opped on the negative
    /// (`amount <= 0` at its own guard), so the staking vault was untouched: REV minted from nothing.
    /// The sibling [`Self::refund`] guarded `amount <= 0` all along — the asymmetry is what the fix
    /// removes, by making the negative unrepresentable instead of guarded against.
    pub async fn pre_charge(
        &self,
        deployer: &PublicKey,
        amount: NonNegI64,
    ) -> Result<Result<(), String>, String> {
        if amount == NonNegI64::zero() {
            return Ok(Ok(()));
        }
        // Discharged once, into the raw value the balance arithmetic needs. Every use below reads
        // this binding, so there is no second place a sign could enter.
        let raw = i64::from(amount);
        let address = RevAddress::from_public_key(deployer)
            .ok_or_else(|| "preCharge: invalid deployer public key".to_string())?
            .to_base58();
        let balance = match self.vault_balance(&address).await? {
            Some(b) => b,
            None => NonNegI64::try_from(0).map_err(|e| e.to_string())?,
        };
        if i64::from(balance) < raw {
            return Ok(Err(format!(
                "preCharge: insufficient funds ({} < {raw})",
                i64::from(balance)
            )));
        }
        let new_balance =
            NonNegI64::try_from(i64::from(balance) - raw).map_err(|e| format!("preCharge: {e}"))?;
        self.set_vault_balance(&address, new_balance);
        // The charge is deposited into the staking vault (`Pos.rhox:397-404`:
        // `deposit!(deployerId, amount, posVaultAddr)`), which is where an epoch's reward pot comes
        // from. The charge is the deploy's *maximum* phlo; what the deploy does not consume comes
        // back out in [`Self::refund`], so what the vault keeps is exactly the phlo burned.
        self.credit_pos_vault(amount).await?;
        Ok(Ok(()))
    }

    /// Refund `amount` to `deployer`'s vault out of the staking vault (port of the PoS
    /// `refundDeploy` behavior, `Pos.rhox:417-454`: `posVault!("transfer", deployerRevAddress,
    /// refundAmount, posAuthKey)`).
    ///
    /// `amount <= 0` succeeds without moving anything — the contract's own `if (refundAmount > 0)`
    /// branch. The Scala takes the deployer from the `currentDeployerData` cell that `chargeDeploy`
    /// filled, because the contract's `refundDeploy` is called with only the amount; this port has no
    /// such limitation and carries the deployer in the system deploy itself (`Refund { deployer,
    /// amount }` in `casper::system_deploy`), the same way the pre-charge already does — so the payer
    /// is in the type rather than in a mutable cell.
    ///
    /// This used to be a **documented no-op** (`refund_is_a_documented_no_op`): the charged phlo was
    /// burned rather than returned. Now that the staking vault exists it is the transfer the contract
    /// describes, and the test pins the two-way movement — including that the over-charge does *not*
    /// reach the reward pot.
    pub async fn refund(
        &self,
        deployer: &PublicKey,
        amount: NonNegI64,
    ) -> Result<Result<(), String>, String> {
        if amount == NonNegI64::zero() {
            return Ok(Ok(()));
        }
        let address = RevAddress::from_public_key(deployer)
            .ok_or_else(|| "refund: invalid deployer public key".to_string())?
            .to_base58();
        self.debit_pos_vault(amount).await?;
        let balance = self
            .vault_balance(&address)
            .await?
            .unwrap_or(NonNegI64::zero());
        self.set_vault_balance(
            &address,
            balance_plus(balance, i64::from(amount), "refund")?,
        );
        Ok(Ok(()))
    }

    /// **Pay the block's producer for the work it did** (B2, #150): `burned * executor_share / 10000`
    /// leaves the staking vault and arrives in the producer's own REV vault.
    ///
    /// **Why this is a payment and not a new pot.** The deploy's phlo was charged *into* the staking
    /// vault and the unconsumed part refunded *out* of it, so what the vault keeps for a deploy is
    /// exactly what that deploy burned — and that is what an epoch's reward pot is made of. Paying the
    /// producer a share of it moves money between two parties who are both already paid out of the
    /// same vault, so the total paid out is unchanged: the pie is the same, and this decides **who**
    /// gets a slice of it. Nothing is minted, nothing is destroyed, and the conservation law the
    /// epoch's own transitions carry holds here too.
    ///
    /// **The producer is the block's own signed `sender`**, which is the only production signal the
    /// protocol can read without new state. It is set before the block's deploys run and is identical
    /// on play and replay, so the payment is deterministic; it needs no participation score, and no
    /// third party can steer it — the address is the one that signed the block.
    ///
    /// `burned` is the deploy's *whole* burned amount and the share is applied **here**, from this
    /// node's own `PosParams`, so the number lives in consensus state rather than in each caller.
    /// A short vault is the platform error [`Self::debit_pos_vault`] describes, and it cannot arise
    /// here: the payment is a fraction of an amount this vault just retained.
    pub async fn pay_executor(
        &self,
        executor: &PublicKey,
        burned: NonNegI64,
    ) -> Result<Result<(), String>, String> {
        let share = i64::from(self.params().await?.executor_share);
        // `i128`, because the product of an `i64`-bounded amount and a `i64`-bounded share is not an
        // `i64` — and a wrapped product here would pay the producer a *different* amount than the one
        // the share names, silently, on a consensus path.
        let payment = i128::from(i64::from(burned)) * i128::from(share) / 10000;
        let payment = checked_i64(payment, "executor payment")?;
        let payment = NonNegI64::try_from(payment).map_err(|e| format!("payExecutor: {e}"))?;
        if payment == NonNegI64::zero() {
            return Ok(Ok(()));
        }
        let address = RevAddress::from_public_key(executor)
            .ok_or_else(|| "payExecutor: invalid executor public key".to_string())?
            .to_base58();
        self.debit_pos_vault(payment).await?;
        let balance = self
            .vault_balance(&address)
            .await?
            .unwrap_or(NonNegI64::zero());
        self.set_vault_balance(
            &address,
            balance_plus(balance, i64::from(payment), "payExecutor")?,
        );
        Ok(Ok(()))
    }

    /// The REV address (base58) of a validator's public key.
    fn vault_address(&self, validator: &Validator) -> Result<String, String> {
        let pk = PublicKey::new(validator.as_bytes().to_vec());
        RevAddress::from_public_key(&pk)
            .map(|a| a.to_base58())
            .ok_or_else(|| "invalid validator public key".to_string())
    }
}

#[cfg(test)]
mod tests {
    /// A `NonNegI64` for test amounts. The charge path takes the refinement since AUDIT C109, so a
    /// test that wants an amount spells it as one — which is the point of the change: there is no way
    /// to write the negative that used to mint.
    fn nn(v: i64) -> NonNegI64 {
        NonNegI64::try_from(v).expect("a test amount is non-negative")
    }

    use super::*;

    fn validator(byte: u8) -> Validator {
        Validator::from_slice(&[byte; 65])
    }

    /// The **fringe** state hash a test's `close_block` is anchored to.
    ///
    /// It only has to be deterministic — play and replay must derive the same seed from it — and
    /// distinct per boundary where a test crosses two; no test here asserts a particular draw from it
    /// except the known-answer one, which spells its own.
    fn fringe_state(byte: u8) -> Blake2b256Hash {
        Blake2b256Hash::create(&[byte])
    }

    fn vault_address_of(v: &Validator) -> String {
        RevAddress::from_public_key(&PublicKey::new(v.as_bytes().to_vec()))
            .unwrap()
            .to_base58()
    }

    async fn native_with(
        trusted: &[Validator],
        params: PosParams,
        pool: &[(Validator, i64)],
    ) -> NativeSystemState {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let bonds: BTreeMap<Validator, NonNegI64> = pool
            .iter()
            .map(|(v, s)| (*v, NonNegI64::try_from(*s).unwrap()))
            .collect();
        native
            .install_genesis(&PosGenesis {
                bonds,
                trusted: trusted.iter().copied().collect(),
                params,
                dev_vesting: None,
            })
            .unwrap();
        for (v, _) in pool {
            native.set_vault_balance(&vault_address_of(v), NonNegI64::zero());
        }
        native
    }

    async fn fund(native: &NativeSystemState, v: &Validator, amount: i64) {
        let address = vault_address_of(v);
        let balance = native
            .vault_balance(&address)
            .await
            .unwrap()
            .map(i64::from)
            .unwrap_or(0);
        native.set_vault_balance(&address, NonNegI64::try_from(balance + amount).unwrap());
    }

    /// The total REV the native state holds: the addresses given, the Coop vault, and the staking
    /// vault. Every step of the mechanism — bond, slash, phlo charge, refund, withdrawal payment —
    /// is a transfer *between* these places, so this total is invariant. Checking it is what makes
    /// "the staking vault is the only source of every payout" a property rather than a claim about
    /// the code: the version before this one credited the Coop vault on a slash without debiting
    /// anywhere, and this is the assertion that would have caught it.
    async fn total_rev(native: &NativeSystemState, addresses: &[String]) -> i64 {
        let mut total = i64::from(native.coop_balance().await.unwrap())
            + i64::from(native.pos_vault_balance().await.unwrap());
        for address in addresses {
            total += native
                .vault_balance(address)
                .await
                .unwrap()
                .map(i64::from)
                .unwrap_or(0);
        }
        total
    }

    /// **The conservation invariant behind AUDIT C109, and why it had to be built rather than
    /// asserted.**
    ///
    /// `total_rev` above sums a list the *test* supplies. That is right for the transfers those tests
    /// make between named accounts, and it is **blind by construction** to a credit at an address
    /// nobody named — which is exactly what a mint is. So this test never enumerates the address it
    /// checks: it compares [`NativeSystemState::total_value`] before and after, and an address that
    /// appears out of nowhere moves the total.
    #[tokio::test]
    async fn total_value_sees_a_balance_at_an_address_no_test_named() {
        let native =
            native_with(&[validator(1)], PosParams::default(), &[(validator(1), 10)]).await;
        let before = native.total_value().await.unwrap();

        // An address this test never mentions, credited straight into the store — from the
        // invariant's point of view, exactly the shape a credit to an unlisted account has.
        let ghost = "1111111111111111111111111111111111111111111111111111";
        native.store.put(
            PREFIX_VAULT,
            vault_key(ghost),
            500i64.to_le_bytes().to_vec(),
        );

        assert_eq!(
            native.total_value().await.unwrap(),
            before + 500,
            "a credit at an address nobody named must move the total. This is the assertion \
             `total_rev` could not make, and its absence is why the negative-phlo_limit mint \
             survived: the deployer's vault went up while every check looked at names it had listed"
        );
    }

    /// Every operation that moves value is a **transfer**, so the total is invariant across all of
    /// them: fund, bond, charge, refund, untrust, slash (AUDIT C109, C111).
    ///
    /// Each step asserts rather than only the endpoints, because a defect that both mints and burns
    /// in equal measure nets to zero and would pass an end-to-end check — it is the step that did it
    /// which has to be caught.
    #[tokio::test]
    async fn every_value_moving_operation_conserves_the_total() {
        let native = native_with(
            &[validator(1), validator(2)],
            PosParams::default(),
            &[(validator(1), 10)],
        )
        .await;
        let v2 = validator(2);
        fund(&native, &v2, 100).await;

        // The deployer whose vault will be charged, funded through the store.
        let deployer = PublicKey::new(vec![7u8; 65]);
        let payer = RevAddress::from_public_key(&deployer).unwrap().to_base58();
        native.set_vault_balance(&payer, NonNegI64::try_from(60).unwrap());

        let total = native.total_value().await.unwrap();
        macro_rules! conserved {
            ($what:expr) => {{
                // No `total = after`: the assertion below requires `after == total`, so the
                // reassignment was a no-op — and on this macro's last expansion `clippy::
                // unused_assignments` reads it as dead, which is what removed it. The total is the
                // *same* quantity throughout, and that is the whole content of the test.
                let after = native.total_value().await.unwrap();
                assert_eq!(
                    after, total,
                    "{} moved value — every one of these is a transfer, so the total is invariant",
                    $what
                );
            }};
        }

        native
            .bond(&v2, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();
        conserved!("bond");

        native
            .pre_charge(&deployer, NonNegI64::try_from(25).unwrap())
            .await
            .unwrap()
            .unwrap();
        conserved!("pre_charge");

        native
            .refund(&deployer, NonNegI64::try_from(10).unwrap())
            .await
            .unwrap()
            .unwrap();
        conserved!("refund");

        native
            .fund_pot(&v2, NonNegI64::try_from(5).unwrap())
            .await
            .unwrap()
            .unwrap();
        conserved!("fund_pot");

        // Revoking trust must move **nothing**: it used to confiscate the target's bond (C111).
        native.untrust(&validator(1), &v2).await.unwrap().unwrap();
        conserved!("untrust");

        // A slash leaves the staking vault for the Coop vault.
        native
            .slash(&v2, SlashSeverity::Malicious)
            .await
            .unwrap()
            .unwrap();
        conserved!("slash");
    }

    /// **Releasing the developer allocation is a transfer, not a mint.** The allocation sits under
    /// `PREFIX_POS` until it vests; `total_value` must count the unreleased amount, or the release
    /// into the recipient's vault looks like new value. A zero term releases the whole allocation at
    /// the first boundary, which is the strongest form of the case.
    #[tokio::test]
    async fn releasing_the_developer_allocation_conserves_the_total() {
        let params = PosParams {
            dev_tax_vesting_blocks: NonNegI64::zero(),
            ..PosParams::default()
        };
        let native = native_with(&[validator(1)], params.clone(), &[(validator(1), 10)]).await;
        let recipient = RevAddress::from_public_key(&PublicKey::new(vec![9u8; 65]))
            .unwrap()
            .to_base58();
        let allocation = NonNegI64::try_from(1_000).unwrap();
        native.set_dev_vesting(Some(&DevVesting {
            address: recipient.clone(),
            allocation,
            unreleased: allocation,
        }));

        let total = native.total_value().await.unwrap();
        native.release_dev_tax(1, &params).await.unwrap();

        assert_eq!(
            native.total_value().await.unwrap(),
            total,
            "releasing the allocation moved value — it is a transfer from the vesting account"
        );
        let leaf = native
            .dev_vesting()
            .await
            .unwrap()
            .expect("the vesting leaf remains after a full release");
        assert_eq!(leaf.unreleased, NonNegI64::zero());
        assert_eq!(
            native.vault_balance(&recipient).await.unwrap(),
            Some(allocation),
            "the released amount is in the recipient's ordinary vault"
        );
    }

    #[test]
    fn bonds_round_trip() {
        let mut bonds = BTreeMap::new();
        bonds.insert(validator(2), NonNegI64::try_from(20).unwrap());
        bonds.insert(validator(1), NonNegI64::try_from(10).unwrap());
        let encoded = encode_bonds(&bonds);
        assert_eq!(decode_bonds(&encoded).unwrap(), bonds);
    }

    /// A liveness config is all-or-nothing: a half-configured rule is refused at decode and at
    /// genesis, not clamped into a rule nobody chose.
    #[test]
    fn an_inconsistent_liveness_config_is_refused() {
        let base = PosParams {
            epoch_length: 10,
            quarantine_length: 50,
            liveness_absence_threshold: 10,
            liveness_jail_after_boundaries: 2,
            liveness_jail_epochs: 5,
            liveness_vote_window: 3,
            liveness_slash_severity: 2,
            ..PosParams::default()
        };
        base.validate().expect("a coherent config");
        let mut one_lapse = base;
        one_lapse.liveness_jail_after_boundaries = 1;
        assert!(
            one_lapse.validate().is_err(),
            "one lapse is not sustained absence"
        );
        let mut long_vote = base;
        long_vote.liveness_vote_window = 6;
        assert!(
            long_vote.validate().is_err(),
            "a vote cannot outlive the jail"
        );
        let mut no_severity = base;
        no_severity.liveness_slash_severity = 0;
        assert!(no_severity.validate().is_err(), "voting needs a severity");
        let mut long_escrow = base;
        long_escrow.quarantine_length = 20;
        assert!(
            long_escrow.validate().is_err(),
            "a window longer than the withdrawal quarantine could outlive the bond"
        );
    }

    #[test]
    fn decode_bonds_rejects_trailing_bytes() {
        assert!(decode_bonds(&[0u8; 3]).is_err());
    }

    #[test]
    fn trusted_round_trip() {
        let trusted: BTreeSet<Validator> = [validator(1), validator(2)].into_iter().collect();
        assert_eq!(decode_trusted(&encode_trusted(&trusted)).unwrap(), trusted);
    }

    /// Pin the wire slots independently of the codec: changing both sides together must still fail.
    #[test]
    fn params_wire_slots_are_append_only() {
        let params = PosParams {
            minimum_bond: NonNegI64::try_from(1).unwrap(),
            maximum_bond: NonNegI64::try_from(100).unwrap(),
            epoch_length: 11,
            quarantine_length: 50,
            number_of_active_validators: 3,
            executor_share: NonNegI64::try_from(2500).unwrap(),
            absence_slack: NonNegI64::try_from(7).unwrap(),
            participation_grace: NonNegI64::try_from(13).unwrap(),
            min_phlo_price: NonNegI64::try_from(19).unwrap(),
            max_number_of_parents: 255,
            dev_tax_vesting_blocks: NonNegI64::try_from(23).unwrap(),
            liveness_absence_threshold: 0,
            liveness_jail_after_boundaries: 0,
            liveness_jail_epochs: 0,
            liveness_vote_window: 0,
            liveness_slash_severity: 0,
        };
        let wire: Vec<u8> = [1i64, 100, 11, 50, 3, 2500, 7, 13, 19, 255, 23]
            .into_iter()
            .flat_map(i64::to_le_bytes)
            .collect();
        let mut tagged = vec![2];
        tagged.extend_from_slice(&wire);
        assert_eq!(PARAMS_FORMAT, 3);
        assert_eq!(PARAMS_FORMAT_LEGACY, 2);
        assert_eq!(PARAMS_FIELDS, 16);
        assert_eq!(PARAMS_LEN, 129);
        assert_eq!(PARAMS_LEN_V2, 89);
        assert_eq!(PARAMS_LEN_WITH_GRACE, 64);
        // **A zeroed liveness config is the format-2 record, byte-for-byte.**
        assert!(!params.liveness_configured());
        assert_eq!(params.encode(), tagged);
        assert_eq!(decode_params(&tagged).unwrap(), params);

        let enabled = PosParams {
            liveness_absence_threshold: 11,
            liveness_jail_after_boundaries: 2,
            liveness_jail_epochs: 5,
            liveness_vote_window: 3,
            liveness_slash_severity: 2,
            ..params
        };
        enabled.validate().expect("a coherent liveness config");
        let tagged_v3 = enabled.encode();
        assert_eq!(tagged_v3[0], PARAMS_FORMAT);
        assert_eq!(tagged_v3.len(), PARAMS_LEN);
        assert_eq!(decode_params(&tagged_v3).unwrap(), enabled);
        assert!(decode_params(&tagged_v3[..tagged_v3.len() - 1]).is_err());

        // These are prefixes of a fixed wire vector, not of the encoder under test.
        for fields in 5..=8 {
            let decoded = decode_params(&wire[..fields * 8]).unwrap();
            let mut expected = PosParams {
                min_phlo_price: NonNegI64::zero(),
                max_number_of_parents: 0,
                dev_tax_vesting_blocks: NonNegI64::zero(),
                ..params
            };
            if fields < 6 {
                expected.executor_share = NonNegI64::zero();
            }
            if fields < 7 {
                expected.absence_slack = NonNegI64::zero();
            }
            if fields < 8 {
                expected.participation_grace = expected.absence_slack;
            }
            assert_eq!(decoded, expected, "{fields}-field record");
        }
        for fields in 5..11 {
            assert!(
                decode_params(&tagged[..1 + fields * 8]).is_err(),
                "format 2 requires exactly eleven fields, not {fields}"
            );
        }
        for fields in 9..=11 {
            assert!(
                decode_params(&wire[..fields * 8]).is_err(),
                "legacy encodings stop at eight fields, not {fields}"
            );
        }
        let defaults = PosParams::default();
        assert_eq!(defaults.min_phlo_price, NonNegI64::zero());
        assert_eq!(defaults.max_number_of_parents, 0);
        assert_eq!(defaults.dev_tax_vesting_blocks, NonNegI64::zero());
        assert_eq!(decode_params(&defaults.encode()).unwrap(), defaults);
    }

    /// **Every field carries a distinct, non-zero value, and that is the point of the fixture.**
    ///
    /// The record is positional and an absent field decodes to `0`, and for the fields added later the
    /// *pre-change rule is `0`* — so a fixture that sets them to zero round-trips green even if `encode`
    /// and `decode` disagree about their **order**. That is exactly the shape a bad merge of two
    /// branches that each append to `PosParams` produces: no error, no red test, just a genesis
    /// parameter holding the neighbouring field's number. Distinct values make a transposition fail
    /// here instead.
    ///
    /// The length assertion is the other half: `PARAMS_LEN` decides how many fields `decode` reads, so
    /// a fixture that passes while the constant and the encoder disagree is a second silent failure.
    #[test]
    fn params_round_trip() {
        let params = PosParams {
            minimum_bond: NonNegI64::try_from(1).unwrap(),
            maximum_bond: NonNegI64::try_from(1000).unwrap(),
            epoch_length: 10,
            quarantine_length: 50,
            number_of_active_validators: 3,
            executor_share: NonNegI64::try_from(2500).unwrap(),
            absence_slack: NonNegI64::try_from(7).unwrap(),
            participation_grace: NonNegI64::try_from(13).unwrap(),
            min_phlo_price: NonNegI64::try_from(19).unwrap(),
            max_number_of_parents: 255,
            dev_tax_vesting_blocks: NonNegI64::try_from(11).unwrap(),
            liveness_absence_threshold: 17,
            liveness_jail_after_boundaries: 3,
            liveness_jail_epochs: 9,
            liveness_vote_window: 4,
            liveness_slash_severity: 1,
        };
        assert_eq!(
            params.encode().len(),
            PARAMS_LEN,
            "the encoder writes exactly the fields the decoder reads"
        );
        assert_eq!(decode_params(&params.encode()).unwrap(), params);
    }

    /// **A shorter record is a chain that predates the fields it does not carry, and each missing
    /// field reads as the rule of that chain** (B2 and B4, #150) — the same compatibility rule the
    /// slash tiers use (`SlashSeverity::Unspecified` is code `0`, the pre-tier rule). The alternative,
    /// refusing a short record, would make a node that upgrades unable to read its own genesis params
    /// leaf.
    ///
    /// Both new fields are *off* at zero and both zeros are the older behaviour: no producer's share,
    /// and no absence rule. And the *upper* bound on the share is refused rather than clamped: a share
    /// above 10 000 basis points would pay the producer more than the deploy burned, out of the same
    /// vault other validators' stake sits in — a reward funded by everyone else's bond.
    #[test]
    fn a_short_params_record_reads_the_pre_change_rule_and_an_over_share_is_refused() {
        let current = PosParams {
            minimum_bond: NonNegI64::try_from(1).unwrap(),
            maximum_bond: NonNegI64::try_from(1000).unwrap(),
            epoch_length: 10,
            quarantine_length: 5,
            number_of_active_validators: 3,
            executor_share: NonNegI64::try_from(2500).unwrap(),
            absence_slack: NonNegI64::try_from(7).unwrap(),
            participation_grace: NonNegI64::try_from(3).unwrap(),
            min_phlo_price: NonNegI64::try_from(1).unwrap(),
            max_number_of_parents: 255,
            dev_tax_vesting_blocks: NonNegI64::zero(),
            liveness_absence_threshold: 0,
            liveness_jail_after_boundaries: 0,
            liveness_jail_epochs: 0,
            liveness_vote_window: 0,
            liveness_slash_severity: 0,
        };
        // The unversioned forms every earlier chain wrote: the current fields, without the format byte,
        // cut at each length that ever shipped.
        let fields = current.encode()[1..].to_vec();
        let original = &fields[..PARAMS_LEN_ORIGINAL];
        assert_eq!(
            decode_params(original).unwrap(),
            PosParams {
                executor_share: NonNegI64::zero(),
                absence_slack: NonNegI64::zero(),
                participation_grace: NonNegI64::zero(),
                min_phlo_price: NonNegI64::zero(),
                max_number_of_parents: 0,
                ..current.clone()
            },
            "the five-field record is every pre-change rule at once"
        );
        // And the intermediate form — the producer's share without the absence slack — reads the share
        // it carries and defaults only what it lacks.
        let share_only = &fields[..PARAMS_LEN_ORIGINAL + 8];
        assert_eq!(
            decode_params(share_only).unwrap(),
            PosParams {
                absence_slack: NonNegI64::zero(),
                participation_grace: NonNegI64::zero(),
                min_phlo_price: NonNegI64::zero(),
                max_number_of_parents: 0,
                ..current.clone()
            },
            "the six-field record carries the share it has and defaults the slack it lacks"
        );
        // **The grace is the one field whose absence is not zero, and this is the arm that says so.**
        // A record carrying the knee but not the grace describes a chain running the *binary* rule,
        // which is `grace == knee`; defaulting to zero would silently ramp every armed chain on upgrade.
        let knee_only = &fields[..PARAMS_LEN_ORIGINAL + 16];
        assert_eq!(
            decode_params(knee_only).unwrap(),
            PosParams {
                participation_grace: NonNegI64::try_from(7).unwrap(),
                min_phlo_price: NonNegI64::zero(),
                max_number_of_parents: 0,
                ..current.clone()        },
            "a record without the grace reads the binary rule — the knee — and not a ramp from zero"
        );
        // **The eight-field record is every chain built before the fee floor and the parent bound were
        // committed.** It keeps its grace — the grace is the eighth field, so it is carried — and reads
        // both committed bounds as "not committed", which is the rule that chain was running: neither
        // bound was part of its state.
        let pre_commit = &fields[..PARAMS_LEN_WITH_GRACE];
        assert_eq!(
            decode_params(pre_commit).unwrap(),
            PosParams {
                min_phlo_price: NonNegI64::zero(),
                max_number_of_parents: 0,
                ..current.clone()
            },
            "an eight-field record keeps its grace and commits neither bound"
        );
        let mut negative_floor = current.encode();
        negative_floor[65..73].copy_from_slice(&(-1i64).to_le_bytes());
        let err = decode_params(&negative_floor).expect_err("a negative fee floor is not a floor");
        assert!(err.contains("min_phlo_price"), "{err}");

        // A record that is not a whole number of fields, or shorter than the original, is refused: the
        // tolerance is "a field the record does not carry", not "any byte string".
        for len in [0usize, 8, 39, 41, 47, 57, 72, 80, 81, 88] {
            assert!(
                decode_params(&vec![0u8; len]).is_err(),
                "{len} bytes is not a params record this protocol has ever written"
            );
        }

        let mut over = current.encode();
        over[41..49].copy_from_slice(&10_001i64.to_le_bytes());
        let err = decode_params(&over).expect_err("a share above the whole is not a share");
        assert!(err.contains("executor_share"), "{err}");
    }

    /// **A record in a format this node does not know is refused, not read positionally.** This is
    /// the guard against two branches appending fields in different orders: the record that would have
    /// decoded to transposed values under the length rule now carries a format number, and a number
    /// the decoder was not built for fails here, at the right length as at the wrong one.
    #[test]
    fn an_unknown_params_format_is_refused() {
        let mut next_format = PosParams::default().encode();
        next_format[0] = PARAMS_FORMAT + 1;
        let err = decode_params(&next_format).expect_err("an unknown format is not read");
        assert!(err.contains("format"), "{err}");

        let mut longer = PosParams::default().encode();
        longer.extend_from_slice(&[0u8; 8]);
        let err = decode_params(&longer).expect_err("format 2 has exactly eleven fields");
        assert!(err.contains("bytes"), "{err}");

        // Format 1 is a known-shape record from the unmerged committed-bounds branch. It has no
        // vesting term and is refused rather than read positionally with an absent field defaulted to
        // zero; no released chain ever wrote one.
        let mut format_one = PosParams::default().encode();
        format_one.truncate(1 + 10 * 8);
        format_one[0] = 1;
        let err =
            decode_params(&format_one).expect_err("format 1 is refused, not read positionally");
        assert!(err.contains("format"), "{err}");
    }

    /// **Deferred item 1d, and the shape of the hole it closed.** The wire record held the two bond
    /// bounds as raw `i64`s, so a stored `-1` was representable — and a *negative* minimum accepts
    /// every bond, including the zero-stake one `bond`'s range check exists to refuse. The bounds are
    /// `NonNegI64` now, and this is the boundary that makes that true for values that arrive as
    /// bytes: a negative one is refused, not clamped, because clamping a corrupted leaf to zero turns
    /// it into "no minimum", which is the permissive reading of the same bytes.
    ///
    /// Each field is exercised on its own, so this is a test of two checks rather than of one that
    /// happens to run first, and the control is the untouched record — a refusal function that
    /// refused everything would satisfy the other two assertions.
    #[test]
    fn a_negative_bond_bound_is_refused_at_the_wire() {
        let params = PosParams {
            minimum_bond: NonNegI64::try_from(7).unwrap(),
            maximum_bond: NonNegI64::try_from(1000).unwrap(),
            epoch_length: 10,
            quarantine_length: 5,
            number_of_active_validators: 3,
            executor_share: NonNegI64::zero(),
            absence_slack: NonNegI64::zero(),
            participation_grace: NonNegI64::zero(),
            min_phlo_price: NonNegI64::zero(),
            max_number_of_parents: 0,
            dev_tax_vesting_blocks: NonNegI64::zero(),
            liveness_absence_threshold: 0,
            liveness_jail_after_boundaries: 0,
            liveness_jail_epochs: 0,
            liveness_vote_window: 0,
            liveness_slash_severity: 0,
        };
        assert!(
            decode_params(&params.encode()).is_ok(),
            "the control: an in-range record still decodes"
        );

        let mut bytes = params.encode();
        bytes[1..9].copy_from_slice(&(-1i64).to_le_bytes());
        let err = decode_params(&bytes).expect_err("a negative minimum must not decode");
        assert!(
            err.contains("minimum_bond"),
            "and the refusal names the field that was wrong: {err}"
        );

        let mut bytes = params.encode();
        bytes[9..17].copy_from_slice(&(-1i64).to_le_bytes());
        let err = decode_params(&bytes).expect_err("a negative maximum must not decode");
        assert!(
            err.contains("maximum_bond"),
            "and the refusal names the field that was wrong: {err}"
        );
    }

    #[test]
    fn select_active_draws_a_deterministic_subset_of_the_cap() {
        // **This test used to assert `vec![validator(2), validator(3)]` — the top two by stake.**
        // That rule is gone; which validators are active is now a draw, so pinning a specific set here
        // would only pin the seed. What is pinned instead is the shape the rule guarantees for *any*
        // seed, and the exact set is pinned once, by the known-answer vector below.
        let pool = pool_of(&[(1, 10), (2, 30), (3, 20)]);
        let params = PosParams {
            number_of_active_validators: 2,
            ..PosParams::default()
        };
        let seed = genesis_epoch_seed();

        let first = select_active(&pool, &BTreeMap::<Validator, ()>::new(), &params, &seed);
        let again = select_active(&pool, &BTreeMap::<Validator, ()>::new(), &params, &seed);

        assert_eq!(
            first, again,
            "the same pool and seed must draw the same set"
        );
        assert_eq!(first.len(), 2, "the cap is respected");
        for (v, stake) in &first {
            assert_eq!(
                pool.get(v),
                Some(stake),
                "a drawn validator carries its pool stake, not a synthesised one"
            );
        }
    }

    /// **Distinct seeds must be able to draw distinct sets.** This is the property the old rule
    /// lacked in kind: it had exactly one answer for a given pool, so nothing could ever rotate. A
    /// draw that returned the same set for every seed would pass every other test here.
    #[test]
    fn distinct_seeds_draw_distinct_sets() {
        let pool = pool_of(&[(1, 10), (2, 20), (3, 30), (4, 40), (5, 50), (6, 60)]);
        let params = PosParams {
            number_of_active_validators: 3,
            ..PosParams::default()
        };
        let mut seen: BTreeSet<Vec<Validator>> = BTreeSet::new();
        for epoch in 0..64i64 {
            let seed = EpochSeed {
                epoch,
                anchors: vec![Blake2b256Hash::create(&epoch.to_le_bytes())],
            };
            seen.insert(
                select_active(&pool, &BTreeMap::<Validator, ()>::new(), &params, &seed)
                    .keys()
                    .copied()
                    .collect(),
            );
        }
        assert!(
            seen.len() > 1,
            "64 seeds produced one set: this is not a draw"
        );
    }

    /// **A zero cap means unlimited, and this is a regression test by name.** The first draft mapped
    /// it to `cap = 0` and drew *nothing*, so a genesis-installed node came up with an empty active
    /// set and no validator could propose. The old truncate-when-positive rule could not have had
    /// this bug, which is what makes it the shape a draw introduces.
    #[test]
    fn a_zero_cap_returns_the_whole_pool() {
        let pool = pool_of(&[(1, 10), (2, 20), (3, 30)]);
        let params = PosParams {
            number_of_active_validators: 0,
            ..PosParams::default()
        };
        let active = select_active(
            &pool,
            &BTreeMap::<Validator, ()>::new(),
            &params,
            &genesis_epoch_seed(),
        );
        assert_eq!(active.len(), 3, "zero means unlimited, not 'draw none'");
    }

    /// **A known-answer vector: this pool, this seed, exactly this set.**
    ///
    /// Every other test here asserts a *property*, so a refactor that changed the draw would satisfy
    /// all of them — only a full chain replay would notice. This pins the output itself, in the style
    /// of `Blake2b512Random`'s own vectors.
    ///
    /// **The vector moved on 2026-10-02 and that is the point of it.** It read
    /// `[1, 3, 5]` under the uniform draw. The weighted rule draws a stake-heavier set: `[3, 4, 5]`,
    /// the three largest stakes in the pool, where the uniform rule's set took the smallest. Both are
    /// the same kind of statement, and the movement is the point: a change here is a consensus
    /// change.
    #[test]
    fn the_draw_matches_a_known_answer_vector() {
        let pool = pool_of(&[(1, 10), (2, 20), (3, 30), (4, 40), (5, 50)]);
        let params = PosParams {
            number_of_active_validators: 3,
            ..PosParams::default()
        };
        let active = select_active(
            &pool,
            &BTreeMap::<Validator, ()>::new(),
            &params,
            &genesis_epoch_seed(),
        );
        assert_eq!(
            active.keys().copied().collect::<Vec<_>>(),
            vec![validator(3), validator(4), validator(5)],
            "the draw for this pool and seed; a change here is a consensus change"
        );
    }

    /// **The book's "cap bites" table, re-measured on the weighted rule — and the direction of the
    /// reversal has flipped.**
    ///
    /// `epoch_rewards` pays a pooled validator `bond / active_bonds` of the pot when it is drawn and
    /// zero otherwise, so a staker's expected income is the drawn holdings over the drawn total,
    /// averaged over seeds. The published table (pot 1, six rival keys at stake 10, cap 4) read
    /// **0.326 / 0.400 / 0.528** for one stake of 40 held as one key, four keys of 10 and twenty keys of
    /// 2 — splitting bought 62 %, holding whole was *penalised*, and every key was an independent
    /// lottery ticket. That is the sybil exposure O3 named.
    ///
    /// On the weighted rule the same measurement reads **0.5328 / 0.4000 / 0.1770**. Splitting a stake
    /// into twenty keys now **loses** 56 % of the fair share instead of gaining 32 %, so the per-key
    /// lever is gone — and the reason the four-equal-keys case sits exactly at the pro-rata 0.4 is that
    /// it *is* the rivals' configuration, which is the cleanest statement of proportionality available.
    ///
    /// **The concentration side of this is real and is stated rather than hidden.** Above the cap, a
    /// single large key earns ~33 % *more* than the flat pro-rata share, because the drawn set is capped
    /// at four and a large key both draws more often and crowds the denominator when it does. The
    /// uniform rule had the same magnitude pointing the other way. Neither is "pro-rata"; the cap is
    /// what breaks proportionality, and the two rules differ only in which side of it a staker is on.
    /// Weighting is the side chosen, for two reasons: a per-key income rule *mints finality weight for
    /// free* (a consensus-safety problem, not a preference), and a large bond is already the thing the
    /// cap exists to bound — a bond is bounded above by `maximum_bond`, and the operator carries the
    /// concentration risk itself, since everything at risk is per-validator. The seeds are fixed, so
    /// these are pins rather than samples.
    #[test]
    fn the_cap_regime_no_longer_rewards_a_split_stake() {
        let params = PosParams {
            number_of_active_validators: 4,
            ..PosParams::default()
        };
        let seeds = 4096i64;
        // Six rivals of stake 10, plus the holder's 40 however it is held: every configuration has the
        // same total weight, so the same pool total and the same denominator distribution.
        let measure = |held: &[i64]| -> f64 {
            let mut pool = pool_of(&[(1, 10), (2, 10), (3, 10), (4, 10), (5, 10), (6, 10)]);
            let mut held_keys = BTreeSet::new();
            for (index, stake) in held.iter().enumerate() {
                let address = u8::try_from(100 + index).expect("a test key");
                let key = validator(address);
                pool.insert(key, NonNegI64::try_from(*stake).expect("a test stake"));
                held_keys.insert(key);
            }
            let mut total = 0.0;
            for epoch in 0..seeds {
                let seed = EpochSeed {
                    epoch,
                    anchors: vec![Blake2b256Hash::create(&epoch.to_le_bytes())],
                };
                let active =
                    select_active(&pool, &BTreeMap::<Validator, ()>::new(), &params, &seed);
                let drawn_total: i64 = active.values().map(|s| i64::from(*s)).sum();
                let drawn_holdings: i64 = active
                    .iter()
                    .filter(|(v, _)| held_keys.contains(v))
                    .map(|(_, s)| i64::from(*s))
                    .sum();
                if drawn_total > 0 {
                    total += drawn_holdings as f64 / drawn_total as f64;
                }
            }
            total / seeds as f64
        };

        let whole = measure(&[40]);
        let four = measure(&[10, 10, 10, 10]);
        let twenty = measure(&[2; 20]);
        for (label, value, expected) in [
            ("one key of 40", whole, 0.5328),
            ("four keys of 10", four, 0.4000),
            ("twenty keys of 2", twenty, 0.1770),
        ] {
            assert!(
                (value - expected).abs() < 0.005,
                "{label} earned {value}, not {expected}: the weights moved, so the rule moved"
            );
        }
        assert!(
            whole > four && four > twenty,
            "income must not fall as a stake is held in fewer keys — the uniform rule had it the other \
             way round, which is the defect; got {whole}, {four}, {twenty}"
        );
        assert!(
            (four - 0.4).abs() < 0.005,
            "a staker that looks exactly like its rivals must earn exactly the pro-rata share; got {four}"
        );
    }

    /// How often, over `seeds` epochs, the single slot of a one-member draw goes to `target`.
    ///
    /// A *measurement*, in the register's sense: the drawn set is a deterministic function of the seed,
    /// so "in proportion to stake" is a property of the distribution over seeds rather than of one
    /// call, and the only way to state it is to draw many. The seeds are fixed constants, so the
    /// measurement is reproducible.
    fn slot_share(pool: &BTreeMap<Validator, NonNegI64>, target: &Validator, seeds: i64) -> f64 {
        let params = PosParams {
            number_of_active_validators: 1,
            ..PosParams::default()
        };
        let hits = (0..seeds)
            .filter(|epoch| {
                let seed = EpochSeed {
                    epoch: *epoch,
                    anchors: vec![Blake2b256Hash::create(&epoch.to_le_bytes())],
                };
                select_active(pool, &BTreeMap::<Validator, ()>::new(), &params, &seed)
                    .contains_key(target)
            })
            .count();
        hits as f64 / seeds as f64
    }

    /// **The B1 falsifier: the one slot is awarded in proportion to stake, not per key.**
    ///
    /// Three parts of stake against one. Under the uniform rule this was `0.5` — the ratio of *keys* —
    /// and that is the whole defect: a staker could split its stake across keys and buy slots, because
    /// each key was a ticket regardless of what it was worth. Under the weighted rule it is `0.75`,
    /// the ratio of stake.
    ///
    /// The tolerance is four standard deviations of the binomial at 512 trials, so this is a
    /// measurement of the rule rather than a flake: a correct rule misses the band about once in
    /// sixteen thousand runs, and restoring the uniform draw moves the value to `0.5`, far outside it.
    #[test]
    fn the_first_slot_is_awarded_in_proportion_to_stake() {
        let pool = pool_of(&[(1, 3), (2, 1)]);
        let share = slot_share(&pool, &validator(1), 512);
        assert!(
            (0.70..=0.80).contains(&share),
            "a three-to-one stake must take the slot about three times in four; got {share}"
        );
    }

    /// **And splitting a stake across keys does not buy slots** — the same measurement, stated the way
    /// a staker would try it.
    ///
    /// The stake is 4 either way. Held as one key against four equal keys it takes about half the
    /// draws; split into four keys of 1 against the same four, the *four keys together* still take
    /// about half — not four fifths, which is what a per-key rule would give them.
    #[test]
    fn splitting_a_stake_across_keys_does_not_buy_slots() {
        let whole = pool_of(&[(1, 4), (2, 1), (3, 1), (4, 1), (5, 1)]);
        let split = pool_of(&[
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 1),
            (6, 1),
            (7, 1),
            (8, 1),
        ]);
        let params = PosParams {
            number_of_active_validators: 1,
            ..PosParams::default()
        };
        let split_group: BTreeSet<Validator> = (1..=4).map(validator).collect();
        let seeds = 512i64;

        let wins = |pool: &BTreeMap<Validator, NonNegI64>, group: &BTreeSet<Validator>| {
            (0..seeds)
                .filter(|epoch| {
                    let seed = EpochSeed {
                        epoch: *epoch,
                        anchors: vec![Blake2b256Hash::create(&epoch.to_le_bytes())],
                    };
                    select_active(pool, &BTreeMap::<Validator, ()>::new(), &params, &seed)
                        .keys()
                        .any(|v| group.contains(v))
                })
                .count() as f64
                / seeds as f64
        };

        let whole_share = wins(&whole, &BTreeSet::from([validator(1)]));
        let split_share = wins(&split, &split_group);
        assert!(
            (0.45..=0.55).contains(&whole_share),
            "half the stake takes about half the draws; got {whole_share}"
        );
        assert!(
            (0.40..=0.60).contains(&split_share),
            "the same half of the stake, split across four keys, must still take about half the \
             draws — a per-key rule gives it four fifths; got {split_share}"
        );
    }

    /// A pool whose bonds are the given `(index, stake)` pairs.
    fn pool_of(bonds: &[(u8, i64)]) -> BTreeMap<Validator, NonNegI64> {
        bonds
            .iter()
            .map(|(i, stake)| {
                (
                    validator(*i),
                    NonNegI64::try_from(*stake).expect("test stake is non-negative"),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn genesis_installs_pool_active_and_trusted() {
        let native = native_with(
            &[validator(1), validator(2)],
            PosParams::default(),
            &[(validator(1), 10), (validator(2), 20)],
        )
        .await;
        assert_eq!(native.bonds().await.unwrap().len(), 2, "pool");
        assert_eq!(native.active().await.unwrap().len(), 2, "active");
        assert_eq!(native.trusted().await.unwrap().len(), 2, "trusted");
        assert_eq!(i64::from(native.coop_balance().await.unwrap()), 0);
    }

    #[tokio::test]
    async fn bond_requires_trust_admission() {
        // v2 is NOT trusted; v1 (trusted) admits it.
        let native =
            native_with(&[validator(1)], PosParams::default(), &[(validator(1), 10)]).await;
        let v2 = validator(2);
        fund(&native, &v2, 100).await;

        let rejected = native
            .bond(&v2, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap();
        assert!(rejected.is_err(), "untrusted observer must not bond");

        native.trust(&validator(1), &v2).await.unwrap().unwrap();
        native
            .bond(&v2, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();
        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(native.active().await.unwrap().contains_key(&v2));
    }

    #[tokio::test]
    async fn trust_requires_a_trusted_caller() {
        let native =
            native_with(&[validator(1)], PosParams::default(), &[(validator(1), 10)]).await;
        let result = native.trust(&validator(9), &validator(2)).await.unwrap();
        assert!(result.is_err(), "a non-stakeholder cannot admit validators");
    }

    #[tokio::test]
    async fn bond_enforces_min_and_max() {
        let params = PosParams {
            minimum_bond: NonNegI64::try_from(10).unwrap(),
            maximum_bond: NonNegI64::try_from(50).unwrap(),
            ..PosParams::default()
        };
        let native =
            native_with(&[validator(1), validator(2)], params, &[(validator(1), 10)]).await;
        let v = validator(2);
        fund(&native, &v, 100).await;

        assert!(native
            .bond(&v, NonNegI64::try_from(5).unwrap(), 0)
            .await
            .unwrap()
            .is_err());
        assert!(native
            .bond(&v, NonNegI64::try_from(51).unwrap(), 0)
            .await
            .unwrap()
            .is_err());
        assert!(native
            .bond(&v, NonNegI64::try_from(20).unwrap(), 0)
            .await
            .unwrap()
            .is_ok());
    }

    /// A bond escrows the stake and joins the pool immediately, and becomes **active at the epoch
    /// boundary** — the contract's `bond` writes only `allBonds` (`Pos.rhox:355`) and
    /// `activeValidators` is recomputed inside `closeBlock` (`:546`). With the permissive default
    /// parameters every block is a boundary, so the boundary is the same block's `close_block`; a
    /// validator active before it would be a consensus set that changes inside an epoch.
    #[tokio::test]
    async fn bond_escrows_the_stake_and_activates_at_the_boundary() {
        let native = native_with(&[validator(1)], PosParams::default(), &[]).await;
        let v = validator(1);
        fund(&native, &v, 100).await;

        native
            .bond(&v, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            i64::from(
                native
                    .vault_balance(&vault_address_of(&v))
                    .await
                    .unwrap()
                    .unwrap()
            ),
            60
        );
        assert_eq!(i64::from(native.bonds().await.unwrap()[&v]), 40, "pooled");
        assert!(
            !native.active().await.unwrap().contains_key(&v),
            "a bonded validator is not in the consensus set until the boundary"
        );

        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(native.active().await.unwrap().contains_key(&v));
    }

    /// **Law 79, the arithmetic**: linear, no cliff, total at the end, and truncating toward the
    /// network so the released amount never runs ahead of the schedule. `duration <= 0` is *no
    /// schedule*, not a zero-length one.
    #[test]
    fn the_vesting_schedule_is_linear_and_never_runs_ahead() {
        assert_eq!(
            dev_tax_vested(1_000, 1_000, 0).unwrap(),
            0,
            "nothing at genesis"
        );
        assert_eq!(dev_tax_vested(1_000, 1_000, 1).unwrap(), 1);
        assert_eq!(dev_tax_vested(1_000, 1_000, 500).unwrap(), 500);
        assert_eq!(dev_tax_vested(1_000, 1_000, 1_000).unwrap(), 1_000);
        assert_eq!(
            dev_tax_vested(1_000, 1_000, 5_000).unwrap(),
            1_000,
            "past the end it is the whole allocation, not more"
        );
        assert_eq!(
            dev_tax_vested(1_000, 0, 7).unwrap(),
            1_000,
            "`0` is no schedule at all: spendable at the first boundary"
        );
        assert_eq!(
            dev_tax_vested(1_000, 1_000, -5).unwrap(),
            0,
            "before genesis, nothing"
        );

        // Truncation, and monotone: at every height the vested part is at most the linear share.
        let (allocation, duration) = (7i64, 3i64);
        let mut last = 0i64;
        for height in 0..=6 {
            let vested = dev_tax_vested(allocation, duration, height).unwrap();
            assert!(vested >= last, "the schedule never goes backwards");
            assert!(vested <= allocation, "and never exceeds the allocation");
            assert!(
                vested * duration <= allocation * height.max(1),
                "never ahead of linear"
            );
            last = vested;
        }
        // 7/3 truncates rather than rounding up: 2 at the sixth of nine, not 3.
        assert_eq!(dev_tax_vested(7, 3, 1).unwrap(), 2);
    }

    /// **Law 79's Rust witness: the allocation vests linearly and releases at boundaries.**
    ///
    /// The schedule is the whole of the deal — the allocation is credited to a *vesting account* at
    /// genesis rather than to a spendable vault, and each boundary moves the newly-vested part into
    /// the recipient's own vault, where nothing else in the chain has to know where it came from.
    /// `epoch_length: 1` makes every block a boundary, so a height *is* a schedule position.
    #[tokio::test]
    async fn the_dev_allocation_vests_linearly_and_releases_at_boundaries() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let recipient = "1111111111111111111111111111111111";
        let allocation = 1_000_000i64;
        native
            .install_genesis(&PosGenesis {
                bonds: BTreeMap::new(),
                trusted: BTreeSet::new(),
                params: PosParams {
                    epoch_length: 1,
                    dev_tax_vesting_blocks: NonNegI64::try_from(1_000).unwrap(),
                    ..PosParams::default()
                },
                dev_vesting: Some((
                    recipient.to_string(),
                    NonNegI64::try_from(allocation).unwrap(),
                )),
            })
            .unwrap();

        async fn balance(native: &NativeSystemState, address: &str) -> i64 {
            i64::from(
                native
                    .vault_balance(address)
                    .await
                    .unwrap()
                    .unwrap_or(NonNegI64::zero()),
            )
        }

        // Genesis is not a release: the whole allocation is unreleased and the vault is empty.
        assert_eq!(
            i64::from(native.dev_vesting().await.unwrap().unwrap().unreleased),
            allocation
        );
        assert_eq!(
            balance(&native, recipient).await,
            0,
            "nothing is spendable at genesis"
        );

        for (height, released) in [
            (0i64, 0i64),
            (100, 100_000),
            (500, 500_000),
            (1_000, 1_000_000),
            (2_000, 1_000_000),
        ] {
            native
                .close_block(height, fringe_state((height % 256) as u8), &BTreeMap::new())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                balance(&native, recipient).await,
                released,
                "at height {height}"
            );
            assert_eq!(
                i64::from(native.dev_vesting().await.unwrap().unwrap().unreleased),
                allocation - released,
                "the account and the vault are two halves of one number, at height {height}"
            );
        }

        // Idempotence: a second boundary at the same height moves nothing, because the account's own
        // numbers are the whole of its state.
        native
            .close_block(2_000, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(balance(&native, recipient).await, allocation);
    }

    /// **A chain with no allocation writes nothing at a boundary.** The release returns before
    /// touching a key, which is what keeps an allocation-free chain's post-state what it was.
    #[tokio::test]
    async fn a_chain_with_no_allocation_releases_nothing() {
        let native = native_with(
            &[validator(1)],
            PosParams::default(),
            &[(validator(1), 100)],
        )
        .await;
        assert_eq!(
            native.dev_vesting().await.unwrap(),
            None,
            "no allocation, no account"
        );
        native
            .close_block(1_000, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(native.dev_vesting().await.unwrap(), None, "and still none");
    }

    #[tokio::test]
    async fn bond_rejects_already_bonded() {
        let native = native_with(&[validator(1)], PosParams::default(), &[]).await;
        let v = validator(1);
        fund(&native, &v, 100).await;

        native
            .bond(&v, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();
        let result = native
            .bond(&v, NonNegI64::try_from(10).unwrap(), 0)
            .await
            .unwrap();
        assert!(result.is_err(), "already bonded must be rejected");
    }

    /// **Law 47 — a withdrawal is staged, not immediate.** The request records a deadline and changes
    /// nothing else: the validator stays bonded and stays in the active set (the contract's
    /// `withdraw` writes only `pendingWithdrawers`, `Pos.rhox:377-382`). It is moved out of the pool
    /// at the *next* epoch boundary and paid at the first boundary at which its quarantine has
    /// elapsed.
    ///
    /// `epoch_length: 1` makes every block a boundary, so the arithmetic is the contract's own:
    /// `quarantineLength + epochLength * (1 + blockNumber / epochLength)` at block 5 with a
    /// quarantine of 10 is `10 + 1 * 6 = 16`.
    #[tokio::test]
    async fn withdraw_stages_the_validator_until_the_next_boundary() {
        let params = PosParams {
            epoch_length: 1,
            quarantine_length: 10,
            ..PosParams::default()
        };
        let native = native_with(&[validator(1)], params, &[(validator(1), 40)]).await;
        let v = validator(1);
        native.set_vault_balance(&vault_address_of(&v), NonNegI64::zero());

        native.withdraw(&v, 5).await.unwrap().unwrap();
        assert_eq!(
            native.pending_withdrawers().await.unwrap()[&v],
            16,
            "the request stages a deadline, and only that"
        );
        assert!(
            native.bonds().await.unwrap().contains_key(&v),
            "the stake stays in the pool until the boundary"
        );
        assert!(
            native.active().await.unwrap().contains_key(&v),
            "and the validator keeps validating — and earning — until then"
        );

        // The boundary moves it out of the pool and into an escrowed claim.
        native
            .close_block(10, fringe_state(10), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(native.pending_withdrawers().await.unwrap().is_empty());
        assert!(
            !native.bonds().await.unwrap().contains_key(&v),
            "withdrawn validator removed from the pool at the boundary"
        );
        assert_eq!(
            native.withdrawers().await.unwrap()[&v],
            Withdrawal {
                bond: NonNegI64::try_from(40).unwrap(),
                deadline: 16,
            }
        );
        assert!(
            !native.active().await.unwrap().contains_key(&v),
            "and out of the active set"
        );
        assert_eq!(
            i64::from(
                native
                    .vault_balance(&vault_address_of(&v))
                    .await
                    .unwrap()
                    .unwrap()
            ),
            0,
            "the stake is still escrowed: block 10 is before the deadline of 16"
        );

        native
            .close_block(16, fringe_state(16), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(native.withdrawers().await.unwrap().is_empty());
        assert_eq!(
            i64::from(
                native
                    .vault_balance(&vault_address_of(&v))
                    .await
                    .unwrap()
                    .unwrap()
            ),
            40,
            "the stake is paid out at the first boundary past the quarantine"
        );
    }

    /// **AUDIT C135.** The withdrawal deadline uses the **normalised** epoch divisor, so
    /// `epoch_length <= 0` — which every other epoch site reads as a one-block epoch — cannot
    /// collapse the quarantine to an absolute constant.
    ///
    /// The regression is exactly that collapse, and it is why the two arms are *compared* rather
    /// than each asserted alone: with the raw field multiplied, `epoch_length = 0` gave
    /// `quarantine_length` while `epoch_length = 1` gave `quarantine_length + 1 + block_number`, so
    /// the quarantine the row is about was silently dropped for one setting and honoured for the
    /// other. The concrete value then pins the formula itself, so "both arms agree" cannot be
    /// satisfied by both being wrong the same way.
    #[tokio::test]
    async fn a_non_positive_epoch_length_keeps_the_withdrawal_quarantine() {
        let v = validator(3);
        let params = |epoch_length: i64| PosParams {
            epoch_length,
            quarantine_length: 100,
            ..PosParams::default()
        };

        let zero = native_with(&[v], params(0), &[(v, 100)]).await;
        zero.withdraw(&v, 10).await.unwrap().unwrap();
        let staged_at_zero = zero.pending_withdrawers().await.unwrap()[&v];

        let one = native_with(&[v], params(1), &[(v, 100)]).await;
        one.withdraw(&v, 10).await.unwrap().unwrap();
        let staged_at_one = one.pending_withdrawers().await.unwrap()[&v];

        assert_eq!(
            staged_at_zero, staged_at_one,
            "a zero epoch length means a one-block epoch at the withdrawal deadline as it does at \
             every other epoch site"
        );
        assert_eq!(
            staged_at_one,
            100 + 1 + 10,
            "the deadline is quarantine_length + divisor * (1 + block_number)"
        );
    }

    /// **Law 44 — the epoch gate.** `close_block` changes *nothing* except at a boundary
    /// (`Pos.rhox:517-519`): no reward is committed, no staged withdrawal moves, no claim is paid,
    /// and the active set is not recomputed. Done at a non-boundary, the whole sequence is a no-op
    /// even when there is a pending withdrawal and a full pot waiting.
    #[tokio::test]
    async fn the_epoch_gate_does_nothing_off_a_boundary() {
        let params = PosParams {
            epoch_length: 10,
            quarantine_length: 0,
            ..PosParams::default()
        };
        let native = native_with(&[validator(1)], params, &[(validator(1), 40)]).await;
        let v = validator(1);
        native.set_vault_balance(&vault_address_of(&v), NonNegI64::zero());
        let addr = vault_address_of(&v);
        // Fund the pot the way the protocol does: a deploy pays its phlo into the staking vault.
        let payer = PublicKey::new(vec![9u8; 65]);
        let payer_addr = RevAddress::from_public_key(&payer).unwrap().to_base58();
        native.set_vault_balance(&payer_addr, NonNegI64::try_from(10).unwrap());
        native.pre_charge(&payer, nn(10)).await.unwrap().unwrap();
        native.withdraw(&v, 3).await.unwrap().unwrap();
        let before = (
            native.pending_withdrawers().await.unwrap(),
            native.committed_rewards().await.unwrap(),
            native.bonds().await.unwrap(),
            native.active().await.unwrap(),
        );

        // 7 is not a multiple of 10: nothing happens.
        native
            .close_block(7, fringe_state(7), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                native.pending_withdrawers().await.unwrap(),
                native.committed_rewards().await.unwrap(),
                native.bonds().await.unwrap(),
                native.active().await.unwrap(),
            ),
            before,
            "an epoch that does not occur changes nothing at all"
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            50,
            "the pot is untouched — no reward was committed"
        );
        assert_eq!(
            i64::from(native.vault_balance(&addr).await.unwrap().unwrap()),
            0,
            "and no claim was paid"
        );

        // 10 is a boundary: the sequence runs. The request staged at block 3 was given the deadline
        // `0 + 10 * (1 + 3 / 10) = 10`, so this boundary both moves it out of the pool and pays it.
        native
            .close_block(10, fringe_state(10), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(
            native.pending_withdrawers().await.unwrap().is_empty(),
            "the staged withdrawal moved at the boundary"
        );
        assert!(
            !native.bonds().await.unwrap().contains_key(&v),
            "out of the pool"
        );
        assert_eq!(
            i64::from(native.vault_balance(&addr).await.unwrap().unwrap()),
            40,
            "and paid: the bond, plus the epoch's committed reward"
        );

        // The gate's other half, with an epoch length a block cannot reach: a bond joins the pool at
        // once but joins the *consensus set* only at a boundary.
        let native = native_with(&[validator(1)], params, &[]).await;
        let v2 = validator(1);
        fund(&native, &v2, 100).await;
        native
            .bond(&v2, NonNegI64::try_from(40).unwrap(), 7)
            .await
            .unwrap()
            .unwrap();
        assert!(
            native.bonds().await.unwrap().contains_key(&v2),
            "pooled at once"
        );
        assert!(!native.active().await.unwrap().contains_key(&v2));
        native
            .close_block(9, fringe_state(9), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !native.active().await.unwrap().contains_key(&v2),
            "9 is not a boundary either"
        );
        native
            .close_block(10, fringe_state(10), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(
            native.active().await.unwrap().contains_key(&v2),
            "the boundary is where a bond becomes a validator"
        );
    }

    /// **Laws 45 and 46 — the split, checked against the model rather than a remembered number.**
    /// `Rchain/Pos.lean`'s `the_dust_is_real` is the `decide`d case `minimumBond = 3`, bonds
    /// `[4, 8]`, pot `10`: the normaliser is `12 / 3 = 4`, the scaled shares are `4 / 3 = 1` and
    /// `8 / 3 = 2`, and the validators are paid `10 * 1 / 4 = 2` and `10 * 2 / 4 = 5` — **7
    /// distributed of 10**, the rest being the dust of two integer divisions. This test builds exactly
    /// that state and reads the split back, so the implementation is checked against the arithmetic the
    /// Lean proves rather than against itself.
    ///
    /// **Why these bonds and not a smaller pair (AUDIT C149).** Bonds `[4, 5]` were the fixture until
    /// this was measured: both are `1` after the integer division by `minimum_bond = 3`, so the
    /// proportionality factor `bond / minimum_bond` was the *identity* for every validator the test
    /// built, and deleting it from `epoch_reward` left this test green — two deliberately different
    /// bonds producing the same share by construction. `4` and `8` straddle the divisor, so the factor
    /// is `1` for one validator and `2` for the other, and a split that ignores the bond is visible.
    /// `the_dust_is_real` carried the identical degeneracy over the same instance; it was moved to
    /// this one in the same pass, because a row certified on one side and unobservable on the other is
    /// not certified at all.
    #[tokio::test]
    async fn an_epoch_splits_the_pot_and_keeps_the_dust() {
        let params = PosParams {
            minimum_bond: NonNegI64::try_from(3).unwrap(),
            epoch_length: 1,
            ..PosParams::default()
        };
        let native = native_with(
            &[validator(1), validator(2)],
            params,
            &[(validator(1), 4), (validator(2), 8)],
        )
        .await;
        // Fill the pot with exactly 10 the way a deploy's phlo does.
        let payer = PublicKey::new(vec![9u8; 65]);
        let payer_addr = RevAddress::from_public_key(&payer).unwrap().to_base58();
        native.set_vault_balance(&payer_addr, NonNegI64::try_from(10).unwrap());
        native.pre_charge(&payer, nn(10)).await.unwrap().unwrap();
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            12 + 10,
            "the vault holds the bonds plus the phlo"
        );

        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();

        let committed = native.committed_rewards().await.unwrap();
        assert_eq!(
            i64::from(committed[&validator(1)]),
            2,
            "10 * (4 / 3) / (12 / 3) = 2"
        );
        assert_eq!(
            i64::from(committed[&validator(2)]),
            5,
            "10 * (8 / 3) / (12 / 3) = 5"
        );
        assert!(
            i64::from(committed[&validator(1)]) + i64::from(committed[&validator(2)]) < 10,
            "the shares do not sum to the pot — that inequality is the law"
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            22,
            "the vault is not debited by a commitment: the reward is paid when the validator leaves"
        );
        assert_eq!(
            epoch_pot(
                native.pos_vault_balance().await.unwrap(),
                &native.bonds().await.unwrap(),
                &native.withdrawers().await.unwrap(),
                &native.committed_rewards().await.unwrap(),
                &native.delegated_rewards().await.unwrap(),
            )
            .unwrap(),
            3,
            "the three units of dust stay in the pot, and the next epoch distributes them"
        );
    }

    /// **Law 47's payoff.** A validator that earns, then asks to leave, is paid **its bond plus every
    /// reward committed while it was bonded** — including the epoch it spent its last blocks in, which
    /// is exactly what the sequence's order (commit, then move, then pay) is for.
    #[tokio::test]
    async fn a_released_withdrawal_pays_the_bond_plus_the_committed_rewards() {
        let params = PosParams {
            epoch_length: 1,
            quarantine_length: 0,
            // A positive minimum, or the split is the undefined case and pays nothing.
            minimum_bond: NonNegI64::try_from(3).unwrap(),
            ..PosParams::default()
        };
        let native = native_with(&[validator(1)], params, &[(validator(1), 40)]).await;
        let v = validator(1);
        native.set_vault_balance(&vault_address_of(&v), NonNegI64::zero());
        // 5 of phlo for the epoch, charged to a deployer.
        let payer = PublicKey::new(vec![9u8; 65]);
        let payer_addr = RevAddress::from_public_key(&payer).unwrap().to_base58();
        native.set_vault_balance(&payer_addr, NonNegI64::try_from(5).unwrap());
        native.pre_charge(&payer, nn(5)).await.unwrap().unwrap();

        // Stage the request, then close the block: the boundary pays the epoch's reward into the
        // committed map *and* moves the validator out of the pool, in that order.
        native.withdraw(&v, 1).await.unwrap().unwrap();
        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.committed_rewards().await.unwrap()[&v]),
            5,
            "the epoch it was still active for paid it the whole pot (one validator, no rounding)"
        );
        assert!(
            !native.bonds().await.unwrap().contains_key(&v),
            "and it left the pool at the same boundary"
        );

        // The claim's deadline is 0 + 1 * (1 + 1) = 2, so the next boundary pays it.
        assert_eq!(
            native.withdrawers().await.unwrap()[&v].deadline,
            2,
            "quarantineLength + epochLength * (1 + blockNumber / epochLength)"
        );
        native
            .close_block(2, fringe_state(2), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(
                native
                    .vault_balance(&vault_address_of(&v))
                    .await
                    .unwrap()
                    .unwrap()
            ),
            45,
            "bond 40 + committed 5, out of the staking vault"
        );
        assert!(
            native.committed_rewards().await.unwrap().is_empty(),
            "the claim is spent: committed and withdrawers both cleared"
        );
        assert!(native.withdrawers().await.unwrap().is_empty());
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            0,
            "the vault is exactly emptied — the phlo went to the validator, not to nowhere"
        );
    }

    /// Where the contract's formula is undefined the port pays nothing rather than faulting. With
    /// `minimum_bond == 0` the Scala's `bonds / $$minimumBond$$` is a division by zero, and the
    /// port's parameters are permissive by default, so a fault is not a rule it can copy; a
    /// normaliser of zero (`activeBonds < minimumBond`) is the same story. `Pos.lean` states its
    /// theorem for `0 < activeBonds / minimumBond` and says nothing about the rest — this is what the
    /// port does with the rest.
    #[tokio::test]
    async fn an_epoch_with_a_zero_normaliser_pays_nothing() {
        // minimum_bond 0: the normaliser is a division by zero in the contract.
        let native = native_with(
            &[validator(1)],
            PosParams {
                epoch_length: 1,
                ..PosParams::default()
            },
            &[(validator(1), 40)],
        )
        .await;
        let payer = PublicKey::new(vec![9u8; 65]);
        let payer_addr = RevAddress::from_public_key(&payer).unwrap().to_base58();
        native.set_vault_balance(&payer_addr, NonNegI64::try_from(5).unwrap());
        native.pre_charge(&payer, nn(5)).await.unwrap().unwrap();

        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.committed_rewards().await.unwrap()[&validator(1)]),
            0,
            "an undefined split pays zero"
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            45,
            "and the pot is left where it was"
        );

        // A normaliser of zero with a positive minimum: bonds below the minimum scale to nothing.
        let native = native_with(
            &[validator(1)],
            PosParams {
                minimum_bond: NonNegI64::try_from(100).unwrap(),
                epoch_length: 1,
                ..PosParams::default()
            },
            &[(validator(1), 40)],
        )
        .await;
        let native = native;
        native.set_vault_balance(&payer_addr, NonNegI64::try_from(5).unwrap());
        native.pre_charge(&payer, nn(5)).await.unwrap().unwrap();
        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.committed_rewards().await.unwrap()[&validator(1)]),
            0
        );
    }

    /// The pot floors at zero instead of wrapping. The state this needs cannot be reached through the
    /// protocol — every debit is bounded by the credit that funded it, so the vault always covers its
    /// claims — so the test builds the diverged ledger by hand to show the floor holds. The Scala
    /// computes the pot in `Long` and would produce a *negative* reward here; `NonNegI64` has no such
    /// value, and the model's subtraction is `Nat` (floored), so the port floors with it.
    #[tokio::test]
    async fn a_drafted_vault_floors_the_pot_instead_of_wrapping() {
        let native = native_with(
            &[validator(1)],
            PosParams {
                epoch_length: 1,
                minimum_bond: NonNegI64::try_from(3).unwrap(),
                ..PosParams::default()
            },
            &[(validator(1), 40)],
        )
        .await;
        // Drain the vault below the bonded pool: a ledger that has already diverged.
        native.debit_pos_vault(nn(35)).await.unwrap();
        assert_eq!(i64::from(native.pos_vault_balance().await.unwrap()), 5);

        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.committed_rewards().await.unwrap()[&validator(1)]),
            0,
            "a pot of nothing pays nothing — not a wrap, and not a negative reward"
        );
    }

    #[tokio::test]
    async fn slash_confiscates_to_coop_vault() {
        let native = native_with(
            &[validator(1), validator(2)],
            PosParams::default(),
            &[(validator(1), 10)],
        )
        .await;
        let v = validator(2);
        fund(&native, &v, 100).await;
        native
            .bond(&v, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();

        // v1's genesis bond (10) plus v2's (40) are in the staking vault.
        assert_eq!(i64::from(native.pos_vault_balance().await.unwrap()), 50);
        let addresses = [vault_address_of(&v)];
        let before = total_rev(&native, &addresses).await;

        native
            .slash(&v, SlashSeverity::Unspecified)
            .await
            .unwrap()
            .unwrap();

        assert!(!native.bonds().await.unwrap().contains_key(&v));
        assert!(!native.active().await.unwrap().contains_key(&v));
        assert_eq!(
            i64::from(native.coop_balance().await.unwrap()),
            40,
            "slashed stake is confiscated to the Coop vault"
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            10,
            "…and it came *out* of the staking vault (`Pos.rhox:470-482`'s transfer), leaving v1's bond"
        );
        assert_eq!(
            total_rev(&native, &addresses).await,
            before,
            "a slashing is a transfer to the Coop vault, not a mint"
        );
    }

    /// The current epoch pot, read the way `close_block` reads it.
    async fn pot_of(native: &NativeSystemState) -> i64 {
        epoch_pot(
            native.pos_vault_balance().await.unwrap(),
            &native.bonds().await.unwrap(),
            &native.withdrawers().await.unwrap(),
            &native.committed_rewards().await.unwrap(),
            &native.delegated_rewards().await.unwrap(),
        )
        .unwrap()
    }

    /// **`fundPot` raises the pot by exactly what it moves, and nothing else.** The funder's vault goes
    /// down by the amount, the staking vault up by it, and no claim ledger moves — so the whole amount
    /// is pot, which is what makes it a way to raise validator rewards without a fork. A `bond` of the
    /// same size is the control: it credits the vault too, but records an equal claim, so the pot does
    /// not move. Zero and an amount the vault cannot cover are refused and move nothing.
    #[tokio::test]
    async fn funding_the_pot_adds_unclaimed_rev_and_bonding_does_not() {
        let native = native_with(
            &[validator(1), validator(2)],
            PosParams::default(),
            &[(validator(1), 10)],
        )
        .await;
        let donor = validator(2);
        fund(&native, &donor, 100).await;
        let pot = pot_of(&native).await;
        let bonds = native.bonds().await.unwrap();

        native
            .fund_pot(&donor, NonNegI64::try_from(30).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pot_of(&native).await, pot + 30, "the whole amount is pot");
        assert_eq!(native.bonds().await.unwrap(), bonds, "no claim is recorded");
        let left = native
            .vault_balance(&vault_address_of(&donor))
            .await
            .unwrap()
            .map(i64::from);
        assert_eq!(left, Some(70), "the funder pays exactly the amount");

        for refused in [0, 71] {
            let msg = native
                .fund_pot(&donor, NonNegI64::try_from(refused).unwrap())
                .await
                .unwrap()
                .expect_err("refused");
            assert!(msg.contains("fund"), "{msg}");
            assert_eq!(pot_of(&native).await, pot + 30, "a refusal moves nothing");
        }

        native
            .bond(&donor, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            pot_of(&native).await,
            pot + 30,
            "a bond credits the vault and claims the same amount, so the pot is unchanged"
        );
    }

    #[tokio::test]
    async fn pot_funding_overflow_moves_neither_balance() {
        let donor = validator(2);
        let native = native_with(&[donor], PosParams::default(), &[]).await;
        fund(&native, &donor, 100).await;
        native.set_pos_vault_balance(NonNegI64::try_from(i64::MAX).unwrap());
        let before = native
            .vault_balance(&vault_address_of(&donor))
            .await
            .unwrap();
        let err = native
            .fund_pot(&donor, NonNegI64::try_from(1).unwrap())
            .await
            .expect_err("overflow is a platform error");
        assert!(err.contains("overflow"), "{err}");
        assert_eq!(
            native
                .vault_balance(&vault_address_of(&donor))
                .await
                .unwrap(),
            before
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            i64::MAX
        );
    }

    /// **The C197 falsifier, restated for the tiers: a slash clears the offender's accrued rewards
    /// and *accounts* for them.** `Pos.rhox` deletes the entry; this port left it, and it was then
    /// unreachable both ways — never paid (the epoch pays pool members and claims in `withdrawers`,
    /// and the validator is out of both) and never removed (the only `committed.remove` is the claim
    /// payment) — so the balance sat in the staking vault shrinking the pot for every *other*
    /// validator, for ever.
    ///
    /// The consequence is now visible in two places at once, and they must agree: the entry is gone,
    /// **and** the amount left the staking vault — to the Coop vault under `Unspecified`, which is the
    /// pre-tier rule and takes everything. Red before C197: the entry survived and the Coop vault did
    /// not move by it.
    #[tokio::test]
    async fn a_slash_clears_the_accrued_rewards_and_accounts_for_them() {
        let native =
            native_with(&[validator(1)], PosParams::default(), &[(validator(1), 10)]).await;
        let v = validator(1);
        // Some phlo burned since the last boundary, so the pot is positive.
        native
            .credit_pos_vault(NonNegI64::try_from(30).unwrap())
            .await
            .unwrap();
        // …and an epoch's share this validator has earned and not withdrawn.
        let accrued = NonNegI64::try_from(7).unwrap();
        native.set_committed_rewards(&BTreeMap::from([(v, accrued)]));

        let pot_before = pot_of(&native).await;
        assert_eq!(
            pot_before, 23,
            "40 of vault (10 bonded + 30 burned) less the 10 bonded less the 7 accrued"
        );

        native
            .slash(&v, SlashSeverity::Unspecified)
            .await
            .unwrap()
            .unwrap();

        assert!(
            !native.committed_rewards().await.unwrap().contains_key(&v),
            "the offender's accrued rewards go with the bond"
        );
        assert_eq!(
            i64::from(native.coop_balance().await.unwrap()),
            17,
            "the bond (10) and the accrued (7) both left for the Coop vault — the pre-tier rule \
             takes everything at risk"
        );
        assert_eq!(
            pot_of(&native).await,
            pot_before,
            "and the pot is unchanged: the vault loses exactly what the claims lose, so a slash \
             neither shrinks nor subsidises what the epoch distributes"
        );
    }

    /// **The A3 falsifier, first tier: a misdemeanour takes a quarter and gives the rest back.**
    ///
    /// Everything the validator holds in the PoS system is at risk — the bond, the accrued rewards,
    /// and an escrowed withdrawal claim — and the tier takes a share of *that sum* while the
    /// remainder returns to the validator's own vault. The two halves must add up: the floor
    /// division's remainder is returned, never kept, so the loss is exactly the tier.
    ///
    /// Red before C199: the whole bond went to the Coop vault and nothing came back.
    #[tokio::test]
    async fn a_misdemeanour_takes_a_quarter_and_returns_the_rest() {
        let native = native_with(
            &[validator(1), validator(2)],
            PosParams::default(),
            &[(validator(1), 10)],
        )
        .await;
        let v = validator(2);
        fund(&native, &v, 100).await;
        native
            .bond(&v, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();
        // An accrual that does not divide evenly by four, so the floor's remainder is exercised.
        native.set_committed_rewards(&BTreeMap::from([(v, NonNegI64::try_from(7).unwrap())]));
        native
            .credit_pos_vault(NonNegI64::try_from(1_000).unwrap())
            .await
            .unwrap();

        let wallet_before = i64::from(
            native
                .vault_balance(&vault_address_of(&v))
                .await
                .unwrap()
                .unwrap_or(NonNegI64::zero()),
        );
        let at_risk = 40 + 7;
        let confiscated = at_risk * 2_500 / 10_000; // 11
        let returned = at_risk - confiscated; // 36

        native
            .slash(&v, SlashSeverity::Misdemeanour)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            i64::from(native.coop_balance().await.unwrap()),
            confiscated,
            "a quarter of everything at risk, floored"
        );
        let wallet_after = i64::from(
            native
                .vault_balance(&vault_address_of(&v))
                .await
                .unwrap()
                .unwrap_or(NonNegI64::zero()),
        );
        assert_eq!(
            wallet_after,
            wallet_before + returned,
            "and the remainder is the validator's again — including the floor's remainder, so \
             confiscated + returned is exactly what was at risk"
        );
        assert!(!native.bonds().await.unwrap().contains_key(&v));
        assert!(
            !native.committed_rewards().await.unwrap().contains_key(&v),
            "the validator is out of the pool and its claims are cleared either way"
        );
    }

    /// **The tier a validator answers for is the worst it committed.** Two failed blocks from one
    /// sender — a tenth and a quarter — cost a quarter, not two tenths and not the whole bond.
    ///
    /// And the *scope* is every holding, not the bond alone: an escrowed claim is at risk too, which
    /// is the half a bond-only cap would have missed (and which used to be forfeited outright, since
    /// `slash` removed the claim and nothing paid it).
    #[tokio::test]
    async fn a_slash_counts_the_escrowed_claim_and_takes_the_worst_tier() {
        let native = native_with(
            &[validator(1), validator(2)],
            PosParams::default(),
            &[(validator(1), 10)],
        )
        .await;
        let v = validator(2);
        fund(&native, &v, 100).await;
        native
            .bond(&v, NonNegI64::try_from(30).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();
        // Staged and past a boundary: 20 of the bond has left the pool and is escrowed, so what the
        // validator has in the PoS system is a 10 bond plus a 20 claim.
        native.set_bonds(&BTreeMap::from([
            (validator(1), NonNegI64::try_from(10).unwrap()),
            (v, NonNegI64::try_from(10).unwrap()),
        ]));
        native.set_withdrawers(&BTreeMap::from([(
            v,
            Withdrawal {
                bond: NonNegI64::try_from(20).unwrap(),
                deadline: 10_000,
            },
        )]));
        native
            .credit_pos_vault(NonNegI64::try_from(1_000).unwrap())
            .await
            .unwrap();

        // Bond 10 + claim 20 = 30 at risk; a quarter is 7.
        native
            .slash(&v, SlashSeverity::Misdemeanour)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            i64::from(native.coop_balance().await.unwrap()),
            7,
            "the escrowed claim is part of what the tier is a share of — a bond-only cap would have \
             missed it, and before C199 the claim was forfeited outright"
        );
        assert!(
            !native.withdrawers().await.unwrap().contains_key(&v),
            "and the claim is cleared with the rest of the validator's records"
        );
    }

    #[tokio::test]
    async fn untrust_removes_trust_and_leaves_the_bond_alone() {
        // This test used to be `untrust_removes_and_confiscates` and asserted the opposite of its
        // last two lines: that the bond was gone and the Coop vault was up by the stake. It passed
        // for as long as the behaviour did, which is the point — the test was written from the code
        // rather than from the rule, so it could not see that one trusted stakeholder had been given
        // a one-signature power to take any peer's stake (AUDIT C111). The assertions below are the
        // rule: trust is admission, a bond is property, and revocation moves only the first.
        let native = native_with(
            &[validator(1), validator(2)],
            PosParams::default(),
            &[(validator(1), 10)],
        )
        .await;
        let v = validator(2);
        fund(&native, &v, 100).await;
        native
            .bond(&v, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();

        let coop_before = i64::from(native.coop_balance().await.unwrap());
        let vault_before = i64::from(native.pos_vault_balance().await.unwrap());

        native.untrust(&validator(1), &v).await.unwrap().unwrap();

        assert!(
            !native.trusted().await.unwrap().contains(&v),
            "untrust must remove the target from the trusted set"
        );
        assert!(
            native.bonds().await.unwrap().contains_key(&v),
            "untrust must NOT confiscate the target's bond — the stake is the validator's property, \
             and taking it was a one-signature robbery available to any single trusted stakeholder"
        );
        assert_eq!(
            i64::from(native.coop_balance().await.unwrap()),
            coop_before,
            "the Coop vault must not be enriched by a revocation"
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            vault_before,
            "and no value may move out of the staking vault: a revocation is not a transfer"
        );
        assert_eq!(
            i64::from(
                native
                    .vault_balance(&native.vault_address(&v).unwrap())
                    .await
                    .unwrap()
                    .unwrap()
            ),
            60,
            "the validator keeps what it did not bond, so it can still withdraw the stake"
        );
    }

    /// **Law 82: untrust does not deactivate.** Trust is an admission list, not the consensus set: a
    /// validator that is already active when it is untrusted stays in `pos:active` — and keeps its
    /// bond — until a boundary re-selects. The old module header said the opposite ("`untrust`
    /// removes the validator and confiscates the stake"); the code and AUDIT C111 say this, and the
    /// stale prose was the defect.
    #[tokio::test]
    async fn untrust_does_not_change_the_active_set() {
        let native = native_with(&[validator(1), validator(2)], PosParams::default(), &[]).await;
        let v = validator(1);
        fund(&native, &v, 100).await;
        native
            .bond(&v, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();
        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(
            native.active().await.unwrap().contains_key(&v),
            "the fixture is only meaningful if the validator is active before the revocation"
        );

        native.untrust(&validator(2), &v).await.unwrap().unwrap();

        assert!(
            !native.trusted().await.unwrap().contains(&v),
            "admission is revoked"
        );
        assert!(
            native.active().await.unwrap().contains_key(&v),
            "…and consensus membership is not: `phaseOf` reads the stake ledgers before admission"
        );
        assert!(
            native.bonds().await.unwrap().contains_key(&v),
            "…nor is the bond touched"
        );
    }

    #[tokio::test]
    async fn active_set_respects_the_cap() {
        let params = PosParams {
            number_of_active_validators: 2,
            ..PosParams::default()
        };
        let native = native_with(
            &[validator(1), validator(2), validator(3)],
            params,
            &[(validator(1), 10), (validator(2), 20), (validator(3), 30)],
        )
        .await;

        let active: Vec<Validator> = native
            .active_validators()
            .await
            .unwrap()
            .into_iter()
            .collect();
        // Which two are active is a draw now, so this pins the *cap* and the *provenance* rather
        // than the set: exactly two, all from the pool. The exact set is pinned by the
        // known-answer vector beside `select_active`.
        assert_eq!(active.len(), 2, "the cap is respected");
        assert!(
            active
                .iter()
                .all(|v| [validator(1), validator(2), validator(3)].contains(v)),
            "every active validator comes from the pool: {active:?}"
        );
        assert_eq!(
            native.bonds().await.unwrap().len(),
            3,
            "pool keeps the third"
        );
    }

    #[tokio::test]
    async fn close_block_is_idempotent_for_active_set() {
        let native = native_with(
            &[validator(1), validator(2)],
            PosParams::default(),
            &[(validator(1), 10), (validator(2), 20)],
        )
        .await;
        native
            .close_block(1, fringe_state(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        native
            .close_block(2, fringe_state(2), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(native.active_validators().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn pre_charge_deducts_and_rejects_insufficient() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let pk = PublicKey::new(vec![1u8; 65]);
        let addr = RevAddress::from_public_key(&pk).unwrap().to_base58();
        native.set_vault_balance(&addr, NonNegI64::try_from(100).unwrap());

        // Deduct 40 -> 60.
        native.pre_charge(&pk, nn(40)).await.unwrap().unwrap();
        assert_eq!(
            i64::from(native.vault_balance(&addr).await.unwrap().unwrap()),
            60
        );

        // Deducting more than the balance fails via the user-error branch.
        let result = native.pre_charge(&pk, nn(100)).await.unwrap();
        assert!(result.is_err(), "insufficient funds must be rejected");
    }

    /// **The producer is paid a share of what the deploy burned** (B2, #150) — and it is a *transfer*:
    /// the staking vault loses exactly what the producer's own vault gains, so the pie is unchanged and
    /// only the recipient moves.
    ///
    /// The three facts, one arm each: the two vaults move by the same amount and in opposite
    /// directions; the bond pool is untouched, because a payment that could touch a stake would be a
    /// slash wearing a reward's clothes; and a share of zero — the contract's own behaviour, and what a
    /// params record written before this field decodes to — moves nothing at all.
    #[tokio::test]
    async fn the_producer_is_paid_a_share_of_what_the_deploy_burned() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let producer = PublicKey::new(vec![9u8; 65]);
        let producer_addr = RevAddress::from_public_key(&producer).unwrap().to_base58();
        let deployer = PublicKey::new(vec![1u8; 65]);
        let deployer_addr = RevAddress::from_public_key(&deployer).unwrap().to_base58();
        let validator = validator(1);
        native
            .install_genesis(&PosGenesis {
                bonds: BTreeMap::from([(validator, nn(100))]),
                trusted: BTreeSet::from([validator]),
                params: PosParams {
                    executor_share: NonNegI64::try_from(2500).expect("a quarter"),
                    absence_slack: NonNegI64::zero(),
                    ..PosParams::default()
                },
                dev_vesting: None,
            })
            .unwrap();
        let staking_before = i64::from(native.pos_vault_balance().await.unwrap());

        // A deploy charged 100 that burns 40: `pre_charge` puts the whole charge in the vault and
        // `refund` takes the unconsumed 60 back out, so the vault keeps the burned 40.
        native.set_vault_balance(&deployer_addr, NonNegI64::try_from(1000).unwrap());
        native
            .pre_charge(&deployer, nn(100))
            .await
            .unwrap()
            .unwrap();
        native.refund(&deployer, nn(60)).await.unwrap().unwrap();
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            staking_before + 40,
            "the control: the vault keeps exactly what was burned"
        );

        native
            .pay_executor(&producer, nn(40))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.vault_balance(&producer_addr).await.unwrap().unwrap()),
            10,
            "a quarter of 40, in the address that signed the block"
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            staking_before + 30,
            "and the same 10 left the staking vault"
        );
        assert_eq!(
            native.bonds().await.unwrap().get(&validator),
            Some(&nn(100)),
            "the bond is not a payment source"
        );

        // Zero share: the pre-field rule, and it must move nothing — not even a rounding remainder.
        let no_share = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        no_share
            .install_genesis(&PosGenesis {
                bonds: BTreeMap::from([(validator, nn(100))]),
                trusted: BTreeSet::from([validator]),
                params: PosParams::default(),
                dev_vesting: None,
            })
            .unwrap();
        let before = i64::from(no_share.pos_vault_balance().await.unwrap());
        no_share
            .pay_executor(&producer, nn(40))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(no_share.pos_vault_balance().await.unwrap()),
            before
        );
        assert_eq!(
            no_share.vault_balance(&producer_addr).await.unwrap(),
            None,
            "a share of zero does not even create the producer's vault"
        );
    }

    #[tokio::test]
    async fn find_or_create_vault_creates_zero_balance_once() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let addr = "someRevAddress".to_string();

        // First call creates the vault with a zero balance.
        native.find_or_create_vault(&addr).await.unwrap();
        assert_eq!(
            native.vault_balance(&addr).await.unwrap(),
            Some(NonNegI64::zero())
        );

        // A subsequent call must not reset an existing balance.
        native.set_vault_balance(&addr, NonNegI64::try_from(42).unwrap());
        native.find_or_create_vault(&addr).await.unwrap();
        assert_eq!(
            i64::from(native.vault_balance(&addr).await.unwrap().unwrap()),
            42
        );
    }

    /// **The absence rule, read from the fringe** (B4, #150), and the three conditions on it.
    ///
    /// The participation is now an *input* to the boundary rather than a store the rule reads for
    /// itself: `close_block` takes each validator's latest message height in the last finalised fringe,
    /// which the validator derives from the DAG (see `multi_parent_casper::participation_for_block`).
    /// The properties this test pins:
    ///
    /// 1. **the rule is income only** — a validator that has gone quiet loses its epoch reward and
    ///    keeps its bond, stays in the pool, and stays in the active set;
    /// 2. **and it recovers fully** — one message from it puts it back in full at the next boundary,
    ///    which is what makes this an incentive rather than a confiscation;
    /// 3. with `absence_slack = 0` the rule does nothing at all, which is the contract's behaviour.
    ///
    /// What it does **not** pin, and a reader should not read into it: that the map it passes is the
    /// one a node derives. That is `participation_for_block`'s subject, and `casper`'s tests are where
    /// the derivation is exercised.
    #[tokio::test]
    async fn the_absence_rule_takes_income_only_and_releases_it_on_a_single_block() {
        let params = PosParams {
            minimum_bond: NonNegI64::try_from(1).unwrap(),
            epoch_length: 1,
            absence_slack: NonNegI64::try_from(5).unwrap(),
            // **`grace == knee` is the binary rule** — the cliff the tree shipped before the weight was
            // graded — so this test is about that shape and the graded ramp has its own arms.
            participation_grace: NonNegI64::try_from(5).unwrap(),
            ..PosParams::default()
        };
        let native = native_with(
            &[validator(1), validator(2)],
            params.clone(),
            &[(validator(1), 4), (validator(2), 8)],
        )
        .await;
        // A pot, the way a deploy's phlo makes one.
        let payer = PublicKey::new(vec![9u8; 65]);
        let payer_addr = RevAddress::from_public_key(&payer).unwrap().to_base58();
        native.set_vault_balance(&payer_addr, nn(10));
        native.pre_charge(&payer, nn(10)).await.unwrap().unwrap();

        // Validator 1's latest message in the fringe is at height 4; validator 2 has no entry at all.
        let participation = BTreeMap::from([(validator(1), BlockHeight::try_from(4).unwrap())]);

        // The boundary is height 12, so the slack of 5 reaches back to 7: validator 1 (height 4) is
        // out of time and validator 2 (no entry) is as absent as it gets.
        native
            .close_block(12, fringe_state(12), &participation)
            .await
            .unwrap()
            .unwrap();
        let committed = native.committed_rewards().await.unwrap();
        // **The key survives with a zero, rather than going away** — the rule scales now where it used
        // to filter, so "not paid" is `Some(0)` and not `None`. A test asserting absence would fail
        // against the real rule, which is the honest kind of failure but not the property wanted.
        assert_eq!(
            committed.get(&validator(1)).map(|r| i64::from(*r)),
            Some(0),
            "a quiet validator is not paid for the boundary it sat out"
        );
        assert_eq!(
            committed.get(&validator(2)).map(|r| i64::from(*r)),
            Some(0),
            "nor is a silent stranger"
        );

        // **Income only.** The bond is where it was and both are still bonded, so the rule cannot
        // reach a stake — a validator that was never paid is not a validator that was slashed.
        let pool = native.bonds().await.unwrap();
        assert_eq!(pool.get(&validator(1)).map(|s| i64::from(*s)), Some(4));
        assert_eq!(pool.get(&validator(2)).map(|s| i64::from(*s)), Some(8));

        // **And one block recovers it in full.** Validator 1's message is at 16 in the next fringe, and
        // the next boundary is 20 (slack reaches back to 15, and 16 is inside it), so it is paid the
        // share it would have had.
        let recovered = BTreeMap::from([(validator(1), BlockHeight::try_from(16).unwrap())]);
        native
            .close_block(20, fringe_state(20), &recovered)
            .await
            .unwrap()
            .unwrap();
        let committed = native.committed_rewards().await.unwrap();
        let paid = committed
            .get(&validator(1))
            .map(|r| i64::from(*r))
            .unwrap_or(0);
        assert!(
            paid > 0,
            "a validator that has signed again is paid again — the rule has no memory of the lapse"
        );

        // **Slack zero is the contract's behaviour**: no record, no penalty.
        let off = native_with(
            &[validator(1)],
            PosParams {
                minimum_bond: NonNegI64::try_from(1).unwrap(),
                epoch_length: 1,
                ..PosParams::default()
            },
            &[(validator(1), 4)],
        )
        .await;
        off.set_vault_balance(&payer_addr, nn(10));
        off.pre_charge(&payer, nn(10)).await.unwrap().unwrap();
        off.close_block(12, fringe_state(12), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            off.committed_rewards()
                .await
                .unwrap()
                .get(&validator(1))
                .map(|r| i64::from(*r)),
            Some(10),
            "with the rule off, an absent validator is paid for being drawn — exactly as Pos.rhox \\
             does: the only drawn validator takes the whole pot, never having signed a block"
        );
    }

    /// **The boundary pays on the participation it is handed, and on nothing else** (B4, #150).
    ///
    /// The test is deliberately a **pair**: one validator, one stake, one boundary played twice with two
    /// different participations — a lag inside the grace and a lag past the knee — must be paid the
    /// whole share and nothing respectively. A single generous arm would pass against a rule that
    /// ignored the argument entirely, which is the shape this is here to exclude.
    ///
    /// **What this test used to be, and why it is not that any more.** Until `pos:last_spoke` was
    /// retired it was a *falsifier*: it wrote a store entry contradicting the map and asserted the map
    /// won, so restoring `close_block`'s read to `self.last_spoke()` turned it red. With the store gone
    /// there is only one read and no rival to falsify against — the old form's strength came from both
    /// reads existing — so the pair above carries what is left: the argument is load-bearing, in both
    /// directions.
    #[tokio::test]
    async fn the_boundary_pays_on_the_participation_it_is_handed() {
        let params = PosParams {
            minimum_bond: NonNegI64::try_from(1).unwrap(),
            epoch_length: 1,
            absence_slack: NonNegI64::try_from(10).unwrap(),
            // The cliff, so the pair below is "inside" against "past" with no ramp in between.
            participation_grace: NonNegI64::try_from(10).unwrap(),
            ..PosParams::default()
        };
        let native = native_with(&[validator(1)], params, &[(validator(1), 4)]).await;
        let payer = PublicKey::new(vec![9u8; 65]);
        let payer_addr = RevAddress::from_public_key(&payer).unwrap().to_base58();

        // **Inside the grace at height 20**: behind by one, so the whole pot — the only drawn
        // validator takes all of it.
        native.set_vault_balance(&payer_addr, nn(10));
        native.pre_charge(&payer, nn(10)).await.unwrap().unwrap();
        let inside = BTreeMap::from([(validator(1), BlockHeight::try_from(19).unwrap())]);
        native
            .close_block(20, fringe_state(20), &inside)
            .await
            .unwrap()
            .unwrap();
        let paid_inside = i64::from(
            *native
                .committed_rewards()
                .await
                .unwrap()
                .get(&validator(1))
                .expect("a validator inside the grace is paid"),
        );
        assert_eq!(
            paid_inside, 10,
            "the whole pot, and the only drawn validator's"
        );

        // **Past the knee at height 40**: behind by ten, so nothing is added. The assertion is that the
        // *committed* figure is unchanged rather than absent — a scaling keeps the key and pays a zero.
        let outside = BTreeMap::from([(validator(1), BlockHeight::try_from(30).unwrap())]);
        native
            .close_block(40, fringe_state(40), &outside)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            native
                .committed_rewards()
                .await
                .unwrap()
                .get(&validator(1))
                .map(|r| i64::from(*r)),
            Some(10),
            "a boundary played with a validator past the knee adds nothing to what it was already owed"
        );
    }

    #[test]
    fn txn_record_round_trip() {
        let rec = TxnRecord {
            state: TxnState::Committed,
            coordinator: PublicKey::new(vec![7u8; 65]),
            amount: NonNegI64::try_from(12345).unwrap(),
            from: "fromRevAddress".to_string(),
            to: "toRevAddress".to_string(),
        };
        let encoded = encode_txn(&rec);
        assert_eq!(decode_txn(&encoded).unwrap(), rec);
    }

    #[tokio::test]
    async fn txn_prepare_commit_round_trip() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let coordinator = PublicKey::new(vec![4u8; 65]);
        let from = "fromAddr".to_string();
        let to = "toAddr".to_string();
        let id: &[u8] = b"txn-1";

        native.set_vault_balance(&from, NonNegI64::try_from(100).unwrap());

        // Prepare escrows 40 REV from `from`.
        let state = native
            .txn_prepare(
                id,
                &coordinator,
                NonNegI64::try_from(40).unwrap(),
                &from,
                &to,
            )
            .await
            .unwrap();
        assert_eq!(state, TxnState::Prepared);
        assert_eq!(
            i64::from(native.vault_balance(&from).await.unwrap().unwrap()),
            60
        );

        // Commit transfers 40 REV to `to`, and is idempotent.
        let state = native.txn_commit(id).await.unwrap();
        assert_eq!(state, TxnState::Committed);
        assert_eq!(
            i64::from(native.vault_balance(&to).await.unwrap().unwrap()),
            40
        );
        assert_eq!(native.txn_commit(id).await.unwrap(), TxnState::Committed);
        assert_eq!(
            i64::from(native.vault_balance(&to).await.unwrap().unwrap()),
            40
        );
    }

    #[tokio::test]
    async fn txn_prepare_abort_returns_escrow() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let coordinator = PublicKey::new(vec![4u8; 65]);
        let from = "fromAddr".to_string();
        let to = "toAddr".to_string();
        let id: &[u8] = b"txn-2";

        native.set_vault_balance(&from, NonNegI64::try_from(100).unwrap());
        native
            .txn_prepare(
                id,
                &coordinator,
                NonNegI64::try_from(30).unwrap(),
                &from,
                &to,
            )
            .await
            .unwrap();

        // Abort returns the escrow to `from`, and is idempotent.
        assert_eq!(native.txn_abort(id).await.unwrap(), TxnState::Aborted);
        assert_eq!(
            i64::from(native.vault_balance(&from).await.unwrap().unwrap()),
            100
        );
        assert_eq!(native.txn_abort(id).await.unwrap(), TxnState::Aborted);
    }

    #[tokio::test]
    async fn law28_txn_prepare_rejects_overdraw_and_is_idempotent() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let coordinator = PublicKey::new(vec![4u8; 65]);
        let from = "fromAddr".to_string();
        let to = "toAddr".to_string();
        let id: &[u8] = b"txn-3";

        native.set_vault_balance(&from, NonNegI64::try_from(50).unwrap());

        // Overdraw is rejected and leaves the vault untouched.
        assert!(native
            .txn_prepare(
                id,
                &coordinator,
                NonNegI64::try_from(51).unwrap(),
                &from,
                &to
            )
            .await
            .is_err());
        assert_eq!(
            i64::from(native.vault_balance(&from).await.unwrap().unwrap()),
            50
        );

        // A successful prepare is idempotent (a duplicate re-deducts nothing).
        native
            .txn_prepare(
                id,
                &coordinator,
                NonNegI64::try_from(40).unwrap(),
                &from,
                &to,
            )
            .await
            .unwrap();
        let state = native
            .txn_prepare(
                id,
                &coordinator,
                NonNegI64::try_from(40).unwrap(),
                &from,
                &to,
            )
            .await
            .unwrap();
        assert_eq!(state, TxnState::Prepared);
        assert_eq!(
            i64::from(native.vault_balance(&from).await.unwrap().unwrap()),
            10
        );
    }

    #[test]
    fn http_records_round_trip() {
        let mut records = BTreeMap::new();
        records.insert(
            "https://example.com/price".to_string(),
            ("42.5".to_string(), 7i64),
        );
        records.insert(
            "https://example.com/time".to_string(),
            ("".to_string(), 0i64),
        );
        let bytes = encode_http_records(&records);
        assert_eq!(decode_http_records(&bytes).unwrap(), records);
        assert!(decode_http_records(&bytes[..bytes.len() - 1]).is_err());
        assert!(decode_http_records(&[]).is_err());
    }

    #[tokio::test]
    async fn http_record_is_first_writer_wins() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let url = "https://example.com/price";

        // Nothing recorded yet.
        assert_eq!(native.http_record(url).await.unwrap(), None);

        // First capture wins and records the capture height.
        assert!(native.record_http(url, "42.5", 7).await.unwrap());
        assert_eq!(
            native.http_record(url).await.unwrap(),
            Some(("42.5".to_string(), 7))
        );

        // A later capture does not overwrite (this is what makes `check` meaningful).
        assert!(!native.record_http(url, "99.9", 8).await.unwrap());
        assert_eq!(
            native.http_record(url).await.unwrap(),
            Some(("42.5".to_string(), 7))
        );

        // Distinct URLs are independent.
        assert!(native
            .record_http("https://example.com/other", "x", 9)
            .await
            .unwrap());
        assert_eq!(native.http_records().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn bond_moves_the_stake_into_the_staking_vault() {
        let native = native_with(&[validator(1)], PosParams::default(), &[]).await;
        let v = validator(1);
        fund(&native, &v, 100).await;
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            0,
            "a genesis with an empty pool funds the vault with nothing"
        );
        let addresses = [vault_address_of(&v)];
        let before = total_rev(&native, &addresses).await;

        native
            .bond(&v, NonNegI64::try_from(40).unwrap(), 0)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            40,
            "the stake is escrowed in the staking vault"
        );
        assert_eq!(
            total_rev(&native, &addresses).await,
            before,
            "a bond is a transfer, not a mint"
        );
    }

    /// The charge goes into the staking vault and the refund takes the surplus back out, so what the
    /// vault keeps is exactly the phlo the deploy burned. This is the flow an epoch's reward pot
    /// comes from: `Pos.rhox:397-404`'s `deposit!(deployerId, amount, posVaultAddr)` on the charge,
    /// `Pos.rhox:417-454`'s `posVault!("transfer", deployerRevAddress, refundAmount, posAuthKey)` on
    /// the refund. Before the staking vault existed the charge was burned, so the pot could only ever
    /// be zero and a reward law over it could not have failed.
    #[tokio::test]
    async fn the_phlo_charge_funds_the_pot_and_the_refund_returns_the_surplus() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let pk = PublicKey::new(vec![1u8; 65]);
        let addr = RevAddress::from_public_key(&pk).unwrap().to_base58();
        native.set_vault_balance(&addr, NonNegI64::try_from(100).unwrap());
        let addresses = [addr.clone()];
        let before = total_rev(&native, &addresses).await;

        // Pre-charge the maximum phlo, then return all but 30 of it.
        native.pre_charge(&pk, nn(100)).await.unwrap().unwrap();
        assert_eq!(i64::from(native.pos_vault_balance().await.unwrap()), 100);
        assert_eq!(
            i64::from(native.vault_balance(&addr).await.unwrap().unwrap()),
            0
        );
        native.refund(&pk, nn(70)).await.unwrap().unwrap();

        assert_eq!(
            i64::from(native.vault_balance(&addr).await.unwrap().unwrap()),
            70,
            "the surplus returns to the deployer"
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            30,
            "the vault keeps the phlo the deploy burned — the epoch's pot"
        );
        assert_eq!(
            total_rev(&native, &addresses).await,
            before,
            "charging and refunding is a transfer, not a mint"
        );

        // A zero refund succeeds without moving anything (`Pos.rhox:426`'s guard).
        native.refund(&pk, nn(0)).await.unwrap().unwrap();
        // **The negative case is gone, and its absence is the fix** (AUDIT C109). This line used to
        // read `native.refund(&pk, -5)` and assert the no-op — which was the sibling of the very bug
        // that minted: `refund` happened to guard `amount <= 0` while `pre_charge` guarded only
        // `amount == 0`, and the asymmetry was invisible because both were tested only for the
        // behaviour they already had. A negative amount is now unrepresentable, so there is no
        // runtime assertion to write; the compile-time one is that `nn` cannot produce it.
        assert_eq!(i64::from(native.pos_vault_balance().await.unwrap()), 30);
    }

    /// A payout the staking vault cannot cover **fails the deploy** instead of half-paying. The
    /// Scala's `payWithdrawer` ignores its failed transfer and removes the withdrawer from the maps
    /// anyway (`// FIXME fix transfer in failure case`, `Pos.rhox:603`), which loses the bond; the
    /// port refuses. The invariant that makes this unreachable in normal operation — every debit is
    /// bounded by the credit that funded it — is checked by the conservation assertions in the tests
    /// around this one.
    #[tokio::test]
    async fn a_short_staking_vault_fails_the_transfer_rather_than_half_paying() {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let pk = PublicKey::new(vec![1u8; 65]);

        // No charge has happened, so the vault holds nothing to refund out of.
        assert!(
            native.refund(&pk, nn(1)).await.is_err(),
            "a refund cannot come out of an empty vault"
        );
        assert!(native.debit_pos_vault(nn(1)).await.is_err());
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            0,
            "a refused debit leaves the vault untouched"
        );
    }

    /// **A slash must not leave the pending withdrawal it removes behind.**
    ///
    /// `slash` deletes the validator's pending-withdrawal entry from a local copy of the map. Forgetting to
    /// write that copy back leaves the stale entry in the store, and the next key to occupy the validator's
    /// place - a later accepted bond, say - inherits it and is moved into a claim at the epoch boundary
    /// instead of joining the active set. Found by the PoS review, which measured the entry surviving slash
    /// in 9/9 configurations; this is the falsifier.
    #[tokio::test]
    async fn slash_persists_the_pending_map_it_edits() {
        let bonded = validator(1);
        let slashed = validator(2);
        let native = native_with(
            &[bonded],
            PosParams::default(),
            &[(bonded, 100), (slashed, 100)],
        )
        .await;

        native.set_pending_withdrawers(&BTreeMap::from([(slashed, 100)]));
        assert!(
            native
                .pending_withdrawers()
                .await
                .unwrap()
                .contains_key(&slashed),
            "the staged withdrawal must be in the store before the slash"
        );

        native
            .slash(&slashed, SlashSeverity::Malicious)
            .await
            .unwrap()
            .unwrap();

        assert!(
            !native.pending_withdrawers().await.unwrap().contains_key(&slashed),
            "slash removed the entry in memory and must persist that: otherwise the next holder of this key \
             inherits the withdrawal and never becomes active"
        );
    }
}

// ----------------------------------------------------------------------------------------------
// Appended rather than placed with its siblings, deliberately.
//
// `native_state.rs`'s line numbers are cited by 41 Lean declarations across `Rchain/Pos.lean`,
// `Rchain/Casper/Bonds.lean` and `Rchain/CrossShard.lean`, and by ten rows of hand-maintained spec.
// An insertion anywhere above them shifts every one, and a Lean citation can only be repaired by
// editing the Lean and re-emitting the register. Adding at the end shifts nothing, so the cost of
// putting this code where it reads best is a lake build and 51 citation edits -- paid once here, or
// avoided entirely by keeping the tail of this file append-only. The tail is append-only.
// ----------------------------------------------------------------------------------------------

/// Leaf key for the **seed the next epoch's active-set draw will use**.
///
/// This leaf is why the draw can be strengthened later without touching the rule. `select_active`
/// depends on a *value*, not on a *mechanism*: replacing what writes this leaf — with commit-reveal or
/// a VRF accumulator — changes nothing about the selection rule or any of its consumers. Today it is
/// written one boundary ahead, anchored to that boundary block's **pre-state hash**, so the proposer
/// of the drawing block cannot choose it.
pub fn pos_epoch_seed_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:epoch_seed")
}

/// The seed one epoch's active-set draw is made from, as stored in `pos:epoch_seed`.
///
/// **Why this exists as a leaf rather than as a parameter.** The value is written one boundary ahead
/// and read by the *next* boundary's draw, so the block that draws is not the block that chose the
/// entropy — which is the whole point (see the module's `select_active`). Keeping it in state rather
/// than threading it through `SystemDeploy::close_block` means replay agrees *by construction*: the
/// seed is a function of the pre-state, not a second derivation that has to be kept in sync.
///
/// `anchors` are the digests this seed folds in, in order. Today's writer puts two there — the previous
/// seed's anchor and the boundary block's pre-state hash — but nothing in the rule depends on the
/// count: it is a `Vec` rather than a fixed array because a later writer (commit-reveal, a VRF
/// accumulator, a beacon window) may want a different number, and the codec length-checks it so a
/// malformed leaf cannot be read as a shorter list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochSeed {
    pub epoch: i64,
    pub anchors: Vec<Blake2b256Hash>,
}

/// A `Blake2b256Hash` on the wire.
const HASH_LEN: usize = 32;

impl EpochSeed {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.anchors.len() * HASH_LEN);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&(self.anchors.len() as u64).to_le_bytes());
        for h in &self.anchors {
            out.extend_from_slice(h.as_bytes());
        }
        out
    }

    /// Decode (inverse of [`Self::encode`]). Refuses a length that disagrees with the declared count,
    /// so a truncated or padded leaf is an error rather than a silently shorter list — an anchor list of
    /// the wrong length would still draw a set, and the divergence would only surface as a post-state
    /// mismatch much later.
    pub fn decode(bytes: &[u8]) -> Result<EpochSeed, String> {
        if bytes.len() < 16 {
            return Err(format!(
                "epoch seed encoding has {} bytes, under the 16-byte header",
                bytes.len()
            ));
        }
        let epoch = i64::from_le_bytes(
            bytes[..8]
                .try_into()
                .map_err(|_| "epoch seed: invalid epoch field".to_string())?,
        );
        let declared = u64::from_le_bytes(
            bytes[8..16]
                .try_into()
                .map_err(|_| "epoch seed: invalid count field".to_string())?,
        );
        let count = usize::try_from(declared)
            .map_err(|_| format!("epoch seed: anchor count {declared} does not fit a usize"))?;
        let rest = &bytes[16..];
        let expected = count
            .checked_mul(HASH_LEN)
            .ok_or_else(|| format!("epoch seed: anchor count {count} overflows"))?;
        if rest.len() != expected {
            return Err(format!(
                "epoch seed declares {count} anchors ({expected} bytes) but carries {}",
                rest.len()
            ));
        }
        let anchors = rest
            .chunks_exact(HASH_LEN)
            .map(|chunk| {
                let bytes: [u8; HASH_LEN] = chunk
                    .try_into()
                    .map_err(|_| "epoch seed: invalid hash length".to_string())?;
                Ok(Blake2b256Hash::from_byte_array(&bytes))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(EpochSeed { epoch, anchors })
    }
}

impl NativeSystemState {
    /// Read the seed the next epoch's active-set draw will use.
    ///
    /// An absent leaf is a *distinguishable* state, not a default: it means no draw has been seeded
    /// yet (a genesis-installed state, before its first boundary). The caller decides what that means —
    /// genesis supplies a constant, the boundary supplies its pre-state hash — so this returns `Option` rather
    /// than inventing a seed that would silently change the draw.
    pub async fn epoch_seed(&self) -> Result<Option<EpochSeed>, String> {
        let bytes = self
            .store
            .get(PREFIX_POS, &pos_epoch_seed_key())
            .await
            .map_err(|e| e.to_string())?;
        match bytes {
            Some(b) => EpochSeed::decode(&b).map(Some),
            None => Ok(None),
        }
    }

    /// Write the seed the next epoch's active-set draw will use.
    pub fn set_epoch_seed(&self, seed: &EpochSeed) {
        self.store
            .put(PREFIX_POS, pos_epoch_seed_key(), seed.encode());
    }
}

#[cfg(test)]
mod epoch_seed_tests {
    use super::*;

    /// The epoch seed round-trips, anchor list and all.
    #[test]
    fn epoch_seed_round_trips() {
        let seed = EpochSeed {
            epoch: 7,
            anchors: vec![
                Blake2b256Hash::create(b"block:1"),
                Blake2b256Hash::create(b"block:2"),
                Blake2b256Hash::create(b"block:3"),
            ],
        };
        assert_eq!(EpochSeed::decode(&seed.encode()).unwrap(), seed);
    }

    /// **An anchor list of the wrong length is an error, not a shorter list.** This is the failure this
    /// codec exists to prevent: a truncated or padded leaf would still *draw a set*, so a
    /// mis-encoded seed would not crash — it would produce a deterministic-but-wrong active set and
    /// surface much later as a post-state mismatch. Every length disagreement is refused here.
    #[test]
    fn an_epoch_seed_anchor_list_of_the_wrong_length_is_refused() {
        let seed = EpochSeed {
            epoch: 1,
            anchors: vec![Blake2b256Hash::create(b"a"), Blake2b256Hash::create(b"b")],
        };
        let encoded = seed.encode();

        // One byte short of the declared anchor list.
        assert!(
            EpochSeed::decode(&encoded[..encoded.len() - 1]).is_err(),
            "a truncated anchor list must be refused"
        );
        // One byte long.
        let mut padded = encoded.clone();
        padded.push(0);
        assert!(
            EpochSeed::decode(&padded).is_err(),
            "a padded anchor list must be refused"
        );
        // A header that disagrees with the body: claim three hashes, carry two.
        let mut lying = encoded.clone();
        lying[8..16].copy_from_slice(&3u64.to_le_bytes());
        assert!(
            EpochSeed::decode(&lying).is_err(),
            "a count that disagrees with the body must be refused"
        );
        // And a header too short to hold the epoch and count.
        assert!(EpochSeed::decode(&[0u8; 15]).is_err());
    }

    /// An *absent* seed is distinguishable from a zeroed one — the reader a boundary uses to decide
    /// whether a draw has been seeded at all.
    #[test]
    fn an_epoch_seed_with_no_anchors_is_still_a_seed() {
        let seed = EpochSeed {
            epoch: 0,
            anchors: vec![],
        };
        assert_eq!(EpochSeed::decode(&seed.encode()).unwrap(), seed);
    }
}

// ----------------------------------------------------------------------------------------------
// Randomised active-set selection (audit follow-on; see `docs/src/node/security-audit.md` §8).
//
// In the append-only tail for the reason the seam above gives: everything here is new, and new code
// at the end costs no citations.
// ----------------------------------------------------------------------------------------------

/// Domain separator for the epoch draw's RNG input.
///
/// Binding the epoch *and* a purpose string means a seed leaf cannot be replayed for the wrong epoch,
/// and cannot collide with any other use of `Blake2b512Random` seeded from similar bytes.
const EPOCH_DRAW_DOMAIN: &[u8] = b"rchain:pos:epoch-draw:v1";

/// Domain separator for the **anchor** one epoch's seed contributes to the next.
///
/// A separate string from [`EPOCH_DRAW_DOMAIN`] on purpose: the same `EpochSeed` is both an RNG input
/// and an input to its successor, and a shared prefix would make those two uses one value.
const EPOCH_SEED_ANCHOR_DOMAIN: &[u8] = b"rchain:pos:epoch-seed-anchor:v1";

/// One epoch's seed reduced to the digest the next epoch's seed folds in.
///
/// This is what makes the seed sequence a hash chain: `seed_{k+1}` depends on `seed_k`, so every seed
/// commits to all of its predecessors and no epoch's entropy can be replaced without changing every
/// later seed. Freshness comes free with it — consecutive seeds differ even on a chain where nothing
/// else did.
pub fn previous_seed_anchor(seed: &EpochSeed) -> Blake2b256Hash {
    let encoded = seed.rng_input();
    Blake2b256Hash::create_many(&[EPOCH_SEED_ANCHOR_DOMAIN, &encoded])
}

// Imported here rather than at the top of the file: a `use` above line 3115 shifts every cited line.
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;

impl EpochSeed {
    /// The bytes handed to [`Blake2b512Random::from_init`] — domain-separated and epoch-bound.
    pub fn rng_input(&self) -> Vec<u8> {
        let body = self.encode();
        let mut out = Vec::with_capacity(EPOCH_DRAW_DOMAIN.len() + body.len());
        out.extend_from_slice(EPOCH_DRAW_DOMAIN);
        out.extend_from_slice(&body);
        out
    }
}

/// The seed the **genesis** active set is drawn from.
///
/// A fixed constant rather than a `PosGenesis` field, and that is a deliberate choice: a config value
/// would let two nodes disagree about the genesis post-state hash, which is a config-driven fork — the
/// same failure `spec/RUST-VS-SCALA.md` §3 item 5 names for `MAX_BLOCK_DEPLOYS`. A constant cannot
/// disagree. It is also *sound* despite being public: the genesis pool is fixed in the genesis file, so
/// a seed that produced a set you preferred would be a different chain's genesis, not this one's.
///
/// An empty anchor list is not "unseeded" — [`EPOCH_DRAW_DOMAIN`] plus epoch zero is the seed. It is
/// empty because genesis has no prior state to anchor to, and inventing a fake anchor would pretend the
/// chain had history it does not.
pub fn genesis_epoch_seed() -> EpochSeed {
    EpochSeed {
        epoch: 0,
        anchors: Vec::new(),
    }
}

/// A uniform `i128` in `[0, bound)`, rejection-sampled — never reduced modulo.
///
/// A modulo biases the low values when `bound` does not divide the draw's range, and "negligible bias"
/// is not a phrase this codebase accepts on a consensus path. `accept` is the largest multiple of
/// `bound` that fits a non-negative `i128`, so every accepted value maps to exactly
/// `i128::MAX / bound` rejected-or-taken values and `value % bound` is uniform over `[0, bound)`.
///
/// **Why the draw is taken as a signed `i128` and not a `u128`.** `i128::from_le_bytes` is *total*,
/// where a `u128` bound would have to come from the pool's stakes through a conversion that either
/// flattens an impossible failure to a value (the silent-defaulting class the type-system gate exists
/// for) or carries a cast. Drawing in the signed domain keeps every conversion in this function
/// infallible, at the cost of halving the acceptance rate — the non-negative half of the space — which
/// is one extra stream read on average and nothing else.
///
/// **The rejection loop consumes the stream a data-dependent number of times.** That is harmless here
/// and it is checked rather than assumed: nothing reads the stream after a draw, and the draw's output
/// is pinned by a known-answer test, so a change in consumption shows up there.
fn uniform_below_i128(rand: &mut Blake2b512Random, bound: i128) -> i128 {
    if bound <= 1 {
        return 0;
    }
    let accept = (i128::MAX / bound) * bound;
    let mut buf = [0u8; std::mem::size_of::<i128>()];
    loop {
        let draw = rand.next();
        buf.copy_from_slice(&draw[..std::mem::size_of::<i128>()]);
        let value = i128::from_le_bytes(buf);
        if value >= 0 && value < accept {
            return value % bound;
        }
    }
}

/// Draw `count` distinct entries from `candidates` **in proportion to their weight**, without
/// replacement, in draw order.
///
/// The walk is a sequential weighted draw: take a uniform value in `[0, remaining_total)`, walk the
/// candidate list in its canonical order accumulating weights, take the first entry whose cumulative
/// weight exceeds the value, remove it, repeat. Exact integer arithmetic throughout — no floats, no
/// logarithms, no rounding, so `(pool, seed)` determines the result on every machine. `candidates` is
/// built from a `BTreeMap`, so its order is key order, and `Vec::remove` preserves that order across
/// the iteration, which is what makes the walk canonical.
///
/// **Exact for the first slot, and that is the property that matters.** The first pick is
/// `stake / total` exactly. The later picks renormalise over what is left, so they are proportional to
/// the *remaining* weights rather than to the original ones; an item's expected share of `count` slots
/// is therefore near — not identically — `count · stake / total`. The exact-proportional alternative
/// is a systematic (rotated-interval) scheme, which is a different rule with a different variance and
/// a collision problem for any stake above `1/count` of the total. This is the sequential rule, and
/// the residual is stated where the rule is documented rather than left implicit.
fn draw_weighted_without_replacement<T>(
    candidates: &[T],
    count: usize,
    rand: &mut Blake2b512Random,
) -> Vec<T>
where
    T: Clone + Weighted,
{
    let mut remaining: Vec<T> = candidates.to_vec();
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        // A sum of `i128` weights: each is `i64`-bounded (the stake type), so the sum leaves `i128`
        // only at ~2^64 candidates — a pool that cannot be held, let alone drawn from.
        let total: i128 = remaining.iter().map(|c| c.weight()).sum();
        if total <= 0 {
            break;
        }
        let draw = uniform_below_i128(rand, total);
        let mut cumulative: i128 = 0;
        let mut pick = remaining.len() - 1;
        for (index, candidate) in remaining.iter().enumerate() {
            cumulative += candidate.weight();
            if draw < cumulative {
                pick = index;
                break;
            }
        }
        out.push(remaining.remove(pick));
    }
    out
}

/// A candidate's weight in the active-set draw: its stake.
///
/// A trait rather than a closure so the helper stays generic and the weight is read in exactly one
/// place. It is implemented for the `(validator, stake)` pair the pool yields, and it returns an
/// `i128` because `i128::from` is a *total* widening of the `i64` the refinement wraps — every other
/// width would need a conversion that can fail, and a weight that silently fell to `0` would drop a
/// candidate out of the draw without saying so.
trait Weighted {
    fn weight(&self) -> i128;
}

impl Weighted for (&Validator, NonNegI64) {
    fn weight(&self) -> i128 {
        // Total: `i128::from(i64)` is a widening conversion, and `i64::from` on the refinement is the
        // macro's own infallible accessor. Nothing here can fail, so nothing here can default.
        i128::from(i64::from(self.1))
    }
}

// ----------------------------------------------------------------------------------------------
// The seed writer (phase 3 of the randomised selection): `close_block` step 5.
//
// Appended for the seam's reason. These tests are the ones that make step 5 more than a claim:
// `close_block` writing the wrong thing — the drawing block's own pre-state, or nothing at all —
// leaves every other test in this file passing.
// ----------------------------------------------------------------------------------------------

#[cfg(test)]
mod epoch_seed_writer_tests {
    use super::*;

    /// A genesis-installed state with `size` equal-stake validators and an active-set cap of `cap`.
    ///
    /// `epoch_length: 0` means every block is a boundary and every block's epoch index is its own
    /// block number, which keeps the tests below about the *seed* rather than about the arithmetic of
    /// where boundaries fall.
    async fn pos_with_pool(size: u8, cap: i64) -> NativeSystemState {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let validators: Vec<Validator> = (1..=size)
            .map(|i| Validator::from_slice(&[i; 65]))
            .collect();
        let bonds: BTreeMap<Validator, NonNegI64> = validators
            .iter()
            .map(|v| (*v, NonNegI64::try_from(10).expect("a test stake")))
            .collect();
        native
            .install_genesis(&PosGenesis {
                bonds,
                trusted: validators.into_iter().collect(),
                params: PosParams {
                    epoch_length: 0,
                    number_of_active_validators: cap,
                    ..PosParams::default()
                },
                dev_vesting: None,
            })
            .unwrap();
        native
    }

    /// **The property the old rule did not have, and the reason this change exists.**
    ///
    /// The set drawn at a boundary comes from the seed the *previous* boundary wrote, so nothing the
    /// drawing block's proposer can vary moves it. Under the rule this replaced the seed was the
    /// drawing block's own pre-state, which the proposer reaches through its justification set; now the
    /// block's contribution is the value it *writes* for the next boundary. Two runs over the same
    /// history, drawing at the same boundary with two different values written there, must therefore
    /// draw the **same** set.
    ///
    /// The second assertion is what makes the first mean something. Without it the test passes if the
    /// value is ignored entirely, which is the same as not having it: the seed written for the
    /// *following* boundary must differ between the runs, so the mutation provably reached the code.
    ///
    /// **Falsified by construction, and the numbers are the point.** Replacing step 4's `epoch_seed`
    /// read with the drawing block's own pre-state — the rule this change removes — fails this test
    /// and exactly two others in this file; the other 45, and the whole rest of `native_state`'s
    /// suite, still pass. That is the audit's claim about the old rule, reproduced as a failing test:
    /// a proposer that varied its justifications was varying the draw, and nothing else in the tree
    /// noticed.
    #[tokio::test]
    async fn the_drawn_set_does_not_move_with_the_anchor_the_drawing_block_writes() {
        let anchor_b1 = Blake2b256Hash::create(b"B1 pre-state");
        let a = pos_with_pool(4, 2).await;
        let b = pos_with_pool(4, 2).await;
        a.close_block(1, anchor_b1, &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        b.close_block(1, anchor_b1, &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            a.epoch_seed().await.unwrap(),
            b.epoch_seed().await.unwrap(),
            "the two histories agree up to the boundary that seeds epoch 2"
        );

        // The same boundary, two different pre-states — i.e. two different justification sets.
        a.close_block(
            2,
            Blake2b256Hash::create(b"B2 pre-state, justification set 1"),
            &BTreeMap::new(),
        )
        .await
        .unwrap()
        .unwrap();
        b.close_block(
            2,
            Blake2b256Hash::create(b"B2 pre-state, justification set 2"),
            &BTreeMap::new(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(
            a.active().await.unwrap(),
            b.active().await.unwrap(),
            "the set drawn at a block must not depend on that block's own pre-state"
        );
        assert_ne!(
            a.epoch_seed().await.unwrap(),
            b.epoch_seed().await.unwrap(),
            "…and the pre-state must still reach the seed the next boundary draws with, or this test \
             would pass for the wrong reason"
        );
    }

    /// The seed is a **hash chain**: the previous seed is folded into the next one, so consecutive
    /// boundaries cannot produce the same seed even when the pre-state hash repeats.
    ///
    /// Falsifiable by deletion, which is how it was checked: drop `previous_seed_anchor` from
    /// `close_block` step 5 and this test fails while every other test in the file still passes.
    #[tokio::test]
    async fn consecutive_seeds_differ_even_when_the_pre_state_hash_repeats() {
        let native = pos_with_pool(4, 2).await;
        let repeated = Blake2b256Hash::create(b"the same pre-state twice");

        native
            .close_block(1, repeated, &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        let first = native.epoch_seed().await.unwrap().expect("seeded at B1");
        native
            .close_block(2, repeated, &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        let second = native.epoch_seed().await.unwrap().expect("seeded at B2");

        assert_ne!(
            first, second,
            "the same pre-state at two boundaries must still give two seeds"
        );
        assert_eq!(
            second.anchors.first().copied(),
            Some(previous_seed_anchor(&first)),
            "and the first anchor must be the previous seed's anchor, which is what carries the chain"
        );
    }

    /// The anchors the writer records are the previous seed's anchor and **the fringe state hash this
    /// block extends**, in that order. Pinned exactly, because a swap is invisible to the two tests
    /// above — both anchors still move when the pre-state moves, and the chain still advances.
    #[tokio::test]
    async fn the_written_seed_anchors_are_the_previous_seed_and_this_blocks_fringe_state() {
        let native = pos_with_pool(4, 2).await;
        let b1 = Blake2b256Hash::create(b"B1 pre-state");
        native
            .close_block(1, b1, &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        let seed_at_b1 = native.epoch_seed().await.unwrap().expect("seeded at B1");

        let b2 = Blake2b256Hash::create(b"B2 pre-state");
        native
            .close_block(2, b2, &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        let seed_at_b2 = native.epoch_seed().await.unwrap().expect("seeded at B2");

        assert_eq!(
            seed_at_b2.anchors,
            vec![previous_seed_anchor(&seed_at_b1), b2],
            "the seed written at a boundary must anchor to the previous seed and this block's fringe"
        );
        assert_eq!(
            seed_at_b2.epoch, 3,
            "and it must be labelled with the epoch that will draw from it"
        );
    }

    /// A genesis-installed state has **no** seed leaf until its first boundary. The distinction is
    /// load-bearing: `close_block` treats an absent leaf as "draw from the genesis constant", so a
    /// zeroed seed and no seed at all must not be the same value.
    #[tokio::test]
    async fn genesis_has_no_seed_leaf_and_its_first_boundary_writes_one() {
        let native = pos_with_pool(2, 1).await;
        assert_eq!(
            native.epoch_seed().await.unwrap(),
            None,
            "genesis must not write a seed: the constant is not a window, and the leaf's absence is \
             how the boundary tells the two apart"
        );
        native
            .close_block(1, Blake2b256Hash::create(b"B1"), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(
            native.epoch_seed().await.unwrap().is_some(),
            "the first boundary must leave a seed behind for the next one"
        );
    }

    /// Why the genesis constant is a constant and not a `PosGenesis` field: the draw it seeds has to
    /// be identical on two nodes that installed the same genesis, with no configuration input. This
    /// is the property `casper/tests/consensus.rs`'s bonds-cache canary fails on if it is broken.
    #[tokio::test]
    async fn two_genesis_installs_of_the_same_pool_draw_the_same_set() {
        let a = pos_with_pool(4, 2).await;
        let b = pos_with_pool(4, 2).await;
        assert_eq!(
            a.active().await.unwrap(),
            b.active().await.unwrap(),
            "the genesis draw must be a pure function of the genesis file"
        );
    }
}

// ==================================================================================================
// Delegated stake (`pos:delegations` and its three siblings, law 57, #193)
//
// Appended rather than placed with its siblings, for the reason the `pos:epoch_seed` section above
// gives: this file's line numbers are cited by Lean declarations and by hand-maintained spec rows, and
// an insertion anywhere above them shifts every one. The tail of this file is append-only.
//
// **What the primitive is.** A key that holds REV but does not run a node can stake it on one that
// does. `pos:bonds` becomes the **aggregate** stake on a key — the operator's own plus every
// delegation to it — which is the whole of what delegation changes for consensus: `compute_bonds`
// reads `pos:active`, `select_active` derives it from the pool, and so a delegated principal counts in
// the draw and in what a slash reaches exactly as the operator's own does. The four leaves here
// record *whose* the aggregate is, and the four rules that read them are `delegate`, `undelegate`,
// the epoch split in `close_block`, and the fan-out in `slash`.
//
// **The one shape this file has never had before: a map keyed by a pair.** Every other PoS leaf is
// keyed by a single `Validator`; a delegation belongs to two identities, so the key is
// `DelegationKey` and the canonical encoding is two 65-byte identities rather than one.
// ==================================================================================================

/// **A delegation's key: one operator and one delegator**, so the ledger is a map from pairs.
///
/// Ordered by `Validator`'s own `Ord` — operator first, then delegator — which is what makes the
/// canonical encoding deterministic without a second sort rule: a `BTreeMap<DelegationKey, _>` iterates
/// in exactly the order the fixed-stride encoding writes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DelegationKey {
    pub operator: Validator,
    pub delegator: Validator,
}

/// An escrowed undelegated principal (`pos:delegation_claims`): the amount out of the pool and the
/// block it may be paid at. `Withdrawal`'s shape with a delegator as the payee — and, like it, the
/// amount is the **principal only**: the accrued reward is read from the ledger at payment time, which
/// is what lets a delegator be paid for the epoch it left in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DelegationClaim {
    pub amount: NonNegI64,
    pub deadline: i64,
}

/// Two 65-byte identities and one LE amount. Shared by `pos:delegations`, `pos:pending_delegations`
/// and `pos:delegated_rewards` — the three are the same record read as a principal, a deadline and a
/// credit respectively.
const DELEGATION_ENTRY_LEN: usize = 2 * VALIDATOR_LEN + 8;

/// Two identities, a LE amount and a LE deadline — `pos:delegation_claims`, the analogue of
/// `WITHDRAWER_ENTRY_LEN`.
const DELEGATION_CLAIM_LEN: usize = 2 * VALIDATOR_LEN + 8 + 8;

/// Leaf key for the delegated-principal ledger (`(operator, delegator) → amount`).
pub fn pos_delegations_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:delegations")
}

/// Leaf key for staged undelegation requests (`(operator, delegator) → deadline`).
pub fn pos_pending_delegations_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:pending_delegations")
}

/// Leaf key for escrowed undelegated principals (`(operator, delegator) → (amount, deadline)`).
pub fn pos_delegation_claims_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:delegation_claims")
}

/// Leaf key for a delegator's accrued reward (`(operator, delegator) → amount`). Held **apart** from
/// the operator's `pos:committed` entry on purpose: money in that map is money the operator is paid,
/// so a delegator's share credited there would be the operator's to keep.
pub fn pos_delegated_rewards_key() -> Blake2b256Hash {
    Blake2b256Hash::create(b"pos:delegated_rewards")
}

/// Canonically encode a `(operator, delegator) → NonNegI64` map: two 65-byte identities then the LE
/// amount, in `BTreeMap` order.
pub fn encode_delegations(ledger: &BTreeMap<DelegationKey, NonNegI64>) -> Vec<u8> {
    let mut out = Vec::with_capacity(ledger.len() * DELEGATION_ENTRY_LEN);
    for (key, amount) in ledger {
        out.extend_from_slice(key.operator.as_bytes());
        out.extend_from_slice(key.delegator.as_bytes());
        out.extend_from_slice(&i64::from(*amount).to_le_bytes());
    }
    out
}

/// Decode [`encode_delegations`]' output. Refuses a length that is not a whole number of entries, and
/// refuses a negative amount — a leaf written by a buggy peer is an error rather than a silently
/// negative stake.
pub fn decode_delegations(bytes: &[u8]) -> Result<BTreeMap<DelegationKey, NonNegI64>, String> {
    decode_delegation_amounts(bytes, "delegations")
}

/// Canonically encode the accrued-reward ledger — the same record as [`encode_delegations`], and
/// deliberately the same codec: the port's `committed` map shares `encode_bonds` for the same reason,
/// because the two are the same wire shape and a second codec is a second thing to get wrong.
pub fn encode_delegated_rewards(ledger: &BTreeMap<DelegationKey, NonNegI64>) -> Vec<u8> {
    encode_delegations(ledger)
}

/// Decode [`encode_delegated_rewards`]' output.
pub fn decode_delegated_rewards(
    bytes: &[u8],
) -> Result<BTreeMap<DelegationKey, NonNegI64>, String> {
    decode_delegation_amounts(bytes, "delegated rewards")
}

/// The shared body of the two amount-valued delegation ledgers. `leaf` names the caller for the error
/// message, which is the only thing that differs between them.
fn decode_delegation_amounts(
    bytes: &[u8],
    leaf: &str,
) -> Result<BTreeMap<DelegationKey, NonNegI64>, String> {
    if bytes.len() % DELEGATION_ENTRY_LEN != 0 {
        return Err(format!(
            "{leaf} encoding has {} bytes, not a multiple of {DELEGATION_ENTRY_LEN}",
            bytes.len()
        ));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(DELEGATION_ENTRY_LEN) {
        let operator = Validator::from_slice(&chunk[..VALIDATOR_LEN]);
        let delegator = Validator::from_slice(&chunk[VALIDATOR_LEN..2 * VALIDATOR_LEN]);
        let amount: [u8; 8] = chunk[2 * VALIDATOR_LEN..DELEGATION_ENTRY_LEN]
            .try_into()
            .map_err(|_| format!("{leaf}: invalid amount field"))?;
        let amount = NonNegI64::try_from(i64::from_le_bytes(amount))
            .map_err(|_| format!("{leaf}: negative amount"))?;
        out.insert(
            DelegationKey {
                operator,
                delegator,
            },
            amount,
        );
    }
    Ok(out)
}

/// Canonically encode staged undelegation requests: two identities then the LE deadline.
pub fn encode_pending_delegations(pending: &BTreeMap<DelegationKey, i64>) -> Vec<u8> {
    let mut out = Vec::with_capacity(pending.len() * DELEGATION_ENTRY_LEN);
    for (key, deadline) in pending {
        out.extend_from_slice(key.operator.as_bytes());
        out.extend_from_slice(key.delegator.as_bytes());
        out.extend_from_slice(&deadline.to_le_bytes());
    }
    out
}

/// Decode [`encode_pending_delegations`]' output.
pub fn decode_pending_delegations(bytes: &[u8]) -> Result<BTreeMap<DelegationKey, i64>, String> {
    if bytes.len() % DELEGATION_ENTRY_LEN != 0 {
        return Err(format!(
            "pending delegations encoding has {} bytes, not a multiple of {DELEGATION_ENTRY_LEN}",
            bytes.len()
        ));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(DELEGATION_ENTRY_LEN) {
        let operator = Validator::from_slice(&chunk[..VALIDATOR_LEN]);
        let delegator = Validator::from_slice(&chunk[VALIDATOR_LEN..2 * VALIDATOR_LEN]);
        let deadline: [u8; 8] = chunk[2 * VALIDATOR_LEN..DELEGATION_ENTRY_LEN]
            .try_into()
            .map_err(|_| "pending delegations: invalid deadline field".to_string())?;
        out.insert(
            DelegationKey {
                operator,
                delegator,
            },
            i64::from_le_bytes(deadline),
        );
    }
    Ok(out)
}

/// Canonically encode escrowed undelegated principals: two identities, the LE amount, the LE deadline.
pub fn encode_delegation_claims(claims: &BTreeMap<DelegationKey, DelegationClaim>) -> Vec<u8> {
    let mut out = Vec::with_capacity(claims.len() * DELEGATION_CLAIM_LEN);
    for (key, claim) in claims {
        out.extend_from_slice(key.operator.as_bytes());
        out.extend_from_slice(key.delegator.as_bytes());
        out.extend_from_slice(&i64::from(claim.amount).to_le_bytes());
        out.extend_from_slice(&claim.deadline.to_le_bytes());
    }
    out
}

/// Decode [`encode_delegation_claims`]' output.
pub fn decode_delegation_claims(
    bytes: &[u8],
) -> Result<BTreeMap<DelegationKey, DelegationClaim>, String> {
    if bytes.len() % DELEGATION_CLAIM_LEN != 0 {
        return Err(format!(
            "delegation claims encoding has {} bytes, not a multiple of {DELEGATION_CLAIM_LEN}",
            bytes.len()
        ));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(DELEGATION_CLAIM_LEN) {
        let operator = Validator::from_slice(&chunk[..VALIDATOR_LEN]);
        let delegator = Validator::from_slice(&chunk[VALIDATOR_LEN..2 * VALIDATOR_LEN]);
        let amount: [u8; 8] = chunk[2 * VALIDATOR_LEN..2 * VALIDATOR_LEN + 8]
            .try_into()
            .map_err(|_| "delegation claims: invalid amount field".to_string())?;
        let deadline: [u8; 8] = chunk[2 * VALIDATOR_LEN + 8..DELEGATION_CLAIM_LEN]
            .try_into()
            .map_err(|_| "delegation claims: invalid deadline field".to_string())?;
        let amount = NonNegI64::try_from(i64::from_le_bytes(amount))
            .map_err(|_| "delegation claims: negative amount".to_string())?;
        out.insert(
            DelegationKey {
                operator,
                delegator,
            },
            DelegationClaim {
                amount,
                deadline: i64::from_le_bytes(deadline),
            },
        );
    }
    Ok(out)
}

/// **The pro-rata split both halves of delegated stake run**, mirroring `Rchain.proRata` in
/// `spec/Rchain/Pos.lean` (law 57).
///
/// `amount` is divided across `weights` in order, and `base` is a weight that is **part of the
/// denominator but is issued no share** — the operator's own stake in the epoch split, and `0` in the
/// slash's refund fan-out. One floor per share, so the shares sum to at most `amount` and the
/// difference is the dust, which the caller gives to the operator.
///
/// Exact integers throughout, as everywhere else in this mechanism: `i128` for the product, a checked
/// conversion out, and no float anywhere.
pub(crate) fn pro_rata(amount: i64, base: i64, weights: &[i64]) -> Result<Vec<i64>, String> {
    let total: i128 = i128::from(base) + weights.iter().map(|w| i128::from(*w)).sum::<i128>();
    let mut out = Vec::with_capacity(weights.len());
    for w in weights {
        // A zero denominator divides to zero rather than faulting, which is the same choice
        // `epoch_reward` makes where the contract's arithmetic is undefined, and it can only be
        // reached by an empty delegation list with a zero base.
        let share = if total == 0 {
            0
        } else {
            checked_i64(
                i128::from(amount) * i128::from(*w) / total,
                "pro-rata share",
            )?
        };
        out.push(share);
    }
    Ok(out)
}

/// The principal an operator's key carries on behalf of others: the sum of its delegations, in the
/// ledger's canonical order.
///
/// Fallible rather than saturating. A sum of `NonNegI64` values can exceed `i64`, and clamping it to
/// `i64::MAX` would be a *silent* partiality of exactly the kind `spec/TYPE-SYSTEM.md` forbids: the
/// caller would go on to compare an aggregate against a wrong total instead of refusing.
fn delegated_total(
    ledger: &BTreeMap<DelegationKey, NonNegI64>,
    operator: &Validator,
) -> Result<i64, String> {
    checked_i64(
        ledger
            .iter()
            .filter(|(key, _)| key.operator == *operator)
            .map(|(_, amount)| i128::from(i64::from(*amount)))
            .sum::<i128>(),
        "delegated total",
    )
}

/// The delegators of one operator, with their principals, in canonical order.
fn delegations_of(
    ledger: &BTreeMap<DelegationKey, NonNegI64>,
    operator: &Validator,
) -> Vec<(DelegationKey, i64)> {
    ledger
        .iter()
        .filter(|(key, _)| key.operator == *operator)
        .map(|(key, amount)| (*key, i64::from(*amount)))
        .collect()
}

/// The keys of one operator in any of the four delegation ledgers, in canonical order.
///
/// Generic over the value type so the four ledgers share one traversal: the slash's fan-out and the
/// boundary's move both need "everything that belongs to this key", and four near-identical loops
/// would be four places for the filter to drift.
fn operator_keys<K>(
    ledger: &BTreeMap<DelegationKey, K>,
    operator: &Validator,
) -> Vec<DelegationKey> {
    ledger
        .keys()
        .filter(|key| key.operator == *operator)
        .copied()
        .collect()
}

/// Accumulate one delegator's at-risk holdings, refusing an overflow rather than wrapping or clamping.
///
/// A delegator is **one payee** however many roles it has on one operator's key — a principal, an
/// escrow carried over from an undelegation, and an accrual — so the fan-out sums them under the
/// delegator, and the sum is what its share is proportional to.
fn add_delegator_risk(
    parts: &mut BTreeMap<Validator, i64>,
    delegator: Validator,
    amount: i64,
) -> Result<(), String> {
    let held = parts.entry(delegator).or_insert(0);
    *held = held
        .checked_add(amount)
        .ok_or_else(|| format!("a delegator's holdings overflow: {held} + {amount}"))?;
    Ok(())
}

/// **Split one validator's epoch reward across its own stake and its delegations**, returning the
/// delegators' shares (canonical order, same order as [`delegations_of`]) and the operator's
/// remainder.
///
/// The denominator is the operator's **aggregate** stake — `own + Σ amounts` — which is the pool entry
/// itself, so this is a partition of a reward computed *before* the split: `Σ shares + operator ==
/// reward` exactly, for any inputs (law 57's `split_sums_to_the_reward`). That exactness is what makes
/// a validator with delegators commit the same total it would have committed alone, and so what keeps
/// law 46 true through the split rather than only before it.
///
/// **With no delegations this is the identity**: no shares, and the operator's remainder is the whole
/// reward — the arithmetic half of the port's dormancy requirement.
pub(crate) fn split_reward(
    reward: i64,
    own: i64,
    amounts: &[i64],
) -> Result<(Vec<i64>, i64), String> {
    let shares = pro_rata(reward, own, amounts)?;
    let paid: i128 = shares.iter().map(|s| i128::from(*s)).sum();
    let remainder = checked_i64(i128::from(reward) - paid, "operator reward remainder")?;
    Ok((shares, remainder))
}

impl NativeSystemState {
    /// Write a `pos:*` leaf, or **clear it when the value is empty**.
    ///
    /// This is not the same as writing an empty map. `set_bonds` and its siblings put unconditionally,
    /// so an empty value is a real trie leaf and a **different state root** from an absent one — which
    /// matters here and nowhere else, because the delegation leaves must be *absent* on a chain that
    /// has never delegated. That is what puts this primitive's fork point at the first `delegate`
    /// deploy rather than at genesis (law 57, pass §60).
    ///
    /// The `get` before the `delete` is deliberate: a delete of a key that is not there would still be
    /// a store action, and the point of this function is that it changes nothing when there is nothing
    /// to change.
    async fn store_optional_pos_leaf(
        &self,
        key: Blake2b256Hash,
        value: Option<Vec<u8>>,
    ) -> Result<(), String> {
        match value {
            Some(bytes) => {
                self.store.put(PREFIX_POS, key, bytes);
                Ok(())
            }
            None => {
                if self.store.get(PREFIX_POS, &key).await?.is_some() {
                    self.store.delete(PREFIX_POS, &key);
                }
                Ok(())
            }
        }
    }

    /// The delegated-principal ledger (`pos:delegations`). Absent reads as empty, like every other
    /// ledger here.
    pub async fn delegations(&self) -> Result<BTreeMap<DelegationKey, NonNegI64>, String> {
        match self.store.get(PREFIX_POS, &pos_delegations_key()).await? {
            Some(bytes) => decode_delegations(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Write the delegated-principal ledger, clearing the leaf when it becomes empty.
    pub async fn set_delegations(
        &self,
        ledger: &BTreeMap<DelegationKey, NonNegI64>,
    ) -> Result<(), String> {
        let value = (!ledger.is_empty()).then(|| encode_delegations(ledger));
        self.store_optional_pos_leaf(pos_delegations_key(), value)
            .await
    }

    /// Staged undelegation requests (`pos:pending_delegations`).
    pub async fn pending_delegations(&self) -> Result<BTreeMap<DelegationKey, i64>, String> {
        match self
            .store
            .get(PREFIX_POS, &pos_pending_delegations_key())
            .await?
        {
            Some(bytes) => decode_pending_delegations(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Write the staged undelegation requests, clearing the leaf when it becomes empty.
    pub async fn set_pending_delegations(
        &self,
        pending: &BTreeMap<DelegationKey, i64>,
    ) -> Result<(), String> {
        let value = (!pending.is_empty()).then(|| encode_pending_delegations(pending));
        self.store_optional_pos_leaf(pos_pending_delegations_key(), value)
            .await
    }

    /// Escrowed undelegated principals (`pos:delegation_claims`).
    pub async fn delegation_claims(
        &self,
    ) -> Result<BTreeMap<DelegationKey, DelegationClaim>, String> {
        match self
            .store
            .get(PREFIX_POS, &pos_delegation_claims_key())
            .await?
        {
            Some(bytes) => decode_delegation_claims(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Write the escrowed undelegated principals, clearing the leaf when it becomes empty.
    pub async fn set_delegation_claims(
        &self,
        claims: &BTreeMap<DelegationKey, DelegationClaim>,
    ) -> Result<(), String> {
        let value = (!claims.is_empty()).then(|| encode_delegation_claims(claims));
        self.store_optional_pos_leaf(pos_delegation_claims_key(), value)
            .await
    }

    /// Delegators' accrued rewards (`pos:delegated_rewards`).
    pub async fn delegated_rewards(&self) -> Result<BTreeMap<DelegationKey, NonNegI64>, String> {
        match self
            .store
            .get(PREFIX_POS, &pos_delegated_rewards_key())
            .await?
        {
            Some(bytes) => decode_delegated_rewards(&bytes),
            None => Ok(BTreeMap::new()),
        }
    }

    /// Write the delegators' accrued rewards, clearing the leaf when it becomes empty. `epoch_pot`
    /// subtracts this ledger as well as `committed`, or a boundary would distribute a delegator's
    /// accrued reward a second time.
    pub async fn set_delegated_rewards(
        &self,
        rewards: &BTreeMap<DelegationKey, NonNegI64>,
    ) -> Result<(), String> {
        let value = (!rewards.is_empty()).then(|| encode_delegated_rewards(rewards));
        self.store_optional_pos_leaf(pos_delegated_rewards_key(), value)
            .await
    }

    /// **Delegate stake to an operator's key** (`delegate`, law 57).
    ///
    /// The principal leaves the **delegator's own** vault — the caller's identity is what authorises
    /// it, exactly as `bond` does — and joins the staking vault; the operator's `pos:bonds` entry
    /// becomes the aggregate, and the ledger records whose the difference is. It activates exactly as
    /// `bond` does: the pool follows at once and `pos:active` only at the boundary.
    ///
    /// **Open admission, and the refusals that make it safe.** No commission, no consent step and no
    /// `pos:trusted` requirement: the point of the primitive is a key with REV and no node, which is by
    /// definition a key that was never admitted to the stakeholder group. What is refused is what would
    /// make the accounting ambiguous or unbounded:
    ///
    /// - **self-delegation** — the operator's own stake and a delegation to itself would be the same
    ///   money counted as two parties, and the split would pay one of them twice;
    /// - an **operator that is not in the pool**, because there is no entry to enlarge;
    /// - an operator with a **pending withdrawal**, because that request will move the whole pool entry
    ///   into an escrow the delegator has no claim on (and `withdraw` refuses the other direction, so a
    ///   validator is never simultaneously withdrawing and delegated-to);
    /// - an amount below **`minimum_bond`**, which is the per-delegation floor and therefore the DoS
    ///   control on a ledger unbounded in the number of delegators (`spec/RUST-VS-SCALA.md` §3 item 12,
    ///   residual O5);
    /// - and an amount the delegator's vault cannot cover, which is a failure rather than a partial
    ///   transfer.
    ///
    /// **`maximum_bond` deliberately does not apply.** It bounds what a validator may stake as itself;
    /// a delegation is floored and not capped, because refusing a delegation on account of *another*
    /// delegator's principal would make one staker's entry decide a stranger's.
    pub async fn delegate(
        &self,
        delegator: &Validator,
        operator: &Validator,
        amount: NonNegI64,
    ) -> Result<Result<(), String>, String> {
        if delegator == operator {
            return Ok(Err(
                "A key cannot delegate to itself: its own stake and the delegation would be the \
                 same money counted as two parties."
                    .to_string(),
            ));
        }
        // **A jailed operator is not an operator anyone should be buying into.** Its absence is
        // already being priced; admit no new principal behind it while a jail or an open slash vote
        // runs. Existing delegations are untouched — a jail moves no stake.
        if self
            .liveness()
            .await?
            .get(operator)
            .is_some_and(AbsenceRecord::is_jailed)
            || self.vote_windows().await?.contains_key(operator)
        {
            return Ok(Err(
                "the operator is jailed for sustained absence or has an open slash vote; no new \
                 delegation is admitted while it runs"
                    .to_string(),
            ));
        }
        let mut pool = self.bonds().await?;
        if !pool.contains_key(operator) {
            return Ok(Err(
                "The operator is not bonded: only a pooled key can carry delegated stake."
                    .to_string(),
            ));
        }
        let pending = self.pending_withdrawers().await?;
        if pending.contains_key(operator) {
            return Ok(Err(
                "The operator has a pending withdrawal: delegating would put the principal into an \
                 escrow the delegator has no claim on."
                    .to_string(),
            ));
        }
        let params = self.params().await?;
        let stake = i64::from(amount);
        if stake < i64::from(params.minimum_bond) {
            return Ok(Err(format!(
                "A delegation is below the minimum ({} < {}).",
                stake,
                i64::from(params.minimum_bond)
            )));
        }
        let address = self.vault_address(delegator)?;
        let balance = self
            .vault_balance(&address)
            .await?
            .unwrap_or(NonNegI64::zero());
        if i64::from(balance) < stake {
            return Ok(Err(format!(
                "insufficient funds to delegate {stake} (have {})",
                i64::from(balance)
            )));
        }
        self.set_vault_balance(&address, balance_plus(balance, -stake, "delegation debit")?);
        self.credit_pos_vault(amount).await?;

        let aggregate = pool.get(operator).copied().unwrap_or(NonNegI64::zero());
        pool.insert(*operator, balance_plus(aggregate, stake, "aggregate bond")?);
        self.set_bonds(&pool);

        let mut ledger = self.delegations().await?;
        let key = DelegationKey {
            operator: *operator,
            delegator: *delegator,
        };
        let held = ledger.get(&key).copied().unwrap_or(NonNegI64::zero());
        // Insert-or-accumulate, mirroring `insert(k, lookup k + amount)`: a second delegation from the
        // same delegator **adds** to the first rather than replacing it (law 57's
        // `a_second_delegation_accumulates`). A replace here would silently burn the earlier principal
        // while the pool entry kept it, so the two would disagree for ever.
        ledger.insert(key, balance_plus(held, stake, "delegated principal")?);
        self.set_delegations(&ledger).await?;
        Ok(Ok(()))
    }

    /// **Add to the validators' reward pot** (`fundPot`): move `amount` from the funder's own vault into
    /// the staking vault with **no claim attached**.
    ///
    /// The pot a boundary splits is the staking vault minus every claim against it ([`epoch_pot`]), and
    /// until this existed the only REV that reached the vault unclaimed was the share of burned phlo
    /// `pay_executor` leaves there. `bond` and `delegate` also credit the vault, but each records a
    /// claim of equal size, so neither adds to the pot. So the one way to raise validator rewards was to
    /// change `executor_share` or mint, and both are genesis changes. With this, a later bonus — from a
    /// donor, the Coop, or a governance reserve — is an ordinary deploy, and the next boundary pays it
    /// out by stake like any other pot.
    ///
    /// Open to any key, like a transfer: the caller can only spend its own vault (the `deployerId`
    /// capability authorises it, as for `bond`), and the money it moves cannot come back to it except as
    /// a validator's reward. Refused for a zero amount, which would move nothing, and for an amount the
    /// vault cannot cover, which is a failure rather than a partial transfer.
    pub async fn fund_pot(
        &self,
        funder: &Validator,
        amount: NonNegI64,
    ) -> Result<Result<(), String>, String> {
        let stake = i64::from(amount);
        if stake == 0 {
            return Ok(Err("fundPot expects a positive amount.".to_string()));
        }
        let address = self.vault_address(funder)?;
        let balance = self
            .vault_balance(&address)
            .await?
            .unwrap_or(NonNegI64::zero());
        if i64::from(balance) < stake {
            return Ok(Err(format!(
                "insufficient funds to fund the pot with {stake} (have {})",
                i64::from(balance)
            )));
        }
        // Check both sides before either write: a failed credit must not consume the donor's REV.
        let remaining = balance_plus(balance, -stake, "pot funding debit")?;
        let funded = balance_plus(self.pos_vault_balance().await?, stake, "pot funding credit")?;
        self.set_vault_balance(&address, remaining);
        self.set_pos_vault_balance(funded);
        Ok(Ok(()))
    }

    /// **Stage an undelegation** (`undelegate`, law 57).
    ///
    /// A request and nothing else: the principal stays in the operator's pool entry and in the ledger,
    /// still earning and still at risk, until the boundary moves it into `pos:delegation_claims` with a
    /// deadline — after which it is paid `principal + accrued rewards` to the **delegator's own** vault
    /// at the first boundary past its quarantine. This is law 47's three-stage shape, seen from a
    /// delegator, and the staging is the point: undelegating cannot be used to dodge a slash that is
    /// already in flight, which paying at once would allow.
    ///
    /// Refused when there is no delegation to withdraw — an undelegation of nothing would otherwise
    /// create a claim the boundary would pay zero for, and the pending map would carry a tombstone for
    /// a pair that never delegated.
    pub async fn undelegate(
        &self,
        delegator: &Validator,
        operator: &Validator,
        block_number: i64,
    ) -> Result<Result<(), String>, String> {
        let ledger = self.delegations().await?;
        let key = DelegationKey {
            operator: *operator,
            delegator: *delegator,
        };
        if !ledger.contains_key(&key) {
            return Ok(Err("There is no delegation to withdraw.".to_string()));
        }
        let params = self.params().await?;
        // The same normalised divisor `withdraw` uses (AUDIT C135): the deadline is
        // `quarantine_length + divisor * (1 + block_number / divisor)`.
        let divisor = i128::from(epoch_divisor(&params));
        let deadline = checked_i64(
            i128::from(params.quarantine_length)
                + divisor * (1 + i128::from(block_number) / divisor),
            "undelegate deadline",
        )?;
        let mut pending = self.pending_delegations().await?;
        pending.insert(key, deadline);
        self.set_pending_delegations(&pending).await?;
        Ok(Ok(()))
    }
}

#[cfg(test)]
mod delegation_tests {
    use super::*;

    fn nn(v: i64) -> NonNegI64 {
        NonNegI64::try_from(v).expect("a test amount is non-negative")
    }

    fn validator(byte: u8) -> Validator {
        Validator::from_slice(&[byte; 65])
    }

    fn vault_address_of(v: &Validator) -> String {
        RevAddress::from_public_key(&PublicKey::new(v.as_bytes().to_vec()))
            .expect("a test key is a valid public key")
            .to_base58()
    }

    fn fringe(byte: u8) -> Blake2b256Hash {
        Blake2b256Hash::create(&[byte])
    }

    /// The total REV the native state holds — the two mechanism vaults plus every address given. Every
    /// step of delegated stake is a transfer between these places, so this is the assertion that makes
    /// "the fan-out is a transfer" a property rather than a reading of the code. It is the same
    /// instrument the withdrawal tests use, and it is what would catch a fan-out that paid a delegator
    /// out of a vault nobody debited.
    async fn total_rev(native: &NativeSystemState, addresses: &[String]) -> i64 {
        let mut total = i64::from(native.coop_balance().await.unwrap())
            + i64::from(native.pos_vault_balance().await.unwrap());
        for address in addresses {
            total += native
                .vault_balance(address)
                .await
                .unwrap()
                .map(i64::from)
                .unwrap_or(0);
        }
        total
    }

    async fn balance_of(native: &NativeSystemState, v: &Validator) -> i64 {
        native
            .vault_balance(&vault_address_of(v))
            .await
            .unwrap()
            .map(i64::from)
            .unwrap_or(0)
    }

    /// A state with one bonded, trusted, active validator at `stake`. `minimum_bond` is 1 so the reward
    /// formula has a normaliser (`epoch_reward` pays zero for everyone at a zero minimum), and the other
    /// parameters are the permissive defaults — in particular `absence_slack` 0, which disables the
    /// participation weight, so a boundary's reward here is the formula's and nothing else.
    async fn with_operator(operator: Validator, stake: i64) -> NativeSystemState {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let mut bonds = BTreeMap::new();
        bonds.insert(operator, nn(stake));
        native
            .install_genesis(&PosGenesis {
                bonds,
                trusted: [operator].into_iter().collect(),
                params: PosParams {
                    minimum_bond: nn(1),
                    ..PosParams::default()
                },
                dev_vesting: None,
            })
            .unwrap();
        native
    }

    /// Fund a **user** vault — deliberately not `credit_pos_vault`, so a test can tell phlo in the
    /// staking vault from a delegator's own money.
    async fn fund(native: &NativeSystemState, v: &Validator, amount: i64) {
        native.set_vault_balance(&vault_address_of(v), nn(amount));
    }

    /// **The four leaves are absent until the first delegation.** This is the store half of law 57's
    /// dormancy requirement, and the reason the primitive's fork point is the first `delegate` deploy
    /// rather than genesis: `set_*` is an unconditional `put`, so an *empty* map is already a real trie
    /// leaf and a different state root from an absent one. The assertion reads the **store**, not the
    /// reader — a reader that treats absent and empty alike cannot see the difference, which is exactly
    /// why this test does not use one.
    ///
    /// Red under the mutation that makes the four setters `put` unconditionally, including on an empty
    /// map: the boundary below writes every ledger it is handed.
    #[tokio::test]
    async fn the_delegation_leaves_are_absent_until_the_first_delegation() {
        let native = with_operator(validator(1), 40).await;
        native.credit_pos_vault(nn(100)).await.unwrap();
        native
            .close_block(1, fringe(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        for key in [
            pos_delegations_key(),
            pos_pending_delegations_key(),
            pos_delegation_claims_key(),
            pos_delegated_rewards_key(),
        ] {
            assert!(
                native.store.get(PREFIX_POS, &key).await.unwrap().is_none(),
                "a boundary on a chain that has never delegated must not write the leaf"
            );
        }
        // …and the delegation itself is what creates one.
        let delegator = validator(2);
        fund(&native, &delegator, 60).await;
        native
            .delegate(&delegator, &validator(1), nn(60))
            .await
            .unwrap()
            .unwrap();
        assert!(native
            .store
            .get(PREFIX_POS, &pos_delegations_key())
            .await
            .unwrap()
            .is_some());
    }

    /// The Lean fixture `the_split_is_not_the_identity`, decided on the Rust side too. A split that paid
    /// every delegator an equal share, or dropped the base from the denominator, fails here as well as
    /// in the model — and `[1, 2]` with base 40 is in the fixture because `[1, 1, 1]` (the first version
    /// of the model's clause) is satisfied by the equal-share mutant: three equal weights pay `100 / 3`
    /// either way.
    #[test]
    fn the_split_is_not_the_identity() {
        assert_eq!(pro_rata(100, 30, &[10, 61]).unwrap(), vec![9, 60]);
        assert_eq!(pro_rata(100, 0, &[30, 70]).unwrap(), vec![30, 70]);
        assert_eq!(pro_rata(100, 40, &[1, 2]).unwrap(), vec![2, 4]);
        // **The refund's own instance**, which is the shape `slash`'s fan-out actually takes: a
        // returned amount, the offender's own part as the base, and two unequal delegator parts. The
        // Lean twin is `the_refund_is_split_pro_rata`, and it exists because #193's close condition
        // asks for the delegated-slash refund to be proved *and* held by property tests.
        assert_eq!(pro_rata(100, 40, &[30, 60]).unwrap(), vec![23, 46]);
        // The operator's remainder is the rest, so the two halves add up to the reward exactly.
        let (shares, remainder) = split_reward(100, 30, &[10, 61]).unwrap();
        assert_eq!(shares.iter().sum::<i64>() + remainder, 100);
        // …and with no delegations the split is the identity.
        let (shares, remainder) = split_reward(100, 30, &[]).unwrap();
        assert!(shares.is_empty());
        assert_eq!(remainder, 100);
    }

    #[test]
    fn the_delegation_leaves_round_trip_or_refuse() {
        let operator = validator(1);
        let delegator = validator(2);
        let key = DelegationKey {
            operator,
            delegator,
        };

        let ledger = BTreeMap::from([(key, nn(60))]);
        assert_eq!(
            decode_delegations(&encode_delegations(&ledger)).unwrap(),
            ledger
        );
        assert!(
            decode_delegations(&[0u8; 3]).is_err(),
            "a partial entry is refused, not truncated"
        );
        // A negative amount is refused rather than read as a negative stake.
        let mut bytes = encode_delegations(&ledger);
        let end = bytes.len();
        bytes[end - 8..].copy_from_slice(&(-1i64).to_le_bytes());
        assert!(decode_delegations(&bytes).is_err());

        let rewards = BTreeMap::from([(key, nn(12))]);
        assert_eq!(
            decode_delegated_rewards(&encode_delegated_rewards(&rewards)).unwrap(),
            rewards
        );

        let pending = BTreeMap::from([(key, 7i64)]);
        assert_eq!(
            decode_pending_delegations(&encode_pending_delegations(&pending)).unwrap(),
            pending
        );

        let claims = BTreeMap::from([(
            key,
            DelegationClaim {
                amount: nn(5),
                deadline: 9,
            },
        )]);
        assert_eq!(
            decode_delegation_claims(&encode_delegation_claims(&claims)).unwrap(),
            claims
        );
        // The two strides differ, so a claim leaf read as a delegation ledger is refused rather than
        // silently misparsed — which is the whole reason each has its own length.
        assert!(decode_delegation_claims(&encode_delegations(&ledger)).is_err());
    }

    /// **A delegation enlarges the operator's pool entry by exactly its amount, and a second one from
    /// the same delegator adds to the first rather than replacing it.**
    ///
    /// The second half is the one worth having a test for: the pool entry accumulates either way, so an
    /// implementation that *replaced* the ledger entry would keep the aggregate right and quietly burn
    /// the earlier principal — the ledger and the pool would disagree for ever after, and every later
    /// split would divide by a total that includes stake the ledger says nobody owns.
    ///
    /// Red under the mutation that makes `delegate` insert instead of accumulate.
    #[tokio::test]
    async fn a_delegation_enlarges_the_pool_and_a_second_one_accumulates() {
        let operator = validator(1);
        let delegator = validator(2);
        let key = DelegationKey {
            operator,
            delegator,
        };
        let native = with_operator(operator, 40).await;
        fund(&native, &delegator, 100).await;

        native
            .delegate(&delegator, &operator, nn(60))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(i64::from(native.bonds().await.unwrap()[&operator]), 100);
        assert_eq!(
            balance_of(&native, &delegator).await,
            40,
            "the principal left the delegator's own vault"
        );
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            100,
            "…and joined the staking vault, where the epoch pot is measured"
        );

        native
            .delegate(&delegator, &operator, nn(40))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.delegations().await.unwrap()[&key]),
            100,
            "a second delegation adds to the first"
        );
        assert_eq!(i64::from(native.bonds().await.unwrap()[&operator]), 140);
    }

    /// **A delegation reaches the *active* set, which is what Casper reads** — the measurement this
    /// primitive's "no Casper change was needed" claim rests on.
    ///
    /// `compute_bonds` is the one canonical consensus read (`casper/src/runtime_manager.rs`): it
    /// decodes `pos:active` and nothing else, and finality, `Validate::bonds_cache`, the block's own
    /// `bonds` field and the proposer's `bonded` set all take their bonds from it. `select_active`
    /// derives that leaf from the pool, so once a delegation is in the pool the aggregate reaches all
    /// of them — and *that* is the claim, which the pool-level assertions do not cover: `bonds()` is
    /// the pool, and the pool is not what any of those readers reads.
    ///
    /// Red under a `select_active` that netted the ledger back out of the stake it draws with (the
    /// plausible wrong way to "keep delegation out of consensus"), and under a boundary that wrote
    /// `pos:active` before the aggregate landed.
    #[tokio::test]
    async fn a_delegation_reaches_the_active_set_that_casper_reads() {
        let operator = validator(1);
        let delegator = validator(2);
        let native = with_operator(operator, 40).await;
        fund(&native, &delegator, 60).await;

        native
            .delegate(&delegator, &operator, nn(60))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.active().await.unwrap()[&operator]),
            40,
            "a delegation activates no sooner than a bond does — the pool now, the drawn set at the \
             next boundary"
        );

        native
            .close_block(1, fringe(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.active().await.unwrap()[&operator]),
            100,
            "the drawn set carries the aggregate, so every reader of `pos:active` — finality, the bond \
             cache, the proposer's bonded set — sees the delegated stake"
        );
    }

    /// **A boundary splits a drawn validator's reward across its delegations**, with the operator
    /// keeping the remainder — and `Σ delegators + operator == the reward` exactly, which is what keeps
    /// law 46 a statement about the reward rather than about the split.
    ///
    /// Red under the mutation that credits the whole reward to `committed` (the delegators' entry is
    /// then absent) and under one that credits the whole reward to the delegators (the operator's
    /// remainder is then zero).
    #[tokio::test]
    async fn a_boundary_splits_the_reward_across_the_delegations() {
        let operator = validator(1);
        let delegator = validator(2);
        let key = DelegationKey {
            operator,
            delegator,
        };
        let native = with_operator(operator, 40).await;
        // 100 of phlo into the staking vault, on top of the bond sum genesis funds it with.
        native.credit_pos_vault(nn(100)).await.unwrap();
        fund(&native, &delegator, 60).await;
        native
            .delegate(&delegator, &operator, nn(60))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(i64::from(native.bonds().await.unwrap()[&operator]), 100);

        native
            .close_block(1, fringe(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();

        // The pot is the vault (100 of phlo + 100 of aggregate bond) less the bond and the ledgers:
        // 100. The key holds all of it, so the reward is the whole pot.
        let delegated = native.delegated_rewards().await.unwrap();
        let committed = native.committed_rewards().await.unwrap();
        assert_eq!(
            i64::from(delegated[&key]),
            60,
            "the delegator's proportion of the 100 reward"
        );
        assert_eq!(
            i64::from(committed[&operator]),
            40,
            "the operator's remainder — its own stake's share plus the dust"
        );
        assert_eq!(
            i64::from(delegated[&key]) + i64::from(committed[&operator]),
            100,
            "the split is exact: the key committed what it would have committed alone"
        );
    }

    /// **A slash fans out to each delegator's own vault, at every tier.** This is the sharpest
    /// correctness risk in the primitive: before delegations existed `returned` was one payment to one
    /// address, and the offender's aggregate bond is what the function removes — so a remainder paid
    /// whole to the offender hands a delegator's principal to the party it was delegated to.
    ///
    /// Every tier, because the fan-out is a division: `Malicious` leaves nothing to divide and would
    /// pass vacuously if it were the only case.
    ///
    /// Red under the mutation that credits `returned` to `vault_address(validator)` as before.
    #[tokio::test]
    async fn a_slash_fans_out_to_each_delegators_own_vault_at_every_tier() {
        for severity in [
            SlashSeverity::HonestMistake,
            SlashSeverity::Misdemeanour,
            SlashSeverity::Malicious,
        ] {
            let operator = validator(1);
            let delegator = validator(2);
            let native = with_operator(operator, 40).await;
            fund(&native, &delegator, 60).await;
            native
                .delegate(&delegator, &operator, nn(60))
                .await
                .unwrap()
                .unwrap();
            let addresses = [vault_address_of(&operator), vault_address_of(&delegator)];
            let before = total_rev(&native, &addresses).await;

            native.slash(&operator, severity).await.unwrap().unwrap();

            // The aggregate bond is the whole of what is at risk here — no accrued reward, no escrow —
            // and the offender's own stake is 40 of it against the delegator's 60.
            let at_risk = 100i64;
            let returned = at_risk - at_risk * severity.basis_points() / 10_000;
            let expected = returned * 60 / 100;
            assert_eq!(
                balance_of(&native, &delegator).await,
                expected,
                "the delegator's share of the remainder, at {severity:?}"
            );
            assert_eq!(
                balance_of(&native, &operator).await + balance_of(&native, &delegator).await,
                returned,
                "the fan-out pays out exactly the remainder, at {severity:?}"
            );
            assert_eq!(
                total_rev(&native, &addresses).await,
                before,
                "a slash is a transfer, not a mint, at {severity:?}"
            );
            assert!(
                native.delegations().await.unwrap().is_empty(),
                "the offender leaves every delegation ledger, at {severity:?}"
            );
            assert!(native.delegated_rewards().await.unwrap().is_empty());
            assert!(native.delegation_claims().await.unwrap().is_empty());
        }
    }

    /// **The three stages of an undelegation**, and the accrued reward travelling to the delegator with
    /// the principal: a request that moves nothing, a boundary that takes the principal out of the
    /// operator's **aggregate** and escrows it, and a payout to the **delegator's** own vault of
    /// `principal + accrued` — read from `pos:delegated_rewards`, not from the operator's `committed`
    /// entry, which is the whole reason the two ledgers are separate.
    ///
    /// Red under the mutation that pays a delegation claim to `vault_address(operator)`.
    #[tokio::test]
    async fn an_undelegation_is_staged_then_paid_to_the_delegator() {
        let operator = validator(1);
        let delegator = validator(2);
        let key = DelegationKey {
            operator,
            delegator,
        };
        let native = with_operator(operator, 40).await;
        native.credit_pos_vault(nn(100)).await.unwrap();
        fund(&native, &delegator, 60).await;
        native
            .delegate(&delegator, &operator, nn(60))
            .await
            .unwrap()
            .unwrap();
        // One epoch's split, so there is an accrued reward for the claim to carry.
        native
            .close_block(1, fringe(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.delegated_rewards().await.unwrap()[&key]),
            60
        );

        // Stage: the request moves nothing, and the principal is still in the aggregate.
        native
            .undelegate(&delegator, &operator, 2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.bonds().await.unwrap()[&operator]),
            100,
            "a staged undelegation moves no stake, so it cannot dodge a slash in flight"
        );

        // The boundary takes it out of the aggregate and escrows it in the delegator's name.
        native
            .close_block(2, fringe(2), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(native.bonds().await.unwrap()[&operator]),
            40,
            "the operator is left with its own stake"
        );
        assert_eq!(
            i64::from(native.delegation_claims().await.unwrap()[&key].amount),
            60
        );
        assert!(native.delegations().await.unwrap().is_empty());

        // Past the quarantine the delegator is paid the principal **and** the accrued reward.
        let addresses = [vault_address_of(&operator), vault_address_of(&delegator)];
        let before = total_rev(&native, &addresses).await;
        native
            .close_block(3, fringe(3), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            balance_of(&native, &delegator).await,
            120,
            "60 of principal and 60 of accrued reward, to the delegator's own vault"
        );
        assert_eq!(
            balance_of(&native, &operator).await,
            0,
            "and none of it to the operator"
        );
        assert!(native.delegation_claims().await.unwrap().is_empty());
        assert_eq!(
            total_rev(&native, &addresses).await,
            before,
            "the payout is a transfer"
        );
    }

    /// **The refusals that keep one meaning per ledger.** Each is the negative of an invariant the
    /// other functions rely on, so a missing one shows up later as an ambiguous escrow rather than as a
    /// refusal here.
    #[tokio::test]
    async fn the_delegation_refusals() {
        let operator = validator(1);
        let delegator = validator(2);
        let native = with_operator(operator, 40).await;
        fund(&native, &delegator, 60).await;

        let refusal = |r: Result<Result<(), String>, String>| {
            r.expect("a refusal is a value, not an error").is_err()
        };
        assert!(
            refusal(native.delegate(&operator, &operator, nn(1)).await),
            "a key cannot delegate to itself"
        );
        assert!(
            refusal(native.delegate(&delegator, &validator(9), nn(1)).await),
            "an operator that is not in the pool carries nothing"
        );
        assert!(
            refusal(native.delegate(&delegator, &operator, nn(0)).await),
            "below the per-delegation floor"
        );
        assert!(
            refusal(native.delegate(&delegator, &operator, nn(61)).await),
            "more than the delegator's vault holds"
        );
        assert!(
            refusal(native.undelegate(&delegator, &operator, 1).await),
            "there is no delegation to withdraw"
        );
        // …and once the operator has staged a withdrawal, a delegation may not join the aggregate it
        // is about to move into an escrow the delegator has no claim on.
        native.withdraw(&operator, 1).await.unwrap().unwrap();
        assert!(
            refusal(native.delegate(&delegator, &operator, nn(1)).await),
            "the operator is withdrawing"
        );
    }

    /// The other half of that invariant: a validator whose bond is not all its own cannot stage a
    /// withdrawal of it. Red under the mutation that removes the guard — the boundary then moves the
    /// aggregate, including the delegator's principal, into a claim only the operator can be paid.
    #[tokio::test]
    async fn a_validator_with_delegations_cannot_withdraw() {
        let operator = validator(1);
        let delegator = validator(2);
        let native = with_operator(operator, 40).await;
        fund(&native, &delegator, 60).await;
        native
            .delegate(&delegator, &operator, nn(60))
            .await
            .unwrap()
            .unwrap();

        assert!(
            native.withdraw(&operator, 1).await.unwrap().is_err(),
            "the aggregate is not all the operator's"
        );
        // The delegator's own route out is unaffected, and it is a route the operator cannot take.
        assert!(native
            .undelegate(&delegator, &operator, 1)
            .await
            .unwrap()
            .is_ok());
        // With the delegation gone the operator may withdraw again.
        native
            .close_block(1, fringe(1), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        assert!(native.withdraw(&operator, 2).await.unwrap().is_ok());
    }
}

// ==================================================================================================
// Cost accounting as checkpoint actions (AUDIT C207)
//
// Cost accounting writes `pos:vault` from **every** user deploy, so a block's native sidecar used to
// carry that write — which made every user-deploy block overlap every concurrent sibling on one slot,
// and the merge resolves an overlap by rejecting a whole block, silently taking the deploy's own
// writes (a delegation, a bond, a trust) with it. The merge now re-applies cost accounting per
// accepted deploy instead, and these two functions are the seam: they are the only place that knows
// the staking vault's key, a deployer vault's key and the little-endian encoding, so `casper` can
// build the actions without re-implementing any of it.
//
// Appended at the tail, per this file's own rule: its earlier line numbers are cited by Lean
// declarations and spec rows.
// --------------------------------------------------------------------------------------------------

/// The checkpoint action that **sets a user vault's balance**.
///
/// The caller passes the value the leaf will hold — it read the base itself, because only the caller
/// knows which state the move is being applied to.
pub fn vault_put_action(address: &str, value: NonNegI64) -> NativeStoreAction {
    NativeStoreAction::Put {
        prefix: PREFIX_VAULT,
        key: vault_key(address),
        value: i64::from(value).to_le_bytes().to_vec(),
    }
}

/// The checkpoint action that **sets the staking vault's balance**.
pub fn pos_vault_put_action(value: NonNegI64) -> NativeStoreAction {
    NativeStoreAction::Put {
        prefix: PREFIX_POS,
        key: pos_vault_key(),
        value: i64::from(value).to_le_bytes().to_vec(),
    }
}

// --------------------------------------------------------------------------------------------------
// ERTP (issue #249)
//
// Appended at the tail for the same reason the checkpoint actions above are: this file's earlier line
// numbers are cited by Lean declarations and spec rows, so a new section goes at the end rather than
// in the middle where it would read better.
// --------------------------------------------------------------------------------------------------

impl NativeSystemState {
    /// The bytes of the name that may mint this brand, if it is registered.
    pub async fn ertp_brand_authority(&self, brand: &[u8]) -> Result<Option<Vec<u8>>, String> {
        match self.store.get(PREFIX_ERTP, &ertp_brand_key(brand)).await? {
            Some(bytes) => Ok(Some(decode_brand(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Register a brand and the name that may mint it.
    ///
    /// Idempotent for the *same* authority, and refused for a different one: a brand whose minter can
    /// be replaced is a brand whose scarcity is not a fact, and every amount of it already in
    /// circulation would be denominated in something a second party could also print.
    pub async fn ertp_register_brand(
        &self,
        brand: &[u8],
        authority: &[u8],
    ) -> Result<Result<(), String>, String> {
        match self.ertp_brand_authority(brand).await? {
            Some(existing) if existing == authority => Ok(Ok(())),
            Some(_) => Ok(Err("this brand already has an issuer".to_string())),
            None => {
                self.store
                    .put(PREFIX_ERTP, ertp_brand_key(brand), encode_brand(authority));
                Ok(Ok(()))
            }
        }
    }

    /// What this issuer says `holder` (a purse or a payment) contains. `None` means the issuer has
    /// never heard of it — which is not the same as zero, and is not flattened to one.
    pub async fn ertp_holding(
        &self,
        brand: &[u8],
        holder: &[u8],
    ) -> Result<Option<Holding>, String> {
        match self
            .store
            .get(PREFIX_ERTP, &ertp_holding_key(brand, holder))
            .await?
        {
            Some(bytes) => {
                let (leaf_brand, holding) = decode_holding(&bytes)?;
                // **The leaf names its own brand, and it must be the one asked for.** The key is a
                // hash of `(brand, holder)`, so a leaf whose brand disagrees is a store-integrity
                // failure rather than a caller error — and reading it as data would silently answer
                // for the wrong brand.
                if leaf_brand != brand {
                    return Err(format!(
                        "an ERTP holding under one brand's key names a different brand ({} bytes \
                         vs the {} requested)",
                        leaf_brand.len(),
                        brand.len()
                    ));
                }
                Ok(Some(holding))
            }
            None => Ok(None),
        }
    }

    fn set_ertp_holding(&self, brand: &[u8], holder: &[u8], holding: Holding) {
        self.store.put(
            PREFIX_ERTP,
            ertp_holding_key(brand, holder),
            encode_holding(brand, holding),
        );
    }

    /// **The total of one brand's live ERTP holdings**, derived by enumeration
    /// (`REV_SUPPLY_IS_CONSERVED`).
    ///
    /// The C109 idiom: the total is *derived* from the prefix rather than maintained as a running
    /// sum, so a holding written at a holder nobody named is counted and a mint into a name nobody
    /// thought of shows up as a change. A counter is rejected for the same reason: a redundant sum
    /// that can drift silently is exactly the failure this instrument exists to catch.
    ///
    /// Refuses a store with a base history for the same reason [`Self::total_value`] does — an
    /// enumeration that cannot see the whole state must not report a number as though it had.
    pub async fn ertp_total_value(&self, brand: &[u8]) -> Result<i64, String> {
        if self.store.has_base_history() {
            return Err(
                "ertp_total_value: this store has a base history, whose holdings cannot be \
                 enumerated (NativeHistoryReader is keyed-read only), so a total over it would \
                 silently omit part of the state"
                    .to_string(),
            );
        }
        let mut total: i64 = 0;
        for (key, value) in self.store.live_entries(PREFIX_ERTP) {
            // Brands and holdings share the prefix, so the enumeration reads each leaf's tag rather
            // than assuming the prefix holds only holdings.
            if value.first() != Some(&ERTP_LEAF_HOLDING) {
                continue;
            }
            let (leaf_brand, holding) = decode_holding(&value)
                .map_err(|e| format!("ertp_total_value: leaf {}: {e}", key.to_hex()))?;
            if leaf_brand != brand || !holding.live {
                continue;
            }
            total = total
                .checked_add(i64::from(holding.amount))
                .ok_or_else(|| "ertp_total_value: overflow summing the holdings".to_string())?;
        }
        Ok(total)
    }

    /// Create an **empty purse** holding. Permissionless on purpose: making a purse can only ever
    /// *credit* one, so requiring the issuer's name here would be requiring a key to an empty room —
    /// and the holder's own name is what the balance is keyed by, which nobody else can present.
    ///
    /// A purse gets a zero holding rather than none, because `None` means "this issuer has never
    /// heard of that holder" and an empty purse is not that.
    pub async fn ertp_make_purse(
        &self,
        brand: &[u8],
        purse: &[u8],
    ) -> Result<Result<(), String>, String> {
        if self.ertp_holding(brand, purse).await?.is_some() {
            return Ok(Err("that purse already exists".to_string()));
        }
        self.set_ertp_holding(
            brand,
            purse,
            Holding {
                amount: NonNegI64::zero(),
                live: true,
            },
        );
        Ok(Ok(()))
    }

    /// Create a payment holding of `amount` under `authority` — the only path that makes a holding
    /// out of nothing, and therefore the one that must present the brand's issuer.
    pub async fn ertp_mint(
        &self,
        brand: &[u8],
        authority: &[u8],
        payment: &[u8],
        amount: NonNegI64,
    ) -> Result<Result<(), String>, String> {
        if self.ertp_brand_authority(brand).await?.as_deref() != Some(authority) {
            return Ok(Err(
                "only the name that registered the brand may mint it".to_string()
            ));
        }
        if self.ertp_holding(brand, payment).await?.is_some() {
            return Ok(Err("that payment already exists".to_string()));
        }
        self.set_ertp_holding(brand, payment, Holding { amount, live: true });
        Ok(Ok(()))
    }

    /// Move everything a live payment holds into `purse`, consuming the payment.
    ///
    /// The liveness check and the clearing happen in the same call that credits the purse, so a
    /// second deposit of the same payment is *refused* rather than double-counted. That is ERTP's
    /// double-spend guard and the whole reason a payment is not a purse; doing it here, in one native
    /// step, is what makes it hold against a caller that interleaves two deposits.
    pub async fn ertp_deposit(
        &self,
        brand: &[u8],
        purse: &[u8],
        payment: &[u8],
    ) -> Result<Result<NonNegI64, String>, String> {
        let Some(from) = self.ertp_holding(brand, payment).await? else {
            return Ok(Err("no such payment".to_string()));
        };
        if !from.live {
            return Ok(Err("that payment has already been deposited".to_string()));
        }
        let credited = self
            .ertp_holding(brand, purse)
            .await?
            .map(|h| h.amount)
            .unwrap_or_else(NonNegI64::zero);
        let total = balance_plus(credited, i64::from(from.amount), "ertp deposit")?;
        self.set_ertp_holding(
            brand,
            purse,
            Holding {
                amount: total,
                live: true,
            },
        );
        self.set_ertp_holding(
            brand,
            payment,
            Holding {
                amount: NonNegI64::zero(),
                live: false,
            },
        );
        Ok(Ok(total))
    }

    /// Issue a payment of `amount` out of `purse`, under the brand's authority.
    ///
    /// Only the issuer may withdraw, because issuing is what an issuer *is*; the purse's own name is
    /// the capability to deposit *into* it, which is why `ertp_deposit` asks for nothing more.
    pub async fn ertp_withdraw(
        &self,
        brand: &[u8],
        authority: &[u8],
        purse: &[u8],
        payment: &[u8],
        amount: NonNegI64,
    ) -> Result<Result<(), String>, String> {
        if self.ertp_brand_authority(brand).await?.as_deref() != Some(authority) {
            return Ok(Err(
                "only the name that registered the brand may issue it".to_string()
            ));
        }
        if self.ertp_holding(brand, payment).await?.is_some() {
            return Ok(Err("that payment already exists".to_string()));
        }
        let held = self
            .ertp_holding(brand, purse)
            .await?
            .map(|h| h.amount)
            .unwrap_or_else(NonNegI64::zero);
        let (held, wanted) = (i64::from(held), i64::from(amount));
        if held < wanted {
            return Ok(Err("insufficient funds".to_string()));
        }
        let left = NonNegI64::try_from(held - wanted)
            .map_err(|_| "the withdrawal left a negative balance".to_string())?;
        self.set_ertp_holding(
            brand,
            purse,
            Holding {
                amount: left,
                live: true,
            },
        );
        self.set_ertp_holding(brand, payment, Holding { amount, live: true });
        Ok(Ok(()))
    }

    /// Credit a live holding by `amount`, creating it if the issuer has never heard of the holder.
    ///
    /// **The counterpart of a movement that happened elsewhere**, and therefore native-only: the one
    /// caller is the REV funding path, whose REV has already left a vault for the reserve
    /// (`revFund`, `system_processes.rs`). No Rholang term can reach this, which is what stops
    /// "credit myself" from being an operation.
    ///
    /// A **consumed** holding is refused rather than revived: `live: false` means "this payment has
    /// been deposited", and resurrecting it would make a payment depositable twice.
    pub async fn ertp_credit(
        &self,
        brand: &[u8],
        holder: &[u8],
        amount: NonNegI64,
    ) -> Result<Result<NonNegI64, String>, String> {
        let credited = match self.ertp_holding(brand, holder).await? {
            Some(holding) if !holding.live => {
                return Ok(Err("that holding has been consumed".to_string()))
            }
            Some(holding) => holding.amount,
            None => NonNegI64::zero(),
        };
        let total = balance_plus(credited, i64::from(amount), "ertp credit")?;
        self.set_ertp_holding(
            brand,
            holder,
            Holding {
                amount: total,
                live: true,
            },
        );
        Ok(Ok(total))
    }

    /// Debit `amount` from a holding, refusing an unknown holder and an insufficient balance.
    ///
    /// **Liveness is preserved, not forced** — a debit is not a deposit, so a dead holding stays
    /// dead. The one caller is the REV redeeming path, after the reserve has paid out
    /// (`revRedeem`); keeping the refusal conditions here rather than at the call site is what makes
    /// the two agree about what "insufficient" means.
    pub async fn ertp_debit(
        &self,
        brand: &[u8],
        holder: &[u8],
        amount: NonNegI64,
    ) -> Result<Result<NonNegI64, String>, String> {
        let Some(holding) = self.ertp_holding(brand, holder).await? else {
            return Ok(Err("no such holding".to_string()));
        };
        let (held, wanted) = (i64::from(holding.amount), i64::from(amount));
        if held < wanted {
            return Ok(Err("insufficient funds".to_string()));
        }
        let left = NonNegI64::try_from(held - wanted)
            .map_err(|_| "the debit left a negative balance".to_string())?;
        self.set_ertp_holding(
            brand,
            holder,
            Holding {
                amount: left,
                live: holding.live,
            },
        );
        Ok(Ok(left))
    }
}

/// Leaf key for a registered brand.
fn ertp_brand_key(brand: &[u8]) -> Blake2b256Hash {
    Blake2b256Hash::create(brand)
}

/// Leaf key for a holding. The brand's length is prefixed so that `(ab, c)` and `(a, bc)` cannot
/// collide on the same leaf — the one way a concatenation-keyed map goes wrong.
fn ertp_holding_key(brand: &[u8], holder: &[u8]) -> Blake2b256Hash {
    let mut buf = Vec::with_capacity(4 + brand.len() + holder.len());
    buf.extend_from_slice(&(brand.len() as u32).to_le_bytes());
    buf.extend_from_slice(brand);
    buf.extend_from_slice(holder);
    Blake2b256Hash::create(&buf)
}

/// What an issuer's ledger says one purse or payment holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holding {
    pub amount: NonNegI64,
    /// A payment is consumed by its first deposit; a purse is never consumed.
    pub live: bool,
}

/// `PREFIX_ERTP` holds **two kinds of leaf** — a brand's registration and a holder's holding — so each
/// leaf says which it is. The keys already cannot collide (`blake2b256(brand)` against
/// `blake2b256(len ‖ brand ‖ holder)`), but an *enumeration* of the prefix sees both and has to tell
/// them apart, and the totals below are enumerations.
const ERTP_LEAF_BRAND: u8 = 0x01;
const ERTP_LEAF_HOLDING: u8 = 0x02;

/// A brand-registration leaf: the tag and the bytes of the name that may mint it.
fn encode_brand(authority: &[u8]) -> Vec<u8> {
    let mut out = vec![ERTP_LEAF_BRAND];
    out.extend_from_slice(authority);
    out
}

/// The authority a brand-registration leaf names, refusing any other kind of leaf.
fn decode_brand(bytes: &[u8]) -> Result<Vec<u8>, String> {
    match bytes.split_first() {
        Some((&ERTP_LEAF_BRAND, authority)) => Ok(authority.to_vec()),
        _ => Err("an ERTP brand leaf does not carry the brand tag".to_string()),
    }
}

/// A holding leaf: `u32_le(brand.len()) ‖ brand ‖ i64_le(amount) ‖ live`, behind the holding tag.
///
/// **Why the brand is in the leaf and not only in the key.** The key is `blake2b256(len ‖ brand ‖
/// holder)`, so a leaf's brand is unrecoverable from its key — and the totals below are *derived* by
/// enumerating the prefix rather than maintained as a running sum (the C109 idiom, whose reasoning is
/// that "a total that silently omits part of its subject is the failure this repository keeps
/// finding"). Without the brand in the value there is no way to sum one brand's holdings, so a REV
/// total could not be taken at all. The brand is public, so nothing is disclosed; the **key is
/// unchanged**, so nothing an index exposes changes either.
///
/// This is a consensus-visible change to a committed prefix, taken while it is cheapest: no chain has
/// run `PREFIX_ERTP`, and no genesis post-state hash is pinned.
fn encode_holding(brand: &[u8], h: Holding) -> Vec<u8> {
    let mut out = vec![ERTP_LEAF_HOLDING];
    out.extend_from_slice(
        &u32::try_from(brand.len())
            .map(|len| len.to_le_bytes())
            .unwrap_or(u32::MAX.to_le_bytes()),
    );
    out.extend_from_slice(brand);
    out.extend_from_slice(&i64::from(h.amount).to_le_bytes());
    out.push(u8::from(h.live));
    out
}

/// The brand and the holding a leaf encodes — the inverse of [`encode_holding`], and strict: a leaf
/// that does not carry its brand's length-prefixed prologue is refused rather than guessed at.
fn decode_holding(bytes: &[u8]) -> Result<(Vec<u8>, Holding), String> {
    let Some((&ERTP_LEAF_HOLDING, bytes)) = bytes.split_first() else {
        return Err("an ERTP holding leaf does not carry the holding tag".to_string());
    };
    if bytes.len() < 4 {
        return Err(format!(
            "an ERTP holding is {} bytes, too short for its brand-length prefix",
            bytes.len()
        ));
    }
    let (len_bytes, rest) = bytes.split_at(4);
    let len: [u8; 4] = len_bytes
        .try_into()
        .map_err(|_| "an ERTP holding's brand length is not four bytes".to_string())?;
    let brand_len = u32::from_le_bytes(len) as usize;
    if rest.len() < brand_len + 9 {
        return Err(format!(
            "an ERTP holding is {} bytes but claims a {brand_len}-byte brand",
            bytes.len()
        ));
    }
    let (brand, tail) = rest.split_at(brand_len);
    let [amount @ .., live] = tail else {
        return Err("an ERTP holding has no amount".to_string());
    };
    let amount: [u8; 8] = amount
        .try_into()
        .map_err(|_| format!("an ERTP holding amount is {} bytes", amount.len()))?;
    let amount = NonNegI64::try_from(i64::from_le_bytes(amount))
        .map_err(|_| "an ERTP holding is negative".to_string())?;
    Ok((
        brand.to_vec(),
        Holding {
            amount,
            live: *live != 0,
        },
    ))
}

#[cfg(test)]
mod ertp_tests {
    use super::*;

    fn nn(v: i64) -> NonNegI64 {
        NonNegI64::try_from(v).expect("a test amount is non-negative")
    }

    fn state() -> NativeSystemState {
        NativeSystemState::new(Arc::new(InMemNativeStore::empty()))
    }

    #[tokio::test]
    async fn a_brand_belongs_to_the_issuer_that_registered_it() {
        let native = state();
        assert_eq!(
            native.ertp_register_brand(b"kudos", b"issuer").await,
            Ok(Ok(()))
        );
        // Idempotent for the same issuer...
        assert_eq!(
            native.ertp_register_brand(b"kudos", b"issuer").await,
            Ok(Ok(()))
        );
        // ...and refused for another, because a replaceable minter is not scarcity.
        assert_eq!(
            native.ertp_register_brand(b"kudos", b"someone-else").await,
            Ok(Err("this brand already has an issuer".to_string()))
        );
        assert_eq!(
            native.ertp_brand_authority(b"kudos").await,
            Ok(Some(b"issuer".to_vec()))
        );
    }

    #[tokio::test]
    async fn only_the_name_that_registered_the_brand_may_mint_it() {
        let native = state();
        native
            .ertp_register_brand(b"kudos", b"issuer")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            native.ertp_mint(b"kudos", b"thief", b"p1", nn(10)).await,
            Ok(Err(
                "only the name that registered the brand may mint it".to_string()
            ))
        );
        assert_eq!(
            native.ertp_mint(b"kudos", b"issuer", b"p1", nn(10)).await,
            Ok(Ok(()))
        );
        assert_eq!(
            native.ertp_holding(b"kudos", b"p1").await,
            Ok(Some(Holding {
                amount: nn(10),
                live: true
            }))
        );
    }

    #[tokio::test]
    async fn a_payment_is_consumed_by_its_first_deposit() {
        let native = state();
        native
            .ertp_register_brand(b"kudos", b"issuer")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_mint(b"kudos", b"issuer", b"p1", nn(10))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            native.ertp_deposit(b"kudos", b"purse", b"p1").await,
            Ok(Ok(nn(10)))
        );
        // The second deposit is refused rather than double-counted: the whole point of a payment
        // being distinct from a purse.
        assert_eq!(
            native.ertp_deposit(b"kudos", b"purse", b"p1").await,
            Ok(Err("that payment has already been deposited".to_string()))
        );
        assert_eq!(
            native.ertp_holding(b"kudos", b"purse").await,
            Ok(Some(Holding {
                amount: nn(10),
                live: true
            }))
        );
    }

    #[tokio::test]
    async fn a_holding_of_one_brand_is_not_a_holding_of_another() {
        // A payment is not "worth zero" under another brand — that issuer has never heard of it, and
        // the refusal is what keeps two brands' holdings from being interchangeable names.
        let native = state();
        native
            .ertp_register_brand(b"kudos", b"issuer")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_register_brand(b"other", b"issuer")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_mint(b"kudos", b"issuer", b"p1", nn(10))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            native.ertp_deposit(b"other", b"purse", b"p1").await,
            Ok(Err("no such payment".to_string()))
        );
    }

    #[tokio::test]
    async fn a_withdrawal_beyond_the_balance_is_refused_and_a_live_one_moves_the_amount() {
        let native = state();
        native
            .ertp_register_brand(b"kudos", b"issuer")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_mint(b"kudos", b"issuer", b"seed", nn(10))
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_deposit(b"kudos", b"purse", b"seed")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            native
                .ertp_withdraw(b"kudos", b"issuer", b"purse", b"out", nn(11))
                .await,
            Ok(Err("insufficient funds".to_string()))
        );
        assert_eq!(
            native
                .ertp_withdraw(b"kudos", b"issuer", b"purse", b"out", nn(4))
                .await,
            Ok(Ok(()))
        );
        assert_eq!(
            native.ertp_holding(b"kudos", b"purse").await,
            Ok(Some(Holding {
                amount: nn(6),
                live: true
            }))
        );
        assert_eq!(
            native.ertp_holding(b"kudos", b"out").await,
            Ok(Some(Holding {
                amount: nn(4),
                live: true
            }))
        );
    }

    /// The one way a concatenation-keyed map goes wrong: `(ab, c)` and `(a, bc)` must not be the same
    /// leaf, which is why the brand's length is prefixed rather than the two simply joined.
    #[test]
    fn an_ertp_holding_key_cannot_be_confused_by_a_concatenation() {
        assert_ne!(ertp_holding_key(b"ab", b"c"), ertp_holding_key(b"a", b"bc"));
    }

    /// **A holding leaf names its brand, and a leaf read under the wrong brand's key is a store
    /// error rather than data.**
    ///
    /// The key is a hash of `(brand, holder)`, so nothing else in the leaf could tell the two apart —
    /// and the totals below sum by enumerating the prefix, which is only possible if the brand
    /// travels with the value.
    #[tokio::test]
    async fn a_holding_leaf_names_its_brand() {
        let native = state();
        native
            .ertp_register_brand(b"kudos", b"issuer")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_register_brand(b"reput", b"issuer")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_mint(b"kudos", b"issuer", b"purse", nn(7))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            native.ertp_holding(b"kudos", b"purse").await.unwrap(),
            Some(Holding {
                amount: nn(7),
                live: true
            })
        );
        // The same *holder*, under a different brand's key, is a different leaf — and the one that
        // exists says which brand it belongs to.
        assert_eq!(native.ertp_holding(b"reput", b"purse").await.unwrap(), None);

        // Writing a leaf whose brand disagrees with its key is refused on the way *out*, so a store
        // that was tampered with (or written by an older format) is reported rather than read as
        // value.
        native.store.put(
            PREFIX_ERTP,
            ertp_holding_key(b"kudos", b"forged"),
            encode_holding(
                b"reput",
                Holding {
                    amount: nn(9),
                    live: true,
                },
            ),
        );
        let err = native
            .ertp_holding(b"kudos", b"forged")
            .await
            .expect_err("a leaf naming another brand must be refused");
        assert!(err.contains("names a different brand"), "{err}");
    }

    /// **The ERTP total sees a holding at a holder nobody named** — the falsifier this unit exists
    /// for, and the exact analogue of `total_value_sees_a_balance_at_an_address_no_test_named`.
    ///
    /// A total derived from a *list of holders* cannot see a holding created at one nobody listed,
    /// which is precisely the shape a mint has.
    #[tokio::test]
    async fn the_ertp_total_sees_a_holding_at_a_holder_no_test_named() {
        let native = state();
        let brand = crate::system_processes::rev_brand();
        native
            .ertp_register_brand(&brand, b"issuer")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_mint(&brand, b"issuer", b"purse", nn(10))
            .await
            .unwrap()
            .unwrap();
        let before = native.ertp_total_value(&brand).await.unwrap();
        assert_eq!(before, 10);

        // A holder this test never mentions, credited straight into the store.
        native.store.put(
            PREFIX_ERTP,
            ertp_holding_key(&brand, b"ghost"),
            encode_holding(
                &brand,
                Holding {
                    amount: nn(500),
                    live: true,
                },
            ),
        );

        assert_eq!(
            native.ertp_total_value(&brand).await.unwrap(),
            before + 500,
            "a holding at a holder nobody named must move the total"
        );
        // And a *consumed* holding does not count: the total is over live value.
        native
            .ertp_deposit(&brand, b"purse", b"ghost")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(native.ertp_total_value(&brand).await.unwrap(), 510);
    }

    /// **`REV_SUPPLY_IS_CONSERVED`** — the escrow's instrument, over the whole scripted flow.
    ///
    /// Two claims, in the direction each can be made in:
    ///
    /// 1. **The vault total is invariant**, and this is the *supply* half: funded, redeemed and
    ///    withdrawn REV is *moved* — from a vault to the reserve, from the reserve to an address, or
    ///    from one holding to another — and the ERTP layer never creates vault REV. A second brand
    ///    (which recharges nothing) leaves it untouched too.
    /// 2. **The float is backed**: `Σ REV holdings ≤ the reserve`. This is the *safety* direction, and
    ///    it is the one that holds even under a partial write, because the REV enters the reserve
    ///    *before* the holding is credited and leaves it *before* the holding is debited. It is
    ///    **exact** whenever nobody has sent the reserve an unsolicited transfer — and the donation
    ///    in this script is what makes the strictness visible rather than assumed.
    #[tokio::test]
    async fn rev_supply_is_conserved_and_the_float_is_backed() {
        let native = state();
        let brand = crate::system_processes::rev_brand();
        let reserve = crate::system_processes::rev_reserve();
        let other = b"kudos";
        native
            .ertp_register_brand(&brand, b"issuer")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_register_brand(other, b"issuer")
            .await
            .unwrap()
            .unwrap();

        // The two vaults a chain starts with, seeded *before* the snapshot: a conservation instrument
        // over a state that can still be created from nothing measures the seeding, not the ops.
        native.set_vault_balance("funder", nn(100));
        native.set_vault_balance("donor", nn(50));
        let supply = native.total_value().await.unwrap();
        assert_eq!(supply, 150);

        let step = |what: &str| {
            let native = native.clone();
            let what = what.to_string();
            async move {
                assert_eq!(
                    native.total_value().await.unwrap(),
                    150,
                    "{what} moved vault REV: the supply is conserved by every ERTP op"
                );
            }
        };

        // 1. Fund 10 from the funder's vault into the reserve, then claim it (what `revFund` does).
        native
            .transfer_vault("funder", &reserve, nn(10))
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_credit(&brand, b"purse", nn(10))
            .await
            .unwrap()
            .unwrap();
        step("a fund").await;
        assert_eq!(native.ertp_total_value(&brand).await.unwrap(), 10);
        assert_eq!(
            i64::from(native.vault_balance(&reserve).await.unwrap().unwrap()),
            10
        );

        // 2. Redeem 4: the reserve pays out *first*, then the holding is debited.
        native
            .transfer_vault(&reserve, "payee", nn(4))
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_debit(&brand, b"purse", nn(4))
            .await
            .unwrap()
            .unwrap();
        step("a redeem").await;
        assert_eq!(native.ertp_total_value(&brand).await.unwrap(), 6);

        // 3. Withdraw 2 into a payment — an ERTP-internal move, with no vault movement at all.
        native
            .ertp_withdraw(&brand, b"issuer", b"purse", b"payment", nn(2))
            .await
            .unwrap()
            .unwrap();
        step("a withdraw").await;
        assert_eq!(native.ertp_total_value(&brand).await.unwrap(), 6);

        // 4. A **second brand** through the whole generic flow — mint, withdraw, deposit. It has no
        //    escrow and no vault, and must not perturb either.
        native
            .ertp_mint(other, b"issuer", b"k1", nn(7))
            .await
            .unwrap()
            .unwrap();
        native.ertp_make_purse(other, b"k2").await.unwrap().unwrap();
        native
            .ertp_deposit(other, b"k2", b"k1")
            .await
            .unwrap()
            .unwrap();
        native
            .ertp_withdraw(other, b"issuer", b"k2", b"k3", nn(3))
            .await
            .unwrap()
            .unwrap();
        step("a second brand's flow").await;
        assert_eq!(
            native.ertp_total_value(other).await.unwrap(),
            7,
            "the other brand's total is its own and is unchanged by any REV op"
        );
        assert_eq!(native.ertp_total_value(&brand).await.unwrap(), 6);

        // 5. A refused over-withdraw and a refused cross-brand deposit move nothing.
        assert!(native
            .ertp_withdraw(&brand, b"issuer", b"purse", b"p4", nn(99))
            .await
            .unwrap()
            .is_err());
        assert!(native
            .ertp_deposit(other, b"k2", b"payment")
            .await
            .unwrap()
            .is_err());
        step("two refused ops").await;
        assert_eq!(native.ertp_total_value(&brand).await.unwrap(), 6);
        assert_eq!(native.ertp_total_value(other).await.unwrap(), 7);

        // 6. **A donation**: somebody sends the reserve REV without claiming anything. The vault total
        //    is unchanged (it is a transfer), the float is unchanged, and the reserve now exceeds it —
        //    which is what "exact whenever the reserve receives no unsolicited transfer" means.
        native
            .transfer_vault("donor", &reserve, nn(3))
            .await
            .unwrap()
            .unwrap();
        step("a donation").await;
        let claims = native.ertp_total_value(&brand).await.unwrap();
        let held = i64::from(native.vault_balance(&reserve).await.unwrap().unwrap());
        assert_eq!(held, 9, "10 funded + 3 donated − 4 redeemed");
        assert_eq!(claims, 6);
        assert!(
            claims <= held,
            "the float must never exceed the reserve: {claims} claimed vs {held} held"
        );
    }
}

#[cfg(test)]
mod liveness_tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn validator(byte: u8) -> Validator {
        Validator::from_slice(&[byte; 65])
    }

    fn nn(value: i64) -> NonNegI64 {
        NonNegI64::try_from(value).expect("a non-negative test amount")
    }

    fn fringe(byte: u8) -> Blake2b256Hash {
        Blake2b256Hash::create(&[byte])
    }

    fn armed_params() -> PosParams {
        PosParams {
            minimum_bond: nn(1),
            maximum_bond: nn(1000),
            epoch_length: 10,
            quarantine_length: 50,
            number_of_active_validators: 0,
            executor_share: nn(0),
            absence_slack: nn(0),
            participation_grace: nn(0),
            min_phlo_price: nn(0),
            max_number_of_parents: 0,
            dev_tax_vesting_blocks: nn(0),
            liveness_absence_threshold: 10,
            liveness_jail_after_boundaries: 2,
            liveness_jail_epochs: 5,
            liveness_vote_window: 3,
            liveness_slash_severity: 2,
        }
    }

    fn vault_address_of(validator: &Validator) -> String {
        RevAddress::from_public_key(&PublicKey::new(validator.as_bytes().to_vec()))
            .expect("a test key is a valid public key")
            .to_base58()
    }

    async fn four(params: PosParams) -> NativeSystemState {
        let native = NativeSystemState::new(Arc::new(InMemNativeStore::empty()));
        let bonds: BTreeMap<Validator, NonNegI64> =
            (1..=4).map(|b| (validator(b), nn(10))).collect();
        native
            .install_genesis(&PosGenesis {
                bonds,
                trusted: (1..=4).map(validator).collect(),
                params,
                dev_vesting: None,
            })
            .unwrap();
        native
    }

    fn present(heights: &[(u8, i64)]) -> BTreeMap<Validator, BlockHeight> {
        heights
            .iter()
            .map(|(b, h)| (validator(*b), BlockHeight::try_from(*h).expect("height")))
            .collect()
    }

    async fn boundary(native: &NativeSystemState, block_number: i64, participation: &[(u8, i64)]) {
        native
            .close_block(
                block_number,
                fringe(block_number as u8),
                &present(participation),
            )
            .await
            .expect("store")
            .expect("contract");
    }

    /// Anchor at 10, then two advancing boundaries with v4 absent: the jail is imposed, the bond is
    /// untouched and v4 is no longer drawn.
    async fn jail_v4(native: &NativeSystemState) {
        boundary(native, 10, &[(1, 10), (2, 10), (3, 10), (4, 10)]).await;
        boundary(native, 20, &[(1, 20), (2, 20), (3, 20)]).await;
        boundary(native, 30, &[(1, 30), (2, 30), (3, 30)]).await;
    }

    #[tokio::test]
    async fn a_sustained_absence_jails_without_touching_the_bond() {
        let native = four(armed_params()).await;
        jail_v4(&native).await;
        let v4 = validator(4);
        let record = native
            .liveness()
            .await
            .unwrap()
            .get(&v4)
            .copied()
            .expect("v4 has a liveness record");
        assert!(record.is_jailed());
        assert_eq!(record.jail_remaining, 5);
        assert_eq!(
            native.bonds().await.unwrap().get(&v4).copied(),
            Some(nn(10)),
            "jail must not touch the bond"
        );
        assert!(
            !native.active_validators().await.unwrap().contains(&v4),
            "a jailed validator is not drawn"
        );
        let committed = native.committed_rewards().await.unwrap();
        assert_eq!(
            committed.get(&v4).copied().unwrap_or(nn(0)),
            nn(0),
            "a jailed validator earns nothing for the epoch it missed"
        );
    }

    #[tokio::test]
    async fn one_boundary_omission_does_not_jail() {
        let native = four(armed_params()).await;
        boundary(&native, 10, &[(1, 10), (2, 10), (3, 10), (4, 10)]).await;
        boundary(&native, 20, &[(1, 20), (2, 20), (3, 20)]).await;
        boundary(&native, 30, &[(1, 30), (2, 30), (3, 30), (4, 30)]).await;
        boundary(&native, 40, &[(1, 40), (2, 40), (3, 40)]).await;
        boundary(&native, 50, &[(1, 50), (2, 50), (3, 50), (4, 50)]).await;
        assert!(
            !native.liveness().await.unwrap().contains_key(&validator(4)),
            "a one-boundary lapse is not sustained absence"
        );
    }

    #[tokio::test]
    async fn a_stall_freezes_the_jail_clock() {
        let native = four(armed_params()).await;
        boundary(&native, 10, &[(1, 10), (2, 10), (3, 10), (4, 10)]).await;
        // Nobody has a message: no evidence, no counters.
        boundary(&native, 20, &[]).await;
        // The quorum tip has not advanced past the anchor.
        boundary(&native, 30, &[(1, 10), (2, 10), (3, 10), (4, 10)]).await;
        assert!(
            native.liveness().await.unwrap().is_empty(),
            "a stalled chain must not jail"
        );
    }

    /// A delegation updates the pool before the next active-set draw. Weighting a
    /// slash ballot from that mutable pool lets one 10/30 active-stake voter buy a
    /// 40/30 numerator and eject a jailed validator without a quorum.
    #[tokio::test]
    async fn a_delegation_cannot_buy_slash_voting_power_mid_epoch() {
        let native = four(armed_params()).await;
        jail_v4(&native).await;
        let target = validator(4);
        let voter = validator(1);
        let delegator = validator(9);

        // The extra bond is real: delegate debits the funder's vault and credits
        // the PoS vault, but does not change the already-drawn active snapshot.
        native.set_vault_balance(&vault_address_of(&delegator), nn(30));
        native
            .delegate(&delegator, &voter, nn(30))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(native.bonds().await.unwrap().get(&voter), Some(&nn(40)));
        assert_eq!(native.active().await.unwrap().get(&voter), Some(&nn(10)));

        native
            .cast_vote(&voter, &target, true)
            .await
            .unwrap()
            .unwrap();
        boundary(&native, 40, &[(1, 40), (2, 40), (3, 40)]).await;
        assert!(
            native.bonds().await.unwrap().contains_key(&target),
            "10/30 active stake cannot slash a validator even when its pool stake rises to 40"
        );
        assert!(
            !native.banished().await.unwrap().contains(&target),
            "a minority vote must never permanently ban the target"
        );
    }

    /// The active snapshot's exact 2/3 boundary is not a supermajority, even
    /// when a voter has more bonded principal than at the active-set draw.
    #[tokio::test]
    async fn two_of_three_active_votes_cannot_cross_strict_threshold_via_delegation() {
        let native = four(armed_params()).await;
        jail_v4(&native).await;
        let target = validator(4);
        let delegator = validator(9);
        native.set_vault_balance(&vault_address_of(&delegator), nn(30));
        native
            .delegate(&delegator, &validator(1), nn(30))
            .await
            .unwrap()
            .unwrap();
        for voter in [validator(1), validator(2)] {
            native
                .cast_vote(&voter, &target, true)
                .await
                .unwrap()
                .unwrap();
        }
        boundary(&native, 40, &[(1, 40), (2, 40), (3, 40)]).await;
        assert!(
            native.bonds().await.unwrap().contains_key(&target),
            "20/30 active stake is exactly two thirds, not a passing slash vote"
        );
    }

    #[tokio::test]
    async fn a_vote_below_two_thirds_does_not_slash() {
        let native = four(armed_params()).await;
        jail_v4(&native).await;
        let v4 = validator(4);
        native
            .cast_vote(&validator(1), &v4, true)
            .await
            .unwrap()
            .unwrap();
        boundary(&native, 40, &[(1, 40), (2, 40), (3, 40)]).await;
        assert!(
            native.bonds().await.unwrap().contains_key(&v4),
            "one third of the active stake must not slash"
        );
        assert!(native
            .liveness()
            .await
            .unwrap()
            .get(&v4)
            .is_some_and(AbsenceRecord::is_jailed));
    }

    #[tokio::test]
    async fn a_passing_vote_slashes_and_permanently_ejects() {
        let native = four(armed_params()).await;
        jail_v4(&native).await;
        let v4 = validator(4);
        for voter in 1..=3 {
            native
                .cast_vote(&validator(voter), &v4, true)
                .await
                .unwrap()
                .unwrap();
        }
        boundary(&native, 40, &[(1, 40), (2, 40), (3, 40)]).await;
        assert!(!native.bonds().await.unwrap().contains_key(&v4));
        assert!(!native.active_validators().await.unwrap().contains(&v4));
        assert!(native.banished().await.unwrap().contains(&v4));
        assert!(
            !native.liveness().await.unwrap().contains_key(&v4),
            "a removed validator has no liveness record"
        );
        assert!(
            i64::from(native.coop_balance().await.unwrap()) > 0,
            "the configured tier's share reached the Coop vault"
        );
        // The ban is permanent in this pass: the key cannot bond again.
        assert!(native.banished().await.unwrap().contains(&v4));
    }

    #[tokio::test]
    async fn a_banished_validator_cannot_rebond() {
        let native = four(armed_params()).await;
        let v4 = validator(4);
        native.set_banished(&BTreeSet::from([v4])).await.unwrap();
        // A banished key is refused before the bond's own checks: no vault balance is needed.
        native.set_vault_balance(&vault_address_of(&v4), nn(100));
        let reason = native.bond(&v4, nn(10), 11).await.unwrap();
        assert!(reason.is_err(), "a banished validator must not rebond");
    }

    #[tokio::test]
    async fn a_vote_cannot_open_against_an_active_validator() {
        let native = four(armed_params()).await;
        boundary(&native, 10, &[(1, 10), (2, 10), (3, 10), (4, 10)]).await;
        let reason = native
            .cast_vote(&validator(1), &validator(2), true)
            .await
            .unwrap();
        assert!(reason.is_err(), "an active validator cannot be voted out");
    }

    #[tokio::test]
    async fn a_jailed_validator_returns_after_the_term() {
        let native = four(armed_params()).await;
        jail_v4(&native).await;
        let v4 = validator(4);
        for block in [40i64, 50, 60, 70, 80] {
            boundary(
                &native,
                block,
                &[(1, block), (2, block), (3, block), (4, block)],
            )
            .await;
        }
        assert!(
            !native.liveness().await.unwrap().contains_key(&v4),
            "a returning validator is released when the jail expires"
        );
        assert!(native.active_validators().await.unwrap().contains(&v4));
        assert_eq!(
            native.bonds().await.unwrap().get(&v4).copied(),
            Some(nn(10))
        );
    }

    #[tokio::test]
    async fn a_still_absent_validator_is_renewed() {
        let native = four(armed_params()).await;
        jail_v4(&native).await;
        let v4 = validator(4);
        for block in [40i64, 50, 60, 70, 80] {
            boundary(&native, block, &[(1, block), (2, block), (3, block)]).await;
        }
        let record = native
            .liveness()
            .await
            .unwrap()
            .get(&v4)
            .copied()
            .expect("still jailed");
        assert!(record.is_jailed(), "continued silence renews the jail");
        assert_eq!(record.jail_remaining, 5);
    }

    #[tokio::test]
    async fn a_disabled_liveness_boundary_writes_no_liveness_leaves() {
        let native = four(PosParams::default()).await;
        boundary(&native, 1, &[(1, 1), (2, 1), (3, 1), (4, 1)]).await;
        assert!(native.liveness().await.unwrap().is_empty());
        assert!(native.liveness_anchor().await.unwrap().is_none());
        assert!(native.vote_windows().await.unwrap().is_empty());
    }
}
