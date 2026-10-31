//! **The ERTP object API** — `casper/src/genesis/resources/ERTP.rho` over the native issuer ledger
//! (`rho:rchain:ertp:ledger`, W2.1). Issue #249, condition 3.
//!
//! The contract is installed here the way any deploy is: `compute_genesis` plays the `.rho` file as
//! an ordinary deploy signed by a fixed key, so the URI the file registers itself under is derivable
//! (`registry_insert_signed` derives it from the deployer's public key) and a second deploy can look
//! the contract up and call it. **Nothing in this file consults the blessing** — the alias tier
//! (`rho:rchain:ertp`) is W2.3's, and the fresh-chain gate that uses it is W2.4's. That is the
//! separation the two tests exist for: this one proves the contract, the other proves the blessing.
//!
//! Every assertion is read back out of the **block path**, not out of the contract: the driver
//! deploy reports each observation on its own `@"tag"` channel, and a missing tag is a failed
//! assertion with a name. That matters because an unmatched `for` in Rholang is silent — the failure
//! mode this port keeps meeting is a call that reaches nothing and looks like a client bug.
//!
//! The load-bearing clause is [`a_payment_of_one_brand_is_refused_by_another_brands_purse_and_survives_it`]:
//! a refused cross-brand deposit must leave the payment **live**, which is what distinguishes a
//! brand check from a burn.

mod common;

use rchain_casper::genesis::contracts::{ProofOfStake, Registry, Vault};
use rchain_casper::genesis::default_blessed_terms;
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_crypto::hash::blake2b256;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_models::ast::{Expr, Par};
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_models::par_ops::from_expr;
use rchain_models::rholang::RhoType::{RhoBoolean, RhoName, RhoNumber, RhoString, RhoTupleN};
use rchain_models::sorted::SortedProc;
use rchain_rholang::native_state::PosGenesis;
use rchain_rholang::system_processes::BlockData;
use rchain_shared::refined::NonNegI64;

use common::build_runtime_manager;

/// The contract, exactly as genesis will embed it.
const ERTP_RHO: &str = include_str!("../src/genesis/resources/ERTP.rho");

/// The key the contract is installed under. Its public half determines the `rho:id` the file
/// registers itself under, so a driver can name the contract without the blessing.
const ERTP_SEED: u8 = 21;

fn fixed_rand() -> Blake2b512Random {
    Blake2b512Random::from_init(&[11u8; 32])
}

/// A deploy signed by a **real** key pair, so `rho:rchain:deployerId` is a genuine public key and
/// the registry URI is the one `registry_insert_signed` derives from it.
fn deploy_signed_by(term: &str, seed: u8) -> SignedDeployData {
    deploy_signed_by_at(term, seed, 1)
}

/// The same, with the phlo price chosen. **A REV flow that spends from the deployer's own vault uses
/// `0`**, because pre-charge would otherwise move that same account and the arithmetic on it would
/// be "minus the fee" rather than exact.
fn deploy_signed_by_at(term: &str, seed: u8, phlo_price: i64) -> SignedDeployData {
    let sk = PrivateKey::new(vec![seed; 32]);
    let pk = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid secp256k1 key");
    SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: term.to_string(),
            timestamp: 0,
            phlo_price,
            phlo_limit: 5_000_000,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: pk.bytes().to_vec(),
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// The `rho:id` the ERTP deploy registers itself under — the same derivation
/// `registry_insert_signed` performs, so the two cannot drift without this test failing.
fn ertp_uri() -> String {
    let sk = PrivateKey::new(vec![ERTP_SEED; 32]);
    let pk = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid secp256k1 key");
    rchain_rholang::registry::build_uri(&blake2b256::hash(pk.bytes()))
}

/// A driver: look `target` up, then run `body`. The lookup is the consumer's own idiom
/// (`rho:registry:lookup` → `for (@(_, C) <- ch) { @C!(…) }`), so a contract that registered nothing
/// fails here rather than silently.
///
/// `target` is either the contract's own `rho:id` — what W2.2 uses, so the gate does not depend on
/// the blessing — or the shorthand `rho:rchain:ertp`, which is what W2.4 uses. Both are URI
/// literals to the lookup, which is exactly why one helper serves both and why the two tests differ
/// only in what they are evidence *about*.
fn driver(target: &str, body: &str) -> SignedDeployData {
    driver_by(target, body, ERTP_SEED + 1)
}

/// The same, signed by a chosen key — the REV drivers need a deployer whose vault is seeded, so
/// "who is asking" and "what the contract is" have to be separable.
fn driver_by(target: &str, body: &str, seed: u8) -> SignedDeployData {
    let term = format!(
        r#"new rl(`rho:registry:lookup`), ch in {{
             rl!(`{target}`, *ch) |
             for (@(_, ERTP) <- ch) {{
               {body}
             }}
           }}"#
    );
    deploy_signed_by(&term, seed)
}

/// Play a list of deploys through the genesis path, returning the manager at the post-state and each
/// deploy's verdict: `None` when it succeeded, or the error when it did not.
///
/// **The REV gate needs the failing case**, so it is reported rather than asserted here: a forged
/// mint that presents *bytes* where a name is expected is refused by erroring the deploy
/// (`ertp_name`), which is how a refusal reaches a caller at the chain level — the deploy is not
/// accepted and no state moves.
async fn play_and_report(
    terms: &[SignedDeployData],
    vaults: &[Vault],
) -> (
    RuntimeManager,
    rchain_crypto::hash::blake2b256_hash::Blake2b256Hash,
    Vec<Option<String>>,
) {
    let rm = build_runtime_manager().await;
    let (_, post, results) = rm
        .compute_genesis(
            terms,
            &fixed_rand(),
            BlockData::empty(),
            &PosGenesis::default(),
            vaults,
        )
        .await
        .expect("compute_genesis");
    let verdicts = results
        .iter()
        .map(|r| {
            if r.eval_result.succeeded() {
                None
            } else {
                Some(format!("{:?}", r.eval_result.errors))
            }
        })
        .collect();
    (rm, post, verdicts)
}

/// Play a list of deploys through the genesis path and return the manager at the post-state, so the
/// tags can be read. A deploy that did not succeed is reported by index, because that is the one
/// failure that makes every assertion below vacuous at once.
async fn play_terms(terms: &[SignedDeployData]) -> RuntimeManager {
    let (rm, _, verdicts) = play_and_report(terms, &[]).await;
    for (i, verdict) in verdicts.iter().enumerate() {
        assert!(verdict.is_none(), "deploy #{i} failed: {verdict:?}");
    }
    rm
}

/// **W2.2's vehicle**: the contract installed as an ordinary deploy, then each driver reaching it by
/// its own `rho:id`. Nothing here consults the blessing.
async fn play(bodies: &[&str]) -> RuntimeManager {
    let mut terms = vec![deploy_signed_by(ERTP_RHO, ERTP_SEED)];
    terms.extend(bodies.iter().map(|b| driver(&ertp_uri(), b)));
    play_terms(&terms).await
}

/// **W2.4's vehicle**: a real chain's blessed set, then each driver reaching the contract through
/// the shorthand a consumer hardcodes.
async fn play_blessed(bodies: &[&str]) -> RuntimeManager {
    let mut terms = default_blessed_terms(
        &proof_of_stake(),
        &Registry {
            system_contract_pub_key: String::new(),
        },
        &[],
        "root",
        &ceremony_identity(),
    )
    .expect("the blessed term list builds");
    terms.extend(bodies.iter().map(|b| driver("rho:rchain:ertp", b)));
    play_terms(&terms).await
}

/// The PoS parameters a test genesis needs (the same shape `genesis_registry.rs` uses).
fn proof_of_stake() -> ProofOfStake {
    ProofOfStake {
        minimum_bond: rchain_shared::refined::NonNegI64::try_from(1).unwrap(),
        maximum_bond: rchain_shared::refined::NonNegI64::try_from(100).unwrap(),
        validators: Vec::new(),
        epoch_length: 1,
        quarantine_length: 1,
        number_of_active_validators: 1,
        executor_share: rchain_shared::refined::NonNegI64::try_from(0).unwrap(),
        absence_slack: rchain_shared::refined::NonNegI64::try_from(0).unwrap(),
        participation_grace: rchain_shared::refined::NonNegI64::try_from(0).unwrap(),
        min_phlo_price: rchain_shared::refined::NonNegI64::zero(),
        max_number_of_parents: 0,
        pos_multi_sig_public_keys: Vec::new(),
        pos_multi_sig_quorum: 1,
        pos_vault_pub_key: String::new(),
        dev_tax_vesting_blocks: NonNegI64::zero(),

        liveness_absence_threshold: 0,
        liveness_jail_after_boundaries: 0,
        liveness_jail_epochs: 0,
        liveness_vote_window: 0,
        liveness_slash_severity: 0,
    }
}

/// The genesis ceremony's identity, fixed so a test genesis is deterministic.
fn ceremony_identity() -> rchain_casper::validator_identity::ValidatorIdentity {
    let sk = PrivateKey::new(vec![7u8; 32]);
    let public_key = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid key");
    rchain_casper::validator_identity::ValidatorIdentity {
        public_key,
        private_key: sk,
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// Everything sent to `@"tag"`. An absent tag is a failed assertion, not an error, so every caller
/// checks the length.
async fn read(rm: &RuntimeManager, tag: &str) -> Vec<Par> {
    rm.runtime()
        .get_data_par(&SortedProc::new(from_expr(Expr::GString(tag.to_string()))))
        .await
        .expect("read the driver's output channel")
}

/// The one value sent to `@"tag"`.
async fn one(rm: &RuntimeManager, tag: &str) -> Par {
    let v = read(rm, tag).await;
    assert_eq!(
        v.len(),
        1,
        "`{tag}` must have been sent exactly once: {v:?}"
    );
    v[0].clone()
}

/// The `(true, value)` / `(false, reason)` pair every ERTP op replies in.
async fn pair(rm: &RuntimeManager, tag: &str) -> (bool, Par) {
    let p = one(rm, tag).await;
    let parts = RhoTupleN::unapply(&p).unwrap_or_else(|| panic!("`{tag}` must be a pair: {p:?}"));
    assert_eq!(parts.len(), 2, "`{tag}` must be a pair: {p:?}");
    let ok = RhoBoolean::unapply(&parts[0])
        .unwrap_or_else(|| panic!("`{tag}`'s first element must be a boolean"));
    (ok, parts[1].clone())
}

/// A refusal and its reason — the assertion that an op *declined* rather than answered.
async fn refused(rm: &RuntimeManager, tag: &str) -> String {
    let (ok, reason) = pair(rm, tag).await;
    assert!(!ok, "`{tag}` must be refused, and it was answered instead");
    RhoString::unapply(&reason)
        .unwrap_or_else(|| panic!("`{tag}`'s reason must be a string: {reason:?}"))
        .to_string()
}

/// A reply that must be an answer, not a refusal.
async fn value(rm: &RuntimeManager, tag: &str) -> Par {
    let (ok, v) = pair(rm, tag).await;
    assert!(ok, "`{tag}` must be answered, and it was refused: {v:?}");
    v
}

/// The Int an answered op replied — `(true, n)`, which is the shape every op but `makeIssuerKit`
/// uses.
async fn answered_number(rm: &RuntimeManager, tag: &str) -> i64 {
    let v = value(rm, tag).await;
    RhoNumber::unapply(&v).unwrap_or_else(|| panic!("`{tag}` must answer an Int: {v:?}"))
}

/// The **amount** an answered op replied — `(true, (brand, n))`, the shape `amountMath`'s
/// constructors and arithmetic use.
async fn answered_amount(rm: &RuntimeManager, tag: &str) -> (Par, i64) {
    let v = value(rm, tag).await;
    let parts =
        RhoTupleN::unapply(&v).unwrap_or_else(|| panic!("`{tag}` must answer an amount: {v:?}"));
    assert_eq!(parts.len(), 2, "`{tag}` must answer an amount: {v:?}");
    let n = RhoNumber::unapply(&parts[1])
        .unwrap_or_else(|| panic!("`{tag}`'s amount must hold an Int: {v:?}"));
    (parts[0].clone(), n)
}

async fn boolean(rm: &RuntimeManager, tag: &str) -> bool {
    let v = one(rm, tag).await;
    RhoBoolean::unapply(&v).unwrap_or_else(|| panic!("`{tag}` must be a boolean: {v:?}"))
}

/// The boolean an answered op replied — `(true, true)`, the shape `isEqual` and `isEmpty` use.
async fn answered_boolean(rm: &RuntimeManager, tag: &str) -> bool {
    let v = value(rm, tag).await;
    RhoBoolean::unapply(&v).unwrap_or_else(|| panic!("`{tag}` must answer a boolean: {v:?}"))
}

/// A capability on the wire is `bundle+{*name}` — the shape every ERTP handle takes (the kit, a
/// purse, a payment, the facet). Asserting *that* rather than "not a number" is the difference
/// between checking the reply is a handle and checking the reply is not the wrong thing.
fn assert_capability(p: &Par, what: &str) {
    assert_eq!(
        p.bundles.len(),
        1,
        "{what} must be a bundle over a name: {p:?}"
    );
    assert_eq!(
        p.bundles[0].body.unforgeables.len(),
        1,
        "{what} must be a bundle over an unforgeable name: {p:?}"
    );
}

/// The three parts of a reported kit — `(brand, mint, issuer)`, Agoric's shape.
async fn kit(rm: &RuntimeManager, tag: &str) -> (Par, Par, Par) {
    let v = one(rm, tag).await;
    let parts =
        RhoTupleN::unapply(&v).unwrap_or_else(|| panic!("`{tag}` must be a 3-tuple: {v:?}"));
    assert_eq!(parts.len(), 3, "`{tag}` must be a 3-tuple: {v:?}");
    (parts[0].clone(), parts[1].clone(), parts[2].clone())
}

/// Run an async body on a worker thread with the node's 32 MiB stack: the blessed contracts recurse
/// deeper than the default 2 MiB test stack allows when they parse and normalize.
fn with_big_stack<F>(body: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    const STACK: usize = 32 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .thread_stack_size(STACK)
                .enable_all()
                .build()
                .expect("a tokio runtime")
                .block_on(body)
        })
        .expect("spawn the test thread")
        .join()
        .expect("the test thread panicked");
}

/// **A kit is `(brand, mint, issuer)`, the issuer knows its brand, and a fresh purse is empty.**
///
/// The three are pairwise distinct: a kit whose brand *is* its mint would make "who may mint this?"
/// answerable by anyone holding an amount, which is the whole distinction ERTP exists to draw.
#[test]
fn a_kit_makes_a_brand_a_mint_and_an_issuer() {
    with_big_stack(async {
        let rm = play(&[r#"
new kitCh in {
  @ERTP!("makeIssuerKit", *kitCh) |
  for (@(brand, mint, issuer) <- kitCh) {
    @"kit"!((brand, mint, issuer)) |
    new bc, pc in {
      // One boolean, so a collision names itself rather than three separate reads.
      @"kit-collides"!((brand == mint) || (brand == issuer) || (mint == issuer)) |
      @issuer!("getBrand", *bc) |
      for (@r <- bc) { @"issuer-brand"!(r) } |
      @issuer!("makeEmptyPurse", *pc) |
      for (@r <- pc) {
        match r {
          (true, purse) => {
            new cc in {
              @purse!("getCurrentAmount", *cc) |
              for (@b <- cc) { @"empty-purse"!(b) }
            }
          }
          _ => { @"empty-purse"!(r) }
        }
      }
    }
  }
}
"#])
        .await;

        let (brand, _, _) = kit(&rm, "kit").await;
        assert!(
            !boolean(&rm, "kit-collides").await,
            "the brand, the mint and the issuer must be three different names"
        );
        assert_eq!(
            value(&rm, "issuer-brand").await,
            brand,
            "the issuer's `getBrand` must be the kit's brand, by structural equality"
        );
        assert_eq!(
            answered_number(&rm, "empty-purse").await,
            0,
            "a purse from `makeEmptyPurse` must read zero"
        );
    });
}

/// **A minted payment carries its brand and its amount, a deposit credits the purse, and the second
/// deposit of the same payment is refused with the balance unmoved.** That last clause is ERTP's
/// double-spend guard: a payment that could be deposited twice is a purse.
#[test]
fn a_payment_is_spent_by_its_first_deposit() {
    with_big_stack(async {
        let rm = play(&[r#"
new kitCh in {
  @ERTP!("makeIssuerKit", *kitCh) |
  for (@(brand, mint, issuer) <- kitCh) {
    @"kit"!((brand, mint, issuer)) |
    new amCh in {
      @issuer!("getAmountMath", *amCh) |
      for (@(true, am) <- amCh) {
        new amtCh in {
          @am!("make", brand, 10, *amtCh) |
          for (@(true, amount) <- amtCh) {
            new payCh in {
              @mint!("mintPayment", amount, *payCh) |
              for (@(true, payment) <- payCh) {
                new abCh in {
                  @payment!("getAllegedBrand", *abCh) |
                  for (@r <- abCh) { @"payment-brand"!(r) }
                } |
                new aoCh in {
                  @issuer!("getAmountOf", payment, *aoCh) |
                  for (@r <- aoCh) { @"payment-amount"!(r) }
                } |
                new pc in {
                  @issuer!("makeEmptyPurse", *pc) |
                  for (@(true, purse) <- pc) {
                    new d1 in {
                      @purse!("deposit", payment, *d1) |
                      for (@r1 <- d1) {
                        @"first-deposit"!(r1) |
                        new d2 in {
                          @purse!("deposit", payment, *d2) |
                          for (@r2 <- d2) {
                            @"second-deposit"!(r2) |
                            new cc in {
                              @purse!("getCurrentAmount", *cc) |
                              for (@b <- cc) { @"balance-after"!(b) }
                            }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#])
        .await;

        let (brand, _, _) = kit(&rm, "kit").await;
        assert_eq!(
            value(&rm, "payment-brand").await,
            brand,
            "a minted payment must report the brand it was minted under"
        );
        assert_eq!(answered_number(&rm, "payment-amount").await, 10);
        assert_eq!(answered_number(&rm, "first-deposit").await, 10);
        assert_eq!(
            refused(&rm, "second-deposit").await,
            "that payment has already been deposited",
            "a payment is consumed by its first deposit"
        );
        assert_eq!(
            answered_number(&rm, "balance-after").await,
            10,
            "the refused second deposit must not have moved the balance"
        );
    });
}

/// **The acceptance clause: a payment of one brand is refused by another brand's purse — and
/// survives it.** The refusal must leave the payment *live*, so the deposit into its own brand's
/// purse afterwards succeeds. A purse that burned a foreign payment would pass a weaker test.
///
/// The order is the point: B's refusal finishes, **then** A's deposit runs.
#[test]
fn a_payment_of_one_brand_is_refused_by_another_brands_purse_and_survives_it() {
    with_big_stack(async {
        let rm = play(&[r#"
new kitA, kitB in {
  @ERTP!("makeIssuerKit", *kitA) |
  @ERTP!("makeIssuerKit", *kitB) |
  for (@(brandA, mintA, issuerA) <- kitA & @(brandB, mintB, issuerB) <- kitB) {
    @"kitA"!((brandA, mintA, issuerA)) |
    @"kitB"!((brandB, mintB, issuerB)) |
    new amCh in {
      @issuerA!("getAmountMath", *amCh) |
      for (@(true, am) <- amCh) {
        new amtCh in {
          @am!("make", brandA, 10, *amtCh) |
          for (@(true, amount) <- amtCh) {
            new payCh in {
              @mintA!("mintPayment", amount, *payCh) |
              for (@(true, paymentA) <- payCh) {
                new pbc in {
                  @issuerB!("makeEmptyPurse", *pbc) |
                  for (@(true, purseB) <- pbc) {
                    new d1 in {
                      // B's purse, A's payment, and B's brand stated by B.
                      @purseB!("deposit", paymentA, *d1) |
                      for (@r1 <- d1) {
                        @"b-deposit-a"!(r1) |
                        new cb in {
                          @purseB!("getCurrentAmount", *cb) |
                          for (@b <- cb) { @"b-balance"!(b) }
                        } |
                        // Only now: the same payment, its own brand's purse.
                        new pac in {
                          @issuerA!("makeEmptyPurse", *pac) |
                          for (@(true, purseA) <- pac) {
                            new d2 in {
                              @purseA!("deposit", paymentA, *d2) |
                              for (@r2 <- d2) { @"a-deposit-a"!(r2) }
                            }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#])
        .await;

        let (brand_a, _, _) = kit(&rm, "kitA").await;
        let (brand_b, _, _) = kit(&rm, "kitB").await;
        assert_ne!(brand_a, brand_b, "two kits must not share a brand");

        assert_eq!(
            refused(&rm, "b-deposit-a").await,
            "no such payment",
            "another brand's purse must refuse the payment structurally: it names a token the \
             ledger does not know under this brand"
        );
        assert_eq!(
            answered_number(&rm, "b-balance").await,
            0,
            "the refused deposit must not have credited B's purse"
        );
        assert_eq!(
            answered_number(&rm, "a-deposit-a").await,
            10,
            "**the refused payment must stay live**: A's own purse must still be able to take it"
        );
    });
}

/// **A deposit facet can add and cannot withdraw.** `getDepositFacet` hands back a bundle of a
/// forwarder with one arm, so a facet holder never sees the purse's ledger token. The test sends
/// `withdraw` to the facet and asserts nothing ever answers it — which is the only observable form
/// of "the arm is not there", since an unmatched `for` in Rholang is silent.
#[test]
fn the_deposit_facet_can_add_and_cannot_withdraw() {
    with_big_stack(async {
        let rm = play(&[r#"
new kitCh in {
  @ERTP!("makeIssuerKit", *kitCh) |
  for (@(brand, mint, issuer) <- kitCh) {
    new pc in {
      @issuer!("makeEmptyPurse", *pc) |
      for (@(true, purse) <- pc) {
        new fc, wr in {
          @purse!("getDepositFacet", *fc) |
          for (@(true, facet) <- fc) {
            // Sent, but never answered: a reply here would be the defect.
            @facet!("withdraw", (brand, 1), *wr) |
            @"facet-probe-sent"!(true) |
            for (@_ <- wr) { @"facet-answered-withdraw"!(true) } |
            // …and the facet's one arm does work.
            new amCh in {
              @issuer!("getAmountMath", *amCh) |
              for (@(true, am) <- amCh) {
                new amtCh in {
                  @am!("make", brand, 10, *amtCh) |
                  for (@(true, amount) <- amtCh) {
                    new payCh in {
                      @mint!("mintPayment", amount, *payCh) |
                      for (@(true, payment) <- payCh) {
                        new dr in {
                          @facet!("deposit", payment, *dr) |
                          for (@r <- dr) {
                            @"facet-deposit"!(r) |
                            new cc in {
                              @purse!("getCurrentAmount", *cc) |
                              for (@b <- cc) { @"purse-after-facet-deposit"!(b) }
                            }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#])
        .await;

        assert!(
            boolean(&rm, "facet-probe-sent").await,
            "the probe must have run, or the absence below proves nothing"
        );
        assert!(
            read(&rm, "facet-answered-withdraw").await.is_empty(),
            "a deposit facet must not answer `withdraw`"
        );
        assert_eq!(
            answered_number(&rm, "facet-deposit").await,
            10,
            "the facet's `deposit` must be the purse's `deposit`"
        );
        assert_eq!(
            answered_number(&rm, "purse-after-facet-deposit").await,
            10,
            "the facet must have credited the purse it came from"
        );
    });
}

/// **`amountMath`: the pair, the arithmetic, and every refusal that keeps an amount depositable.**
///
/// A value the ledger cannot hold must never come out of `amountMath`, because an amount that
/// exists and cannot be deposited is the silent kind of broken. `make(brand, -1)`, `make(brand,
/// BigInt(2))`, another brand's amount, a subtraction that would go negative and an addition that
/// leaves `i64` are all refusals — never truncations, never wraps.
#[test]
fn amount_math_refuses_what_the_ledger_could_not_hold() {
    with_big_stack(async {
        let rm = play(&[r#"
new kitA, kitB in {
  @ERTP!("makeIssuerKit", *kitA) |
  @ERTP!("makeIssuerKit", *kitB) |
  for (@(brandA, mintA, issuerA) <- kitA & @(brandB, mintB, issuerB) <- kitB) {
    @"kitA"!((brandA, mintA, issuerA)) |
    @"kitB"!((brandB, mintB, issuerB)) |
    new amCh in {
      @issuerA!("getAmountMath", *amCh) |
      for (@(true, am) <- amCh) {
        // --- make
        new r in { @am!("make", brandA, 5, *r) | for (@x <- r) { @"make-ok"!(x) } } |
        new r in { @am!("make", brandA, -1, *r) | for (@x <- r) { @"make-negative"!(x) } } |
        new r in { @am!("make", brandA, BigInt(2), *r) | for (@x <- r) { @"make-bigint"!(x) } } |
        new r in { @am!("make", brandB, 5, *r) | for (@x <- r) { @"make-foreign"!(x) } } |
        // --- getValue, and the pair it insists on
        new r in { @am!("getValue", (brandA, 5), *r) | for (@x <- r) { @"get-value"!(x) } } |
        new r in { @am!("getValue", (brandB, 5), *r) | for (@x <- r) { @"get-value-foreign"!(x) } } |
        new r in { @am!("getValue", 5, *r) | for (@x <- r) { @"get-value-bare"!(x) } } |
        // --- add and subtract, on amounts made here
        new ma in {
          @am!("make", brandA, 5, *ma) |
          for (@(true, five) <- ma) {
            new mb in {
              @am!("make", brandA, 7, *mb) |
              for (@(true, seven) <- mb) {
                new r in { @am!("add", five, seven, *r) | for (@x <- r) { @"add-ok"!(x) } } |
                new r in { @am!("subtract", seven, five, *r) | for (@x <- r) { @"subtract-ok"!(x) } } |
                new r in { @am!("subtract", five, seven, *r) | for (@x <- r) { @"subtract-negative"!(x) } } |
                new r in { @am!("add", five, (brandB, 5), *r) | for (@x <- r) { @"add-foreign"!(x) } } |
                new r in { @am!("isEqual", five, (brandA, 5), *r) | for (@x <- r) { @"is-equal"!(x) } } |
                new r in { @am!("isEmpty", (brandA, 0), *r) | for (@x <- r) { @"is-empty"!(x) } } |
                new r in { @am!("isEmpty", five, *r) | for (@x <- r) { @"is-not-empty"!(x) } }
              }
            }
          }
        } |
        // --- the i64 bound
        new mm in {
          @am!("make", brandA, 9223372036854775807, *mm) |
          for (@(true, max) <- mm) {
            new mo in {
              @am!("make", brandA, 1, *mo) |
              for (@(true, one) <- mo) {
                new r in { @am!("add", max, one, *r) | for (@x <- r) { @"add-overflow"!(x) } } |
                new r in { @am!("subtract", max, max, *r) | for (@x <- r) { @"subtract-to-zero"!(x) } }
              }
            }
          }
        } |
        // --- the brand the math is bound to
        new r in { @am!("getBrand", *r) | for (@x <- r) { @"amount-math-brand"!(x) } }
      }
    }
  }
}
"#])
        .await;

        let (brand_a, _, _) = kit(&rm, "kitA").await;
        let (brand_b, _, _) = kit(&rm, "kitB").await;

        // The pair itself, and the two values `make` refuses.
        let made = value(&rm, "make-ok").await;
        let parts = RhoTupleN::unapply(&made).expect("an amount is a pair");
        assert_eq!(
            parts[0].clone(),
            brand_a,
            "an amount must carry the brand it was made of"
        );
        assert_eq!(RhoNumber::unapply(&parts[1]), Some(5));
        assert_eq!(
            refused(&rm, "make-negative").await,
            "an amount is not negative"
        );
        assert_eq!(
            refused(&rm, "make-bigint").await,
            "an amount's value is an Int",
            "a BigInt is not an `Int` the ledger could hold — refused, never truncated"
        );
        assert_eq!(
            refused(&rm, "make-foreign").await,
            "that amount is of another brand"
        );

        // Reading, and the shapes it refuses.
        assert_eq!(answered_number(&rm, "get-value").await, 5);
        assert_eq!(
            refused(&rm, "get-value-foreign").await,
            "that amount is of another brand"
        );
        assert_eq!(
            refused(&rm, "get-value-bare").await,
            "an amount is a (brand, value) pair"
        );

        // Arithmetic. `add` and `subtract` answer *amounts*, so the brand must be carried through
        // as well as the number.
        let (sum_brand, sum) = answered_amount(&rm, "add-ok").await;
        assert_eq!(sum, 12);
        assert_eq!(sum_brand, brand_a);
        let (difference_brand, difference) = answered_amount(&rm, "subtract-ok").await;
        assert_eq!(difference, 2);
        assert_eq!(difference_brand, brand_a);
        assert_eq!(
            refused(&rm, "subtract-negative").await,
            "the subtraction would be negative"
        );
        assert_eq!(
            refused(&rm, "add-foreign").await,
            "that amount is of another brand",
            "two brands cannot be added"
        );
        assert!(answered_boolean(&rm, "is-equal").await);
        assert!(answered_boolean(&rm, "is-empty").await);
        assert!(!answered_boolean(&rm, "is-not-empty").await);

        // The bound, at the boundary and one past it.
        assert_eq!(
            answered_amount(&rm, "subtract-to-zero").await.1,
            0,
            "`i64::MAX - i64::MAX` must be an amount"
        );
        assert_eq!(
            refused(&rm, "add-overflow").await,
            "the sum is larger than an amount can hold",
            "`i64::MAX + 1` promotes to a BigInt, which no purse could hold"
        );

        // And the math is bound to one brand.
        assert_eq!(value(&rm, "amount-math-brand").await, brand_a);
        assert_ne!(brand_a, brand_b);
    });
}

/// **A purse gives a payment back out of itself, and refuses to give more than it holds.**
///
/// `withdraw` is the arm the issuer's authority reaches through, so it is the one that must not be
/// tricked into a negative balance: the amount is validated by the *purse* before the ledger sees
/// it, because the ledger's own amount parser refuses a negative by failing the deploy rather than
/// answering — and a caller deserves a refusal it can branch on.
#[test]
fn a_purse_withdraws_a_payment_and_refuses_more_than_it_holds() {
    with_big_stack(async {
        let rm = play(&[r#"
new kitCh in {
  @ERTP!("makeIssuerKit", *kitCh) |
  for (@(brand, mint, issuer) <- kitCh) {
    @"kit"!((brand, mint, issuer)) |
    new amCh in {
      @issuer!("getAmountMath", *amCh) |
      for (@(true, am) <- amCh) {
        new pc in {
          @issuer!("makeEmptyPurse", *pc) |
          for (@(true, purse) <- pc) {
            // Fund the purse with a minted 10.
            new mc in {
              @am!("make", brand, 10, *mc) |
              for (@(true, ten) <- mc) {
                new mp in {
                  @mint!("mintPayment", ten, *mp) |
                  for (@(true, minted) <- mp) {
                    new fd in {
                      @purse!("deposit", minted, *fd) |
                      for (@_ <- fd) {
                        // Withdraw 4 of it.
                        new wc in {
                          @am!("make", brand, 4, *wc) |
                          for (@(true, four) <- wc) {
                            new wp in {
                              @purse!("withdraw", four, *wp) |
                              for (@rw <- wp) {
                                @"withdrawn"!(rw) |
                                match rw {
                                  (true, payment) => {
                                    // **Read the payment's brand and amount, then the purse's balance,
                                    // then put the payment back — in that order.** Every read here is
                                    // a read of value that is about to move, so running any of them
                                    // concurrently with the redeposit would observe the *end* state
                                    // and pass for the wrong reason.
                                    new abCh in {
                                      @payment!("getAllegedBrand", *abCh) |
                                      for (@r <- abCh) {
                                        @"withdrawn-brand"!(r) |
                                        new aoCh in {
                                          @issuer!("getAmountOf", payment, *aoCh) |
                                          for (@r2 <- aoCh) {
                                            @"withdrawn-amount"!(r2) |
                                            new bc in {
                                              @purse!("getCurrentAmount", *bc) |
                                              for (@b <- bc) {
                                                @"balance-after-withdraw"!(b) |
                                                // **And the payment is live**: a withdrawn payment
                                                // is an ordinary payment, so the purse takes it back.
                                                new rd in {
                                                  @purse!("deposit", payment, *rd) |
                                                  for (@r3 <- rd) { @"redeposited"!(r3) }
                                                }
                                              }
                                            }
                                          }
                                        }
                                      }
                                    }
                                  }
                                  _ => { Nil }
                                }
                              }
                            }
                          }
                        } |
                        // More than the purse holds, and each way of asking for it.
                        new oc in {
                          @am!("make", brand, 11, *oc) |
                          for (@(true, eleven) <- oc) {
                            new op in {
                              @purse!("withdraw", eleven, *op) |
                              for (@r <- op) { @"over-withdraw"!(r) }
                            }
                          }
                        } |
                        new np in {
                          @am!("make", brand, -1, *np) |
                          for (@r <- np) { @"negative-amount"!(r) }
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#])
        .await;

        let (brand, _, _) = kit(&rm, "kit").await;
        // The withdrawal answered a **payment** — a capability, not a number — and the payment is
        // what the brand and amount reads below go through.
        assert_capability(&value(&rm, "withdrawn").await, "withdraw's payment");
        assert_eq!(value(&rm, "withdrawn-brand").await, brand);
        assert_eq!(answered_number(&rm, "withdrawn-amount").await, 4);
        assert_eq!(
            answered_number(&rm, "balance-after-withdraw").await,
            6,
            "the purse must have been debited by the withdrawal"
        );
        assert_eq!(
            answered_number(&rm, "redeposited").await,
            10,
            "**a withdrawn payment is an ordinary payment**: the purse must take it back, which is \
             what makes `withdraw` a transfer rather than a sink"
        );
        assert_eq!(
            refused(&rm, "over-withdraw").await,
            "insufficient funds",
            "the ledger refuses more than the purse holds, and the purse reports it"
        );
        assert_eq!(
            refused(&rm, "negative-amount").await,
            "an amount is not negative",
            "a negative amount must be refused by `amountMath`, never passed to the ledger, whose \
             own parser fails the deploy instead of answering"
        );
    });
}

/// **W2.4: the same contract, reached the way a consumer reaches it.** On a chain that ran the
/// genesis ceremony, `lookup!(\`rho:rchain:ertp\`, *ch)` must resolve to the ERTP contract and it
/// must behave — so this test reruns the load-bearing clause through the *shorthand*.
///
/// **Why one script and not all six.** The blessing copies one registry entry onto a name; it cannot
/// change the value. That the copied value *is* this contract's is pinned mechanically
/// (`standard_deploys::tests::aliased_contract_uris_are_pinned` asserts the `rho:id` the key
/// derives), and the contract's semantics are W2.2's, so what remains to prove here is the one thing
/// neither of those can: that a fresh chain seeds the alias and that the value behind it answers a
/// real multi-step call. The acceptance clause is the script that exercises the most of the contract
/// per unit of ceremony.
///
/// **Two gates because they fail differently**: W2.2 fails when the contract is wrong, and this one
/// fails when the *blessing* is wrong — and the blessing's failure mode is the one this repository
/// keeps meeting: a deploy that reports success, registers nothing, and leaves `lookup!` answering
/// `Nil` for ever, which is silent.
#[test]
fn a_fresh_chain_serves_ertp_through_the_shorthand_and_the_brand_check_still_holds() {
    with_big_stack(async {
        let rm = play_blessed(&[r#"
new kitA, kitB in {
  @ERTP!("makeIssuerKit", *kitA) |
  @ERTP!("makeIssuerKit", *kitB) |
  for (@(brandA, mintA, issuerA) <- kitA & @(brandB, mintB, issuerB) <- kitB) {
    @"kitA"!((brandA, mintA, issuerA)) |
    new amCh in {
      @issuerA!("getAmountMath", *amCh) |
      for (@(true, am) <- amCh) {
        new mc in {
          @am!("make", brandA, 10, *mc) |
          for (@(true, ten) <- mc) {
            new mp in {
              @mintA!("mintPayment", ten, *mp) |
              for (@(true, paymentA) <- mp) {
                @"payment"!(paymentA) |
                new pbc in {
                  @issuerB!("makeEmptyPurse", *pbc) |
                  for (@(true, purseB) <- pbc) {
                    new d1 in {
                      @purseB!("deposit", paymentA, *d1) |
                      for (@r1 <- d1) {
                        @"b-deposit-a"!(r1) |
                        new pac in {
                          @issuerA!("makeEmptyPurse", *pac) |
                          for (@(true, purseA) <- pac) {
                            new d2 in {
                              @purseA!("deposit", paymentA, *d2) |
                              for (@r2 <- d2) { @"a-deposit-a"!(r2) }
                            }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#])
        .await;

        // The kit arrived whole through the shorthand — a 3-tuple of three distinct names.
        let (brand_a, mint_a, issuer_a) = kit(&rm, "kitA").await;
        assert_ne!(brand_a, mint_a);
        assert_ne!(brand_a, issuer_a);
        assert!(
            mint_a != issuer_a,
            "the blessed contract must be the kit-making contract"
        );
        assert_capability(&one(&rm, "payment").await, "the minted payment");
        assert_eq!(
            refused(&rm, "b-deposit-a").await,
            "no such payment",
            "the structural refusal must hold through the alias tier too"
        );
        assert_eq!(
            answered_number(&rm, "a-deposit-a").await,
            10,
            "**and the refused payment must still be live** — the acceptance clause, on a real chain"
        );
    });
}

// =================================================================================================
// REV: a standard brand whose issuer is native (W2.5)
// =================================================================================================

/// The key the REV drivers are signed by. Its REV address is the vault that funds them, so "who is
/// asking" (`revFund` spends the *caller's* vault) and "what the contract is" stay separable.
const REV_SEED: u8 = 22;

/// A payee address for a redemption. Any address string works: the vault map is keyed by the string,
/// and a redemption is a *payout*, not a proof of ownership.
const PAYEE: &str = "1111XuaDWqJtFmeR132nX6xY6rfMQrgNDjkmizUwDMRZzkvKmatY1";

/// The REV drivers' key, as a funded vault — what a chain must have seeded for them to fund from.
fn rev_deployer_vault() -> Vault {
    let sk = PrivateKey::new(vec![REV_SEED; 32]);
    let pk = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid secp256k1 key");
    Vault {
        rev_address: rchain_rholang::util::rev_address::RevAddress::from_public_key(&pk)
            .expect("the test key is a valid REV address"),
        initial_balance: NonNegI64::try_from(100).expect("a test balance"),
    }
}

/// Play the ERTP install deploy, then each REV driver (signed by [`REV_SEED`]) in order, against a
/// chain whose vault list is given. **One genesis for all of them**: the drivers are deploys on one
/// chain, so a later one sees what an earlier one did.
///
/// **These drivers cannot fund.** `compute_genesis` seeds the vault list *after* the deploy loop (it
/// is there for the next block's pre-charge), so a genesis deploy finds every vault empty — measured,
/// as the first version of the round-trip test failing with "transfer: insufficient balance". A
/// driver that spends from a vault therefore runs in a **block** ([`play_rev_block`]), which is what
/// it is on a real chain.
async fn play_rev(bodies: &[String], vaults: &[Vault]) -> (RuntimeManager, Vec<Option<String>>) {
    let mut terms = vec![deploy_signed_by(ERTP_RHO, ERTP_SEED)];
    terms.extend(bodies.iter().map(|b| driver_by(&ertp_uri(), b, REV_SEED)));
    let (rm, _, verdicts) = play_and_report(&terms, vaults).await;
    (rm, verdicts)
}

/// **The REV drivers that move REV**: the ERTP install and a funded vault through `compute_genesis`
/// (which seeds vaults at the *end* of the ceremony), then the drivers as the deploys of a **block**.
/// Returns the manager at that block's post-state, so the tags can be read.
///
/// The vault's owner is the deployer, so the funding deploy's own phlo charge lands on the same
/// account — which is why the assertions on the funder's balance are a *bound* while the reserve's
/// and the payee's are exact: those two are moved only by `revFund`/`revRedeem`, and phlo never
/// touches them.
async fn play_rev_block(bodies: &[String]) -> (RuntimeManager, Vec<Option<String>>) {
    let (rm, genesis_post, verdicts) = play_and_report(
        &[deploy_signed_by(ERTP_RHO, ERTP_SEED)],
        &[rev_deployer_vault()],
    )
    .await;
    for (i, verdict) in verdicts.iter().enumerate() {
        assert!(verdict.is_none(), "genesis deploy #{i} failed: {verdict:?}");
    }
    let deploys: Vec<SignedDeployData> = bodies
        .iter()
        .map(|b| {
            let term = format!(
                r#"new rl(`rho:registry:lookup`), ch in {{
                     rl!(`{uri}`, *ch) |
                     for (@(_, ERTP) <- ch) {{
                       {b}
                     }}
                   }}"#,
                uri = ertp_uri(),
            );
            // Free: the funder *is* the deployer, and a fee would move the same vault this flow is
            // measuring.
            deploy_signed_by_at(&term, REV_SEED, 0)
        })
        .collect();
    let (post, results, _system) = rm
        .compute_state(
            &genesis_post,
            &deploys,
            &[],
            &fixed_rand(),
            BlockData::empty(),
            &common::fringe_state(1),
        )
        .await
        .expect("compute_state");
    rm.runtime()
        .reset(post)
        .await
        .expect("the block's post-state is readable");
    let verdicts = results
        .iter()
        .map(|r| {
            if r.eval_result.succeeded() {
                None
            } else {
                Some(format!("{:?}", r.eval_result.errors))
            }
        })
        .collect();
    (rm, verdicts)
}

/// The vault balance at the REV reserve, read the way the node reads it.
async fn reserve_balance(rm: &RuntimeManager) -> i64 {
    vault_balance(rm, &rchain_rholang::system_processes::rev_reserve()).await
}

/// A vault balance at an address, read directly.
async fn vault_balance(rm: &RuntimeManager, address: &str) -> i64 {
    let native = rchain_rholang::native_state::NativeSystemState::new(rm.runtime().native_store());
    i64::from(
        native
            .vault_balance(address)
            .await
            .expect("a vault balance is readable")
            .unwrap_or_else(NonNegI64::zero),
    )
}

/// **REV is a standard brand with no mint arm, and the mint cannot be forged from Rholang.**
///
/// `getRevIssuer` replies a **2-tuple** — `(brand, issuer)`, not the kit's three — so the absence of
/// a mint is visible in the reply's *shape*: a caller that destructures a kit finds no third slot,
/// and a `match` on three elements can never fire. The issuer has no `mintPayment` arm either, which
/// is the same fact one level in.
///
/// The authority is a Rust constant (`system_processes.rs::rev_authority`) that is **never replied
/// on any channel**. It is safe to hardcode for one reason, and this test is where that reason is
/// measured rather than asserted: no Rholang term can construct a `GPrivate`, so the authority can
/// never be *presented*. A name-shaped forgery — any name the caller does hold — reaches the ledger
/// and is refused there, with a reply.
#[test]
fn rev_is_a_standard_brand_whose_mint_cannot_be_forged() {
    with_big_stack(async {
        let (rm, verdicts) = play_rev(
            &[r#"
new Ledger(`rho:rchain:ertp:ledger`), deployerId(`rho:rchain:deployerId`), rch in {
  @ERTP!("getRevIssuer", *rch) |
  for (@r <- rch) {
    // The reply's own shape is the first assertion.
    @"rev-reply"!(r) |
    match r {
      (revBrand, issuer) => {
        new bc, mch in {
          @issuer!("getBrand", *bc) |
          for (@b <- bc) { @"rev-issuer-brand"!(b) } |
          // There is no mint arm: this send is never answered.
          @issuer!("mintPayment", (revBrand, 1), *mch) |
          @"rev-mint-probe-sent"!(true) |
          for (@_ <- mch) { @"rev-issuer-answered-mint"!(true) } |
          // …and the ledger refuses any name but the authority.
          new thief, ret in {
            Ledger!("mint", revBrand, *thief, 1, *ret) |
            for (@mr <- ret) { @"rev-forged-mint"!(mr) }
          }
        }
      }
      _ => { Nil }
    }
  }
}
"#
            .to_string()],
            &[],
        )
        .await;
        for (i, verdict) in verdicts.iter().enumerate() {
            assert!(verdict.is_none(), "deploy #{i} failed: {verdict:?}");
        }

        let reply = one(&rm, "rev-reply").await;
        let parts = RhoTupleN::unapply(&reply).expect("`getRevIssuer` must reply a tuple");
        assert_eq!(
            parts.len(),
            2,
            "**REV is a 2-tuple: a brand and an issuer, with no mint** — a kit is three"
        );
        assert_capability(&parts[1], "the REV issuer");
        assert_eq!(
            value(&rm, "rev-issuer-brand").await,
            parts[0].clone(),
            "the issuer must know the brand it was built for"
        );
        assert!(
            boolean(&rm, "rev-mint-probe-sent").await,
            "the probe must have run, or the absence below proves nothing"
        );
        assert!(
            read(&rm, "rev-issuer-answered-mint").await.is_empty(),
            "the REV issuer must have no mint arm at all"
        );
        assert_eq!(
            refused(&rm, "rev-forged-mint").await,
            "only the name that registered the brand may mint it",
            "a forged authority must not mint REV"
        );
    });
}

/// **A forged authority that is not a name at all is refused too — by failing the deploy.**
///
/// One driver per spelling, because each refusal is a *deploy* refusal and would otherwise poison the
/// reporting deploy's tags. The message is the point: "an unforgeable name" is the gate that makes a
/// hardcoded authority safe, and it is a refusal — the deploy is not accepted and no state moves —
/// rather than a crash or, worse, a silent no-op.
#[test]
fn a_rev_mint_whose_authority_is_not_a_name_is_refused() {
    with_big_stack(async {
        // A byte array, a string that *names* the authority, `Nil`, and an empty list — four ways a
        // caller might try to spell a name it does not have.
        let spellings: &[(&str, &str)] = &[
            ("byte-array-shaped", "[1, 2, 3]"),
            ("string-shaped", "\"rchain:rev:authority\""),
            ("Nil", "Nil"),
            ("list-shaped", "[]"),
        ];
        let drivers: Vec<String> = spellings
            .iter()
            .map(|(_, spelling)| {
                format!(
                    r#"new Ledger(`rho:rchain:ertp:ledger`), bch, ret in {{
                         Ledger!("revBrand", *bch) |
                         for (@(true, revBrand) <- bch) {{
                           Ledger!("mint", revBrand, {spelling}, 1, *ret)
                         }}
                       }}"#
                )
            })
            .collect();
        let (_rm, verdicts) = play_rev(&drivers, &[]).await;
        for (i, (label, _)) in spellings.iter().enumerate() {
            let verdict = verdicts
                .get(i + 1)
                .unwrap_or_else(|| panic!("{label}: deploy #{i} must have a verdict"));
            let error = verdict.as_ref().unwrap_or_else(|| {
                panic!("{label}: a mint with a non-name authority must not be accepted")
            });
            assert!(
                error.contains("unforgeable name"),
                "{label}: the refusal must name its reason, got {error}"
            );
        }
    });
}

/// **REV round-trips through the vault boundary: funded in, redeemed out, and the reserve holds
/// exactly what the difference says.**
///
/// `revFund` moves REV from the *caller's own vault* (derived from `deployerId`, never from a
/// supplied address) into the reserve and then credits the purse; `revRedeem` pays a named address
/// out of the reserve and then debits the purse. Both orders are deliberate, and both are checked
/// here against the vault layer read directly: after 10 in and 4 out the funder is down 10, the payee
/// is up 4, and the reserve holds 6 — so a REV holding is **backed**, not conjured.
///
/// The withdrawal at the end moves 2 more out of the purse and is *not* a vault movement: it is an
/// ERTP-internal transfer, which is why the reserve stays at 6.
#[test]
fn rev_is_funded_from_a_vault_and_redeemed_back_to_one() {
    with_big_stack(async {
        let driver = format!(
            r#"
new Ledger(`rho:rchain:ertp:ledger`), deployerId(`rho:rchain:deployerId`), rch in {{
  @ERTP!("getRevIssuer", *rch) |
  for (@(revBrand, issuer) <- rch) {{
    new pc in {{
      @issuer!("makeEmptyPurse", *pc) |
      for (@(true, purse) <- pc) {{
        new cc in {{
          @purse!("getCurrentAmount", *cc) |
          for (@b <- cc) {{ @"rev-empty"!(b) }}
        }} |
        new fc in {{
          @purse!("revFund", *deployerId, 10, *fc) |
          for (@f <- fc) {{
            @"rev-funded"!(f) |
            new bc in {{
              @purse!("getCurrentAmount", *bc) |
              for (@b2 <- bc) {{ @"rev-balance"!(b2) }} |
              new rc in {{
                @purse!("revRedeem", 4, "{payee}", *rc) |
                for (@rd <- rc) {{
                  @"rev-redeemed"!(rd) |
                  new wc in {{
                    @purse!("withdraw", (revBrand, 2), *wc) |
                    for (@w <- wc) {{
                      @"rev-withdrawn"!(w) |
                      new ac in {{
                        @purse!("getCurrentAmount", *ac) |
                        for (@b3 <- ac) {{ @"rev-after"!(b3) }}
                      }}
                    }}
                  }}
                }}
              }}
            }}
          }}
        }}
      }}
    }}
  }}
}}
"#,
            payee = PAYEE,
        );
        let (rm, verdicts) = play_rev_block(&[driver]).await;
        for (i, verdict) in verdicts.iter().enumerate() {
            assert!(verdict.is_none(), "deploy #{i} failed: {verdict:?}");
        }

        assert_eq!(answered_number(&rm, "rev-empty").await, 0);
        assert_eq!(answered_number(&rm, "rev-funded").await, 10);
        assert_eq!(answered_number(&rm, "rev-balance").await, 10);
        assert_eq!(
            answered_number(&rm, "rev-redeemed").await,
            6,
            "redeeming 4 of 10 must leave 6"
        );
        // A REV purse's own `withdraw` works — it is the ledger's REV path, not the absent
        // authority — so a REV purse is not a roach motel.
        assert_capability(
            &value(&rm, "rev-withdrawn").await,
            "the withdrawn REV payment",
        );
        assert_eq!(answered_number(&rm, "rev-after").await, 4);

        // **The vault layer, read directly.** This is the identity that "backed" means.
        assert_eq!(
            vault_balance(&rm, &rev_deployer_vault().rev_address.to_base58()).await,
            90,
            "`revFund` must spend the caller's *own* vault"
        );
        assert_eq!(
            vault_balance(&rm, PAYEE).await,
            4,
            "`revRedeem` must pay the named address out of the reserve"
        );
        assert_eq!(
            reserve_balance(&rm).await,
            6,
            "the reserve must hold exactly what the holdings claim: 10 in, 4 out"
        );
    });
}

/// **The reserve cannot be spent by a deploy — measured, not asserted.**
///
/// The reserve is the REV address of an unforgeable name nobody can construct, so the vault
/// authority map has no entry for it and never can: `unforgeableAuthKey` requires *presenting* the
/// name. A `findOrCreate` over the reserve address is therefore a handle that can read and cannot
/// move — and this is the escrow's falsifier, because if a handle were enough to spend, the reserve
/// would be a donation to whoever asked first.
///
/// **The reserve is read natively here, not through the handle.** The handle's own `balance` arm is
/// unreachable in this port (AUDIT C219: a minted channel keeps one continuation, so
/// `install_vault_handle`'s second install replaces the first) — a defect this unit found and
/// registered rather than worked around. What the test therefore asserts is the *refusal* the
/// transfer arm reports and the vault state either side of it, which is the claim that matters:
/// a handle with no authority cannot move a lamport out of the reserve.
#[test]
fn a_vault_handle_over_the_reserve_cannot_move_anything() {
    with_big_stack(async {
        let fund = r#"
new Ledger(`rho:rchain:ertp:ledger`), deployerId(`rho:rchain:deployerId`), rch, fc in {
  @ERTP!("getRevIssuer", *rch) |
  for (@(revBrand, issuer) <- rch) {
    new pc in {
      @issuer!("makeEmptyPurse", *pc) |
      for (@(true, purse) <- pc) {
        @purse!("revFund", *deployerId, 10, *fc)
      }
    }
  }
}
"#
        .to_string();

        // The handle, and a transfer authorised by a name the caller *does* hold — a fresh one,
        // which resolves to no vault at all.
        let attempt = format!(
            r#"
new rv(`rho:rchain:revVault`), hch, tch, thief in {{
  rv!("findOrCreate", "{reserve}", *hch) |
  for (@fr <- hch) {{
    @"reserve-findOrCreate"!(fr) |
    match fr {{
      (true, *handle) => {{
        handle!("transfer", "{payee}", 10, *thief, *tch) |
        for (@t <- tch) {{ @"reserve-transfer"!(t) }}
      }}
      _ => {{ Nil }}
    }}
  }}
}}
"#,
            reserve = rchain_rholang::system_processes::rev_reserve(),
            payee = PAYEE,
        );

        let (rm, verdicts) = play_rev_block(&[fund, attempt]).await;
        for (i, verdict) in verdicts.iter().enumerate() {
            assert!(verdict.is_none(), "deploy #{i} failed: {verdict:?}");
        }
        assert_eq!(
            reserve_balance(&rm).await,
            10,
            "the funding must have landed"
        );

        // The handle exists and is addressable: `findOrCreate` replied **a name** — minted
        // natively, so an unforgeable name rather than the bundle an interpreted handle would be.
        let handle = value(&rm, "reserve-findOrCreate").await;
        assert!(
            RhoName::unapply(&handle).is_some(),
            "`findOrCreate` must reply a minted handle: {handle:?}"
        );
        // **The refusal is the evidence that the handler ran.** A silent send would leave the same
        // balance, so the reply is what separates "refused" from "reached nothing".
        assert_eq!(
            refused(&rm, "reserve-transfer").await,
            "Invalid AuthKey",
            "a handle over the reserve has no authority to spend from it"
        );
        assert_eq!(
            reserve_balance(&rm).await,
            10,
            "the reserve is unchanged: no deploy can authorize a name-derived vault it does not \
             hold the name of"
        );
        assert_eq!(vault_balance(&rm, PAYEE).await, 0, "nothing was paid out");
    });
}

/// **`revVault.deposit` stays refused, with its message unchanged.**
///
/// REV reaches ERTP through a *new* path (`revFund`/`revRedeem`/`revWithdraw`), so the old one must
/// be exactly as it was: an unauthenticated mint by another name is still an unauthenticated mint.
#[test]
fn rev_vault_deposit_is_still_refused_with_the_same_message() {
    with_big_stack(async {
        let (_rm, verdicts) = play_rev(
            &[
                r#"new rv(`rho:rchain:revVault`), ret in { rv!("deposit", 1000, *ret) }"#
                    .to_string(),
            ],
            &[],
        )
        .await;
        let error = verdicts[1]
            .as_ref()
            .unwrap_or_else(|| panic!("`revVault!(\"deposit\", …)` must still be refused"));
        assert!(
            error.contains("revVault: deposit is not callable"),
            "the refusal's message is part of the contract: {error}"
        );
    });
}
