//! **Live archive-challenge checker.**
//!
//! A challenge answer is evidence only if the client checks it. This test asks an archive for a
//! nonce-derived sample and reconstructs every returned `RawBlock`, so the body is re-hashed with
//! the node's own `hash_block` and the derived hash must equal the claimed `blockHash` at the
//! claimed height. A Python client cannot recompute that hash; the checker therefore lives here and
//! reuses the audit's `reconstruct_block`.
//!
//! It is environment-gated: `RNODE_CHALLENGE_URL` names the archive's client API, and the optional
//! `RNODE_CHALLENGE_NONCE` / `RNODE_CHALLENGE_COUNT` / `RNODE_CHALLENGE_MAX_HEIGHT` set the sample.
//! Without the URL the test reports that it was skipped and passes, so ordinary CI does not need a
//! running archive.
//!
//! The tamper half is the audit's unit test
//! (`runtime::audit::tests::a_raw_block_reconstructs_and_a_tampered_preimage_is_refused`): a
//! corrupted body is refused by the same `reconstruct_block` this checker calls.

use rchain_node::api::dto::HistoryChallenge;
use rchain_node::runtime::audit::reconstruct_block;

#[tokio::test]
async fn an_archive_challenge_answers_with_bodies_that_hash_to_their_claims() {
    let Ok(base) = std::env::var("RNODE_CHALLENGE_URL") else {
        eprintln!("history_challenge: RNODE_CHALLENGE_URL is not set; skipping the live check");
        return;
    };
    let nonce = std::env::var("RNODE_CHALLENGE_NONCE")
        .unwrap_or_else(|_| "rnode-challenge-evidence".to_string());
    let count = std::env::var("RNODE_CHALLENGE_COUNT").unwrap_or_else(|_| "64".to_string());
    let max_height =
        std::env::var("RNODE_CHALLENGE_MAX_HEIGHT").unwrap_or_else(|_| "0".to_string());
    let url = format!(
        "{}/api/v1/history/challenge?nonce={nonce}&count={count}&max_height={max_height}",
        base.trim_end_matches('/')
    );

    let challenge: HistoryChallenge = reqwest::get(&url)
        .await
        .expect("the archive answered the challenge request")
        .json()
        .await
        .expect("the challenge response is a HistoryChallenge");
    assert!(
        challenge.complete,
        "an archive that advertises the range must hold every sampled body"
    );
    assert!(
        !challenge.entries.is_empty(),
        "a challenge with no entries proves nothing"
    );

    for entry in &challenge.entries {
        let body = entry
            .body
            .as_ref()
            .expect("a complete challenge carries every sampled body");
        let block = reconstruct_block(body).expect("the returned body reconstructs and re-hashes");
        assert_eq!(
            entry.block_hash.as_deref(),
            Some(block.block_hash.to_hex().as_str()),
            "height {}: the body hashes to another block",
            entry.height
        );
        assert_eq!(
            i64::from(block.block_number),
            entry.height,
            "the body's height is not the challenged height"
        );
    }
}
