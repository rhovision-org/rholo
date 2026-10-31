import Rchain.Syntax

set_option maxHeartbeats 1000000

/-!
# The flat `Par` ADT (M2 gate)

The Scala source of truth is `models/src/main/protobuf/RhoTypes.proto:32-43`: `Par` is a **flat**
record of 8 *repeated* (list) fields, not a tree. Each field's element type is a small record with
its own arity and flags. This module defines that mutual type family, replacing the Phase-0 binary
`Proc` (which was a fragment). Binders use de Bruijn *levels* (`Var.bound`/`Var.free`).

This is the ADT that Law 1's real `sort`, Law 2's `≡`, Law 3's substitution, and Laws 4–5 all act on.
-/

namespace Rchain

mutual
  /-- A process: 8 parallel `list` fields, each kept sorted by `sort` (Law 1). -/
  inductive Par where
    | mk : List Send → List Receive → List New → List Expr → List Match →
           List GUnforgeable → List Bundle → List Connective → Par

  /-- `Send` — channel, data, and the persistent flag (`!` vs `!!`). -/
  inductive Send where
    | mk : Par → List Par → Bool → Send

  /-- `ReceiveBind` — patterns, source, and the free-var count for the pattern. -/
  inductive ReceiveBind where
    | mk : List Par → Par → Nat → ReceiveBind

  /-- `Receive` — binds, body, persistent flag (`<-` vs `<=`), and bind count. -/
  inductive Receive where
    | mk : List ReceiveBind → Par → Bool → Nat → Receive

  /-- `New` — `new bindCount in body` (fresh unforgeable names). -/
  inductive New where
    | mk : Nat → Par → New

  /-- `MatchCase` — pattern, source/body, and the pattern's free-var count. -/
  inductive MatchCase where
    | mk : Par → Par → Nat → MatchCase

  /-- `Match` — target and cases (first-match-wins). -/
  inductive Match where
    | mk : Par → List MatchCase → Match

  /-- `Expr` — ground values and the arithmetic/logical nodes Laws 2 and 4 need. -/
  inductive Expr where
    | ground : Ground → Expr
    | evar   : Var → Expr
    | eneg   : Par → Expr
    | enot   : Par → Expr
    | eplus  : Par → Par → Expr
    | eminus : Par → Par → Expr
    | emult  : Par → Par → Expr
    | ediv   : Par → Par → Expr
    | emod   : Par → Par → Expr
    | elt    : Par → Par → Expr
    | ele    : Par → Par → Expr
    | egt    : Par → Par → Expr
    | ege    : Par → Par → Expr
    | eeq    : Par → Par → Expr
    | eneq   : Par → Par → Expr
    | eand   : Par → Par → Expr
    | eor    : Par → Par → Expr
    | ematches : Par → Par → Expr
    | eshortand : Par → Par → Expr
    | eshortor  : Par → Par → Expr
    | elist  : List Par → Option Var → Expr
    | etuple : List Par → Expr
    | eset   : List Par → Option Var → Expr
    | emap   : List (Par × Par) → Option Var → Expr
    -- The five the node has and this model did not (2026-09-25, law 1a's remaining unit). Declared in
    -- **score-tag** order, and each is its own class rather than a leaf of `ground` or of an existing
    -- operator, because the node's tags interleave: `BIG_INT` 13 sorts *after* the collections and
    -- *before* the vars, `EMETHOD` 115 *between* `EOR` 114 and `EMATCHES` 118, and `EPERCENT` 119 /
    -- `EPLUSPLUS` 120 / `EMINUSMINUS` 121 between `EMATCHES` 118 and `EMOD` 122. `Ground` is untouched
    -- and that is the design, not an omission: the protobuf's `g_big_int` is an **`Expr`** variant
    -- (`models/proto/RhoTypes.proto:174`, beside `g_bool`/`g_int`/`g_string`/`g_uri`/`g_byte_array`) and
    -- the port's normalizer *converts* the parse-level ground into one
    -- (`normalizer.rs:74-77`, `Ground::GroundBigInt -> Expr::GBigInt`), which is what `ebigint` mirrors.
    -- A `Ground.bigint` would instead force `exprTag (.ground _)` to inspect its payload, and that stuck
    -- `match` is the failure `Rchain/Sort.lean`'s boundary note and AUDIT C58 record as measured.
    | ebigint : Int → Expr
    -- `DECIMAL` 14, next to `BIG_INT` 13. The payload is the value in units of `10^-38`
    -- (`MAX_SCALE`), not the wire `(coefficient, scale)` pair. One argument is deliberate: the
    -- node stores the canonical pair, so every value has exactly one representation and `eq_iff`
    -- is plain `Int` equality. A two-field constructor would let `(10,1)` and `(1,0)` denote one
    -- value in two ways, and the comparator's equality law would then be false. The Rust
    -- `Decimal::scaled_value` produces the same integer (`models/src/decimal.rs`); Law 87
    -- (`spec/Rchain/Decimal.lean`) proves the bridge from the canonical `(coefficient, scale)`
    -- pair to this payload. Changing
    -- `MAX_SCALE` is a hard fork that must move this comment, `Sort.lean`'s tag, the Rust value
    -- and the sorter together.
    | edecimal : Int → Expr
    -- `EMETHOD`'s name is a **code-point list**, the model's convention for strings (`Ground.str`,
    -- `Ground.uri`), rather than a `String`: the node compares `leaf_str` byte-wise and Lean's `Ord
    -- String` would drag its `DecidableRel` instance argument into the comparator's *statements*, which
    -- no law of this file's shape can be stated over consistently. UTF-8 preserves code-point order, so
    -- the two orders agree. `Surface.lean`'s ``charsOf`` is the adapter.
    | emethod : List Nat → Par → List Par → Expr
    | epercentPercent : Par → Par → Expr
    | eplusPlus : Par → Par → Expr
    | eminusMinus : Par → Par → Expr

  /-- `Bundle` — body plus the read/write capability flags. -/
  inductive Bundle where
    | mk : Par → Bool → Bool → Bundle

  /-- `GUnforgeable` — fresh names from `new` and the system/identity tokens. -/
  inductive GUnforgeable where
    | gPrivate    : Nat → GUnforgeable
    | gDeployId   : Nat → GUnforgeable
    | gDeployerId : GUnforgeable
    | gSysAuthToken : GUnforgeable

  /-- `Connective` — pattern connectives (minimal set; expanded in Law 5). -/
  inductive Connective where
    | connAnd   : List Par → Connective
    | connOr    : List Par → Connective
    | connNot   : Par → Connective
    | connVarRef : Nat → Nat → Connective
end

/-- Accessors for `Par`'s 8 fields. -/
@[simp] def Par.sends        : Par → List Send         | Par.mk s _ _ _ _ _ _ _ => s
@[simp] def Par.receives     : Par → List Receive      | Par.mk _ r _ _ _ _ _ _ => r
@[simp] def Par.news         : Par → List New          | Par.mk _ _ n _ _ _ _ _ => n
@[simp] def Par.exprs        : Par → List Expr         | Par.mk _ _ _ e _ _ _ _ => e
@[simp] def Par.matches      : Par → List Match        | Par.mk _ _ _ _ m _ _ _ => m
@[simp] def Par.unforgeables : Par → List GUnforgeable | Par.mk _ _ _ _ _ u _ _ => u
@[simp] def Par.bundles      : Par → List Bundle       | Par.mk _ _ _ _ _ _ b _ => b
@[simp] def Par.connectives  : Par → List Connective   | Par.mk _ _ _ _ _ _ _ c => c

def Send.chan       : Send → Par      | Send.mk c _ _ => c
def Send.data       : Send → List Par | Send.mk _ d _ => d
def Send.persistent : Send → Bool     | Send.mk _ _ p => p

def Receive.binds      : Receive → List ReceiveBind | Receive.mk bs _ _ _ => bs
def Receive.body       : Receive → Par             | Receive.mk _ b _ _ => b
def Receive.persistent : Receive → Bool            | Receive.mk _ _ p _ => p
def Receive.bindCount  : Receive → Nat             | Receive.mk _ _ _ n => n

def ReceiveBind.patterns  : ReceiveBind → List Par | ReceiveBind.mk ps _ _ => ps
def ReceiveBind.source    : ReceiveBind → Par      | ReceiveBind.mk _ s _ => s
def ReceiveBind.freeCount : ReceiveBind → Nat      | ReceiveBind.mk _ _ n => n

def New.bindCount : New → Nat | New.mk n _ => n
def New.body      : New → Par | New.mk _ b => b

def MatchCase.pattern   : MatchCase → Par | MatchCase.mk p _ _ => p
def MatchCase.source    : MatchCase → Par | MatchCase.mk _ s _ => s
def MatchCase.freeCount : MatchCase → Nat | MatchCase.mk _ _ n => n

def Match.target : Match → Par            | Match.mk t _ => t
def Match.cases  : Match → List MatchCase | Match.mk _ cs => cs

def Bundle.body      : Bundle → Par  | Bundle.mk b _ _ => b
def Bundle.writeFlag : Bundle → Bool | Bundle.mk _ w _ => w
def Bundle.readFlag  : Bundle → Bool | Bundle.mk _ _ r => r

/-- The empty process `Nil` — the `Par` with all 8 fields empty. -/
def nilPar : Par :=
  Par.mk [] [] [] [] [] [] [] []

/-! ## Remainders, and the concreteness predicates the matcher gates on

The Rust caches `connective_used`/`locally_free` as fields on `Par` and consults the cache *before*
trying to match: `spatial_match`'s first line is `if !pattern.connective_used { pattern == target }`.
A cache that disagrees with the predicate is therefore not a slower match, it is a *different* one —
which is AUDIT C19/C20/C22’s whole failure mode ("silently never fires"). The law is that the cache
equals the predicate; here the predicate is the definition and the cache is the Rust's business. -/

/-- The remainder of a collection form (`..._` / `...rest`), when it has one. -/
def Expr.remainder : Expr → Option Var
  | .elist _ r => r
  | .eset _ r => r
  | .emap _ r => r
  -- The five added 2026-09-25 carry no remainder, named rather than left to the `_` below: a remainder
  -- is a property of the three collection forms alone (`rholang_mercury.cf:189-193`), so a new
  -- constructor reaching here is a decision, not an accident of the catch-all.
  | .ebigint _ => none
  | .edecimal _ => none
  | .emethod _ _ _ => none
  | .epercentPercent _ _ | .eplusPlus _ _ | .eminusMinus _ _ => none
  | _ => none

/-- A collection form carrying a remainder. In the grammar only `[…]`, `Set(…)` and `{…}` allow one
(`rholang_mercury.cf:189-193`), and a remainder is what makes such a pattern *partial*. -/
def Expr.hasRemainder (e : Expr) : Bool := e.remainder.isSome

/-- Whether a pattern variable is a *connective* in `HasLocallyFree`'s sense: a free var or a
wildcard. A bound var is a back-reference (`=x`), which the reducer substitutes away before matching
(`AUDIT C22`, the `Connective::VarRef` arm). -/
def Var.isConnective : Var → Bool
  | .free _ => true
  | .wildcard => true
  | .bound _ => false

mutual
  /-- `connectiveUsed p` — the predicate `Par.connective_used` caches: does this term contain a
  connective, a free variable, a wildcard, or a **collection remainder**? The remainder half is what
  C19/C20/C22 missed, one collection form at a time. -/
  def connectiveUsed : Par → Bool
    | .mk s r n e m u b c =>
      connectiveUsedListSend s || connectiveUsedListReceive r || connectiveUsedListNew n
      || connectiveUsedListExpr e || connectiveUsedListMatch m
      || connectiveUsedListGUnforgeable u || connectiveUsedListBundle b
      || connectiveUsedListConnective c

  def connectiveUsedListSend : List Send → Bool
    | [] => false
    | a :: as => connectiveUsedSend a || connectiveUsedListSend as

  def connectiveUsedSend : Send → Bool
    | .mk chan data _ => connectiveUsed chan || connectiveUsedListPar data

  def connectiveUsedListPar : List Par → Bool
    | [] => false
    | a :: as => connectiveUsed a || connectiveUsedListPar as

  def connectiveUsedListReceive : List Receive → Bool
    | [] => false
    | a :: as => connectiveUsedReceive a || connectiveUsedListReceive as

  def connectiveUsedReceive : Receive → Bool
    | .mk binds body _ _ => connectiveUsedListReceiveBind binds || connectiveUsed body

  def connectiveUsedListReceiveBind : List ReceiveBind → Bool
    | [] => false
    | a :: as => connectiveUsedReceiveBind a || connectiveUsedListReceiveBind as

  def connectiveUsedReceiveBind : ReceiveBind → Bool
    | .mk pats src _ => connectiveUsedListPar pats || connectiveUsed src

  def connectiveUsedListNew : List New → Bool
    | [] => false
    | a :: as => connectiveUsedNew a || connectiveUsedListNew as

  def connectiveUsedNew : New → Bool
    | .mk _ body => connectiveUsed body

  def connectiveUsedListMatch : List Match → Bool
    | [] => false
    | a :: as => connectiveUsedMatch a || connectiveUsedListMatch as

  def connectiveUsedMatch : Match → Bool
    | .mk target cases => connectiveUsed target || connectiveUsedListMatchCase cases

  def connectiveUsedListMatchCase : List MatchCase → Bool
    | [] => false
    | a :: as => connectiveUsedMatchCase a || connectiveUsedListMatchCase as

  def connectiveUsedMatchCase : MatchCase → Bool
    | .mk pat src _ => connectiveUsed pat || connectiveUsed src

  def connectiveUsedListExpr : List Expr → Bool
    | [] => false
    | a :: as => connectiveUsedExpr a || connectiveUsedListExpr as

  def connectiveUsedExpr : Expr → Bool
    | .ground _ => false
    | .ebigint _ => false
    | .edecimal _ => false
    | .evar v => v.isConnective
    | .eneg p | .enot p => connectiveUsed p
    | .eplus p q | .eminus p q | .emult p q | .ediv p q | .emod p q
    | .elt p q | .ele p q | .egt p q | .ege p q | .eeq p q | .eneq p q
    | .eand p q | .eor p q | .ematches p q | .eshortand p q | .eshortor p q
    | .epercentPercent p q | .eplusPlus p q | .eminusMinus p q =>
      connectiveUsed p || connectiveUsed q
    -- `EMETHOD` is the one the node computes its own way: `target.connective_used || args…`
    -- (`normalizer.rs:973-979`), copied verbatim here — and it is why the node's score carries a
    -- trailing `connective_used` leaf that this model does not (`Rchain/Sort.lean`'s note).
    | .emethod _ t args => connectiveUsed t || connectiveUsedListPar args
    | .elist ps r => connectiveUsedListPar ps || r.isSome
    | .etuple ps => connectiveUsedListPar ps
    | .eset ps r => connectiveUsedListPar ps || r.isSome
    | .emap kvs r => connectiveUsedListParPair kvs || r.isSome

  def connectiveUsedListParPair : List (Par × Par) → Bool
    | [] => false
    | (a, b) :: as => connectiveUsed a || connectiveUsed b || connectiveUsedListParPair as

  def connectiveUsedListBundle : List Bundle → Bool
    | [] => false
    | a :: as => connectiveUsedBundle a || connectiveUsedListBundle as

  def connectiveUsedBundle : Bundle → Bool
    | .mk body _ _ => connectiveUsed body

  def connectiveUsedListGUnforgeable : List GUnforgeable → Bool
    | [] => false
    | a :: as => connectiveUsedGUnforgeable a || connectiveUsedListGUnforgeable as

  def connectiveUsedGUnforgeable : GUnforgeable → Bool
    | _ => false

  def connectiveUsedListConnective : List Connective → Bool
    | [] => false
    | a :: as => connectiveUsedConnective a || connectiveUsedListConnective as

  def connectiveUsedConnective : Connective → Bool
    | .connAnd ps | .connOr ps => connectiveUsedListPar ps
    | .connNot p => connectiveUsed p
    | .connVarRef _ _ => true
end

/-- `parMerge p q` = `p | q` — field-wise multiset union (list append). -/
def parMerge (p q : Par) : Par :=
  Par.mk (p.sends ++ q.sends) (p.receives ++ q.receives) (p.news ++ q.news)
         (p.exprs ++ q.exprs) (p.matches ++ q.matches) (p.unforgeables ++ q.unforgeables)
         (p.bundles ++ q.bundles) (p.connectives ++ q.connectives)

end Rchain
