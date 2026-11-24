# C410 — the public driver boundary

Pinned to the release-campaign-readiness branch on `dev@ad9418aac` (PR #670 merged).

## Defects

- `tests/release-claim-2.py` read `manifest["harness_result"]` and `manifest["cycles"]`;
  `tests/release-claim-6.py` trusted `manifest["join_result"] == "pass"`; `tools/fleet_manifest.py`
  refused neither.
- `tests/release-claim-3.py`'s public path never submitted a marker and waited for a settlement
  that could not exist.
- `tools/fleet-run.py::collect_traces` read only `boot-*` and `merge-*`; finality/state/audit/format
  traces were invisible, so a failure read `evidence-insufficient`.
- A claim runner could leave `fleet-result.json` under `out/<action>/`; the locality gate reads
  only the record root or `raw/`.
- `RC-3/availability` and `RC-6/availability` had no runner.
- `RC-6/availability`'s authored behaviour named a non-producer join; `check_availability` did not
  require it.

## Witnesses

- `tests/test_fleet_arms.py`: two partition cycles rotate the victim; a non-producing victim and an
  unremoved partition are unsafe; ten finalised blocks are read on all four validators; a missing
  body or state is `not_available` and unsafe; the RC-6 availability join precondition is recorded.
- `tests/test_fleet_manifest.py::InputOnlyTests`: `cycles`, `pressure_record`, `join_result`,
  `audit_bootstrap`, `upgrade_record` and `harness_result` are refused by name; a fault cannot name
  the victim; availability needs ten blocks; a join cannot bootstrap from the ceremony node or
  share a validator host; pressure needs a node host and a unique mode order; the upgrade order is a
  permutation.
- `tests/test_release_campaigns.py`: every claim fixture passes and a decisive mutation fails.
- `tests/test_release_arm_runners.py`: the operator-artifact runners keep their false-pass boundary.

## Mutation controls

Removing the output-key refusal lets a result-shaped manifest through. Removing the producing-victim
check lets a non-producing cycle pass. Removing the availability body/state check lets a missing
block pass. Removing the RC-6 join branch lets an availability sweep close RC-6.

## Non-claims

No public arm is closed and no claim reading changes. The `upgrade` driver, the proxy/TLS pressure
surface, and the fifth-host join rehearsal are deferred to their windows. A Docker run is a
rehearsal and is never packed.
