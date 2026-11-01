# M3-14 — the 2026-10-27 fresh-join journal: INCOMPLETE

The 2026-10-27 live record (`spec/audit/evidence/n457-live-join-20261027/acceptance.json`) pins its
fresh-join journal by SHA-256:

```
10c76dbca7bc219fff419e5cfa73c182511979cfa63c8920b5d3d0f045437a78
```

The journal itself is **not committed** in this tree, and it was not retrievable from the session
that wrote this note (the authorized host is not reachable from here). The six-stage analysis of
#428 M3 is therefore **INCOMPLETE**, and no stage is claimed from this record.

What a completed analysis must show, if the journal is recovered on the authorized host:

| stage | key | status now |
|---|---|---|
| 1 joiner request | peer address/identity, block hash or state path | UNVERIFIED (journal not in tree) |
| 2 responder receipt | same key, responder log | UNVERIFIED |
| 3 response construction | same key, responder log | UNVERIFIED |
| 4 transport completion | stream outcome — **not logged by this build** | UNWITNESSED by construction |
| 5 requester receive | same key, joiner log | UNVERIFIED |
| 6 state application | restored post-state hash equals the bootstrap's | UNVERIFIED in this record |

Rules for the recovery:

1. verify the digest above before using a byte;
2. key stages 1/2/3/5 by peer address and block hash or state path — **never** by a send line;
3. mark stage 4 `UNWITNESSED` (the record's own text says the stream completion is dropped at this
   layer), and stage 6 `INCOMPLETE` unless the restored post-state hash is in the record;
4. a stage that cannot be keyed is `INCOMPLETE`, not a pass.

This note is a provenance record, not an M3 run; it is not labelled for the locality checker and
claims no off-host completion.
