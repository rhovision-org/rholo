import Rchain.Laws.Base
import Rchain.Laws.Chunk01
import Rchain.Laws.Chunk02
import Rchain.Laws.Chunk03
import Rchain.Laws.Chunk04
import Rchain.Laws.Chunk05
import Rchain.Laws.Chunk06
import Rchain.Laws.Chunk07
import Rchain.Laws.Chunk08
import Rchain.Laws.Chunk09

/-!
# The law register — the catalog, assembled

Every law, with what each one rests on. The rows live in `Rchain/Laws/Chunk01..09.lean`, one module
per law-number range (at most 31 entries), because a single module holding the whole register
elaborated at ~15 GiB on the 16 GiB CI runner (measured 2026-11-23: `ps` peak 15,102,104 KiB at 187
entries). A `def` boundary is not a memory boundary — one `lean` process elaborates every declaration
in a module — so the chunks are modules, and Lake gives each its own process. The numbering check
(`1..lawCeiling`) and `entryCeiling` in `Rchain/LawsMain.lean` make a dropped or duplicated row across
the boundaries a build failure; the emitter's stable sort makes the generated register independent of
the split. `tools/measure-lean-memory.sh` reports each chunk's peak; the formal gate refuses a peak
over its budget rather than letting the runner's OOM killer decide.
-/

namespace Rchain
namespace Laws

def laws : List Law :=
  lawsChunk01 ++ lawsChunk02 ++ lawsChunk03 ++ lawsChunk04 ++ lawsChunk05 ++
    lawsChunk06 ++ lawsChunk07 ++ lawsChunk08 ++ lawsChunk09

/-- Every law number the catalog defines. Laws with clauses repeat. -/
def numbers : List Nat := laws.map (·.number)

/-- The axioms claimed by some law. `Rchain/LawsMain.lean` requires this to equal the tree's `axiom`
set exactly — the ratchet that makes a new axiom impossible to add without a row that justifies it. -/
def claimedAxioms : List Lean.Name := (laws.map (·.axioms)).join

/-- The declarations every row cites, for the reference-integrity check. -/
def citedDeclarations : List Lean.Name := (laws.map (·.declarations)).join

end Laws
end Rchain
