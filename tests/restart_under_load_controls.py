"""The restart-under-load arm's predicates, tested offline.

A green arm is only evidence if its predicate fails on a bad sample. These are the checks the arm
uses; `test_restart_under_load_controls.py` drives each with a failing sample, so a predicate that
stopped discriminating would fail its own test rather than pass a bad run.
"""


def agreement_verdict(identities, min_height):
    """None iff every validator reports the same finalized `(height, hash)` at or above the floor."""
    if not identities:
        return "no validators"
    heights = [height for height, _ in identities]
    if min(heights) < min_height:
        return f"finalized height below {min_height}"
    if len(set(identities)) != 1:
        return "validators report different finalized identities"
    return None


def baseline_verdict(identities):
    """None iff the pre-kill samples agree with each other on a non-negative height."""
    if not identities:
        return "no validators"
    if any(height < 0 for height, _ in identities):
        return "negative finalized height"
    if len(set(identities)) != 1:
        return "the pre-kill baseline is not agreed"
    return None


def certificate_verdict(checkpoint, finalised_height):
    """None iff the restarted victim holds a verified certificate covering the pre-kill height.

    A validator with a populated DAG and no local certificate cannot attribute its state, and the
    launch refuses; a certificate whose anchor predates the killed finalised height does not cover
    the state the node accepted either.
    """
    if not isinstance(checkpoint, dict):
        return "no checkpoint status after restart"
    if checkpoint.get("kind") != "checkpoint" or not checkpoint.get("digest"):
        return "no verified checkpoint certificate after restart"
    anchor = checkpoint.get("anchorHeight")
    if type(anchor) is not int:
        return "the checkpoint names no anchor height"
    if anchor < finalised_height:
        return f"the certificate anchor {anchor} is below the killed height {finalised_height}"
    return None


def deploy_is_success(status):
    """The deploy-status body carries `ProcessedWithSuccess` (any other arm is not resolved)."""
    return isinstance(status, dict) and "ProcessedWithSuccess" in status


def health_verdict(body):
    """None iff the health body reports no outstanding fault, no orphan and no proposal halt.

    The gate reads `pipelineFaultsOutstanding` (the obligation) rather than the
    lifetime `pipelineFaults` counter: a settled fault is history, not a block.
    """
    if body.get("pipelineFaultsOutstanding") != 0:
        return "pipelineFaultsOutstanding is nonzero"
    if body.get("orphanBodies") != 0:
        return "orphanBodies is nonzero"
    if body.get("proposalHalted") is not False:
        return "proposalHalted is not false (or is missing)"
    return None


def proposed_after_restart(tail):
    """The victim's post-restart log tail carries a proposal."""
    return "proposed and added block" in tail


CAPACITY = 1024


def receiver_verdict(body, allow_full=False):
    """None iff `/health` carries a coherent `blockPipeline` reading below the cap.

    `pending` is `BlockReceiverState.blocks_st.len()` — the queue the 1024 cap is on — not the
    validated mpsc queue the stage observers measure. The reading is advisory; this predicate only
    makes the arm fail when a node reports the queue at (or past) capacity, which is the refusal the
    arm is testing for. A missing or malformed reading is *unknown*, never zero.
    """
    pipeline = body.get("blockPipeline") if isinstance(body, dict) else None
    if not isinstance(pipeline, dict):
        return "blockPipeline is missing"
    capacity = pipeline.get("capacity")
    pending = pipeline.get("pending")
    peak = pipeline.get("sampledPeak")
    if capacity != CAPACITY:
        return f"capacity is {capacity!r}, not {CAPACITY}"
    for name, value in (("pending", pending), ("sampledPeak", peak)):
        if type(value) is not int or value < 0 or value > CAPACITY:
            return f"{name} is not an integer in [0, {CAPACITY}]: {value!r}"
    if peak < pending:
        return "sampledPeak is below pending"
    if not allow_full and pending >= CAPACITY:
        return "the block-receiver queue is at capacity"
    return None


def wrong_genesis_verdict(exit_code, log, entered_running):
    """None iff a wrongly-pinned node refused to start and never entered Running.

    The node can refuse for two reasons, both named in the log: a restored genesis that differs from
    the pin, or a pin with no height-zero genesis to compare against. Either is a visible refusal;
    a zero exit, silence, or a Running transition is not.
    """
    if entered_running:
        return "the node entered Running despite the wrong genesis pin"
    if exit_code == 0:
        return "the node exited zero with the wrong genesis pin"
    named = (
        "differs from pinned genesis" in log
        or "expected genesis hash is pinned but the restored DAG has no height-zero genesis" in log
        or "expected one genesis at height zero" in log
    )
    if not named:
        return "the log does not name the pinned-genesis refusal"
    fatal = (
        "refusing to enter Running" in log
        or "Fatal: NodeLaunch exited with error" in log
        or "stopping rather than serving a chain it never synced" in log
    )
    if not fatal:
        return "the log does not name a fatal Running refusal"
    return None


def incomplete_ancestry_verdict(exit_code, log, entered_running):
    """None iff a node asked for an ancestry no peer can serve and refused to enter Running."""
    if entered_running:
        return "the node entered Running without the requested ancestry"
    if exit_code == 0:
        return "the node exited zero with an unavailable sync anchor"
    if "supplied no authenticated nonempty fringe" not in log:
        return "the log does not name the missing-fringe refusal"
    if "refusing to enter Running" not in log:
        return "the log does not name the Running refusal"
    return None


def interrupted_restart_verdict(boot_ids, expect, ready_error=None):
    """None iff the interrupted-restart sequence is what the control expects.

    `boot_ids` are the victim's distinct process identities observed in order: the baseline, the
    first start, and — for `recovered` — the second start. `detected` requires the bounded readiness
    wait to have raised while the victim was down.
    """
    if not boot_ids:
        return "no bootId was observed"
    if len(set(boot_ids)) < 2:
        return "the victim's process identity did not change"
    if expect == "recovered":
        if len(set(boot_ids)) < 3:
            return "the second start did not produce a third process identity"
        if ready_error is not None:
            return f"the victim did not recover: {ready_error}"
        return None
    if expect == "detected":
        if ready_error is None:
            return "the interrupted restart was not detected as a readiness failure"
        return None
    return f"unknown expectation {expect!r}"


LIVENESS_WINDOW = 5


def catchup_verdict(body):
    """None iff a health body's catch-up coordinates are internally consistent.

    While `nodePhase` is `catching-up`, `catchUp` must name the node's validated tip (`own`), the
    observed peer tip (`peerTip`) and the quorum-supported height (`supported`), and the view must
    be genuinely stale — a catching-up phase whose coordinates are current is a phase that cannot
    say why it is not serving. A `peerTip` of `-1` means no peer has answered the probe yet (a
    joined boot before its first probe); it is unknown, not current, and the consistency test is
    skipped until one arrives.
    """
    if not isinstance(body, dict):
        return "the health body is not an object"
    if body.get("nodePhase") != "catching-up":
        return None
    catchup = body.get("catchUp")
    if not isinstance(catchup, dict):
        return "catching-up without a catchUp coordinate"
    for name in ("own", "peerTip", "supported"):
        if name not in catchup:
            return f"the catchUp coordinate is missing {name}"
        if type(catchup[name]) is not int:
            return f"catchUp.{name} is not an integer"
    own = catchup["own"]
    peer_tip = catchup["peerTip"]
    if peer_tip < 0:
        return None
    if peer_tip - own <= LIVENESS_WINDOW:
        return (
            f"catching-up with a current view (own {own}, peerTip {peer_tip}): the phase must "
            "name a stale view or serve"
        )
    return None


def catchup_progress_verdict(samples):
    """None iff a victim that entered catch-up progressed and reached the window.

    `samples` are `{phase, own, peer_height}` taken after the restart. A victim that was ever
    `catching-up` must have a non-decreasing validated tip, and if the samples end while it is
    still catching up, its last tip must be within the window of the highest survivor height seen.
    A victim that reached `running` passes: the phase's own proof is the window. A victim that
    never entered catch-up passes — an ordinary restart at the tip has nothing to fetch.
    """
    caught_up = [s for s in samples if s.get("phase") == "catching-up"]
    if not caught_up:
        return None
    owns = [s.get("own", -1) for s in caught_up]
    if owns != sorted(owns):
        return f"the victim's validated tip regressed during catch-up: {owns}"
    if samples and samples[-1].get("phase") != "catching-up":
        return None
    peer_height = max((s.get("peer_height", -1) for s in samples), default=-1)
    if peer_height < 0:
        return "no survivor height was sampled"
    own = caught_up[-1].get("own", -1)
    if own < 0:
        return "the catch-up sample named no validated tip"
    if peer_height - own > LIVENESS_WINDOW:
        return (
            f"the victim did not reach the window: own {own} against survivors {peer_height}"
        )
    return None
