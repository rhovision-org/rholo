#!/usr/bin/env bash
set -euo pipefail
cargo test --release --locked -p rchain-block-storage --lib
cargo test --release --locked -p rchain-sdk --lib consensus
cargo test --release --locked -p rchain-casper --lib blocks::proposer
cargo test --release --locked -p rchain-casper --lib block_disposition
cargo test --release --locked -p rchain-casper --lib blocks::block_processor
cargo test --release --locked -p rchain-casper --lib blocks::block_receiver
cargo test --release --locked -p rchain-casper --lib engine::node_phase
cargo test --release --locked -p rchain-rholang --lib liveness_tests
cargo test --release --locked -p rchain-node --lib web::node_list
cargo test --release --locked -p rchain-node --test lean_hostlist_corpus
cargo test --release --locked -p rchain-casper --test lean_recovery_corpus
cargo test --release --locked -p rchain-casper --test finalization
# Counterfactual control inside this disposable test container only.
source_file=casper/src/blocks/proposer/proposer.rs
backup=$(mktemp)
cp "$source_file" "$backup"
trap 'cp "$backup" "$source_file"; rm -f "$backup"' EXIT
sed -i '/^fn take_round_escape(/,/^}/s/    \*since.lock().unpoison() = Some(now);/    \/\/ Negative control: leave the consumed permit open./' "$source_file"
set +e
cargo test --release --locked -p rchain-casper --lib round_escape_permit_is_consumed_even_when_the_round_stays_open > /tmp/round-escape-negative.log 2>&1
result=$?
set -e
cat /tmp/round-escape-negative.log
test "$result" -eq 101
grep -F 'test blocks::proposer::proposer::tests::round_escape_permit_is_consumed_even_when_the_round_stays_open ... FAILED' /tmp/round-escape-negative.log
echo 'PASS: removing permit consumption is detected by the scheduling regression'
