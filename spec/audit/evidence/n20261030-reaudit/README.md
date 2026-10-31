# October 2026 security re-audit — evidence

**Claim.** An adversarial pass over `dev@be5016682` found the cost-model class of the September 2026
audit live again in the collection conversions (a sort for a flat 18 phlo), plus `rho:gov:censure`'s
cubic path, a skipped proposal bound on a no-finality view, an OCapN greeter that dials outside the
dial reserve, a transaction amount bounded only by the coordinator's vault, and a body/disposition
crash window — with two assurance texts that say more than the code does.

**What this does not prove.** No live multi-node attack, no external audit, no mutation score, no
coverage re-measure, no full reading of the three chain trees, no 600 s wall-clock wait, no 100 000-entry
relay exhaustion. The measurements are release-profile probes on one host; the projection to the block
deadline is arithmetic from those measurements, not a wait.

**These are the *before* numbers, and the findings are no longer open.** The eight rows were remediated
in the same PR (pass §149); the *after* measurements from the same probe are in
[`../c345-c352-remediation-20261030/`](../c345-c352-remediation-20261030/). Left here unchanged on
purpose: a findings record that edits itself cannot be checked against.

## Files

- `preregistration.md` — the method, the lens assignment, the 18-candidate queue and the verdict
  ledger.
- `verdicts.md` — each refereed candidate, its verdict and the artifact that decides it.
- `gas-probe.log` — the release output of `rholang/tests/gas_amplification.rs` (`RNODE_GAS_PROBE=1`):
  `toSet` flat 18 phlo at N = 500…40 000 while wall time grows 7 → 724 ms; `set` flat 22; `union`
  proportional for equal operands.
- `three-chain.json` — the pinned revisions of Agave, Sui and Bitcoin SV read for the comparison.

**Pin.** `origin/dev` `be5016682efcfc6dbc2df7cb409ece7a55533e08` (2026-10-30). The pass is report-only:
no production code changed; the one instrumentation file is the env-gated test above.
