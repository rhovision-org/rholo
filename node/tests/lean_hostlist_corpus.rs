//! Law 90's conformance corpus: the same ten cases the Lean `servedIds` was decided against, run
//! through the Rust `derive_hostlist`.
//!
//! The corpus row carries the ids the derivation must produce, so a drift on either side — the
//! filter, the ordering, the dedupe, the bound, or self's presence — fails the tie. The scenarios
//! are named rather than encoded because the *shape* is the contract: two small constructors per
//! side, and the expected answer lives once, in `spec/conformance/hostlist.tsv`.
//!
//! **The id abstraction.** The Lean model's node ids are `Nat`; the Rust's are fixed-length hex
//! strings ordered lexicographically. For equal-length hex ids the two orders coincide, which is
//! the property the model relies on; the cases below use one-character ids, where they also agree.

use rchain_node::web::node_list::{derive_hostlist, OwnView, PeerView, ProbeOutcome, StatusAnswer};

fn peer(id: &str) -> PeerView {
    PeerView {
        node_id: id.to_string(),
        host: "h".to_string(),
        protocol_port: 40400,
    }
}

fn own(self_id: &str, self_proposer: bool) -> OwnView {
    OwnView {
        peer: peer(self_id),
        http_port: 40403,
        network_id: "testnet".to_string(),
        shard_ids: vec!["/root".to_string()],
        primary_shard: "/root".to_string(),
        latest_block_number: 0,
        node_version: "v".to_string(),
        proposer: self_proposer,
        accepts_deploys: true,
    }
}

fn answer(id: &str, network: &str, shard: &str, proposer: bool) -> ProbeOutcome {
    ProbeOutcome {
        peer: peer(id),
        answer: Some(StatusAnswer {
            address: id.to_string(),
            network_id: network.to_string(),
            shard_id: shard.to_string(),
            proposer,
            accepts_deploys: true,
            latest_block_number: 0,
            node_version: "v".to_string(),
        }),
    }
}

/// The Rust half of each named case in `Rchain/Corpus.lean`'s `hostListCases`. A name with no arm is
/// a corpus case the Rust does not run — the tie would be one-sided, so it panics.
fn scenario(name: &str) -> (OwnView, Vec<ProbeOutcome>, usize) {
    match name {
        "self_only" => (own("0", true), vec![], 8),
        "observer_excluded" => (
            own("0", true),
            vec![answer("1", "testnet", "/root", false)],
            8,
        ),
        "wrong_network" => (
            own("0", true),
            vec![answer("1", "othernet", "/root", true)],
            8,
        ),
        "wrong_shard" => (
            own("0", true),
            vec![answer("1", "testnet", "/child", true)],
            8,
        ),
        "unreachable" => (
            own("0", true),
            vec![ProbeOutcome {
                peer: peer("1"),
                answer: None,
            }],
            8,
        ),
        "address_mismatch" => {
            let mut o = answer("1", "testnet", "/root", true);
            o.answer.as_mut().expect("an answer").address = "2".to_string();
            (own("0", true), vec![o], 8)
        }
        "three_valid_sorted" => (
            own("0", true),
            vec![
                answer("5", "testnet", "/root", true),
                answer("3", "testnet", "/root", true),
                answer("1", "testnet", "/root", true),
            ],
            8,
        ),
        "truncation_self_first" => (
            own("0", true),
            vec![
                answer("1", "testnet", "/root", true),
                answer("2", "testnet", "/root", true),
                answer("3", "testnet", "/root", true),
            ],
            2,
        ),
        "duplicate_ids" => (
            own("0", true),
            vec![
                answer("1", "testnet", "/root", true),
                answer("1", "testnet", "/root", true),
            ],
            8,
        ),
        "self_not_a_proposer" => (
            own("0", false),
            vec![answer("7", "testnet", "/root", true)],
            8,
        ),
        other => panic!("no Rust arm for the Lean hostlist case {other:?}"),
    }
}

#[test]
fn the_hostlist_derivation_is_the_lean_models() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../spec/conformance/hostlist.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e}\n(run tools/emit-lean-corpus.sh)",
            path.display()
        )
    });

    let mut seen = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            3,
            "a hostlist row is layer/name/expected: {line:?}"
        );
        assert_eq!(cols[0], "hostlist", "{line:?}");
        let name = cols[1];
        let expected: Vec<String> = if cols[2] == "-" {
            Vec::new()
        } else {
            cols[2].split(',').map(ToString::to_string).collect()
        };

        let (own, outcomes, max_entries) = scenario(name);
        let got: Vec<String> = derive_hostlist(&own, &outcomes, max_entries)
            .into_iter()
            .map(|e| e.node_id)
            .collect();
        assert_eq!(got, expected, "case {name}");

        // No row may pass vacuously: an expected `self` is only meaningful if self could propose.
        if expected.iter().any(|id| id == "0") {
            assert!(
                own.proposer,
                "case {name} expects self but self cannot propose"
            );
        }
        seen += 1;
    }

    assert_eq!(seen, 10, "the corpus carries all ten cases");
}
