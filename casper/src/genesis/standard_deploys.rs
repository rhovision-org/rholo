//! Standard (blessed) genesis deploys (port of `genesis/contracts/StandardDeploys.scala`).
//!
//! The `.rho`/`.rhox` contract sources are embedded via `include_str!`; `load_source`/`load_template`
//! reproduce `CompiledRholangSource.loadSource` / `CompiledRholangTemplate.loadTemplate` (the
//! `//Loaded from resource file <<path>>` comment + `$$name$$` macro substitution).

use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_crypto::signatures::signed::Signed;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_shared::base16;

use crate::genesis::contracts::{rev_generator_code, ProofOfStake, Registry, Vault};

// -------------------------------------------------------------------------------------------------
// Embedded contract sources
// -------------------------------------------------------------------------------------------------

const REGISTRY_RHO: &str = include_str!("resources/Registry.rho");
const LIST_OPS_RHO: &str = include_str!("resources/ListOps.rho");
const EITHER_RHO: &str = include_str!("resources/Either.rho");
const NON_NEGATIVE_NUMBER_RHO: &str = include_str!("resources/NonNegativeNumber.rho");
const MAKE_MINT_RHO: &str = include_str!("resources/MakeMint.rho");
const AUTH_KEY_RHO: &str = include_str!("resources/AuthKey.rho");
const REV_VAULT_RHO: &str = include_str!("resources/RevVault.rho");
const MULTI_SIG_REV_VAULT_RHO: &str = include_str!("resources/MultiSigRevVault.rho");
const ERTP_RHO: &str = include_str!("resources/ERTP.rho");
const POS_RHOX: &str = include_str!("resources/Pos.rhox");

// -------------------------------------------------------------------------------------------------
// Fixed blessed-contract keys + timestamps
// -------------------------------------------------------------------------------------------------

const REGISTRY_PK: &str = "5a0bde2f5857124b1379c78535b07a278e3b9cefbcacc02e62ab3294c02765a1";
const LIST_OPS_PK: &str = "867c21c6a3245865444d80e49cac08a1c11e23b35965b566bbe9f49bb9897511";
const EITHER_PK: &str = "5248f8913f8572d8227a3c7787b54bd8263389f7209adc1422e36bb2beb160dc";
const NON_NEGATIVE_NUMBER_PK: &str =
    "e33c9f1e925819d04733db4ec8539a84507c9e9abd32822059349449fe03997d";
const MAKE_MINT_PK: &str = "de19d53f28d4cdee74bad062342d8486a90a652055f3de4b2efa5eb2fccc9d53";
const AUTH_KEY_PK: &str = "f450b26bac63e5dd9343cd46f5fae1986d367a893cd21eedd98a4cb3ac699abc";
const REV_VAULT_PK: &str = "27e5718bf55dd673cc09f13c2bcf12ed7949b178aef5dcb6cd492ad422d05e9d";
const MULTI_SIG_REV_VAULT_PK: &str =
    "2a2eaa76d6fea9f502629e32b0f8eea19b9de8e2188ec0d589fcafa98fb1f031";
const POS_GENERATOR_PK: &str = "a9585a0687761139ab3587a4938fb5ab9fcba675c79fefba889859674046d4a5";
const REV_GENERATOR_PK: &str = "a06959868e39bb3a8502846686a23119716ecd001700baf9e2ecfa0dbf1a3247";

/// The ERTP contract's key (issue #249). Unlike the nine above, the key is **our own choice**, so it
/// is *derived* from a named string rather than pasted from a vendored source's header:
/// `blake2b256("rnode/genesis/ertp")`, the `rgov::contract_key` idiom. Anyone reading this file can
/// recompute it and check that nobody picked it adversarially —
/// `the_ertp_key_is_derived_from_a_named_string` does exactly that recomputation.
///
/// Its `rho:id` is `rho:id:75fy4nja3qsq958b5k3rmi3sz1g653fa5c1xfkwdx9awpfwybauo`, the constant a
/// consumer may hardcode and the key the genesis alias seed copies onto `rho:rchain:ertp`.
const ERTP_PK: &str = "fe8fa828b930aec3651cacae457b46b248c554e2933fa05ce9763e23c4354066";

const REGISTRY_TIMESTAMP: i64 = 1559156071321;
const LIST_OPS_TIMESTAMP: i64 = 1559156082324;
const EITHER_TIMESTAMP: i64 = 1559156217509;
const NON_NEGATIVE_NUMBER_TIMESTAMP: i64 = 1559156251792;
const MAKE_MINT_TIMESTAMP: i64 = 1559156452968;
const AUTH_KEY_TIMESTAMP: i64 = 1559156356769;
const REV_VAULT_TIMESTAMP: i64 = 1559156183943;
const MULTI_SIG_REV_VAULT_TIMESTAMP: i64 = 1571408470880;
/// The ERTP contract's deploy timestamp. A constant, for the same reason the vendored set's are:
/// genesis deploys carry no real time, and a moving value would move the deploy.
const ERTP_TIMESTAMP: i64 = 1_700_000_001_000;
const POS_GENERATOR_TIMESTAMP: i64 = 1559156420651;
// `revGenerator` has no fixed timestamp — it is batched with `1565818101792 + idx`.

/// `accounting.MAX_VALUE` (`Integer.MAX_VALUE`).
const MAX_VALUE: i64 = i32::MAX as i64;

// -------------------------------------------------------------------------------------------------
// Source/template loading
// -------------------------------------------------------------------------------------------------

/// Append the resource-comment (port of `CompiledRholangSource.loadSource`).
fn load_source(classpath: &str, content: &str) -> String {
    format!("{content}\n//Loaded from resource file <<{classpath}>>\n")
}

/// Substitute `$$name$$` macros then append the resource-comment (port of
/// `CompiledRholangTemplate.loadTemplate`).
fn load_template(classpath: &str, content: &str, macros: &[(&str, &str)]) -> String {
    let mut final_content = content.to_string();
    for (name, value) in macros {
        final_content = final_content.replace(&format!("$${name}$$"), value);
    }
    format!("{final_content}\n //Loaded from resource file <<{classpath}>>\n")
}

fn to_public(private_key_hex: &str) -> Result<PublicKey, String> {
    let private_key = PrivateKey::new(base16::unsafe_decode(private_key_hex));
    Secp256k1.to_public(&private_key).map_err(|e| e.to_string())
}

/// The `rho:id` a blessed contract registers itself under.
///
/// `rho:registry:insertSigned:secp256k1` derives the URI from the **deployer public key** it is
/// called with (`rholang/src/system_processes.rs::registry_insert_signed` →
/// `registry::build_uri(&blake2b256(pub_key))`), never from the deploy signature or a timestamp.
/// Every blessed deploy's key is a fixed constant above, so its URI is a **chain-independent
/// constant**: the value a consumer may hardcode, and the key the genesis alias seed copies onto the
/// shorthand (`spec/GENESIS.md`).
///
/// Note it is this port's own zbase32 encoding (`rholang/src/registry.rs`), not the 54-char
/// mainnet values in the `.rho` header comments — those come from the Scala's CRC14+ZBase32 bit
/// order, which the port deliberately does not reproduce.
pub fn contract_uri(private_key_hex: &str) -> Result<String, String> {
    let public_key = to_public(private_key_hex)?;
    Ok(rchain_rholang::registry::build_uri(
        &rchain_crypto::hash::blake2b256::hash(public_key.bytes()),
    ))
}

// -------------------------------------------------------------------------------------------------
// The genesis registry manifest
// -------------------------------------------------------------------------------------------------

/// What provides the value a shorthand resolves to.
pub enum GenesisAliasSource {
    /// An arity-1 native system channel (`rholang/src/system_processes.rs::definitions`). Only the
    /// *registry alias* is missing on a fresh chain; the value is built by
    /// `system_processes::system_channel_alias` as `(nonce, bundle+{channel})`.
    NativeChannel,
    /// An interpreted contract installed by the blessed deploy built from this fixed key. The genesis
    /// seed copies the entry the deploy registered under [`contract_uri`] onto the shorthand.
    Contract { private_key_hex: &'static str },
}

/// One registry entry a fresh chain seeds, with the consumer that justifies it. Genesis content is
/// consensus identity, so every entry is here for a named reason — `spec/GENESIS.md` carries the
/// full manifest, including what is deliberately *not* installed.
pub struct GenesisAlias {
    /// The name consumers look up, e.g. `rho:rchain:revVault`.
    pub shorthand: &'static str,
    pub source: GenesisAliasSource,
    /// The consumer-side evidence (file:line in the wallet / rgov checkouts) for this entry.
    pub consumer: &'static str,
}

/// The shorthands a fresh chain's registry resolves. Minimal by design: each is reached by
/// `rho:registry:lookup` from a real consumer, or is a dependency of one that is.
pub const GENESIS_ALIASES: &[GenesisAlias] = &[
    GenesisAlias {
        shorthand: "rho:rchain:revVault",
        source: GenesisAliasSource::NativeChannel,
        consumer:
            "wallet + rgov: r-wallet/src/utils/rho.ts:7,18; rgov src/actions/transfer.rho:4, \
                   checkBalance.rho:10 — `lookup!` then `@(_, RevVault)` then the vault methods",
    },
    GenesisAlias {
        shorthand: "rho:rchain:pos",
        source: GenesisAliasSource::NativeChannel,
        consumer: "wallet bonding: r-wallet/src/utils/rho.ts:29 — `lookup!` then `@(_, PoS)` then \
                   `PoS!(\"bond\", …)`",
    },
    GenesisAlias {
        shorthand: "rho:rchain:makeMint",
        source: GenesisAliasSource::Contract {
            private_key_hex: MAKE_MINT_PK,
        },
        consumer: "rgov src/actions/makeMint.rho:13 + wallet snippets.ts:751 — `lookup!` then \
                   `@(nonce, *MakeMint)` then `MakeMint!(*ch)`",
    },
    GenesisAlias {
        shorthand: "rho:rchain:authKey",
        source: GenesisAliasSource::Contract {
            private_key_hex: AUTH_KEY_PK,
        },
        consumer: "`MultiSigRevVault.rho:35` looks it up before it can install, and every \
                   `deployerAuthKey` is made through it — without this alias the multi-signature \
                   vault answers `Nil` to its own dependency",
    },
    GenesisAlias {
        shorthand: "rho:rchain:multiSigRevVault",
        source: GenesisAliasSource::Contract {
            private_key_hex: MULTI_SIG_REV_VAULT_PK,
        },
        consumer: "multi-signature custody: `lookup!` then `@(_, MultiSigRevVault)` then its \
                   `create` / `makeSealerUnsealer` / `deployerAuthKey` methods",
    },
    GenesisAlias {
        shorthand: "rho:lang:listOps",
        source: GenesisAliasSource::Contract {
            private_key_hex: LIST_OPS_PK,
        },
        consumer: "rgov rholang/core/CrowdFund.rho:8 — `lookup!` then `@(_, *ListOps)` then \
                   `ListOps!(\"fold\", …)`",
    },
    GenesisAlias {
        shorthand: "rho:lang:nonNegativeNumber",
        source: GenesisAliasSource::Contract {
            private_key_hex: NON_NEGATIVE_NUMBER_PK,
        },
        consumer:
            "no direct consumer; it is the dependency `MakeMint.rho:27` looks up before it can \
                   install, so it is aliased for makeMint to work at all",
    },
    GenesisAlias {
        // **The shorthand is `rho:rchain:ertp`, and the native ledger keeps the longer
        // `rho:rchain:ertp:ledger`** so the two cannot collide: the alias tier holds the *object
        // API* (`makeIssuerKit`), while the native channel is the consensus ledger the API is
        // written over. A consumer wants the former. Naming the alias after the ledger would make
        // `lookup!(\`rho:rchain:ertp\`)` return the raw ledger — a change of meaning under an
        // unchanged name, which is the silent downgrade AUDIT C114 exists to stop.
        shorthand: "rho:rchain:ertp",
        source: GenesisAliasSource::Contract {
            private_key_hex: ERTP_PK,
        },
        consumer: "the ERTP object API itself (issue #249, the Agoric request): `lookup!` then \
                   `@(_, ERTP)` then `ERTP!(\"makeIssuerKit\", *ch)`; without the alias a client \
                   must hardcode a `rho:id` derived from a key it cannot see",
    },
];

/// The registry value for a native system channel shorthand: `(nonce, bundle+{channel})`.
pub fn native_channel_alias(shorthand: &str) -> Option<rchain_models::ast::Par> {
    rchain_rholang::system_processes::system_channel_alias(shorthand)
}

// -------------------------------------------------------------------------------------------------
// The adapted MakeMint epilogue
// -------------------------------------------------------------------------------------------------

/// `MakeMint.rho` with its registry epilogue adapted for this port.
///
/// The blessed source ends by asking `rho:registry:systemContractManager` for a write-only
/// dispatcher and by defining a `securityCheck` arm that calls `rho:rchain:configPublicKeyCheck`.
/// **Neither channel exists in this port** — both were provided by the interpreted `Registry.rho`,
/// which is not installed (`spec/GENESIS.md`) — so the `for` that waits on them never fires, the
/// deploy registers nothing, and `lookup!(\`rho:rchain:makeMint\`, *ch)` answers `Nil` forever.
///
/// The adaptation registers the contract's own bundle and drops the unused `securityCheck` arm. That
/// is behaviourally identical on the consumer path — `lookup!` returns `(nonce, bundle+{MakeMint})`
/// and the caller invokes it, which is exactly what `MakeMint!(*ch)` in the wallet and rgov does —
/// and neither checkout calls `securityCheck` (`spec/GENESIS.md` records the evidence).
///
/// Each replacement asserts its marker, so a drift in the vendored source fails the genesis build
/// loudly instead of silently shipping an epilogue that cannot register.
fn make_mint_source() -> Result<String, String> {
    let mut source = MAKE_MINT_RHO.to_string();

    // 1. The `new` binding list: the three names only existed to hold the channels this port does not
    //    have. Dropped so the adapted source carries no reference to them at all.
    let bindings = "  deployerId(`rho:rchain:deployerId`),\n  systemContractManagerCh,\n  \
                    dispatcherCh,\n  configPublicKeyCheckCh\nin {";
    let patched_bindings = "  deployerId(`rho:rchain:deployerId`)\nin {";
    if !source.contains(bindings) {
        return Err(
            "MakeMint.rho: the `new` binding marker is missing from the vendored source".into(),
        );
    }
    source = source.replace(bindings, patched_bindings);

    // 2. The two lookups for the absent channels: without them the lookups would answer `Nil` onto
    //    channels nothing reads (stray datums in the genesis state), and the gate below could never
    //    fire anyway.
    let lookups = "  rl!(`rho:registry:systemContractManager`, *systemContractManagerCh)|\n  \
                   rl!(`rho:rchain:configPublicKeyCheck`, *configPublicKeyCheckCh)|\n  ";
    if !source.contains(lookups) {
        return Err("MakeMint.rho: the lookup markers are missing from the vendored source".into());
    }
    source = source.replace(lookups, "  ");

    // 3. The install gate: wait only for the dependency this port actually has.
    let gate = "for(@(_, NonNegativeNumber) <- NonNegativeNumberCh & @(_, systemContractManager) \
                <- systemContractManagerCh& @(_, configPublicKeyCheck)<- configPublicKeyCheckCh) {";
    let patched_gate = "for(@(_, NonNegativeNumber) <- NonNegativeNumberCh) {";
    if !source.contains(gate) {
        return Err(
            "MakeMint.rho: the install-gate marker is missing from the vendored source".into(),
        );
    }
    source = source.replace(gate, patched_gate);

    // 4. The registration: discharge the dispatcher request and register the contract's own bundle.
    let epilogue = "@systemContractManager!(\"createDispatcher\", *MakeMint, *dispatcherCh)|\n    \
                    contract @(*MakeMint, \"securityCheck\")(@deployerId, ret) = {\n      \
                    @configPublicKeyCheck!(deployerId, *ret)\n    } |\n    \
                    for (makeMintdispatcher <- dispatcherCh){\n      // Inserts signed write-only \
                    MakeMint dispatcher contract into the registry\n      rs!(\n        \
                    (9223372036854775807, bundle+{*makeMintdispatcher}),\n        *deployerId,\n        \
                    *uriOut\n      )\n    }";
    let patched_epilogue = "// Adapted (see `make_mint_source`): no systemContractManager in this \
                            port; register the contract's own bundle.\n    \
                            rs!(\n      (9223372036854775807, bundle+{*MakeMint}),\n      \
                            *deployerId,\n      *uriOut\n    )";
    if !source.contains(epilogue) {
        return Err(
            "MakeMint.rho: the registry-epilogue marker is missing from the vendored source".into(),
        );
    }
    source = source.replace(epilogue, patched_epilogue);

    Ok(load_source("MakeMint.rho", &source))
}

/// `AuthKey.rho` and `MultiSigRevVault.rho` with the registry epilogue adapted, the way
/// [`make_mint_source`] adapts `MakeMint.rho`.
///
/// Both sources end by asking `rho:registry:systemContractManager` for a write-only dispatcher and
/// defining a `securityCheck` arm that calls `rho:rchain:configPublicKeyCheck` — **neither channel
/// exists in this port** (they came from the interpreted `Registry.rho`, which is not installed;
/// `spec/GENESIS.md` records the evidence). Left alone, the `for` that waits on them never fires, so
/// the deploy registers nothing and `lookup!` of the contract's urn answers `Nil` forever. The
/// adaptation registers the contract's **own bundle** instead, which is exactly what a consumer
/// destructures (`for (@(_, AuthKey) <- ch)` and then `@AuthKey!(…)`).
///
/// **The tail is cut at a marker rather than a fifteen-line block being string-replaced**, which is
/// the difference from `make_mint_source`: the two epilogues differ in their dispatcher names and in
/// how they reach the manager (`rl!` and a gate here, a direct `@systemContractManager!` call there),
/// and reproducing either verbatim in Rust is how a vendored source's edit goes unnoticed. The marker
/// is asserted, so a source that moves fails the genesis build loudly.
///
/// The head keeps whatever the contract did before the epilogue — `MultiSigRevVault`'s
/// `extractState` arm, for instance — because that is contract behaviour rather than registry
/// plumbing.
fn registry_epilogue_adapted(
    name: &str,
    source: &str,
    marker: &str,
    gate: Option<(&str, &str)>,
    contract: &str,
) -> Result<String, String> {
    let Some(at) = source.find(marker) else {
        return Err(format!(
            "{name}: the registry-epilogue marker is missing from the vendored source, so the \
             adaptation would ship a contract that cannot register"
        ));
    };
    let head = source.split_at(at).0.trim_end();
    // **The absent channels have to go before the gate, not only at the tail.** `AuthKey` reaches the
    // manager from a block at its *end*, so cutting the tail removes the whole thing. This vault
    // reaches it from inside the gate that wraps its contracts — so the gate itself waits on two
    // channels that never answer, the body (registration included) never runs, and the alias seed
    // finds nothing to copy. Measured: that is how this adaptation's first version failed
    // (`rho:rchain:multiSigRevVault must be seeded`).
    let lookups = "  rl!(`rho:registry:systemContractManager`, *systemContractManagerCh)|\n  \
                   rl!(`rho:rchain:configPublicKeyCheck`, *configPublicKeyCheckCh)|\n";
    if !source.contains(lookups) {
        return Err(format!(
            "{name}: the lookup markers are missing from the vendored source"
        ));
    }
    let head = head.replace(lookups, "");
    let head = match gate {
        Some((from, to)) => {
            if !head.contains(from) {
                return Err(format!(
                    "{name}: the install-gate marker is missing from the vendored source"
                ));
            }
            head.replace(from, to)
        }
        None => head,
    };
    // The head ends with the `|` that introduced the epilogue, and the epilogue is what completed
    // that par — so the replacement sits on the other side of the same pipe. Stripping it leaves
    // `} rs!(…)`, which the parser refuses (measured: `expected RBrace, got Ident("rs")`), and that
    // is the failure mode these markers exist to catch.
    if !head.ends_with('|') {
        return Err(format!(
            "{name}: the text before the registry-epilogue marker does not end a par arm, so the \
             adaptation would splice two processes together"
        ));
    }
    // **Close as many braces as the head left open**, rather than assuming one. The two sources
    // differ in exactly this: `MultiSigRevVault`'s contracts sit inside the gate's `for (...) {`, so
    // its head has one more open brace than `AuthKey`'s, and a fixed `}` truncated the par and the
    // parser refused the file (`expected RBrace, got Eof`). The count is over the whole head, so it
    // also notices a source that gains a block above the epilogue.
    // **Close as many braces as the head left open.** The two sources differ in exactly this:
    // `MultiSigRevVault`'s contracts sit inside the gate's `for (...) {`, so its head has one open
    // brace more than `AuthKey`'s — and a fixed `}` truncated the par, which the parser refused
    // (`expected RBrace, got Eof`). Counted with `checked_sub`, so a head that balances (or closes
    // more than it opens) is a refusal naming the marker rather than an arithmetic surprise.
    let Some(open) = head
        .matches('{')
        .count()
        .checked_sub(head.matches('}').count())
    else {
        return Err(format!(
            "{name}: the text before the registry-epilogue marker balances its braces, so there is \
             no block for the epilogue to be the last arm of — the marker found the wrong place"
        ));
    };
    if open == 0 {
        return Err(format!(
            "{name}: the text before the registry-epilogue marker opens no block, so the \
             adaptation would append a process outside every `new`"
        ));
    }
    let closing = "}".repeat(open);
    Ok(format!(
        "{head}\n  rs!(\n    (9223372036854775807, bundle+{{*{contract}}}),\n    *deployerId,\n    \
         *uriOut\n  )\n{closing}\n"
    ))
}

/// `AuthKey.rho`, adapted (see [`registry_epilogue_adapted`]).
fn auth_key_source() -> Result<String, String> {
    let source = AUTH_KEY_RHO.to_string();
    let marker = "  rl!(`rho:registry:systemContractManager`, *systemContractManagerCh)|";
    registry_epilogue_adapted("AuthKey.rho", &source, marker, None, "AuthKey")
}

/// `MultiSigRevVault.rho`, adapted (see [`registry_epilogue_adapted`]).
fn multi_sig_rev_vault_source() -> Result<String, String> {
    let source = MULTI_SIG_REV_VAULT_RHO.to_string();
    let marker =
        "    @systemContractManager!(\"createDispatcher\", *MultiSigRevVault, *dispatcherCh)|";
    // Its gate waits on the two channels this port does not have; the rest of it (`listOps`,
    // `authKey`, `revVault`) is real, so the two conjuncts come out and the gate stays.
    let gate = (
        " &\n      @(_, systemContractManager) <- systemContractManagerCh &\n      \
         @(_, configPublicKeyCheck)<- configPublicKeyCheckCh) {",
        ") {",
    );
    registry_epilogue_adapted(
        "MultiSigRevVault.rho",
        &source,
        marker,
        Some(gate),
        "MultiSigRevVault",
    )
}

// -------------------------------------------------------------------------------------------------
// Standard deploys
// -------------------------------------------------------------------------------------------------

/// The standard (blessed) genesis deploys (port of `StandardDeploys`).
pub struct StandardDeploys;

impl StandardDeploys {
    /// Build + sign a standard deploy (port of `StandardDeploys.toDeploy`).
    fn to_deploy(
        term: String,
        private_key_hex: &str,
        timestamp: i64,
        shard_id: &str,
    ) -> Result<SignedDeployData, String> {
        let sk = PrivateKey::new(base16::unsafe_decode(private_key_hex));
        let data = DeployData {
            attachments: Vec::new(),
            term,
            timestamp,
            phlo_price: 0,
            phlo_limit: MAX_VALUE,
            valid_after_block_number: 0,
            shard_id: shard_id.to_string(),
        };
        let signed = Signed::new(data, &Secp256k1, &sk).map_err(|e| e.to_string())?;
        Ok(SignedDeployData {
            data: signed.data,
            deployer: signed.pk.bytes().to_vec(),
            sig: signed.sig,
            sig_algorithm: signed.sig_algorithm.name().to_string(),
        })
    }

    /// The public keys of the standard contracts, in deploy order (port of `systemPublicKeys`).
    pub fn system_public_keys() -> Result<Vec<PublicKey>, String> {
        Ok(vec![
            to_public(REGISTRY_PK)?,
            to_public(LIST_OPS_PK)?,
            to_public(EITHER_PK)?,
            to_public(NON_NEGATIVE_NUMBER_PK)?,
            to_public(MAKE_MINT_PK)?,
            to_public(AUTH_KEY_PK)?,
            to_public(REV_VAULT_PK)?,
            to_public(MULTI_SIG_REV_VAULT_PK)?,
            to_public(ERTP_PK)?,
            to_public(POS_GENERATOR_PK)?,
            to_public(REV_GENERATOR_PK)?,
        ])
    }

    pub fn registry_generator(
        registry: &Registry,
        shard_id: &str,
    ) -> Result<SignedDeployData, String> {
        let term = load_template(
            "Registry.rho",
            REGISTRY_RHO,
            &[(
                "systemContractPubKey",
                registry.system_contract_pub_key.as_str(),
            )],
        );
        Self::to_deploy(term, REGISTRY_PK, REGISTRY_TIMESTAMP, shard_id)
    }

    pub fn list_ops(shard_id: &str) -> Result<SignedDeployData, String> {
        Self::to_deploy(
            load_source("ListOps.rho", LIST_OPS_RHO),
            LIST_OPS_PK,
            LIST_OPS_TIMESTAMP,
            shard_id,
        )
    }

    pub fn either(shard_id: &str) -> Result<SignedDeployData, String> {
        Self::to_deploy(
            load_source("Either.rho", EITHER_RHO),
            EITHER_PK,
            EITHER_TIMESTAMP,
            shard_id,
        )
    }

    pub fn non_negative_number(shard_id: &str) -> Result<SignedDeployData, String> {
        Self::to_deploy(
            load_source("NonNegativeNumber.rho", NON_NEGATIVE_NUMBER_RHO),
            NON_NEGATIVE_NUMBER_PK,
            NON_NEGATIVE_NUMBER_TIMESTAMP,
            shard_id,
        )
    }

    pub fn make_mint(shard_id: &str) -> Result<SignedDeployData, String> {
        Self::to_deploy(
            make_mint_source()?,
            MAKE_MINT_PK,
            MAKE_MINT_TIMESTAMP,
            shard_id,
        )
    }

    /// The `AuthKey` contract, with its registry epilogue adapted
    /// ([`registry_epilogue_adapted`]). Without the adaptation the deploy registers nothing, and the
    /// multi-signature vault's `deployerAuthKey` — which is built through this contract — answers
    /// `Nil` forever.
    pub fn auth_key(shard_id: &str) -> Result<SignedDeployData, String> {
        Self::to_deploy(
            auth_key_source()?,
            AUTH_KEY_PK,
            AUTH_KEY_TIMESTAMP,
            shard_id,
        )
    }

    pub fn rev_vault(shard_id: &str) -> Result<SignedDeployData, String> {
        Self::to_deploy(
            load_source("RevVault.rho", REV_VAULT_RHO),
            REV_VAULT_PK,
            REV_VAULT_TIMESTAMP,
            shard_id,
        )
    }

    /// The multi-signature REV vault, adapted like `AuthKey` — and **installed** rather than
    /// refused. AUDIT C114's channel used to answer with the single-signer handler, was changed to
    /// refuse (because that was single-key custody under a multi-signature name), and is now answered
    /// by this contract.
    pub fn multi_sig_rev_vault(shard_id: &str) -> Result<SignedDeployData, String> {
        Self::to_deploy(
            multi_sig_rev_vault_source()?,
            MULTI_SIG_REV_VAULT_PK,
            MULTI_SIG_REV_VAULT_TIMESTAMP,
            shard_id,
        )
    }

    /// The ERTP contract (issue #249): the object API over the native issuer ledger. Its epilogue
    /// needs no adaptation — unlike `MakeMint`/`AuthKey`/`MultiSigRevVault` it reaches no channel
    /// this port does not have; it registers its own bundle with the plain `rs!`.
    pub fn ertp(shard_id: &str) -> Result<SignedDeployData, String> {
        Self::to_deploy(
            load_source("ERTP.rho", ERTP_RHO),
            ERTP_PK,
            ERTP_TIMESTAMP,
            shard_id,
        )
    }

    pub fn pos_generator(pos: &ProofOfStake, shard_id: &str) -> Result<SignedDeployData, String> {
        let minimum_bond = i64::from(pos.minimum_bond).to_string();
        let maximum_bond = i64::from(pos.maximum_bond).to_string();
        let initial_bonds = ProofOfStake::initial_bonds(&pos.validators);
        let epoch_length = pos.epoch_length.to_string();
        let quarantine_length = pos.quarantine_length.to_string();
        let number_of_active_validators = pos.number_of_active_validators.to_string();
        let pos_multi_sig_public_keys = ProofOfStake::public_keys(&pos.pos_multi_sig_public_keys);
        let pos_multi_sig_quorum = pos.pos_multi_sig_quorum.to_string();
        let pos_vault_pub_key = pos.pos_vault_pub_key.clone();

        let macros: &[(&str, &str)] = &[
            ("minimumBond", minimum_bond.as_str()),
            ("maximumBond", maximum_bond.as_str()),
            ("initialBonds", initial_bonds.as_str()),
            ("epochLength", epoch_length.as_str()),
            ("quarantineLength", quarantine_length.as_str()),
            (
                "numberOfActiveValidators",
                number_of_active_validators.as_str(),
            ),
            ("posMultiSigPublicKeys", pos_multi_sig_public_keys.as_str()),
            ("posMultiSigQuorum", pos_multi_sig_quorum.as_str()),
            ("posVaultPubKey", pos_vault_pub_key.as_str()),
        ];
        let term = load_template("Pos.rhox", POS_RHOX, macros);
        Self::to_deploy(term, POS_GENERATOR_PK, POS_GENERATOR_TIMESTAMP, shard_id)
    }

    pub fn rev_generator(
        vaults: &[Vault],
        timestamp: i64,
        is_last_batch: bool,
        shard_id: &str,
    ) -> Result<SignedDeployData, String> {
        let term = rev_generator_code(vaults, is_last_batch);
        Self::to_deploy(term, REV_GENERATOR_PK, timestamp, shard_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_source_appends_comment() {
        let code = load_source("Foo.rho", "new x in { Nil }");
        assert!(code.starts_with("new x in { Nil }"));
        assert!(code.contains("//Loaded from resource file <<Foo.rho>>"));
    }

    #[test]
    fn load_template_substitutes_and_appends_comment() {
        let code = load_template("T.rhox", "contract( $$name$$ )", &[("name", "42")]);
        assert!(code.contains("contract( 42 )"));
        assert!(code.contains(" //Loaded from resource file <<T.rhox>>"));
    }

    #[test]
    fn system_public_keys_has_eleven_entries() {
        assert_eq!(StandardDeploys::system_public_keys().unwrap().len(), 11);
    }

    /// **The ERTP key is derived, not chosen** — recomputed here from its named string, so a
    /// constant that drifted from the documented derivation (or a derivation that was quietly
    /// changed to move the `rho:id`) fails rather than ships.
    #[test]
    fn the_ertp_key_is_derived_from_a_named_string() {
        let derived = rchain_shared::base16::encode(&rchain_crypto::hash::blake2b256::hash(
            b"rnode/genesis/ertp",
        ));
        assert_eq!(
            derived, ERTP_PK,
            "ERTP_PK must be blake2b256(\"rnode/genesis/ertp\") — the provenance a reader checks"
        );
    }

    /// Each alias entry is seedable: a native channel resolves to a value, a contract alias resolves
    /// to a `rho:id` — and the URI is a pure function of the fixed key, so two calls agree (the
    /// property that lets a consumer hardcode it, and the genesis alias seed recompute it).
    #[test]
    fn every_genesis_alias_has_a_source() {
        for alias in GENESIS_ALIASES {
            match alias.source {
                GenesisAliasSource::NativeChannel => {
                    let value = native_channel_alias(alias.shorthand).unwrap_or_else(|| {
                        panic!(
                            "{} names a native channel with no alias value",
                            alias.shorthand
                        )
                    });
                    assert!(
                        !value.exprs.is_empty(),
                        "{} must resolve to a `(nonce, bundle)` value",
                        alias.shorthand
                    );
                }
                GenesisAliasSource::Contract { private_key_hex } => {
                    let uri = contract_uri(private_key_hex).expect("the fixed key derives a URI");
                    assert!(uri.starts_with("rho:id:"), "{uri}");
                    assert_eq!(uri.len(), "rho:id:".len() + 52, "{uri}");
                    assert_eq!(uri, contract_uri(private_key_hex).unwrap(), "deterministic");
                }
            }
            assert!(
                !alias.consumer.is_empty(),
                "{} must record the consumer that justifies it",
                alias.shorthand
            );
        }
    }

    /// The `rho:id` of every *aliased* blessed contract, pinned. These are the constants a consumer
    /// may hardcode (`spec/GENESIS.md`); they are this port's own zbase32 encoding, so they are not
    /// the 54-char mainnet ids in the `.rho` header comments. A change here is a genesis change.
    #[test]
    fn aliased_contract_uris_are_pinned() {
        // The `rho:id` each seeded shorthand resolves to, as `spec/GENESIS.md` publishes them. These
        // are consensus-visible: a consumer hardcodes them, so a change here is a genesis change and
        // must be recorded in the manifest — which is why they are asserted, not printed.
        let expected: &[(&str, &str)] = &[
            (
                "rho:rchain:makeMint",
                "rho:id:asysrwfgzf8bf7sxkiowp4b3tcsy4f8ombi3w96ysox4u3qdmn1o",
            ),
            (
                "rho:lang:listOps",
                "rho:id:6fzorimqngeedepkrizgiqms6zjt76zjeciktt1eifequy4osz3o",
            ),
            (
                "rho:lang:nonNegativeNumber",
                "rho:id:hxyadh1ffypra47ry9mk6b8r1i33ar1w9wjsez4khfe9huzrfcyo",
            ),
            (
                "rho:rchain:ertp",
                "rho:id:75fy4nja3qsq958b5k3rmi3sz1g653fa5c1xfkwdx9awpfwybauo",
            ),
        ];
        for (shorthand, uri) in expected {
            let alias = GENESIS_ALIASES
                .iter()
                .find(|a| a.shorthand == *shorthand)
                .unwrap_or_else(|| panic!("{shorthand} must be in the manifest"));
            let GenesisAliasSource::Contract { private_key_hex } = alias.source else {
                panic!("{shorthand} must be an installed contract");
            };
            assert_eq!(
                contract_uri(private_key_hex).unwrap(),
                *uri,
                "{shorthand}: its URI is consensus-visible and published in spec/GENESIS.md"
            );
        }
    }

    /// The MakeMint adaptation is applied, and applied *loudly*: the term must no longer contain the
    /// epilogue that waits on channels this port does not have, and must carry the direct
    /// registration instead.
    #[test]
    fn the_make_mint_epilogue_is_adapted() {
        let term = make_mint_source().expect("the vendored source still has both markers");
        assert!(
            !term.contains("createDispatcher"),
            "the unadapted dispatcher request must be gone"
        );
        // Asserted against the *use* forms, not the bare names: the adaptation's own comment names
        // the channels it removed, and a comment cannot resolve anything.
        for gone in [
            "createDispatcher",
            "@systemContractManager!(",
            "rl!(`rho:registry:systemContractManager`",
            "configPublicKeyCheck!(",
            "rl!(`rho:rchain:configPublicKeyCheck`",
            "systemContractManagerCh",
            "configPublicKeyCheckCh",
        ] {
            assert!(
                !term.contains(gone),
                "the adapted term must not contain {gone:?}"
            );
        }
        assert!(
            term.contains("(9223372036854775807, bundle+{*MakeMint})"),
            "the contract registers its own bundle"
        );
        // …and the adapted term still parses and normalizes: the patch edits text, so this is what
        // catches an edit that leaves the source unbalanced. On a worker thread with the 32 MiB
        // stack the runtime gives genesis deploys — the blessed terms recurse past the 2 MiB default.
        let normalized = std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || rchain_rholang::normalizer::source_to_adt(&term).is_ok())
            .expect("spawn")
            .join()
            .expect("join");
        assert!(
            normalized,
            "the adapted MakeMint term must parse and normalize"
        );
    }

    /// A drift in the vendored `MakeMint.rho` must fail the genesis *build*, not silently ship an
    /// epilogue that cannot register. Asserted by patching a copy of the source with the marker
    /// removed.
    #[test]
    fn a_drifted_make_mint_source_is_an_error() {
        // The markers are matched exactly, so a source edit that moves them is caught here.
        let source = MAKE_MINT_RHO;
        // Every marker `make_mint_source` replaces, asserted independently: a source that loses one
        // of them must fail here (and in the genesis build) rather than ship unpatched.
        for marker in [
            "@systemContractManager!(\"createDispatcher\"",
            "for(@(_, NonNegativeNumber) <- NonNegativeNumberCh & @(_, systemContractManager)",
            "rl!(`rho:registry:systemContractManager`, *systemContractManagerCh)|",
            "systemContractManagerCh,\n  dispatcherCh,\n  configPublicKeyCheckCh\nin {",
        ] {
            assert!(
                source.contains(marker),
                "vendored MakeMint.rho lost the marker {marker:?}"
            );
        }
    }
}

/// The standard-contract builders, asserted as a table: each must produce a **signed** deploy for
/// the requested shard, carrying a non-empty term — the essence of "the genesis deploy set is
/// complete and shard-scoped" (Law 26).
#[cfg(test)]
mod builder_tests {
    use super::*;

    use crate::genesis::contracts::{ProofOfStake, Validator};

    /// Assert the shared shape of a standard deploy: the term is non-empty, the shard id is the one
    /// asked for, it is signed by the contract's own key (which is what `system_public_keys`
    /// advertises), the phlo limit is the maximum (a genesis deploy must not run out) and the phlo
    /// price is zero.
    fn assert_standard(deploy: &SignedDeployData, shard_id: &str) {
        assert!(
            !deploy.data.term.is_empty(),
            "a standard deploy must carry a term"
        );
        assert_eq!(deploy.data.shard_id, shard_id);
        assert_eq!(deploy.data.phlo_price, 0, "genesis deploys are free");
        assert_eq!(deploy.data.phlo_limit, MAX_VALUE, "…and unbounded in phlo");
        assert!(!deploy.sig.is_empty(), "the deploy is signed");
        assert_eq!(deploy.sig_algorithm, "secp256k1");
        assert_eq!(deploy.deployer.len(), 65, "an uncompressed public key");
        assert!(
            crate::construct_deploy::source_deploy(
                &deploy.data.term,
                deploy.data.timestamp,
                deploy.data.phlo_limit,
                deploy.data.phlo_price,
                &PrivateKey::new(vec![1u8; 32]),
                deploy.data.valid_after_block_number,
                &deploy.data.shard_id,
            )
            .is_ok(),
            "the term is well-formed enough to sign"
        );
    }

    /// Every parameterless builder (the nine standard contracts) produces a standard deploy.
    #[test]
    fn every_standard_contract_builder_produces_a_signed_deploy() {
        let builders: Vec<(&str, SignedDeployData)> = vec![
            (
                "registry",
                StandardDeploys::registry_generator(
                    &Registry {
                        system_contract_pub_key: "aa".repeat(65),
                    },
                    "/root",
                )
                .expect("registry"),
            ),
            (
                "list_ops",
                StandardDeploys::list_ops("/root").expect("list_ops"),
            ),
            ("either", StandardDeploys::either("/root").expect("either")),
            (
                "non_negative_number",
                StandardDeploys::non_negative_number("/root").expect("non_negative_number"),
            ),
            (
                "make_mint",
                StandardDeploys::make_mint("/root").expect("make_mint"),
            ),
            (
                "auth_key",
                StandardDeploys::auth_key("/root").expect("auth_key"),
            ),
            (
                "rev_vault",
                StandardDeploys::rev_vault("/root").expect("rev_vault"),
            ),
            (
                "multi_sig_rev_vault",
                StandardDeploys::multi_sig_rev_vault("/root").expect("multi_sig_rev_vault"),
            ),
            ("ertp", StandardDeploys::ertp("/root").expect("ertp")),
        ];
        // Each builder signs with a *different* key, so a copy-paste that reused one key would show
        // up as a repeated deployer.
        let mut deployers: Vec<Vec<u8>> = Vec::new();
        for (name, deploy) in &builders {
            assert_standard(deploy, "/root");
            assert!(
                !deployers.contains(&deploy.deployer),
                "{name} reuses another contract's key"
            );
            deployers.push(deploy.deployer.clone());
        }
        assert_eq!(builders.len(), 9);
    }

    /// The two parameterised generators: `pos_generator` substitutes the PoS parameters into the
    /// `Pos.rhox` template (so the term must contain them), and `rev_generator` renders the vault
    /// list — both scoped to the requested shard.
    use rchain_shared::refined::NonNegI64;

    #[test]
    fn the_pos_and_rev_generators_substitute_their_parameters() {
        let pos = ProofOfStake {
            minimum_bond: NonNegI64::try_from(3).unwrap(),
            maximum_bond: NonNegI64::try_from(100).unwrap(),
            validators: vec![Validator {
                pk: PublicKey::new(rchain_shared::base16::unsafe_decode(&"ab".repeat(65))),
                stake: rchain_shared::refined::NonNegI64::try_from(42).expect("non-negative"),
            }],
            epoch_length: 10,
            quarantine_length: 5,
            number_of_active_validators: 7,
            executor_share: NonNegI64::try_from(0).unwrap(),
            absence_slack: NonNegI64::try_from(0).unwrap(),
            participation_grace: NonNegI64::try_from(0).unwrap(),
            min_phlo_price: rchain_shared::refined::NonNegI64::zero(),
            max_number_of_parents: 0,
            pos_multi_sig_public_keys: Vec::new(),
            pos_multi_sig_quorum: 1,
            pos_vault_pub_key: "cd".repeat(65),
            dev_tax_vesting_blocks: NonNegI64::zero(),

            liveness_absence_threshold: 0,
            liveness_jail_after_boundaries: 0,
            liveness_jail_epochs: 0,
            liveness_vote_window: 0,
            liveness_slash_severity: 0,
        };
        let deploy = StandardDeploys::pos_generator(&pos, "/root/child").expect("pos_generator");
        assert_standard(&deploy, "/root/child");
        for expected in ["10", "5", "7"] {
            assert!(
                deploy.data.term.contains(expected),
                "the template must carry the PoS parameter {expected}"
            );
        }

        // The vault generator takes the vault list, the timestamp it is given and the
        // last-batch flag; an empty list is a legal call (a shard with no initial vaults).
        let deploy =
            StandardDeploys::rev_generator(&[], 1234, false, "/root").expect("rev_generator");
        assert_standard(&deploy, "/root");
        assert_eq!(deploy.data.timestamp, 1234, "the timestamp is the caller's");
        let closing =
            StandardDeploys::rev_generator(&[], 1234, true, "/root").expect("rev_generator");
        assert_ne!(
            deploy.data.term, closing.data.term,
            "the last batch renders differently from an intermediate one"
        );
    }
}
