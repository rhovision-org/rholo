import Rchain

/-!
# rchain-tables — emit the C343/C357 contract tables from the Lean

The two external contracts are Lean definitions (`Rchain.PhaseDiagnosis.phaseDiagnosisTable` and
`Rchain.Acceptance.acceptanceTable`). The collector/checker read the emitted TSVs; this exe is the
one writer, so the Python does not encode a second oracle.

Usage: `lake exe rchain-tables --out-dir spec/audit`.
-/

namespace Rchain.Tables

open Rchain.PhaseDiagnosis Rchain.Acceptance

def phaseDiagnosisTsv : String :=
  "priority\tdiagnosis\tconditions\n" ++
    String.intercalate "\n"
      (phaseDiagnosisTable.map fun r =>
        s!"{r.priority}\t{r.diagnosis.label}\t\
          {String.intercalate "," (r.conditions.map Condition.label)}") ++ "\n"

def phaseDiagnosisCasesTsv : String :=
  "case\texpected\n" ++
    String.intercalate "\n"
      ["incident\tproposal-isolation",
       "processor-fault\tprocessor-fault-blocked",
       "syncing\tsyncing-recovery",
       "healthy\thealthy",
       "unmodelled\tunmodelled"] ++ "\n"

def acceptanceTsv : String :=
  "shape\trequirements\n" ++
    String.intercalate "\n"
      (acceptanceTable.map fun r =>
        s!"{r.shape.label}\t{String.intercalate "," r.requirements}") ++ "\n"

def acceptanceCasesTsv : String :=
  "case\texpected\n" ++
    String.intercalate "\n"
      ["valid-new-genesis\ttrue",
       "valid-retained-sweep\ttrue",
       "reconstructed-with-both-acceptances\ttrue",
       "mixed-version\tfalse",
       "missing-reviewer\tfalse",
       "both-shapes\tfalse",
       "missing-digest\tfalse"] ++ "\n"

end Rchain.Tables

def main (args : List String) : IO UInt32 := do
  let outDir : String := match args with
    | "--out-dir" :: d :: _ => d
    | _ => "audit"
  IO.FS.createDirAll outDir
  IO.FS.writeFile (outDir ++ "/phase-diagnosis.tsv") Rchain.Tables.phaseDiagnosisTsv
  IO.FS.writeFile (outDir ++ "/phase-diagnosis-cases.tsv") Rchain.Tables.phaseDiagnosisCasesTsv
  IO.FS.writeFile (outDir ++ "/c357-acceptance.tsv") Rchain.Tables.acceptanceTsv
  IO.FS.writeFile (outDir ++ "/c357-acceptance-cases.tsv") Rchain.Tables.acceptanceCasesTsv
  return 0
