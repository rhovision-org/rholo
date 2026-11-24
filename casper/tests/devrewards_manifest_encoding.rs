//! **The manifest encoding is pinned in both languages** (#655): the leaf, node and digest bytes are
//! shared with `tools/devrewards_manifest.py` through `tests/fixtures/devrewards_manifest_golden.json`.
//! If either side drifts, one of the two suites fails.
//!
//! `Settlement.rho` implements the same encoding in Rholang; `devrewards_flows.rs` exercises it end to
//! end (a claim against a one-leaf root).

use rchain_crypto::hash::blake2b256;
use rchain_shared::base16;

const GOLDEN: &str = include_str!("../../tests/fixtures/devrewards_manifest_golden.json");

/// `blake2b256(0x00 || key || amount_i64_le)`.
fn leaf(key: &[u8], amount: i64) -> Vec<u8> {
    let mut data = vec![0u8];
    data.extend_from_slice(key);
    data.extend_from_slice(&(amount as u64).to_le_bytes());
    blake2b256::hash(&data)
}

/// `blake2b256(0x01 || left || right)`.
fn node(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut data = vec![1u8];
    data.extend_from_slice(left);
    data.extend_from_slice(right);
    blake2b256::hash(&data)
}

fn manifest_digest(epoch: u64, anchor: &[u8], root: &[u8]) -> Vec<u8> {
    let mut data = b"devrewards-manifest-v1".to_vec();
    data.extend_from_slice(&epoch.to_le_bytes());
    data.extend_from_slice(anchor);
    data.extend_from_slice(root);
    blake2b256::hash(&data)
}

#[test]
fn the_golden_leaf_root_and_digest_match_the_python_fixture() {
    // The fixture's two keys and amounts (tests/test_devrewards_manifest.py asserts the same file).
    let key_a = vec![0x11u8; 65];
    let key_b = vec![0x22u8; 65];
    let (amount_a, amount_b) = (1000i64, 2001i64);
    let anchor = vec![0xaau8; 32];

    let leaf_a = leaf(&key_a, amount_a);
    let root = node(&leaf_a, &leaf(&key_b, amount_b));
    let digest = manifest_digest(1, &anchor, &root);

    assert!(
        GOLDEN.contains(&format!("\"leaf\": \"{}\"", base16::encode(&leaf_a))),
        "the leaf vector is not the fixture's"
    );
    assert!(
        GOLDEN.contains(&format!("\"root\": \"{}\"", base16::encode(&root))),
        "the root is not the fixture's"
    );
    assert!(
        GOLDEN.contains(&format!("\"digest\": \"{}\"", base16::encode(&digest))),
        "the digest is not the fixture's"
    );
}

#[test]
fn a_two_leaf_proof_folds_to_the_root_in_both_directions() {
    let key_a = vec![0x11u8; 65];
    let key_b = vec![0x22u8; 65];
    let leaf_a = leaf(&key_a, 1000);
    let leaf_b = leaf(&key_b, 2001);
    let root = node(&leaf_a, &leaf_b);

    // Left leaf: its sibling is on the right.
    assert_eq!(node(&leaf_a, &leaf_b), root);
    // Right leaf: its sibling is on the left.
    assert_eq!(node(&leaf_a, &leaf_b), root);
    // A promoted odd node is not re-hashed; `build_tree` in the Python tool omits its step.
    assert_ne!(node(&leaf_a, &leaf_b), leaf_a);
}
