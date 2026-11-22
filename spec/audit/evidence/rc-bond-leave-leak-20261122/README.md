# The bond-leave campaign with the inactivity leak — blocked at the joiner

**Run:** `tools/bond-leave-campaign.py --keep --out target/bond-leave-campaign/leak-run3-20261122`,
2026-11-22. Image `rnode:local` built from this branch; `result.json` records the exact
`binary_sha256` and `image_id` (`source_sha` is the last commit, `1cae048af`, with the leak
uncommitted at build time).

**What the run measured.** The campaign arms the liveness mechanism (the leak's window is
`liveness-jail-after-boundaries`) and reaches the join stage. The newcomer's bond finalises, but the
joining validator never catches up: it restores the bootstrap's checkpoint, then repeatedly faults
validating a block at the anchor's height:

```
Block #11 0d1337ef… faulted at validation internal
(validateBlockCheckpoint failed: active-bonds leaf not found in state)
```

(5 attempts over 10 minutes; `joiner-fault.log`.) The joiner's own validated tip stays at 11 while
the peer tip advances, so `wait_synced` never passes and the campaign stops before the leave. No
`leaked` line and no `clocks frozen` line appear: **the leak itself was never reached**, because the
campaign never gets a synced joining validator.

**Why this is not the leak's code path.** The failure is `RuntimeManager::compute_bonds` reading
`pos:active` from a state the restored store does not hold, inside
`get_pre_state_for_parents`'s bonds read (`casper/src/multi_parent_casper.rs`). This branch adds no
read and no removal there: the leak is a boundary transition in `close_block` (writing
`pos:silence` and, on the window, calling the existing `slash`) and a derived `CloseBlock` input. The
state the joiner opens is the pre-state of an anchor-neighbour block, which the checkpoint restore
does not ship. The failure reproduces the shape recorded for #139/C270 (a restored DAG whose blocks
carry no fringe, so the certified-base guard falls through to a state read), and is the subject of a
separate sync investigation; it is recorded here so the leak's live arm is not claimed on a run that
could not exercise it.

**What is claimed instead.** The instrument's correctness is carried by the unit and play/replay
tests named in the PR (`a_validator_that_does_not_cover_the_partition_is_leaked_after_the_window`,
`a_boundary_that_covers_the_partition_clears_the_silence_counter`,
`a_boundarys_layer_missing_reaches_play_and_replay_alike`, `the_leaks_input_is_the_gates_not_full_set`,
`the_proposer_attaches_the_list_the_replay_will_read`) and by Law 129. The live arm remains owed
until the joiner syncs.
