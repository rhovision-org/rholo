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


def deploy_is_success(status):
    """The deploy-status body carries `ProcessedWithSuccess` (any other arm is not resolved)."""
    return isinstance(status, dict) and "ProcessedWithSuccess" in status


def health_verdict(body):
    """None iff the health body reports no fault, no orphan and no proposal halt."""
    if body.get("pipelineFaults") != 0:
        return "pipelineFaults is nonzero"
    if body.get("orphanBodies") != 0:
        return "orphanBodies is nonzero"
    if body.get("proposalHalted") is not False:
        return "proposalHalted is not false (or is missing)"
    return None


def proposed_after_restart(tail):
    """The victim's post-restart log tail carries a proposal."""
    return "proposed and added block" in tail
