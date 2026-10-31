# Paper B — Auditable development of a consensus-critical node by AI agents

> **Draft. Not a published paper, and not a claim to be one.** This is a checkpoint. Its subject is the
> *process* by which this repository was built, and its evidence is the process's own written record —
> including its failures, which are the part worth publishing. §5 states what would falsify the claim and
> what measurement is still missing.

## Abstract

The literature on AI agents writing software measures *capability*: how many issues a model can close on
a benchmark. It says almost nothing about *auditability* — whether the work an agent did can be checked
afterwards by someone who was not there. This draft describes a discipline that makes agentic work on a
consensus-critical codebase reviewable: a small set of rules about where a claim lives, when it may
close, what evidence it must carry, and what a reviewer is entitled to assume. The rules are cheap to
state and were each written after a specific failure. The failures are recorded in the same artifacts
that enforce the rules, so the record is both the method and its falsification. We claim no result beyond
this: the discipline produced a working node, and its failure catalogue is the paper.

## 1. Introduction

An agent session is powerful and amnesiac. It can hold a large tree in context, edit many files
coherently, and run a build; it forgets everything when the session ends, and it has no stake in what it
told you last week. Both properties matter for software that cannot be rolled back.

The auditability problem is not "can the agent write correct code". It is: *when the agent says a thing
is done, what exactly has been established, and who can check it?* The answer an ordinary agent workflow
gives is "the tests passed and the session said so" — which is a claim about a moment, not a durable
fact, and which a reviewer cannot re-enter.

The discipline below is an attempt to make agent claims durable. Its central move is to give every claim
**exactly one home and a machine that reads that home**, so that "done" is a property of the repository
rather than of a conversation.

## 2. Setting

- **Sessions are the unit of work, plans are the unit's contract.** A unit begins with a plan file
  outside the repository; the plan names the deliverable, the constraints already known, the verification
  steps, and what is explicitly out of scope. A plan is reviewed before work starts, which is where a
  misread of the target is cheapest to catch — and where it is not always caught (§4).
- **Memory is a durable fact store, one fact per file, with an index loaded each session.** It carries
  the things a session cannot re-derive cheaply: which branch is which, which trap cost a day last time,
  which rule the user set and why. It is deliberately *not* a copy of the repository's own documentation
  — a fact the repo records belongs in the repo.
- **Isolation is structural.** Work happens in a git worktree, so parallel sessions do not fight over a
  working tree. Staging is by explicit path, never `git add -A`, because two sessions may share a
  checkout.
- **The repository is the record.** Registers, gates, and committed evidence live in-tree; the
  conversation does not.

## 3. The discipline

Each rule below exists because something went wrong without it.

### 3.1 One finding, one home

Every finding has exactly one row, in one register (`spec/findings.tsv`), and the register is the record.
An issue tracker entry is not a home: it is a place a finding can be duplicated, lost, or re-filed. The
rule that follows is that **a finding never spawns an issue by itself** — an issue needs a committed
owner, and if the finding is real the register already holds it.

*Prevents:* the same defect filed three times under three names, and the tracker diverging from the
register.

### 3.2 Done means no longer representable

A row closes when the bad state it describes cannot be *represented* in the code, and the row must carry
the citation that forbids it — or it is recorded as a decision, explicitly. A close that rests on "the
test passes" is not a close; it is a snapshot.

*Prevents:* the class of closure that decays — fixed at one call site, reachable at the next.

### 3.3 A claim closes against the bar, and the bar is named

"Done" is judged against the acceptance criteria the plan named, never against register rows, unit tests
or green CI. When the bar is not met, the answer is the verdict first and in one word, without a "yes
but no".

*Prevents:* the drift where "most of it works" is reported as "it works".

### 3.4 Evidence is committed, not cited

Every claimed run has a home in the tree (`spec/audit/evidence/<suite>/`). A citation to a build output
directory is unopenable and therefore not evidence.

*Prevents:* a claim that cannot be re-entered by anyone, including the agent in a later session.

### 3.5 Gates read the documentation

If prose states a machine-checkable total, a tool recomputes it and the build fails on drift. A
hand-written register total in a document is a build error. A register row that cites a record which does
not exist is a build error.

*Prevents:* "a register column nobody checks rots" — eleven rows once cited pass records that had never
been written.

### 3.6 Verification claims need re-running

A commit's "green" is not evidence, and a stated constant is an unproved theorem. Before a claim is
repeated, the check is run against the tree as it stands.

*Prevents:* inherited confidence. A merged, green commit is a claim about a different tree.

### 3.7 One close-out, one pull request

A unit closes as one PR. Stacked or potentially-conflicting PRs are not opened. The test is "can these
ever conflict", not "are they independent".

*Prevents:* a rebase cascade that silently drops a record — which happened, and cost a re-measurement.

### 3.8 Act, don't narrate

The user wants the chore done and merged, not the process explained. A small fix in flight is folded
into the unit rather than narrated as a separate plan.

*Prevents:* the failure mode where the agent's output is a description of work rather than work.

## 4. The failure catalogue

The rules are cheap to state because the failures were expensive. These are recorded, not hypothetical:

- **The target was misread.** A request about "the comparative security section of the docs" was taken as
  the book chapter rather than the README section, and three clarifying questions were asked about the
  wrong file. The correction was a single sentence. *Cost:* one wasted plan.
- **One request became seven issues.** A close-out that offered "one issue per wave" as an option in its
  own plan produced seven open issues where the register was the actual record. The lesson is now a rule:
  the register is the record, and an issue needs a committed owner.
- **A tracker was filed against without reading it.** Three of twenty-three issues filed in one pass were
  re-files of the agent's own earlier work, caught only by reading the *bodies* rather than the titles.
- **A row's shape made it invisible.** A worksheet row formatted differently from its neighbours was
  skipped by every check the page had; three rows went unseen until a later sweep. A check that misses
  the instances it exists for is worse than no check.
- **An instrument was blamed for the defect.** Twice, a probe's own failure was read as the defect it was
  measuring; the fix was to read the operation's reply rather than its success marker, and to check the
  instrument first.
- **An arm measured the test runner.** An assertion that landed inside the bound's own refill window
  measured the harness rather than the system; the corrected form asserts *which* thing won, not *which*
  was faster.
- **A running script was edited mid-run.** Bash reads by byte offset, so an edit to a running script
  fails on a line the interpreter never reached. The run is discarded, not repaired.
- **Two sessions took the same record number.** The tell is a rebase conflict in the register; the second
  merge renumbers.
- **An absence claim from an unfetched clone is a fact about the clone.** Fetch before asserting that
  something is not there.

## 5. Evaluation — what would falsify the claim

The claim is narrow: *the discipline makes agent work checkable by a third party.* It is falsifiable, and
the falsifiers are external to this draft:

1. **An unsupported "done".** Take any commit the agent called complete and check it against the bar its
   own plan named. If a material claim in it is not backed by a committed artifact, the discipline failed
   for that unit. The record is the commit history, public and re-runnable.
2. **A register row that does not survive reading.** Take a `done` row and ask whether the bad state is
   genuinely unrepresentable. A row that closes on a test snapshot rather than a structural change is a
   counterexample.
3. **A gate that does not fire.** Cause the drift the gate exists for and confirm it fails the build. A
   gate whose failure mode is "it did not catch this" is a hole, and the tree already contains three
   instances of that shape (§4).

What is still missing is a *cost* measurement: lines of gate and register maintained per defect
prevented, and the escape rate of defects that reached a later layer than the one that should have caught
them. That measurement is the same §5 table Paper A needs, viewed from the process side, and it is not
written.

## 6. Threats to validity

- **The author's bias is structural.** An account of a process written by the process's own agent
  proves little on its own. What is *not* agent testimony is externally checkable: the commit history,
  the registers, the gate sources, and the failures recorded in them.
- **Survivorship in the failure catalogue.** Failures appear here because a rule made recording them
  cheap. Failures that no rule captured are, by construction, absent.
- **The harness is a moving target.** The agent, the tooling and the rules all changed during the period
  described. The discipline is not a controlled variable, and no claim of causality is made.
- **n = 1.** One project, one team of two (a human and an agent), one language.

## 7. Related work

Agentic software-engineering research is dominated by capability benchmarks: task completion on curated
issue sets, with pass rates as the result. Oversight work addresses human review of model output at the
*human-in-the-loop* level. Neither addresses the question this draft is about: what durable structure
makes the output of an autonomous session auditable after the session is gone. The closest analogues are
process-discipline reports from human engineering organisations — the AWS formal-methods experience
report among them — and the proposal here is that the same move (give the claim one home, and give the
home a machine that reads it) transfers to agents, because it is what the agent's amnesia makes
necessary.

## 8. Conclusion

The rules are not novel and are not meant to be: one home per claim, a close that is structural, evidence
that is committed, gates that read the prose, and a repository that is the record rather than the
conversation. What is offered is the observation that these rules are *sufficient* to make agent work
auditable, that they were derived from recorded failures rather than designed in advance, and that the
failures are the part of the record a reader should actually read.

## References

- `AGENTS.md` — the rules the repository states for agents working in it
- `spec/AUDIT.md`, `spec/findings.tsv` — the findings register and its row schema
- `spec/audit/evidence/` — committed runs
- `spec/README.md` — the specification tree and its gates
- Newcombe et al., *How Amazon Web Services Uses Formal Methods*, CACM 58(4), 2015 — the process-report model
