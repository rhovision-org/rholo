# Testnet acceptance §3 source excerpt

Retrieved 2026-11-18 from `docs/src/spec/testnet-acceptance.md:64-68`. Whitespace is
normalized; the criteria are unchanged.

| | Criterion | The witness that decides it |
|---|---|---|
| **1** | Anyone can propose, and production is bounded | the deploy **finalises** (`last-finalized-block` ≥ its height) — never "the height stopped moving" |
| **2** | Validators can be dropped and joined without risk | the survivors finalise **past** the kill; the rejoiner resumes with no operator action; a new validator **bonds onto a running net** and produces |
| **3** | Every attack vector is handled | a **rate and a class census** against a bounded-adversary statement. Not a pass/fail item, and it must not be reported as one |
