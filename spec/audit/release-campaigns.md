# Mainnet release campaigns

Emitted from `spec/audit/mainnet-gates.external.json` by `tools/build-mainnet-gates.py`.

**These are not gates.** A gate is a boolean test of the current build. Each campaign
needs hosts, a release candidate or an operator window and an evidence record, so it
stays a release blocker in this tracker and is not counted in `mainnet-gates.json`.
Closing one is an operator campaign judged by its executable criterion, not a code edit.

- generated: 2026-11-19
- campaigns: 12

| campaign | family | status | title | locality | owner |
|---|---|---|---|---|---|
| `C343` | C343 | **precondition** | Private-chain surface: live B/C/D read-only phase diagnosis | live-host | Aria / operator |
| `G428-M1` | G428 | **blocked** | Supported capacity and physical fault domains | multi-host | Jim / release coordination |
| `G428-M2` | G428 | **owes** | Bounded public ingress and overload behavior | live-host | Jim / release coordination |
| `G428-M3` | G428 | **blocked** | Off-host bootstrap and fringe completion | off-host | Aria / evidence work |
| `G428-M4` | G428 | **blocked** | Finality, liveness and autonomous recovery | multi-host | Jim / release coordination |
| `G584-G1` | G584 | **external** | Installed pins | live-host | Aria / operator |
| `G584-G2` | G584 | **external** | Signed deploy lifecycle | live-host | Aria / operator |
| `G584-G3` | G584 | **external** | No false settlement | live-host | Aria / operator |
| `G584-G4` | G584 | **external** | Recovery without loss | live-host | Aria / operator |
| `G584-G5` | G584 | **external** | Independent observations | live-host | Aria / operator |
| `TA-C1` | TA | **blocked** | Anyone can propose, and production is bounded | live-host | Jim / release coordination |
| `TA-C2` | TA | **external** | Validators can be dropped and joined without risk | live-host | Aria / evidence work |

`check-release-campaigns.py --assert-ready` exits 3 while any campaign is open, so a
closed gate register is never a release authorization.
