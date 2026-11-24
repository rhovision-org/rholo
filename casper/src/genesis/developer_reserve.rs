//! **Custody of the developer fee reserve** (#655): a `MultiSigRevVault` the genesis creates.
//!
//! The reserve is a genesis-committed share of every deploy's burned phlo
//! (`rholang::native_state::fee_split`). Where that share is paid cannot change after genesis, so the
//! question is not only *which address* but *who can spend from it*. A configured address answers the
//! first and not the second: `RevAddress::parse` proves syntax, and nothing proves that anyone controls
//! the vault behind it. (The review on #660 names this gap.)
//!
//! So the reserve has **no configured address**. A non-zero `developer-share` makes the genesis run one
//! more deploy ([`deploy`]) that creates a `MultiSigRevVault` over the configured
//! `developer-reserve-public-keys` and `developer-reserve-quorum`, and publishes the vault's REV address
//! together with a registry URI for its handle. After the genesis deploys have run, [`bind`] reads that
//! publication and writes the native reserve record with *that* address. The reserve is therefore paid
//! into a vault whose spending authority is the contract's — `transfer`/`confirm` through
//! `deployerAuthKey` — by construction:
//!
//! * the vault's address is derived from a fresh unforgeable name inside `MultiSigRevVault.create`,
//!   so no key, configured or leaked, controls it directly;
//! * spending needs `quorum` of the configured keys, each proving itself as the deployer of the
//!   confirming deploy;
//! * the handle is published under the constant [`reserve_uri`], so the signers can reach it on any
//!   chain without depending on the genesis deploy order.
//!
//! **Fail closed.** If the vault was not created or did not publish, the genesis is refused rather than
//! installed with a reserve nobody can spend.
//!
//! **A zero share adds nothing.** No deploy, no publication, no leaf: a genesis that reserves nothing is
//! byte-identical to one built before the reserve existed.
//!
//! **What this does not give.** The signer set and quorum are fixed at genesis; `MultiSigRevVault` has
//! no rotation. Losing `quorum` keys strands the reserve (it still accrues, unspendable), and `quorum`
//! colluding signers can spend it. Forwarding to a governance treasury contract is a spend from this
//! vault, which is #655's PR-C.

use rchain_crypto::hash::blake2b256::hash as blake2b256;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signed::Signed;
use rchain_models::ast::Par;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_rholang::native_state::{DeveloperReserve, NativeSystemState, PosGenesis};
use rchain_rholang::registry::build_uri;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::base16;
use rchain_shared::refined::NonNegI64;

use super::contracts::ProofOfStake;

/// The channel the reserve deploy publishes `[revAddress, registryUri]` on, for [`bind`] to read.
///
/// A public string channel, like `rgov::URI_PUBLISH_CHANNEL`: it is read only during the genesis, when
/// the only deploys that exist are the genesis's own, and [`bind`] refuses anything but exactly one
/// well-formed publication.
pub const RESERVE_PUBLISH_CHANNEL: &str = "rnode:genesis:developer-reserve";

/// The fixed timestamp of the reserve deploy, for the reason the other genesis deploys have one.
const RESERVE_TIMESTAMP: i64 = 1_700_000_002_000;

/// `accounting.MAX_VALUE`, as for every genesis deploy.
const PHLO_LIMIT: i64 = i32::MAX as i64;

/// **The constant registry key the reserve vault's handle is published under**: a lookup answers
/// `(revAddress, multiSig)`, where `multiSig` is the vault's `bundle+` handle (`balance`, `transfer`,
/// `confirm`).
pub fn reserve_uri() -> String {
    build_uri(&blake2b256(b"rnode/genesis/developer-reserve"))
}

/// The key that signs the reserve deploy, derived from a named string so anyone can recompute it.
///
/// **It holds no authority over the reserve.** The vault's REV address comes from a fresh unforgeable
/// name inside `MultiSigRevVault.create`, and its spend checks are against the configured signer keys
/// only, so a key whose private half is public is the right kind of key to sign this deploy: it proves
/// nothing and can do nothing.
fn deployer_key() -> PrivateKey {
    PrivateKey::new(blake2b256(b"rnode/genesis/developer-reserve/deployer").to_vec())
}

/// **The genesis rule for the reserve's custody**: with a non-zero share, at least one signer key,
/// every key a 65-byte uncompressed secp256k1 public key, no key twice, and a quorum in `1..=keys`.
/// A zero share needs nothing (and the keys are ignored).
pub fn check_custody(share: NonNegI64, keys: &[String], quorum: i32) -> Result<(), String> {
    if share == NonNegI64::zero() {
        return Ok(());
    }
    if keys.is_empty() {
        return Err(
            "a non-zero casper.genesis.developer-share needs developer-reserve-public-keys: the \
             reserve is held by a multi-signature vault the genesis creates for those keys"
                .to_string(),
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for key in keys {
        let bytes = base16::decode(key)
            .ok_or_else(|| format!("developer-reserve-public-keys: {key:?} is not hex"))?;
        if bytes.len() != 65 || bytes[0] != 0x04 {
            return Err(format!(
                "developer-reserve-public-keys: {key:?} is not a 65-byte uncompressed secp256k1 \
                 public key"
            ));
        }
        if RevAddress::from_public_key(&PublicKey::new(bytes.clone())).is_none() {
            return Err(format!(
                "developer-reserve-public-keys: {key:?} does not derive a REV address"
            ));
        }
        if !seen.insert(bytes) {
            return Err(format!(
                "developer-reserve-public-keys names {key:?} twice: a repeated key is one signer, so \
                 the quorum would count a signer the vault does not have"
            ));
        }
    }
    let quorum = i64::from(quorum);
    if quorum < 1 || quorum > keys.len() as i64 {
        return Err(format!(
            "developer-reserve-quorum is {quorum}; it must be between 1 and the {} reserve key(s)",
            keys.len()
        ));
    }
    Ok(())
}

/// The reserve deploy's term: create the vault, register `(revAddress, multiSig)`, and publish
/// `[revAddress, uri]` for [`bind`].
///
/// The third element `MultiSigRevVault.create` returns — the underlying `RevVault` object — is
/// deliberately **not** registered: the handle is the only way to the funds, and it checks the quorum.
fn term(keys: &[String], quorum: i32) -> String {
    let keys = ProofOfStake::public_keys(keys);
    format!(
        r#"new rl(`rho:registry:lookup`), insertArbitrary(`rho:registry:insertArbitrary`),
    msvCh, createCh, uriCh in {{
  rl!(`rho:rchain:multiSigRevVault`, *msvCh) |
  for (@(_, MultiSigRevVault) <- msvCh) {{
    @MultiSigRevVault!("create", {keys}, [], {quorum}, *createCh) |
    for (@created <- createCh) {{
      match created {{
        (true, (multiSig, revAddr, _)) => {{
          insertArbitrary!((revAddr, multiSig), *uriCh) |
          for (@uri <- uriCh) {{
            @"{RESERVE_PUBLISH_CHANNEL}"!([revAddr, uri])
          }}
        }}
        _ => Nil
      }}
    }}
  }}
}}
//Loaded from genesis::developer_reserve"#
    )
}

/// **The reserve deploy**, or `None` when the genesis reserves nothing. Refuses a custody config
/// [`check_custody`] refuses, so a genesis cannot be built around a vault nobody could spend from.
pub fn deploy(pos: &ProofOfStake, shard_id: &str) -> Result<Option<SignedDeployData>, String> {
    if pos.developer_share == NonNegI64::zero() {
        return Ok(None);
    }
    check_custody(
        pos.developer_share,
        &pos.developer_reserve_public_keys,
        pos.developer_reserve_quorum,
    )?;
    let data = DeployData {
        attachments: Vec::new(),
        term: term(
            &pos.developer_reserve_public_keys,
            pos.developer_reserve_quorum,
        ),
        timestamp: RESERVE_TIMESTAMP,
        phlo_price: 0,
        phlo_limit: PHLO_LIMIT,
        valid_after_block_number: 0,
        shard_id: shard_id.to_string(),
    };
    let signed = Signed::new(data, &Secp256k1, &deployer_key()).map_err(|e| e.to_string())?;
    Ok(Some(SignedDeployData {
        data: signed.data,
        deployer: signed.pk.bytes().to_vec(),
        sig: signed.sig,
        sig_algorithm: signed.sig_algorithm.name().to_string(),
    }))
}

/// The publish channel as a `SortedProc`, for the callers that read it.
pub fn publish_channel() -> rchain_models::sorted::SortedProc {
    rchain_models::sorted::SortedProc::new(rchain_models::par_ops::from_expr(
        rchain_models::ast::Expr::GString(RESERVE_PUBLISH_CHANNEL.to_string()),
    ))
}

/// Parse one published `[revAddress, uri]` datum.
fn parse_publication(par: &Par) -> Option<(String, String)> {
    use rchain_models::rholang::RhoType::{RhoList, RhoString, RhoUri};
    let items = RhoList::unapply(par)?;
    if items.len() != 2 {
        return None;
    }
    let address = RhoString::unapply(&items[0])?.to_string();
    let uri = RhoUri::unapply(&items[1])
        .or_else(|| RhoString::unapply(&items[1]))?
        .to_string();
    Some((address, uri))
}

/// **Bind the reserve to the vault the genesis created** — the play/replay-independent step both
/// `compute_genesis` and the genesis replay take after the genesis deploys, from the same published
/// datums, so they write the same native state.
///
/// With no reserve (`None` or a zero share) it does nothing — and refuses a publication it did not
/// expect, because one means the genesis block carries a reserve its configuration does not. With a
/// reserve it requires **exactly one** well-formed publication whose address parses and whose URI is
/// registered, copies the registry entry onto [`reserve_uri`], and writes the reserve record. Anything
/// else refuses the genesis: a reserve with no spendable custody must not be installed.
pub async fn bind(
    published: &[Par],
    native: &NativeSystemState,
    pos_genesis: &PosGenesis,
) -> Result<(), String> {
    let share = pos_genesis.developer_share.unwrap_or(NonNegI64::zero());
    if share == NonNegI64::zero() {
        if !published.is_empty() {
            return Err(
                "the genesis published a developer reserve vault, but its developer-share is zero"
                    .to_string(),
            );
        }
        return Ok(());
    }
    let [datum] = published else {
        return Err(format!(
            "a non-zero developer-share needs exactly one developer reserve vault, and the genesis \
             published {} — the reserve would have no spendable custody",
            published.len()
        ));
    };
    let (address, uri) = parse_publication(datum).ok_or_else(|| {
        "the developer reserve vault's publication is not [revAddress, uri]".to_string()
    })?;
    RevAddress::parse(&address).map_err(|e| {
        format!("the developer reserve vault published {address:?}, not a REV address: {e}")
    })?;
    let value = native
        .registry_lookup(&uri)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!("the developer reserve vault's registry entry {uri} was not written")
        })?;
    native.registry_insert(&reserve_uri(), &value);
    native.bind_developer_reserve(&pos_genesis.params, &DeveloperReserve { share, address })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_crypto::signatures::signatures_alg::SignaturesAlg;

    fn key(seed: u8) -> String {
        let sk = PrivateKey::new(blake2b256(&[seed]).to_vec());
        base16::encode(Secp256k1.to_public(&sk).unwrap().bytes())
    }

    fn nn(v: i64) -> NonNegI64 {
        NonNegI64::try_from(v).unwrap()
    }

    fn pos() -> ProofOfStake {
        ProofOfStake {
            minimum_bond: nn(1),
            maximum_bond: nn(100),
            validators: vec![],
            epoch_length: 0,
            quarantine_length: 0,
            number_of_active_validators: 0,
            executor_share: nn(2500),
            absence_slack: nn(0),
            participation_grace: nn(0),
            min_phlo_price: nn(0),
            max_number_of_parents: 0,
            pos_multi_sig_public_keys: vec![],
            pos_multi_sig_quorum: 0,
            pos_vault_pub_key: String::new(),
            developer_share: nn(0),
            developer_reserve_public_keys: Vec::new(),
            developer_reserve_quorum: 0,
            dev_tax_vesting_blocks: nn(0),
            liveness_absence_threshold: 0,
            liveness_jail_after_boundaries: 0,
            liveness_jail_epochs: 0,
            liveness_vote_window: 0,
            liveness_slash_severity: 0,
        }
    }

    /// **The custody rule refuses every config that would leave the reserve unspendable or
    /// under-signed**, and a zero share needs nothing.
    #[test]
    fn the_custody_rule_refuses_a_reserve_nobody_could_spend() {
        assert!(
            check_custody(nn(0), &[], 0).is_ok(),
            "no reserve, no custody"
        );
        assert!(check_custody(nn(500), &[key(1)], 1).is_ok());
        assert!(check_custody(nn(500), &[key(1), key(2), key(3)], 2).is_ok());

        let err = check_custody(nn(500), &[], 1).unwrap_err();
        assert!(err.contains("developer-reserve-public-keys"), "{err}");
        assert!(check_custody(nn(500), &[key(1)], 0).is_err(), "quorum 0");
        assert!(
            check_custody(nn(500), &[key(1)], 2).is_err(),
            "quorum above the keys"
        );
        assert!(
            check_custody(nn(500), &[key(1), key(1)], 2).is_err(),
            "a repeated key is one signer"
        );
        assert!(
            check_custody(nn(500), &["zz".to_string()], 1).is_err(),
            "not hex"
        );
        assert!(
            check_custody(nn(500), &["04".repeat(33)], 1).is_err(),
            "wrong length"
        );
    }

    /// **A zero share adds no deploy**, so a genesis that reserves nothing is the genesis it was.
    #[test]
    fn a_zero_share_adds_no_deploy_and_a_reserve_adds_one() {
        let mut pos = pos();
        assert!(deploy(&pos, "root").unwrap().is_none());
        pos.developer_share = nn(500);
        assert!(
            deploy(&pos, "root").is_err(),
            "a reserve with no signers is refused"
        );
        pos.developer_reserve_public_keys = vec![key(1), key(2)];
        pos.developer_reserve_quorum = 2;
        let d = deploy(&pos, "root")
            .unwrap()
            .expect("a reserve adds its vault deploy");
        assert!(d.data.term.contains(&key(1)) && d.data.term.contains(&key(2)));
        assert!(d.data.term.contains("], [], 2, *createCh)"));
        assert!(d.data.term.contains(RESERVE_PUBLISH_CHANNEL));
    }
}
