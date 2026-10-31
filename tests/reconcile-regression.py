#!/usr/bin/env python3
"""Offline full-script reconciliation regressions with deterministic API fixtures.

Each case slices one section out of `tools/reconcile-network.sh` by its own banner and runs it with the
API surface stubbed, so the assertion is about the *section's* arithmetic rather than about a live net.
The sections are located by exact banner text, so a banner that moves breaks a case here rather than
passing silently — the anchors are listed at the end of the file."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

SCRIPT = Path(__file__).resolve().parents[1] / "tools/reconcile-network.sh"

# The slice a §3 case drives: the meet's helpers *and* the section, so the real parsing, the real
# refusals and the real arithmetic run — only the two artefact calls are stubbed. The stubs are spliced
# in *between* the helpers and the section, because a shell script is read and executed incrementally: a
# definition at the end of the file has not been read when a call near the top runs.
MEET_HELPERS = ("# --- the meet's artefacts", "# --- 3. the anchor")
MEET_SECTION = ("# --- 3. the anchor", "# --- 4. what is above")

STUB_OVERRIDES = """attestation_of() { printf '%s' "${STUB_ATT[$4]}"; }
verified_block() { printf '%s' "${STUB_BLOCK[$3]}"; }
is_ancestor_of() { [ "$2" = "${STUB_BAD_CAND:-}" ] && return 1; return 0; }
proven_equivocations() { printf '%s' "${STUB_PROVEN:-}"; }
anchor_height() { printf '%s' "${STUB_ANCHOR_HEIGHT:-}"; }
"""


def run_meet(name, nodes, bonds, expect_rc, required, bad_candidate=None, extra_blocks=None,
             anchor=None, anchor_height=None, proven=None):
    """Drive §3 with the attestation and block calls stubbed, and the rest of the code real.

    `nodes` is a list of `{name, height, hash, key}` (plus optional `bonds` — the bond map that node's
    verified block carries — and `justifications`). `bonds` is the map every node reports unless it
    carries its own: `[(validator_key, stake)]`. `bad_candidate` makes `is_ancestor_of` answer *no* for
    one candidate, which is how the descent below a divergent subset is exercised.
    """
    src = SCRIPT.read_text()
    helpers = src[src.index(MEET_HELPERS[0]):src.index(MEET_HELPERS[1])]
    section = src[src.index(MEET_SECTION[0]):src.index(MEET_SECTION[1])]
    default_bonds = [{"validator": k, "stake": s} for k, s in bonds]

    preamble = [
        "set -uo pipefail",
        "MAX_WALK=512",
        "EQUIV_WINDOW=16",
        # `verified_block` memoises on disk; the fixture keeps that real by pointing it at a scratch dir.
        'CACHE_DIR=""',
        "declare -A STUB_ATT STUB_BLOCK",
        "NAMES=(%s)" % " ".join(n["name"] for n in nodes),
        "declare -A HOST PORT",
        # §1's own reading, which the operator-anchor branch reports back to the operator (it does not
        # need to *compute* with it — that is what changed — but a person naming an anchor wants to see
        # what the nodes say).
        "declare -A LFB LFH",
        "MASTER=%s" % nodes[0]["name"],
    ]
    for n in nodes:
        preamble.append("HOST[%s]=%s; PORT[%s]=1" % (n["name"], n["name"], n["name"]))
        preamble.append("LFB[%s]='%s'; LFH[%s]='%s'"
                        % (n["name"], n.get("lfb_height", ""), n["name"], n.get("lfb_hash", "")))
        att = {"verified": True, "blockHash": n["hash"], "sender": n["key"], "nonce": "stub"}
        preamble.append("STUB_ATT[%s]='%s'" % (n["name"], json.dumps(att)))
        blk = {
            "verified": True,
            "blockHash": n["hash"],
            "blockNumber": n["height"],
            "justifications": n.get("justifications", []),
            "bonds": n.get("bonds", default_bonds),
        }
        preamble.append("STUB_BLOCK[%s]='%s'" % (n["hash"], json.dumps(blk)))
    for h, blk in (extra_blocks or {}).items():
        preamble.append("STUB_BLOCK[%s]='%s'" % (h, json.dumps(blk)))

    # The stubs are spliced between the helpers and the section: the real `jfield`, `refuse`, `rnode_bin`
    # and arithmetic run, while the three artefact calls answer from the tables above. They must come
    # *before* §3 textually — a shell reads incrementally, so a definition after the section would not
    # exist yet when the section runs (which is how the first draft of this fixture passed everything to
    # the real function and then failed on a missing `api`).
    script = "\n".join(preamble) + "\n" + helpers + STUB_OVERRIDES + section
    if anchor is not None:
        script = 'RECONCILE_ANCHOR="%s"\n' % anchor + script
    env = dict(os.environ)
    if bad_candidate is not None:
        env["STUB_BAD_CAND"] = bad_candidate
    if anchor_height is not None:
        env["STUB_ANCHOR_HEIGHT"] = anchor_height
    if proven:
        env["STUB_PROVEN"] = "\n".join(proven)

    p = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True, timeout=30)
    combined = p.stdout + p.stderr
    assert p.returncode == expect_rc, f"{name}: exit={p.returncode}, expected={expect_rc}\n{combined}"
    assert required in combined, f"{name}: missing {required!r}\n{combined}"
    print(f"PASS {name} (exit {p.returncode})")


if __name__ == "__main__":
    # --- section 3: the meet, over verified attestations ------------------------------------------
    KEYS = ["k" + n for n in "ABCD"]
    EQUAL = [(k, 100) for k in KEYS]
    def node(name, height, key, **kw):
        return dict(name=name, height=height, hash="h" + name, key=key, **kw)

    # **The case the gate could not handle, and why the gate was replaced.** Three of four validators
    # are a few rounds apart in what they have finalised (110 / 102 / 97) and the fourth is far behind
    # (50): three of four is 300 of 400, a strict supermajority, so the meet is the *deepest* block
    # they all finalised — the shallowest of the three — and the laggard is excluded rather than asked
    # to agree, and excluded rather than dragging the point back to its own 50.
    run_meet("a lagging validator neither vetoes nor drags",
             [node("A", 110, KEYS[0]), node("B", 102, KEYS[1]),
              node("C", 97, KEYS[2]), node("D", 50, KEYS[3])],
             EQUAL, 0, "the meet: height 97")
    run_meet("the laggard is named, not silently dropped",
             [node("A", 110, KEYS[0]), node("B", 102, KEYS[1]),
              node("C", 97, KEYS[2]), node("D", 50, KEYS[3])],
             EQUAL, 0, "not in the subset (lagging, not vetoing): D")
    run_meet("four validators in lockstep still compute",
             [node("A", 10, KEYS[0]), node("B", 10, KEYS[1]),
              node("C", 10, KEYS[2]), node("D", 10, KEYS[3])],
             EQUAL, 0, "stake: 400 of 400 = 100%")
    # **The double-count that deleted the first meet (#302)**, in the shape it would return: two
    # endpoints attesting as *one* validator. Counting the bond twice would fabricate a supermajority
    # out of one stake, so it is a refusal either way it is read.
    run_meet("two endpoints attesting as one validator",
             [node("A", 10, KEYS[0]), node("B", 10, KEYS[0])],
             EQUAL, 4, "both attested as")
    # A key the pool does not bond weighs nothing, and a node whose key is absent from the pool is not
    # running the chain this pool describes.
    run_meet("an attesting key the bond set does not bond",
             [node("A", 10, "kX"), node("B", 10, KEYS[1])],
             EQUAL, 4, "which the bond set does not bond")
    # A denominator that differs between nodes is not a denominator: different pools is the state
    # divergence the design refuses on.
    run_meet("the nodes disagree on the pool",
             [node("A", 10, KEYS[0]), node("B", 10, KEYS[1],
                                           bonds=[{"validator": KEYS[0], "stake": 100}])],
             EQUAL, 4, "disagree on the bond set")
    # **A strict supermajority, not a majority and not two thirds.** Two of four equal validators is
    # 200 of 400 (half); two of three is 200 of 300 — exactly two thirds, which the protocol's own rule
    # (`3 * stake > 2 * total`) refuses. Both must refuse here.
    run_meet("no strict supermajority",
             [node("A", 10, KEYS[0]), node("B", 10, KEYS[1])],
             EQUAL, 4, "no strict supermajority of stake attested")
    run_meet("exactly two thirds is not a supermajority",
             [node("A", 10, KEYS[0]), node("B", 10, KEYS[1])],
             [(KEYS[0], 100), (KEYS[1], 100), (KEYS[2], 100)], 4, "no strict supermajority of stake attested")
    # **The descent.** When the subset's attested blocks are not on one branch the shallowest is not an
    # ancestor of the others, and the meet is *below* it — the common ancestor is finalised by both
    # sides, so the walk steps down, verifies each step, and reports how far it went.
    run_meet("the meet descends when the subset diverged",
             [node("A", 10, KEYS[0]), node("B", 8, KEYS[1], justifications=["hP"]),
              node("C", 9, KEYS[2])],
             [(KEYS[0], 100), (KEYS[1], 100), (KEYS[2], 100)], 0, "the meet: height 7",
             bad_candidate="hB",
             extra_blocks={"hP": {"verified": True, "blockHash": "hP", "blockNumber": 7,
                                  "justifications": [], "bonds": []}})
    # **A proven equivocator leaves both sides of the fraction** — the rule the issue states, and the
    # one the denominator may legitimately shrink by. Here it is the whole point of the rule: without
    # the exclusion, two of three equal validators is 200 of 300 — *exactly* two thirds, refused; with
    # the third proven, the pool is the honest 200 and the threshold is met. Never by silence: C is out
    # because two signed blocks were re-derived, not because it went quiet.
    run_meet("a proven equivocator leaves numerator and denominator",
             [node("A", 10, KEYS[0]), node("B", 10, KEYS[1]), node("C", 10, KEYS[2])],
             [(KEYS[0], 100), (KEYS[1], 100), (KEYS[2], 100)], 0, "stake: 200 of 200 = 100%",
             proven=["10 %s aa11 bb22" % KEYS[2]])
    run_meet("the proof is named, and the pool it shrinks",
             [node("A", 10, KEYS[0]), node("B", 10, KEYS[1]), node("C", 10, KEYS[2])],
             [(KEYS[0], 100), (KEYS[1], 100), (KEYS[2], 100)], 0, "the pool is now 200",
             proven=["10 %s aa11 bb22" % KEYS[2]])
    # A proven equivocator's own attestation must not decide the point: it is out of the subset, so its
    # (here, shallower) block cannot become the meet.
    run_meet("the equivocator's attestation does not set the meet",
             [node("A", 10, KEYS[0]), node("B", 10, KEYS[1]), node("C", 3, KEYS[2])],
             [(KEYS[0], 100), (KEYS[1], 100), (KEYS[2], 100)], 0, "the meet: height 10",
             proven=["3 %s aa11 bb22" % KEYS[2]])

    # **The operator's anchor** (C269): when nothing can be computed, the operator names the block — and
    # the tool says whose root it is rather than presenting it as a meet. The two cases below now also
    # *shape* the refusal: a net with no finality left cannot attest anything, so this is the only path
    # left, and the message has to say so.
    run_meet("the operator names the anchor",
             [node("A", 0, KEYS[0]), node("B", 0, KEYS[1])], EQUAL, 0, "the operator's anchor: height 41",
             bad_candidate=None, anchor="ab" * 32, anchor_height="41")
    run_meet("the operator names an anchor nobody holds",
             [node("A", 0, KEYS[0]), node("B", 0, KEYS[1])], EQUAL, 4, "could not be read from A",
             anchor="cd" * 32, anchor_height="")

    # Same first block but a different second block must not be reported as convergence.
    # Verification is behind --apply, so test the actual set-comparison fragment below.
    fragment = SCRIPT.read_text()
    start = fragment.index('  for (( h=MEET; h<=MAXH; h++ )); do', fragment.index('== 9. verification'))
    end = fragment.index('  echo "  [$i]', start)
    verify = fragment[start:end]
    with tempfile.TemporaryDirectory(prefix="reconcile-dag-") as d:
        for case, left, right, expected in [
            ("equal sets different order", "a\\nb", "b\\na", 1),
            ("extra sibling block", "a\\nb", "a", 0),
            ("different second block", "a\\nb", "a\\nc", 0),
            ("missing height", "a", "", 0),
            # A height nobody has produced is the frontier, not a disagreement: `MAXH` comes from
            # `/api/status`, which sits one round ahead of the highest height the block API serves.
            ("height nobody has produced", "", "", 1),
        ]:
            script = """set -uo pipefail
MEET=0; MAXH=0; ok=1
NAMES=(A B)
declare -A HOST PORT
HOST[A]=A; HOST[B]=B; PORT[A]=1; PORT[B]=2
blocks_at() { if [ "$1" = A ]; then printf '%b\\n' "$LEFT"; else printf '%b\\n' "$RIGHT"; fi; }
""" + verify + "\nprintf '%s' \"$ok\"\n"
            p = subprocess.run(["bash", "-c", script], env={**os.environ, "LEFT": left, "RIGHT": right},
                               text=True, capture_output=True, timeout=10)
            assert p.returncode == 0 and p.stdout == str(expected), (case, p.stdout, p.stderr)
            print(f"PASS {case}")

    # --- section 2: the report fires, and nothing is consumed on it ------------------------------
    # The equivocation report had never been exercised — the drill's own note says so — and after the
    # #308 rename the section's contract is that it *prints* and decides nothing. Two distinct blocks
    # by one sender at one height is the shape it looks for; the assertion is that the report appears
    # and that no line claims a stake was dropped, reweighted or decided.
    src = SCRIPT.read_text()
    # The slice must start at the declarations above the banner, not at the banner: `REPORTED` is an
    # associative array, and a fragment that begins below the `declare -A` re-reads it as an
    # arithmetic index — the failure mode this slice-by-marker style has.
    a = src.index("declare -A REPORTED=() REPORTED_WHY=()")
    b = src.index("# --- 3. the anchor", a)
    section2 = src[a:b]
    script = """set -uo pipefail
MAXH=1
NAMES=(A)
declare -A HOST PORT
HOST[A]=A; PORT[A]=1
blocks_at() { printf '%s\\n' 'aa11 sender9 post 1 []' 'bb22 sender9 post 1 []'; }
""" + section2
    p = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=10)
    assert p.returncode == 0, p.stdout + p.stderr
    assert "REPORTED sender9" in p.stdout, p.stdout
    assert "two distinct blocks at height 0" in p.stdout, p.stdout
    # §2 still decides nothing — and now says where a *proof* is made instead, so a reader does not go
    # away thinking the report is the last word on equivocation.
    assert "these sender fields are the endpoints' word" in p.stdout, p.stdout
    assert "§3 *proves* what it can from" in p.stdout, p.stdout
    print("PASS section 2 reports an equivocation and consumes nothing")

    # --- section 4: one block served by several nodes is one block -------------------------------
    # RED before the fix: three records for two blocks and a total of 8, because the section walked
    # every node's answer and counted each *observation* — the #302 double-count, one section down.
    # `/api/blocks/h/h` is answered per node, so this is what a four-validator net does at every
    # height: each node serves the sibling blocks it holds, and the same block comes back more than
    # once. The stub is that shape in miniature: A serves two blocks, B serves one of A's and one of
    # its own.
    a = src.index('echo "== 4. what is above the point =="')
    # The slice stops at the movement marker: `report_movement` samples heights through helpers this
    # fixture does not load, and the marker is the boundary the section's own output ends at.
    b = src.index("# --- movement:", a)
    section4 = src[a:b]
    with tempfile.TemporaryDirectory(prefix="reconcile-s4-") as d:
        report = os.path.join(d, "deploys.jsonl")
        script = """set -uo pipefail
MEET=0; MAXH=1; MASTER=A
NAMES=(A B)
declare -A HOST PORT
HOST[A]=A; PORT[A]=1; HOST[B]=B; PORT[B]=2
blocks_at() {   # the five fields the section reads, last one never empty
  [ "$3" = "1" ] || return 0
  printf '%s\\n' 'h1 sender1 post1 3 ["deadbeef"]'
  [ "$1" = B ] && printf '%s\\n' 'h2 sender2 post2 2 []'
  return 0
}
""" + section4
        p = subprocess.run(["bash", "-c", script], env={**os.environ, "RECONCILE_REPORT": report},
                           capture_output=True, text=True, timeout=10)
        assert p.returncode == 0, f"section 4 exited {p.returncode}\n{p.stdout}{p.stderr}"
        recs = [json.loads(l) for l in Path(report).read_text().splitlines() if l.strip()]
        assert len(recs) == 2, f"two unique blocks served by two nodes produced {len(recs)} records"
        by_hash = {r["blockHash"]: r for r in recs}
        assert by_hash["h1"]["observers"] == ["A", "B"], by_hash["h1"]
        assert by_hash["h2"]["observers"] == ["B"], by_hash["h2"]
        assert by_hash["h1"]["rejectedDeploys"] == ["deadbeef"], by_hash["h1"]
        # 3 + 2, once each — the observation count of the same record is 3 answers, and the old
        # section totalled 8 by adding h1's deploys a second time.
        assert "5 deploy(s) above the point across 2 unique block(s)" in p.stdout, p.stdout
        assert "(heights 1..1; snapshot 1 taken at unknown)" in p.stdout, p.stdout
        print("PASS section 4 counts a block once and names the nodes that served it")

    # --- section 4: two nodes may not disagree about one block's own numbers ----------------------
    # RED before the fix: the first answer for a hash won, and a second node's answer for the *same*
    # hash was only appended to `observers` — its sender, deploy count and rejected set were never
    # compared. A block's hash fixes its content, so two different answers for one hash is one of the
    # two being wrong (or lying), and the report is what an operator re-submits deploys from. The tool
    # refuses instead, which is its rule everywhere else. PR #309 raised this; the green case above is
    # the control, because it is the same shape with the second node agreeing.
    #
    # The **real `refuse`** is spliced in rather than stubbed, so what this asserts is the tool's own
    # message and exit code and not a copy of them.
    refuse_src = src[src.index("refuse() {") : src.index("jfield() {")]
    agreed = 'h1 sender1 post1 3 ["deadbeef"]'
    for name, second_node_answer in [
        ("a different deploy count", 'h1 sender1 post1 4 ["deadbeef"]'),
        ("a different sender", "h1 sender9 post1 3 [\"deadbeef\"]"),
        ("a different rejected set", "h1 sender1 post1 3 []"),
    ]:
        with tempfile.TemporaryDirectory(prefix="reconcile-s4-conflict-") as d:
            report = os.path.join(d, "deploys.jsonl")
            script = (
                "set -uo pipefail\n"
                "MEET=0; MAXH=1; MASTER=A\n"
                "NAMES=(A B)\n"
                "declare -A HOST PORT\n"
                "HOST[A]=A; PORT[A]=1; HOST[B]=B; PORT[B]=2\n"
                "blocks_at() {   # the five fields the section reads, last one never empty\n"
                '  [ "$3" = "1" ] || return 0\n'
                '  if [ "$1" = A ]; then printf \'%s\\n\' \'' + agreed + "'\n"
                "  else printf '%s\\n' '" + second_node_answer + "'; fi\n"
                "  return 0\n"
                "}\n"
            ) + refuse_src + section4
            p = subprocess.run(
                ["bash", "-c", script], env={**os.environ, "RECONCILE_REPORT": report},
                capture_output=True, text=True, timeout=10)
            assert p.returncode == 4, (
                f"{name}: expected the tool's refusal (exit 4), got {p.returncode}\n{p.stdout}{p.stderr}")
            assert "REFUSING: two nodes disagree about block h1 at height 1" in p.stderr, p.stderr
            assert agreed.split()[0] in p.stderr, p.stderr
            assert (
                Path(report).read_text().strip() == ""
            ), f"{name}: the report was written despite the refusal"
        print(f"PASS section 4 refuses when two nodes disagree — {name}")

    # --- section 10: it must assert, not print ---------------------------------------------------
    # RED before the fix on every negative case below: the old section printed each node's finalised
    # block and exited 0 whatever it said — which is exactly what the committed drill transcript
    # (`spec/audit/evidence/n-reconcile-drill/restore-run.txt`) shows, four `finalised ?` lines and a
    # zero exit, under a closing sentence claiming to be #287's falsifier. Each node is
    # `name -> (finalised height, finalised hash, tip height, hashes its block API serves)`.
    a = src.index('echo "== 10. finality past the point =="')
    b = src.index("echo \"  (this section plus section 9 is #287's falsifier", a)
    section10 = src[a:b]

    def run_finality(name, nodes, expect_rc, required):
        lines = ["set -uo pipefail", "MEET=0", "MAXH=0", "RECONCILE_FINALITY_ROUNDS=1",
                 "sleep() { :; }", "trigger_block() { :; }",
                 # Range refresh behavior is exercised by reconcile-cache-regression.py;
                 # this fixture isolates the existing finality arithmetic over stubbed blocks.
                 "refresh_block_ranges() { :; }",
                 "NAMES=(%s)" % " ".join(nodes), "declare -A HOST PORT"]
        for n in nodes:
            lines.append("HOST[%s]=%s; PORT[%s]=1" % (n, n, n))
        for fn, i in (("lfb_num", 0), ("lfb_hash", 1), ("height_of", 2)):
            lines.append('%s() { case "$1" in' % fn)
            for n, spec in nodes.items():
                lines.append("  %s) printf '%%s' '%s' ;;" % (n, spec[i]))
            lines.append("esac; }")
        lines.append('blocks_at() { case "$1" in')
        for n, spec in nodes.items():
            body = " ; ".join("printf '%%s\\n' '%s'" % h for h in spec[3]) or ":"
            lines.append("  %s) %s ;;" % (n, body))
        lines.append("esac; }")
        p = subprocess.run(["bash", "-c", "\n".join(lines) + "\n" + section10],
                           capture_output=True, text=True, timeout=10)
        combined = p.stdout + p.stderr
        assert p.returncode == expect_rc, f"{name}: exit={p.returncode}, expected={expect_rc}\n{combined}"
        assert required in combined, f"{name}: missing {required!r}\n{combined}"
        print(f"PASS {name} (exit {p.returncode})")

    # A — the transcript's own shape: nothing finalised anywhere, so every node reads unusable.
    run_finality("finality unavailable everywhere",
                 {"A": ("", "", "3", []), "B": ("", "", "4", [])}, 9, "no usable finalized block")
    # B — the anchor itself is not progress: the acceptance clause is *past* the point.
    run_finality("finalized at the anchor, not past it",
                 {"A": ("0", "anchor", "3", ["anchor"]), "B": ("0", "anchor", "3", ["anchor"])},
                 9, "not past the point")
    # C — two finalised siblings at one height, **both** in the common DAG: the pairwise rule alone
    # accepts this, and two finalised heads at one height is the opposite of one head.
    run_finality("two finalized blocks at one height",
                 {"A": ("1", "hxA", "2", ["hxA", "hxB"]), "B": ("1", "hxB", "2", ["hxA", "hxB"])},
                 9, "two finalized blocks at height 1")
    # D — a finalised hash that a node which reached its height does not hold: a reported string,
    # not a block.
    run_finality("finalized block absent from a node that reached its height",
                 {"A": ("1", "hxA", "2", ["hxA"]), "B": ("1", "hxA", "2", ["hxB"])},
                 9, "does not hold it")
    run_finality("a taller node missing a shorter node's finalized block",
                 {"A": ("3", "hxA", "3", ["hxA"]), "B": ("1", "hxB", "1", ["hxB"])},
                 9, "does not hold it")
    # GREEN — differing finalised heights are a lag, not a divergence, and a node below a finalised
    # height is exempt from holding that block.
    run_finality("advanced, differing heights, each in the common DAG",
                 {"A": ("3", "hxA", "3", ["hxA", "hxB"]), "B": ("1", "hxB", "1", ["hxA", "hxB"])},
                 0, "finality advanced past")

    # --- the restore arm: the point is the point (W-N11-07, W-N11-09) ------------------------------
    # RED before the fix: `--restore-from-master` copied the survivor's whole DAG and only *printed*
    # that blocks above the point came with it, so a survivor holding a block its peers never accepted
    # installed that block on every joiner — the operator asserting a winner the rule never picked.
    # The arm now refuses and names them, and the operator who means it says so again at the moment it
    # costs something (`RECONCILE_ADOPT_ABOVE=1`). The refusal must come *before* the stop loop, which
    # is why the fixture can assert it with no node stubs doing anything.
    a = src.index('echo "== 5. apply (--restore-from-master)')
    b = src.index("trigger_block || true", a)
    b = src.index("\n", b) + 1
    restore_arm = src[a:b]

    def run_restore(name, report_lines, expect_rc, required, adopt=False):
        with tempfile.TemporaryDirectory(prefix="reconcile-restore-") as d:
            report = Path(d) / "deploys.jsonl"
            report.write_text("".join(line + "\n" for line in report_lines))
            lines = [
                "set -uo pipefail",
                "RESTORE=1; APPLY=1",
                "MEET=41; MEET_HASH=aa11",
                "MASTER=A",
                "NAMES=(A B)",
                "declare -A HOST PORT UNIT",
                "HOST[A]=a; HOST[B]=b; PORT[A]=1; PORT[B]=1; UNIT[A]=ctrA; UNIT[B]=ctrB",
                "REPORT=%s" % report,
                "date() { printf 'TS'; }",
                "sleep() { :; }",
                "stop_node() { :; }",
                "start_node() { :; }",
                "restore_chain_state_from_master() { :; }",
                "trigger_block() { :; }",
            ]
            env = dict(os.environ)
            if adopt:
                env["RECONCILE_ADOPT_ABOVE"] = "1"
            p = subprocess.run(["bash", "-c", "\n".join(lines) + "\n" + restore_arm],
                               capture_output=True, text=True, timeout=10, env=env)
            combined = p.stdout + p.stderr
            assert p.returncode == expect_rc, f"{name}: exit={p.returncode}, expected={expect_rc}\n{combined}"
            assert required in combined, f"{name}: missing {required!r}\n{combined}"
            print(f"PASS {name} (exit {p.returncode})")

    def rec(height, bh, observers):
        return ('{"height":%d,"blockHash":"%s","sender":"k","deployCount":1,"observers":[%s],'
                '"rejectedDeploys":[]}' % (height, bh, ",".join('"%s"' % o for o in observers)))

    # The survivor holds a block above the point that the other node does not: the copy would install it.
    run_restore("the survivor's blocks above the point are refused",
                [rec(42, "bb22", ["A"]), rec(43, "cc33", ["A", "B"])],
                9, "holds 2 block(s) above the point")
    # …and the refusal names the heights, so the operator can read what it is being asked to adopt.
    run_restore("the refusal names the blocks' heights",
                [rec(42, "bb22", ["A"])], 9, "heights: 42")
    # A survivor at the point, with the other node holding what it does not: nothing above the point is
    # the survivor's, so the copy proceeds and nothing is refused.
    run_restore("a survivor at the point is not refused",
                [rec(42, "bb22", ["B"])], 0, "== 6. the fiat")
    # The acknowledgement is the operator saying it again, and the adoption is printed.
    run_restore("the acknowledgement adopts and says so",
                [rec(42, "bb22", ["A"])], 0, "adopting 1 block(s) above the point", adopt=True)

    # --- the copy loop: an absent source is not a licence to keep the joiner's own (W-N11-08) ------
    # RED before the fix: the survivor lacking an environment (`transaction` on this build — LMDB
    # creates it on first write) meant the loop `continue`d and the joiner kept its own, which is
    # exactly the divergence residue the restore exists to remove. The environment is now moved aside
    # (never deleted), and the manifest names what was adopted — the half of W-N11-04 that is code.
    a = src.index("restore_chain_state_from_master() {")
    b = src.index("\nstop_node() {", a)
    copy_fn = src[a:b]

    DOCKER_STUB = """
docker() {
  local src="" dst="" script="" a
  while [ $# -gt 0 ]; do
    case "$1" in
      -v) case "$2" in
            *:/src:ro) src="${2%:/src:ro}" ;;
            *:/dst)    dst="${2%:/dst}" ;;
          esac
          shift 2 ;;
      -c) script="$2"; shift 2 ;;
      *)  shift ;;
    esac
  done
  printf '%s' "$script" | sed "s#/src#$src#g; s#/dst#$dst#g" > "$STUB_SCRIPT"
  sh "$STUB_SCRIPT"
}
"""

    with tempfile.TemporaryDirectory(prefix="reconcile-copy-") as d:
        src_vol = Path(d) / "srcvol"
        dst_vol = Path(d) / "dstvol"
        for vol, envs in ((src_vol, ["blockstorage", "dagstorage", "rspace/history"]),
                          (dst_vol, ["blockstorage", "dagstorage", "rspace/history", "transaction"])):
            for e in envs:
                (vol / e).mkdir(parents=True)
                (vol / e / "data.mdb").write_bytes(b"x" * 16)
        # The joiner's own leftover, which the survivor does not have.
        (dst_vol / "transaction" / "data.mdb").write_bytes(b"joiner-only" * 4)
        (src_vol / "blockstorage" / "data.mdb").write_bytes(b"survivor" * 16)

        script_path = Path(d) / "inner.sh"
        preamble = [
            "set -uo pipefail",
            "CONTROL=docker",
            "MASTER=A",
            "NAMES=(A B)",
            "declare -A HOST PORT UNIT",
            "UNIT[A]=ctrA; UNIT[B]=ctrB",
            'CHAIN_ENVS="blockstorage dagstorage rspace/history transaction"',
            'STUB_SCRIPT=%s' % script_path,
            'data_volume() { case "$1" in ctrA) printf %%s %s ;; ctrB) printf %%s %s ;; esac; }'
            % (src_vol, dst_vol),
            DOCKER_STUB,
        ]
        p = subprocess.run(["bash", "-c", "\n".join(preamble) + "\n" + copy_fn + "\n"
                           'restore_chain_state_from_master "B"\n'],
                           capture_output=True, text=True, timeout=30)
        combined = p.stdout + p.stderr
        assert p.returncode == 0, f"the copy loop failed: {p.returncode}\n{combined}"
        assert not (dst_vol / "transaction").exists(), \
            f"the joiner-only env must not survive the restore: {combined}"
        assert (dst_vol / "transaction.reconcile-orphan").is_dir(), \
            f"it is moved aside, never deleted: {combined}"
        assert "moved aside here: transaction" in combined, combined
        assert "adopted blockstorage:" in combined, combined
        assert "list-digest" in combined, combined
        # The environments the survivor has are the survivor's, not the joiner's stale copies.
        assert (dst_vol / "blockstorage" / "data.mdb").read_bytes() == b"survivor" * 16
        print("PASS the copy loop moves an absent source aside and prints the manifest")

    # --- movement: a producing chain is reported, not silently absorbed ---------------------------
    # Issue 488's residual: the audit is a snapshot of MAXH, and a node that advances past it must
    # say so rather than leaving the operator to infer the audit's range. Non-fatal by design.
    a = src.index("report_movement() {")
    b = src.index("\n}\n", a) + 3
    movement = src[a:b]
    script = """set -uo pipefail
MAXH=57
declare -A HGT HOST PORT
HGT[A]=57; HGT[D]=55
NAMES=(A D)
HOST[A]=a; PORT[A]=1; HOST[D]=d; PORT[D]=2
height_of() { case "$1" in a) printf 60 ;; d) printf 55 ;; esac; }
""" + movement + """
report_movement
"""
    p = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=10)
    assert p.returncode == 0, f"report_movement exited {p.returncode}\n{p.stdout}{p.stderr}"
    assert "NOTE: A advanced 57 → 60 since the snapshot; the audit above covers only heights 0..57" in p.stdout, p.stdout
    assert "NOTE: D" not in p.stdout, p.stdout
    assert "Movement during an audit is normal" in p.stdout, p.stdout
    print("PASS movement is reported for an advancing node, not a static one, and is not fatal")
