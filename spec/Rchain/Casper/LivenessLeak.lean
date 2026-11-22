import Rchain.Casper.Liveness
import Rchain.Liveness

/-!
# Law 89's leak clause, and its conjunction with the jail/vote half

`Rchain/Casper/Liveness.lean` models law 52 and, since 2026-11-22, the inactivity leak
(`law89Leak_statement`). `Rchain/Liveness.lean` models the jail/vote machinery. They are one
instrument with two responses to silence, so the register's law-89 row takes their conjunction as its
formal.

**Why this is its own module.** `Rchain/Liveness.lean` imports Mathlib, and putting that import into
`Rchain/Casper/Liveness.lean` changes the simp set `Rchain/Casper/Rounds.lean` elaborates under — its
proof at `Rounds.lean:98` stops finding its rewrite pattern. The conjunction therefore lives here,
downstream of both halves and imported only by the umbrella (`Rchain.lean`) that the register's
reference check elaborates against, so no proof module's environment moves.
-/

namespace Rchain

/-- **Law 89 as amended**: the jail/vote machinery and the priced leak are one instrument with two
    responses to silence. `Rchain.Liveness.law89` is the jail/vote half and
    `law89Leak_statement_holds` is the leak's. -/
def law89_with_leak_statement : Prop :=
  Rchain.Liveness.law89_statement ∧ law89Leak_statement

theorem law89_with_leak_statement_holds : law89_with_leak_statement :=
  ⟨Rchain.Liveness.law89, law89Leak_statement_holds⟩

end Rchain
