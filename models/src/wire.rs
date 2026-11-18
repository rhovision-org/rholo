//! Wire bridge between the hand-written rholang AST and the generated `RhoTypes.proto` types.
//!
//! This is the foundation of `Serialize[Par]` (Law 16 content addressing / rspace channel hashing).
//! The three custom scalapb `TypeMapper` encodings are reproduced bit-for-bit:
//! - `locallyFree` (a `scala.collection.immutable.BitSet`) → little-endian `Long` bit-mask with
//!   trailing zero bytes stripped.
//! - `g_big_int` (`scala.math.BigInt`) → signed big-endian two's-complement bytes.
//! - `random_state` (`Blake2b512Random`) → its 128-byte state.

use num_bigint::BigInt;
use prost::Message as _;
use rchain_crypto::hash::blake2b512_random::{Blake2b512Random, SerializedRandom};
use rchain_shared::serialize::Serialize;

use crate::ast as a;
use crate::errors::ModelsError;
use crate::proto::rholang as p;
use crate::runtime::{BindPattern, ListParWithRandom, ParWithRandom, TaggedContinuation};
use crate::sorted::SortedProc;
use crate::types::FreeCount;

// --- TypeMapper encodings -----------------------------------------------------------------------

/// Serialize a `BitSet` (`Vec<i32>` of set positions) to bytes (port of `bitSetToByteString`).
pub fn bitset_to_bytes(bitset: &[i32]) -> Vec<u8> {
    let max = bitset.iter().map(|e| *e as usize).max().unwrap_or(0);
    let num_words = if max == 0 && bitset.is_empty() {
        0
    } else {
        max / 64 + 1
    };
    let mut words = vec![0u64; num_words];
    for &e in bitset {
        let e = e as usize;
        words[e / 64] |= 1u64 << (e % 64);
    }
    let mut bytes = Vec::with_capacity(num_words * 8);
    for w in &words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    bytes
}

/// Deserialize bytes to a `BitSet` (port of `byteStringToBitSet`).
pub fn bytes_to_bitset(bytes: &[u8]) -> Vec<i32> {
    let buffer_size = (bytes.len() + 7) / 8 * 8;
    let mut padded = vec![0u8; buffer_size];
    padded[..bytes.len()].copy_from_slice(bytes);
    let mut out = Vec::new();
    for (word_idx, chunk) in padded.chunks(8).enumerate() {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(chunk);
        let word = u64::from_le_bytes(arr);
        for bit in 0..64 {
            if (word >> bit) & 1 == 1 {
                out.push((word_idx * 64 + bit) as i32);
            }
        }
    }
    out
}

// --- Var ----------------------------------------------------------------------------------------

pub fn var_to_proto(v: &a::Var) -> p::Var {
    let var_instance = match v {
        a::Var::BoundVar(i) => p::var::VarInstance::BoundVar(*i),
        a::Var::FreeVar(i) => p::var::VarInstance::FreeVar(*i),
        a::Var::Wildcard => p::var::VarInstance::Wildcard(p::var::WildcardMsg {}),
        a::Var::Empty => return p::Var { var_instance: None },
    };
    p::Var {
        var_instance: Some(var_instance),
    }
}

pub fn var_from_proto(p: &p::Var) -> a::Var {
    match &p.var_instance {
        Some(p::var::VarInstance::BoundVar(i)) => a::Var::BoundVar(*i),
        Some(p::var::VarInstance::FreeVar(i)) => a::Var::FreeVar(*i),
        Some(p::var::VarInstance::Wildcard(_)) => a::Var::Wildcard,
        None => a::Var::Empty,
    }
}

// --- Connective ---------------------------------------------------------------------------------

pub fn connective_to_proto(c: &a::Connective) -> p::Connective {
    let inst = match c {
        a::Connective::ConnAnd(b) => {
            p::connective::ConnectiveInstance::ConnAndBody(connective_body_to_proto(b))
        }
        a::Connective::ConnOr(b) => {
            p::connective::ConnectiveInstance::ConnOrBody(connective_body_to_proto(b))
        }
        a::Connective::ConnNot(p_) => {
            p::connective::ConnectiveInstance::ConnNotBody(par_to_proto(p_))
        }
        a::Connective::VarRef(v) => p::connective::ConnectiveInstance::VarRefBody(p::VarRef {
            index: v.index,
            depth: v.depth,
        }),
        a::Connective::ConnBool(b) => p::connective::ConnectiveInstance::ConnBool(*b),
        a::Connective::ConnInt(b) => p::connective::ConnectiveInstance::ConnInt(*b),
        a::Connective::ConnBigInt(b) => p::connective::ConnectiveInstance::ConnBigInt(*b),
        a::Connective::ConnString(b) => p::connective::ConnectiveInstance::ConnString(*b),
        a::Connective::ConnUri(b) => p::connective::ConnectiveInstance::ConnUri(*b),
        a::Connective::ConnByteArray(b) => p::connective::ConnectiveInstance::ConnByteArray(*b),
        a::Connective::Empty => {
            return p::Connective {
                connective_instance: None,
            }
        }
    };
    p::Connective {
        connective_instance: Some(inst),
    }
}

fn connective_body_to_proto(b: &a::ConnectiveBody) -> p::ConnectiveBody {
    p::ConnectiveBody {
        ps: b.ps.iter().map(par_to_proto).collect(),
    }
}

pub fn connective_from_proto(p: &p::Connective) -> Result<a::Connective, ModelsError> {
    Ok(match &p.connective_instance {
        Some(p::connective::ConnectiveInstance::ConnAndBody(b)) => {
            a::Connective::ConnAnd(a::ConnectiveBody {
                ps: b
                    .ps
                    .iter()
                    .map(par_from_proto)
                    .collect::<Result<Vec<_>, ModelsError>>()?,
            })
        }
        Some(p::connective::ConnectiveInstance::ConnOrBody(b)) => {
            a::Connective::ConnOr(a::ConnectiveBody {
                ps: b
                    .ps
                    .iter()
                    .map(par_from_proto)
                    .collect::<Result<Vec<_>, ModelsError>>()?,
            })
        }
        Some(p::connective::ConnectiveInstance::ConnNotBody(p_)) => {
            a::Connective::ConnNot(Box::new(par_from_proto(p_)?))
        }
        Some(p::connective::ConnectiveInstance::VarRefBody(v)) => {
            a::Connective::VarRef(a::VarRef {
                index: v.index,
                depth: v.depth,
            })
        }
        Some(p::connective::ConnectiveInstance::ConnBool(b)) => a::Connective::ConnBool(*b),
        Some(p::connective::ConnectiveInstance::ConnInt(b)) => a::Connective::ConnInt(*b),
        Some(p::connective::ConnectiveInstance::ConnBigInt(b)) => a::Connective::ConnBigInt(*b),
        Some(p::connective::ConnectiveInstance::ConnString(b)) => a::Connective::ConnString(*b),
        Some(p::connective::ConnectiveInstance::ConnUri(b)) => a::Connective::ConnUri(*b),
        Some(p::connective::ConnectiveInstance::ConnByteArray(b)) => {
            a::Connective::ConnByteArray(*b)
        }
        None => a::Connective::Empty,
    })
}

// --- GUnforgeable -------------------------------------------------------------------------------

pub fn unforgeable_to_proto(u: &a::GUnforgeable) -> p::GUnforgeable {
    let inst = match u {
        a::GUnforgeable::GPrivate(g) => {
            p::g_unforgeable::UnfInstance::GPrivateBody(p::GPrivate { id: g.id.clone() })
        }
        a::GUnforgeable::GDeployId(g) => {
            p::g_unforgeable::UnfInstance::GDeployIdBody(p::GDeployId { sig: g.sig.clone() })
        }
        a::GUnforgeable::GDeployerId(g) => {
            p::g_unforgeable::UnfInstance::GDeployerIdBody(p::GDeployerId {
                public_key: g.public_key.clone(),
            })
        }
        a::GUnforgeable::GSysAuthToken => {
            p::g_unforgeable::UnfInstance::GSysAuthTokenBody(p::GSysAuthToken {})
        }
        a::GUnforgeable::Empty => return p::GUnforgeable { unf_instance: None },
    };
    p::GUnforgeable {
        unf_instance: Some(inst),
    }
}

pub fn unforgeable_from_proto(p: &p::GUnforgeable) -> Result<a::GUnforgeable, ModelsError> {
    Ok(match &p.unf_instance {
        Some(p::g_unforgeable::UnfInstance::GPrivateBody(g)) => {
            a::GUnforgeable::GPrivate(a::GPrivate { id: g.id.clone() })
        }
        Some(p::g_unforgeable::UnfInstance::GDeployIdBody(g)) => {
            a::GUnforgeable::GDeployId(a::GDeployId { sig: g.sig.clone() })
        }
        Some(p::g_unforgeable::UnfInstance::GDeployerIdBody(g)) => {
            a::GUnforgeable::GDeployerId(a::GDeployerId {
                public_key: g.public_key.clone(),
            })
        }
        Some(p::g_unforgeable::UnfInstance::GSysAuthTokenBody(_)) => a::GUnforgeable::GSysAuthToken,
        None => a::GUnforgeable::Empty,
    })
}

// --- Expr ---------------------------------------------------------------------------------------

pub fn expr_to_proto(e: &a::Expr) -> p::Expr {
    use p::expr::ExprInstance as E;
    let inst = match e {
        a::Expr::GBool(b) => E::GBool(*b),
        a::Expr::GInt(i) => E::GInt(*i),
        a::Expr::GBigInt(bi) => E::GBigInt(bi.to_signed_bytes_be()),
        // **`g_decimal` (tag 35) was here and is retired** (issue #527): the proto field is
        // `reserved 35`, so an encode arm cannot name it. A peer that still sends tag 35 decodes to
        // no `expr_instance` and gets the `Malformed("expr_instance")` refusal below.
        a::Expr::GString(s) => E::GString(s.clone()),
        a::Expr::GUri(u) => E::GUri(u.clone()),
        a::Expr::GByteArray(b) => E::GByteArray(b.clone()),
        a::Expr::ENot(p_) => E::ENotBody(p::ENot {
            p: Some(par_to_proto(p_)),
        }),
        a::Expr::ENeg(p_) => E::ENegBody(p::ENeg {
            p: Some(par_to_proto(p_)),
        }),
        a::Expr::EMult(p1, p2) => E::EMultBody(p::EMult {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EDiv(p1, p2) => E::EDivBody(p::EDiv {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EMod(p1, p2) => E::EModBody(p::EMod {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EPlus(p1, p2) => E::EPlusBody(p::EPlus {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EMinus(p1, p2) => E::EMinusBody(p::EMinus {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::ELt(p1, p2) => E::ELtBody(p::ELt {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::ELte(p1, p2) => E::ELteBody(p::ELte {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EGt(p1, p2) => E::EGtBody(p::EGt {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EGte(p1, p2) => E::EGteBody(p::EGte {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EEq(p1, p2) => E::EEqBody(p::EEq {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::ENeq(p1, p2) => E::ENeqBody(p::ENeq {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EAnd(p1, p2) => E::EAndBody(p::EAnd {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EOr(p1, p2) => E::EOrBody(p::EOr {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EShortAnd(p1, p2) => E::EShortAndBody(p::EShortAnd {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EShortOr(p1, p2) => E::EShortOrBody(p::EShortOr {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EMatches(t, pat) => E::EMatchesBody(p::EMatches {
            target: Some(par_to_proto(t)),
            pattern: Some(par_to_proto(pat)),
        }),
        a::Expr::EPercentPercent(p1, p2) => E::EPercentPercentBody(p::EPercentPercent {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EPlusPlus(p1, p2) => E::EPlusPlusBody(p::EPlusPlus {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EMinusMinus(p1, p2) => E::EMinusMinusBody(p::EMinusMinus {
            p1: Some(par_to_proto(p1)),
            p2: Some(par_to_proto(p2)),
        }),
        a::Expr::EVar(v) => E::EVarBody(p::EVar {
            v: Some(var_to_proto(v)),
        }),
        a::Expr::EList(el) => E::EListBody(elist_to_proto(el)),
        a::Expr::ETuple(et) => E::ETupleBody(etuple_to_proto(et)),
        a::Expr::ESet(es) => E::ESetBody(eset_to_proto(es)),
        a::Expr::EMap(em) => E::EMapBody(emap_to_proto(em)),
        a::Expr::EMethod(em) => E::EMethodBody(emethod_to_proto(em)),
    };
    p::Expr {
        expr_instance: Some(inst),
    }
}

pub fn expr_from_proto(p: &p::Expr) -> Result<a::Expr, ModelsError> {
    use p::expr::ExprInstance as E;
    Ok(match &p.expr_instance {
        Some(E::GBool(b)) => a::Expr::GBool(*b),
        Some(E::GInt(i)) => a::Expr::GInt(*i),
        Some(E::GBigInt(b)) => a::Expr::GBigInt(BigInt::from_signed_bytes_be(b)),
        Some(E::GString(s)) => a::Expr::GString(s.clone()),
        Some(E::GUri(u)) => a::Expr::GUri(u.clone()),
        Some(E::GByteArray(b)) => a::Expr::GByteArray(b.clone()),
        Some(E::ENotBody(m)) => a::Expr::ENot(Box::new(par_from_proto(
            m.p.as_ref().ok_or(ModelsError::Malformed("p"))?,
        )?)),
        Some(E::ENegBody(m)) => a::Expr::ENeg(Box::new(par_from_proto(
            m.p.as_ref().ok_or(ModelsError::Malformed("p"))?,
        )?)),
        Some(E::EMultBody(m)) => a::Expr::EMult(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EDivBody(m)) => a::Expr::EDiv(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EModBody(m)) => a::Expr::EMod(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EPlusBody(m)) => a::Expr::EPlus(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EMinusBody(m)) => a::Expr::EMinus(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::ELtBody(m)) => a::Expr::ELt(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::ELteBody(m)) => a::Expr::ELte(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EGtBody(m)) => a::Expr::EGt(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EGteBody(m)) => a::Expr::EGte(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EEqBody(m)) => a::Expr::EEq(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::ENeqBody(m)) => a::Expr::ENeq(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EAndBody(m)) => a::Expr::EAnd(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EOrBody(m)) => a::Expr::EOr(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EShortAndBody(m)) => a::Expr::EShortAnd(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EShortOrBody(m)) => a::Expr::EShortOr(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EMatchesBody(m)) => a::Expr::EMatches(
            Box::new(par_from_proto(
                m.target.as_ref().ok_or(ModelsError::Malformed("target"))?,
            )?),
            Box::new(par_from_proto(
                m.pattern
                    .as_ref()
                    .ok_or(ModelsError::Malformed("pattern"))?,
            )?),
        ),
        Some(E::EPercentPercentBody(m)) => a::Expr::EPercentPercent(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EPlusPlusBody(m)) => a::Expr::EPlusPlus(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EMinusMinusBody(m)) => a::Expr::EMinusMinus(
            Box::new(par_from_proto(
                m.p1.as_ref().ok_or(ModelsError::Malformed("p1"))?,
            )?),
            Box::new(par_from_proto(
                m.p2.as_ref().ok_or(ModelsError::Malformed("p2"))?,
            )?),
        ),
        Some(E::EVarBody(m)) => a::Expr::EVar(Box::new(var_from_proto(
            m.v.as_ref().ok_or(ModelsError::Malformed("v"))?,
        ))),
        Some(E::EListBody(m)) => a::Expr::EList(elist_from_proto(m)?),
        Some(E::ETupleBody(m)) => a::Expr::ETuple(etuple_from_proto(m)?),
        Some(E::ESetBody(m)) => a::Expr::ESet(eset_from_proto(m)?),
        Some(E::EMapBody(m)) => a::Expr::EMap(emap_from_proto(m)?),
        Some(E::EMethodBody(m)) => a::Expr::EMethod(emethod_from_proto(m)?),
        // Missing and unknown-only oneof values are not the boolean false.
        None => return Err(ModelsError::Malformed("expr_instance")),
    })
}

// --- Collection helpers -------------------------------------------------------------------------

fn elist_to_proto(el: &a::EList) -> p::EList {
    p::EList {
        ps: el.ps.iter().map(par_to_proto).collect(),
        locally_free: bitset_to_bytes(&el.locally_free.0),
        connective_used: el.connective_used,
        remainder: el.remainder.as_deref().map(var_to_proto),
    }
}
fn elist_from_proto(p: &p::EList) -> Result<a::EList, ModelsError> {
    Ok(a::EList {
        ps: p
            .ps
            .iter()
            .map(par_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
        remainder: p.remainder.as_ref().map(var_from_proto).map(Box::new),
    })
}

fn etuple_to_proto(et: &a::ETuple) -> p::ETuple {
    p::ETuple {
        ps: et.ps.iter().map(par_to_proto).collect(),
        locally_free: bitset_to_bytes(&et.locally_free.0),
        connective_used: et.connective_used,
    }
}
fn etuple_from_proto(p: &p::ETuple) -> Result<a::ETuple, ModelsError> {
    Ok(a::ETuple {
        ps: p
            .ps
            .iter()
            .map(par_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
    })
}

fn eset_to_proto(es: &a::ParSet) -> p::ESet {
    p::ESet {
        ps: es.ps.iter().map(par_to_proto).collect(),
        locally_free: bitset_to_bytes(&es.locally_free.0),
        connective_used: es.connective_used,
        remainder: es.remainder.as_deref().map(var_to_proto),
    }
}
fn eset_from_proto(p: &p::ESet) -> Result<a::ParSet, ModelsError> {
    Ok(a::ParSet {
        ps: p
            .ps
            .iter()
            .map(par_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
        remainder: p.remainder.as_ref().map(var_from_proto).map(Box::new),
    })
}

fn emap_to_proto(em: &a::ParMap) -> p::EMap {
    p::EMap {
        kvs: em
            .kvs
            .iter()
            .map(|(k, v)| p::KeyValuePair {
                key: Some(par_to_proto(k)),
                value: Some(par_to_proto(v)),
            })
            .collect(),
        locally_free: bitset_to_bytes(&em.locally_free.0),
        connective_used: em.connective_used,
        remainder: em.remainder.as_deref().map(var_to_proto),
    }
}
fn emap_from_proto(p: &p::EMap) -> Result<a::ParMap, ModelsError> {
    Ok(a::ParMap {
        kvs: p
            .kvs
            .iter()
            .map(|kv| {
                Ok((
                    par_from_proto(kv.key.as_ref().ok_or(ModelsError::Malformed("key"))?)?,
                    par_from_proto(kv.value.as_ref().ok_or(ModelsError::Malformed("value"))?)?,
                ))
            })
            .collect::<Result<Vec<_>, ModelsError>>()?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
        remainder: p.remainder.as_ref().map(var_from_proto).map(Box::new),
    })
}

fn emethod_to_proto(em: &a::EMethod) -> p::EMethod {
    p::EMethod {
        method_name: em.method_name.clone(),
        target: Some(par_to_proto(&em.target)),
        arguments: em.arguments.iter().map(par_to_proto).collect(),
        locally_free: bitset_to_bytes(&em.locally_free.0),
        connective_used: em.connective_used,
    }
}
fn emethod_from_proto(p: &p::EMethod) -> Result<a::EMethod, ModelsError> {
    Ok(a::EMethod {
        method_name: p.method_name.clone(),
        target: Box::new(par_from_proto(
            p.target.as_ref().ok_or(ModelsError::Malformed("target"))?,
        )?),
        arguments: p
            .arguments
            .iter()
            .map(|x| par_from_proto(x))
            .collect::<Result<Vec<_>, ModelsError>>()?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
    })
}

// --- Terms --------------------------------------------------------------------------------------

pub fn send_to_proto(s: &a::Send) -> p::Send {
    p::Send {
        chan: Some(par_to_proto(s.chan.as_ref())),
        data: s.data.iter().map(|d| par_to_proto(d)).collect(),
        persistent: s.persistent,
        locally_free: bitset_to_bytes(&s.locally_free.0),
        connective_used: s.connective_used,
    }
}
pub fn send_from_proto(p: &p::Send) -> Result<a::Send, ModelsError> {
    Ok(a::Send {
        chan: Box::new(par_from_proto::<a::NameSort>(
            p.chan.as_ref().ok_or(ModelsError::Malformed("chan"))?,
        )?),
        data: p
            .data
            .iter()
            .map(|d| par_from_proto::<a::NameSort>(d))
            .collect::<Result<Vec<_>, ModelsError>>()?,
        persistent: p.persistent,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
    })
}

pub fn receive_to_proto(r: &a::Receive) -> p::Receive {
    p::Receive {
        binds: r.binds.iter().map(receive_bind_to_proto).collect(),
        body: Some(par_to_proto(r.body.as_ref())),
        persistent: r.persistent,
        peek: r.peek,
        bind_count: i32::from(r.bind_count),
        locally_free: bitset_to_bytes(&r.locally_free.0),
        connective_used: r.connective_used,
    }
}
pub fn receive_from_proto(p: &p::Receive) -> Result<a::Receive, ModelsError> {
    Ok(a::Receive {
        binds: p
            .binds
            .iter()
            .map(receive_bind_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        body: Box::new(par_from_proto(
            p.body.as_ref().ok_or(ModelsError::Malformed("body"))?,
        )?),
        persistent: p.persistent,
        peek: p.peek,
        // Refused here when negative, like the `free_count` of `ReceiveBind`/`MatchCase` — the same
        // field family, the same boundary (U1; the carrier is `FreeCount`, so the term cannot hold a
        // negative count and `well_scoped_par`'s `depth + bind_count` needs no clamp).
        bind_count: FreeCount::try_from(p.bind_count).map_err(ModelsError::Decode)?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
    })
}

pub fn receive_bind_to_proto(rb: &a::ReceiveBind) -> p::ReceiveBind {
    p::ReceiveBind {
        patterns: rb.patterns.iter().map(|p| par_to_proto(p)).collect(),
        source: Some(par_to_proto(rb.source.as_ref())),
        remainder: rb.remainder.as_deref().map(var_to_proto),
        free_count: i32::from(rb.free_count),
    }
}
pub fn receive_bind_from_proto(p: &p::ReceiveBind) -> Result<a::ReceiveBind, ModelsError> {
    Ok(a::ReceiveBind {
        patterns: p
            .patterns
            .iter()
            .map(|p| par_from_proto(p))
            .collect::<Result<Vec<_>, ModelsError>>()?,
        source: Box::new(par_from_proto::<a::NameSort>(
            p.source.as_ref().ok_or(ModelsError::Malformed("source"))?,
        )?),
        remainder: p.remainder.as_ref().map(var_from_proto).map(Box::new),
        free_count: FreeCount::try_from(p.free_count).map_err(ModelsError::Decode)?,
    })
}

pub fn new_to_proto(n: &a::New) -> p::New {
    p::New {
        bind_count: i32::from(n.bind_count),
        p: Some(par_to_proto(n.p.as_ref())),
        uri: n.uri.clone(),
        injections: n
            .injections
            .iter()
            .map(|(k, v)| (k.clone(), par_to_proto(v)))
            .collect(),
        locally_free: bitset_to_bytes(&n.locally_free.0),
    }
}
pub fn new_from_proto(p: &p::New) -> Result<a::New, ModelsError> {
    Ok(a::New {
        bind_count: FreeCount::try_from(p.bind_count).map_err(ModelsError::Decode)?,
        p: Box::new(par_from_proto(
            p.p.as_ref().ok_or(ModelsError::Malformed("p"))?,
        )?),
        uri: p.uri.clone(),
        injections: p
            .injections
            .iter()
            .map(|(k, v)| Ok((k.clone(), par_from_proto(v)?)))
            .collect::<Result<std::collections::BTreeMap<_, _>, ModelsError>>()?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
    })
}

pub fn match_to_proto(m: &a::Match) -> p::Match {
    p::Match {
        target: Some(par_to_proto(m.target.as_ref())),
        cases: m.cases.iter().map(match_case_to_proto).collect(),
        locally_free: bitset_to_bytes(&m.locally_free.0),
        connective_used: m.connective_used,
    }
}
pub fn match_from_proto(p: &p::Match) -> Result<a::Match, ModelsError> {
    Ok(a::Match {
        target: Box::new(par_from_proto::<a::NameSort>(
            p.target.as_ref().ok_or(ModelsError::Malformed("target"))?,
        )?),
        cases: p
            .cases
            .iter()
            .map(match_case_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
    })
}

pub fn match_case_to_proto(mc: &a::MatchCase) -> p::MatchCase {
    p::MatchCase {
        pattern: Some(par_to_proto(mc.pattern.as_ref())),
        source: Some(par_to_proto(mc.source.as_ref())),
        free_count: i32::from(mc.free_count),
    }
}
pub fn match_case_from_proto(p: &p::MatchCase) -> Result<a::MatchCase, ModelsError> {
    Ok(a::MatchCase {
        pattern: Box::new(par_from_proto::<a::NameSort>(
            p.pattern
                .as_ref()
                .ok_or(ModelsError::Malformed("pattern"))?,
        )?),
        source: Box::new(par_from_proto(
            p.source.as_ref().ok_or(ModelsError::Malformed("source"))?,
        )?),
        free_count: FreeCount::try_from(p.free_count).map_err(ModelsError::Decode)?,
    })
}

pub fn bundle_to_proto(b: &a::Bundle) -> p::Bundle {
    p::Bundle {
        body: Some(par_to_proto(&b.body)),
        write_flag: b.write_flag,
        read_flag: b.read_flag,
    }
}
pub fn bundle_from_proto(p: &p::Bundle) -> Result<a::Bundle, ModelsError> {
    Ok(a::Bundle {
        body: Box::new(par_from_proto(
            p.body.as_ref().ok_or(ModelsError::Malformed("body"))?,
        )?),
        write_flag: p.write_flag,
        read_flag: p.read_flag,
    })
}

pub fn par_to_proto<S: a::Sort>(par: &a::Par<S>) -> p::Par {
    p::Par {
        sends: par.sends.iter().map(send_to_proto).collect(),
        receives: par.receives.iter().map(receive_to_proto).collect(),
        news: par.news.iter().map(new_to_proto).collect(),
        exprs: par.exprs.iter().map(expr_to_proto).collect(),
        matches: par.matches.iter().map(match_to_proto).collect(),
        unforgeables: par.unforgeables.iter().map(unforgeable_to_proto).collect(),
        bundles: par.bundles.iter().map(bundle_to_proto).collect(),
        connectives: par.connectives.iter().map(connective_to_proto).collect(),
        locally_free: bitset_to_bytes(&par.locally_free.0),
        connective_used: par.connective_used,
    }
}

pub fn par_from_proto<S: a::Sort>(p: &p::Par) -> Result<a::Par<S>, ModelsError> {
    Ok(a::Par {
        sends: p
            .sends
            .iter()
            .map(send_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        receives: p
            .receives
            .iter()
            .map(receive_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        news: p
            .news
            .iter()
            .map(new_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        exprs: p
            .exprs
            .iter()
            .map(expr_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        matches: p
            .matches
            .iter()
            .map(match_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        unforgeables: p
            .unforgeables
            .iter()
            .map(unforgeable_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        bundles: p
            .bundles
            .iter()
            .map(bundle_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        connectives: p
            .connectives
            .iter()
            .map(connective_from_proto)
            .collect::<Result<Vec<_>, ModelsError>>()?,
        locally_free: a::AlwaysEqual(bytes_to_bitset(&p.locally_free)),
        connective_used: p.connective_used,
        ..Default::default()
    })
}

// --- Runtime types ------------------------------------------------------------------------------

pub fn bind_pattern_to_proto(bp: &BindPattern) -> p::BindPattern {
    p::BindPattern {
        patterns: bp
            .patterns
            .iter()
            .map(|p| par_to_proto(p.as_par()))
            .collect(),
        remainder: bp.remainder.as_ref().map(var_to_proto),
        free_count: bp.free_count,
    }
}
pub fn bind_pattern_from_proto(p: &p::BindPattern) -> Result<BindPattern, ModelsError> {
    Ok(BindPattern {
        patterns: p
            .patterns
            .iter()
            .map(|p| par_from_proto(p).map(SortedProc::new))
            .collect::<Result<Vec<_>, ModelsError>>()?,
        remainder: p.remainder.as_ref().map(var_from_proto),
        // Validated at the boundary, as `ReceiveBind` (`:697`) and `MatchCase` (`:770`) already do —
        // this decoder was the odd one out on the same path. A negative count is not merely
        // malformed: `RhoMatch::get` fills `0..pattern.free_count` from the match's free map, so a
        // negative count silently applies the continuation with *no* bound values, and a count larger
        // than the pattern's free variables pads the extra bindings with `Nil`. Both are a peer's
        // message changing execution without an error, which is the silent partiality the port's type
        // discipline exists to refuse.
        free_count: i32::from(FreeCount::try_from(p.free_count).map_err(ModelsError::Decode)?),
    })
}

pub fn list_par_with_random_to_proto(l: &ListParWithRandom) -> p::ListParWithRandom {
    p::ListParWithRandom {
        pars: l.pars.iter().map(|p| par_to_proto(p.as_par())).collect(),
        random_state: l.random_state.to_bytes(),
    }
}
pub fn list_par_with_random_from_proto(
    p: &p::ListParWithRandom,
) -> Result<ListParWithRandom, String> {
    Ok(ListParWithRandom {
        pars: p
            .pars
            .iter()
            .map(|p| par_from_proto(p).map(SortedProc::new))
            .collect::<Result<Vec<_>, ModelsError>>()
            .map_err(|e| e.to_string())?,
        random_state: Blake2b512Random::from_bytes(
            &SerializedRandom::try_from(p.random_state.as_slice()).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?,
    })
}

pub fn par_with_random_to_proto(pw: &ParWithRandom) -> p::ParWithRandom {
    p::ParWithRandom {
        body: Some(par_to_proto(pw.body.as_par())),
        random_state: pw.random_state.to_bytes(),
    }
}
pub fn par_with_random_from_proto(p: &p::ParWithRandom) -> Result<ParWithRandom, String> {
    Ok(ParWithRandom {
        body: SortedProc::new(
            par_from_proto(
                p.body
                    .as_ref()
                    .ok_or(ModelsError::Malformed("body"))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?,
        ),
        random_state: Blake2b512Random::from_bytes(
            &SerializedRandom::try_from(p.random_state.as_slice()).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?,
    })
}

pub fn tagged_continuation_to_proto(tc: &TaggedContinuation) -> p::TaggedContinuation {
    let tagged_cont = match tc {
        TaggedContinuation::ParBody(pw) => {
            p::tagged_continuation::TaggedCont::ParBody(par_with_random_to_proto(pw))
        }
        TaggedContinuation::ScalaBodyRef(r) => p::tagged_continuation::TaggedCont::ScalaBodyRef(*r),
        TaggedContinuation::Empty => return p::TaggedContinuation { tagged_cont: None },
    };
    p::TaggedContinuation {
        tagged_cont: Some(tagged_cont),
    }
}
pub fn tagged_continuation_from_proto(
    p: &p::TaggedContinuation,
) -> Result<TaggedContinuation, String> {
    match &p.tagged_cont {
        Some(p::tagged_continuation::TaggedCont::ParBody(pw)) => {
            Ok(TaggedContinuation::ParBody(par_with_random_from_proto(pw)?))
        }
        Some(p::tagged_continuation::TaggedCont::ScalaBodyRef(r)) => {
            Ok(TaggedContinuation::ScalaBodyRef(*r))
        }
        None => Ok(TaggedContinuation::Empty),
    }
}

// --- Serialize instances ------------------------------------------------------------------------

impl<S: a::Sort> Serialize<a::Par<S>> for a::Par<S> {
    fn encode(a: &a::Par<S>) -> Vec<u8> {
        par_to_proto(a).encode_to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<a::Par<S>, String> {
        let proto = <p::Par as ::prost::Message>::decode(bytes).map_err(|e| e.to_string())?;
        par_from_proto(&proto).map_err(|e| e.to_string())
    }
}

impl Serialize<BindPattern> for BindPattern {
    fn encode(a: &BindPattern) -> Vec<u8> {
        bind_pattern_to_proto(a).encode_to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<BindPattern, String> {
        let proto =
            <p::BindPattern as ::prost::Message>::decode(bytes).map_err(|e| e.to_string())?;
        bind_pattern_from_proto(&proto).map_err(|e| e.to_string())
    }
}

impl Serialize<ListParWithRandom> for ListParWithRandom {
    fn encode(a: &ListParWithRandom) -> Vec<u8> {
        list_par_with_random_to_proto(a).encode_to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<ListParWithRandom, String> {
        let proto =
            <p::ListParWithRandom as ::prost::Message>::decode(bytes).map_err(|e| e.to_string())?;
        list_par_with_random_from_proto(&proto)
    }
}

impl Serialize<TaggedContinuation> for TaggedContinuation {
    fn encode(a: &TaggedContinuation) -> Vec<u8> {
        tagged_continuation_to_proto(a).encode_to_vec()
    }
    fn decode(bytes: &[u8]) -> Result<TaggedContinuation, String> {
        let proto = <p::TaggedContinuation as ::prost::Message>::decode(bytes)
            .map_err(|e| e.to_string())?;
        tagged_continuation_from_proto(&proto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitset_round_trips() {
        let bits = vec![0, 1, 65, 130];
        let bytes = bitset_to_bytes(&bits);
        assert_eq!(bytes_to_bitset(&bytes), bits);
        assert!(bitset_to_bytes(&[]).is_empty());
        assert_eq!(bytes_to_bitset(&[]), Vec::<i32>::new());
    }

    #[test]
    fn par_serialize_round_trips() {
        let par = a::Par {
            exprs: vec![a::Expr::EPlus(
                Box::new(a::Par {
                    exprs: vec![a::Expr::GInt(2)],
                    ..Default::default()
                }),
                Box::new(a::Par {
                    exprs: vec![a::Expr::GInt(3)],
                    ..Default::default()
                }),
            )],
            locally_free: a::AlwaysEqual(vec![1, 7]),
            ..Default::default()
        };
        let bytes = <a::Par as Serialize<a::Par>>::encode(&par);
        let decoded = <a::Par as Serialize<a::Par>>::decode(&bytes).unwrap();
        assert_eq!(decoded, par);
    }
}

/// Differential tests against the Scala scalapb `TypeMapper` encodings and `Serialize[Par]` wire
/// bytes. Golden vectors are captured in `testdata/differential/wire.tsv`. The custom `locallyFree`
/// `BitSet` encoding (little-endian `Long` mask with trailing zeros stripped) is the Scala-specific
/// behavior that must be reproduced byte-for-byte.
#[cfg(test)]
mod differential {
    use super::*;
    use rchain_shared::base16;

    /// **The three codecs' *decode* halves.** `par_serialize_round_trips` covers `Par`; the
    /// `Serialize` impls for `BindPattern`, `ListParWithRandom` and `TaggedContinuation` were exercised
    /// only in their encoding direction, so every `decode` arm was unexecuted. A codec whose decode is
    /// untested is the one place a wire change stays silent: the encoder is what the tests check and
    /// the decoder is what the node *runs*.
    ///
    /// `TaggedContinuation` is asserted tag by tag, because the tag is the whole point of the type —
    /// the store's dispatch sends a `ParBody` to the matcher and a `ScalaBodyRef` to the native
    /// executor, so a decoder that collapsed the arms would answer the wrong one. Its value carries a
    /// random generator, which is not `PartialEq`, so each arm's fields are compared rather than the
    /// values (and the comparison is what shows the tag survived).
    #[test]
    fn the_codec_round_trips_are_checked_in_both_directions() {
        use crate::ast::Expr;
        use crate::par_ops::from_expr;
        use crate::runtime::{ParWithRandom, TaggedContinuation};
        use rchain_crypto::hash::blake2b512_random::Blake2b512Random;

        let sorted = || SortedProc::new(from_expr(Expr::GInt(1)));

        let pattern = BindPattern {
            patterns: vec![sorted(), sorted()],
            remainder: None,
            free_count: 2,
        };
        let back = <BindPattern as Serialize<BindPattern>>::decode(&<BindPattern as Serialize<
            BindPattern,
        >>::encode(&pattern))
        .expect("decode");
        assert_eq!(back, pattern, "a bind pattern survives its own codec");

        let list = ListParWithRandom {
            pars: vec![sorted()],
            random_state: Blake2b512Random::from_init(&[3u8; 32]),
        };
        let back = <ListParWithRandom as Serialize<ListParWithRandom>>::decode(
            &<ListParWithRandom as Serialize<ListParWithRandom>>::encode(&list),
        )
        .expect("decode");
        assert_eq!(back.pars.len(), 1, "the pars come back");
        assert_eq!(
            back.random_state.to_bytes(),
            list.random_state.to_bytes(),
            "and so does the random state, byte for byte"
        );

        for case in [
            TaggedContinuation::ParBody(ParWithRandom {
                body: sorted(),
                random_state: Blake2b512Random::from_init(&[4u8; 32]),
            }),
            TaggedContinuation::ScalaBodyRef(7),
            TaggedContinuation::Empty,
        ] {
            let back = <TaggedContinuation as Serialize<TaggedContinuation>>::decode(
                &<TaggedContinuation as Serialize<TaggedContinuation>>::encode(&case),
            )
            .expect("decode");
            match (&case, &back) {
                (TaggedContinuation::ParBody(a), TaggedContinuation::ParBody(b)) => {
                    assert_eq!(a.body, b.body, "the body comes back");
                    assert_eq!(
                        a.random_state.to_bytes(),
                        b.random_state.to_bytes(),
                        "with its random state"
                    );
                }
                (TaggedContinuation::ScalaBodyRef(a), TaggedContinuation::ScalaBodyRef(b)) => {
                    assert_eq!(a, b, "the body reference comes back");
                }
                (TaggedContinuation::Empty, TaggedContinuation::Empty) => {}
                other => panic!("the tag did not survive the codec: {other:?}"),
            }
        }

        // Bytes that are not a message are refused rather than decoded into a default.
        assert!(
            <BindPattern as Serialize<BindPattern>>::decode(&[0xff; 8]).is_err(),
            "a malformed bind pattern is an error"
        );
        assert!(
            <TaggedContinuation as Serialize<TaggedContinuation>>::decode(&[0xff; 8]).is_err(),
            "and so is a malformed continuation"
        );
    }

    /// The golden file's rows are `id<TAB>value<TAB>provenance`; `#` lines are its legend. The
    /// *value* column is read by position, so adding the provenance column (and any later one)
    /// cannot corrupt a vector.
    fn rows() -> Vec<(String, String, String)> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/differential/wire.tsv"
        );
        std::fs::read_to_string(path)
            .expect("the golden file")
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                let mut f = l.split('\t');
                (
                    f.next().unwrap_or_default().to_string(),
                    f.next().unwrap_or_default().to_string(),
                    f.next().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    fn load(case: &str) -> String {
        rows()
            .into_iter()
            .find(|(id, _, _)| id == case)
            .unwrap_or_else(|| panic!("missing differential case: {case}"))
            .1
    }

    /// **Golden-drift guard.** Every row of the file must be consumed by a test in this module, and
    /// every case the tests load must exist — a file that grows a row nobody asserts on (or a test
    /// that starts reading a row that was deleted) fails here rather than passing quietly.
    #[test]
    fn every_golden_row_is_consumed() {
        const CONSUMED: &[&str] = &[
            "bitset_empty",
            "bitset_0",
            "bitset_7",
            "bitset_8",
            "bitset_64",
            "par_empty",
        ];
        let ids: Vec<String> = rows().into_iter().map(|(id, _, _)| id).collect();
        assert_eq!(ids.len(), CONSUMED.len(), "row count: {ids:?}");
        for case in CONSUMED {
            assert!(ids.iter().any(|id| id == case), "{case} is not in the file");
            assert!(
                !load(case).is_empty() || *case == "bitset_empty" || *case == "par_empty",
                "{case} has an empty value and no empty-value case expects one"
            );
        }
        // Every row is `scala-rule-transcribed`: the wire format comes from the proto, not a run.
        for (id, _, provenance) in rows() {
            assert_eq!(
                provenance, "scala-rule-transcribed",
                "{id} lost its provenance"
            );
        }
    }

    fn hex(bytes: &[u8]) -> String {
        base16::encode(bytes)
    }

    #[test]
    fn differential_bitset_encoding() {
        assert_eq!(hex(&bitset_to_bytes(&[])), load("bitset_empty"));
        assert_eq!(hex(&bitset_to_bytes(&[0])), load("bitset_0"));
        assert_eq!(hex(&bitset_to_bytes(&[7])), load("bitset_7"));
        assert_eq!(hex(&bitset_to_bytes(&[8])), load("bitset_8"));
        assert_eq!(hex(&bitset_to_bytes(&[64])), load("bitset_64"));
    }

    #[test]
    fn differential_empty_par_serializes_to_empty() {
        let bytes = <a::Par as Serialize<a::Par>>::encode(&a::Par::default());
        assert_eq!(hex(&bytes), load("par_empty"));
    }

    #[test]
    fn par_with_g_string_serializes_to_proto() {
        // `Par { exprs: [GString("foo")] }` -> `2a 05 (1a 03 "foo")` (field 5 exprs, field 3 g_string).
        let par = a::Par {
            exprs: vec![a::Expr::GString("foo".to_string())],
            ..Default::default()
        };
        let bytes = <a::Par as Serialize<a::Par>>::encode(&par);
        assert_eq!(hex(&bytes), "2a051a03666f6f");
    }

    #[test]
    fn par_with_g_private_serializes_to_proto() {
        // `Par { unforgeables: [GPrivate([0x01; 32])] }` -> `3a 24 (0a 22 (0a 20 <32x01>))`
        // (field 7 unforgeables, field 1 g_private_body, field 1 id).
        let par = a::Par {
            unforgeables: vec![a::GUnforgeable::GPrivate(a::GPrivate {
                id: vec![0x01; 32],
            })],
            ..Default::default()
        };
        let bytes = <a::Par as Serialize<a::Par>>::encode(&par);
        let expected = format!("3a240a220a20{}", "01".repeat(32));
        assert_eq!(hex(&bytes), expected);
    }

    // **A golden vector for `g_decimal` (tag 35) was here and is retired** (issue #527). It pinned
    // `Par { exprs: [GDecimal(15, 1)] }` -> `2a 08 (9a 02 05 (0a 01 0f 10 01))` and the canonical
    // zero `2a 06 9a 02 03 0a 01 00`. Tag 35 is now `reserved`, and `models/tests/expr_refusal.rs`
    // pins the refusal a sender gets instead.

    /// The **round-trip** half of the codec, which the golden vectors cannot reach: every `Expr`
    /// variant, every connective and every unforgeable is converted out and back. A variant that
    /// `expr_to_proto` writes into the wrong proto arm, or `expr_from_proto` reads back out of the
    /// wrong one, survives a golden test that only covers a handful of shapes.
    mod round_trips {
        use super::*;

        /// A par carrying a distinct integer, so two pars in one value are distinguishable.
        fn par(n: i64) -> a::Par {
            a::Par {
                exprs: vec![a::Expr::GInt(n)],
                ..a::Par::default()
            }
        }

        /// The same term in **name** position: the AST's sort split means a channel is a
        /// `Par<NameSort>`, and the wire type is one `Par` for both.
        fn name(n: i64) -> a::Par<a::NameSort> {
            a::Par {
                exprs: vec![a::Expr::GInt(n)],
                ..a::Par::default()
            }
        }

        fn bit(n: i32) -> a::AlwaysEqual<a::BitSet> {
            a::AlwaysEqual(vec![bit_or_zero(n)])
        }

        fn bit_or_zero(n: i32) -> i32 {
            n
        }

        fn var(idx: i32) -> a::Var {
            a::Var::BoundVar(idx)
        }

        /// Every `Expr` variant the AST has, one instance each — including the collections and the
        /// method call, which carry their own nested structure.
        fn every_expr() -> Vec<(&'static str, a::Expr)> {
            let boxed = |n: i64| Box::new(par(n));
            vec![
                ("GBool", a::Expr::GBool(true)),
                ("GInt", a::Expr::GInt(-7)),
                (
                    "GBigInt",
                    a::Expr::GBigInt(num_bigint::BigInt::from(1_000_000_000_000i64)),
                ),
                ("GString", a::Expr::GString("text".to_string())),
                ("GUri", a::Expr::GUri("rho:io".to_string())),
                ("GByteArray", a::Expr::GByteArray(vec![1, 2, 3])),
                ("ENot", a::Expr::ENot(boxed(1))),
                ("ENeg", a::Expr::ENeg(boxed(2))),
                ("EMult", a::Expr::EMult(boxed(3), boxed(4))),
                ("EDiv", a::Expr::EDiv(boxed(5), boxed(6))),
                ("EMod", a::Expr::EMod(boxed(7), boxed(8))),
                ("EPlus", a::Expr::EPlus(boxed(9), boxed(10))),
                ("EMinus", a::Expr::EMinus(boxed(11), boxed(12))),
                ("ELt", a::Expr::ELt(boxed(13), boxed(14))),
                ("ELte", a::Expr::ELte(boxed(15), boxed(16))),
                ("EGt", a::Expr::EGt(boxed(17), boxed(18))),
                ("EGte", a::Expr::EGte(boxed(19), boxed(20))),
                ("EEq", a::Expr::EEq(boxed(21), boxed(22))),
                ("ENeq", a::Expr::ENeq(boxed(23), boxed(24))),
                ("EAnd", a::Expr::EAnd(boxed(25), boxed(26))),
                ("EOr", a::Expr::EOr(boxed(27), boxed(28))),
                ("EShortAnd", a::Expr::EShortAnd(boxed(29), boxed(30))),
                ("EShortOr", a::Expr::EShortOr(boxed(31), boxed(32))),
                ("EMatches", a::Expr::EMatches(boxed(33), boxed(34))),
                (
                    "EPercentPercent",
                    a::Expr::EPercentPercent(boxed(35), boxed(36)),
                ),
                ("EPlusPlus", a::Expr::EPlusPlus(boxed(37), boxed(38))),
                ("EMinusMinus", a::Expr::EMinusMinus(boxed(39), boxed(40))),
                ("EVar", a::Expr::EVar(Box::new(a::Var::FreeVar(3)))),
                (
                    "EList",
                    a::Expr::EList(a::EList {
                        ps: vec![par(41)],
                        locally_free: bit(2),
                        connective_used: true,
                        remainder: Some(Box::new(var(1))),
                    }),
                ),
                (
                    "ETuple",
                    a::Expr::ETuple(a::ETuple {
                        ps: vec![par(42), par(43)],
                        locally_free: bit(3),
                        connective_used: false,
                    }),
                ),
                (
                    "ESet",
                    a::Expr::ESet(a::ParSet {
                        ps: vec![par(44)],
                        connective_used: true,
                        locally_free: bit(4),
                        remainder: None,
                    }),
                ),
                (
                    "EMap",
                    a::Expr::EMap(a::ParMap {
                        kvs: vec![(par(45), par(46))],
                        connective_used: false,
                        locally_free: bit(5),
                        remainder: Some(Box::new(var(2))),
                    }),
                ),
                (
                    "EMethod",
                    a::Expr::EMethod(a::EMethod {
                        method_name: "get".to_string(),
                        target: boxed(47),
                        arguments: vec![par(48), par(49)],
                        locally_free: bit(6),
                        connective_used: true,
                    }),
                ),
            ]
        }

        /// Each variant survives `expr_to_proto` → `expr_from_proto` unchanged: the assertion is on
        /// the whole `Expr`, so a variant that round-trips *through the wrong arm* (a `GInt` coming
        /// back as `GBool`) fails.
        #[test]
        fn every_expr_variant_round_trips() {
            let variants = every_expr();
            assert_eq!(
                variants.len(),
                33,
                "the table covers every variant the enum has (33 since the decimal was removed by \
                 issue #527; 34 before)"
            );
            for (name, expr) in &variants {
                let proto = expr_to_proto(expr);
                let back = expr_from_proto(&proto)
                    .unwrap_or_else(|e| panic!("{name} did not decode: {e}"));
                assert_eq!(&back, expr, "{name} did not round trip");
            }
        }

        /// The **protobuf tag** each variant uses is the Scala's `ExprInstance` arm: asserted
        /// separately from the round trip, because a variant written into the wrong *arm* still
        /// round-trips through the Rust pair while producing bytes a Scala node would read
        /// differently.
        #[test]
        fn the_expr_variants_use_the_scala_proto_arms() {
            use p::expr::ExprInstance as E;
            assert!(matches!(
                expr_to_proto(&a::Expr::GBool(true)).expr_instance,
                Some(E::GBool(true))
            ));
            assert!(matches!(
                expr_to_proto(&a::Expr::GInt(-7)).expr_instance,
                Some(E::GInt(-7))
            ));
            assert!(matches!(
                expr_to_proto(&a::Expr::GString("x".to_string())).expr_instance,
                Some(E::GString(_))
            ));
            assert!(matches!(
                expr_to_proto(&a::Expr::EList(a::EList::default())).expr_instance,
                Some(E::EListBody(_))
            ));
            assert!(matches!(
                expr_to_proto(&a::Expr::EMethod(a::EMethod::default())).expr_instance,
                Some(E::EMethodBody(_))
            ));
            // A big integer is written as its signed big-endian bytes.
            let huge = a::Expr::GBigInt(num_bigint::BigInt::from(258i64));
            match expr_to_proto(&huge).expr_instance {
                Some(E::GBigInt(bytes)) => assert_eq!(bytes, vec![1, 2]),
                other => panic!("expected GBigInt, got {other:?}"),
            }
        }

        /// Connectives and unforgeables: the two sum types with their own proto arms.
        #[test]
        fn every_connective_and_unforgeable_round_trips() {
            let connectives = vec![
                a::Connective::ConnAnd(a::ConnectiveBody { ps: vec![par(1)] }),
                a::Connective::ConnOr(a::ConnectiveBody {
                    ps: vec![par(2), par(3)],
                }),
                a::Connective::ConnNot(Box::new(par(4))),
                a::Connective::VarRef(a::VarRef { index: 1, depth: 2 }),
                a::Connective::ConnBool(true),
                a::Connective::ConnInt(false),
                a::Connective::ConnBigInt(true),
                a::Connective::ConnString(false),
                a::Connective::ConnUri(true),
                a::Connective::ConnByteArray(false),
            ];
            for connective in &connectives {
                let back = connective_from_proto(&connective_to_proto(connective))
                    .unwrap_or_else(|e| panic!("{connective:?} did not decode: {e}"));
                assert_eq!(&back, connective, "{connective:?} did not round trip");
            }

            let unforgeables = vec![
                a::GUnforgeable::GPrivate(a::GPrivate { id: vec![7; 32] }),
                a::GUnforgeable::GDeployId(a::GDeployId { sig: vec![8; 32] }),
                a::GUnforgeable::GDeployerId(a::GDeployerId {
                    public_key: vec![9; 32],
                }),
                a::GUnforgeable::GSysAuthToken,
            ];
            for unforgeable in &unforgeables {
                let back = unforgeable_from_proto(&unforgeable_to_proto(unforgeable))
                    .unwrap_or_else(|e| panic!("{unforgeable:?} did not decode: {e}"));
                assert_eq!(&back, unforgeable);
            }
        }

        /// A `Par` with **every field** populated round-trips: the eight lists, the bitset, the
        /// `connective_used` flag and the sort marker all survive.
        #[test]
        fn a_par_with_every_field_round_trips() {
            let full: a::Proc = a::Par {
                sends: vec![a::Send {
                    chan: Box::new(name(1)),
                    data: vec![name(2), name(3)],
                    persistent: true,
                    locally_free: bit(1),
                    connective_used: true,
                }],
                receives: vec![a::Receive {
                    binds: vec![a::ReceiveBind {
                        patterns: vec![name(4)],
                        source: Box::new(name(5)),
                        remainder: Some(Box::new(var(1))),
                        free_count: FreeCount::new(2).expect("non-negative"),
                    }],
                    body: Box::new(par(6)),
                    persistent: true,
                    peek: true,
                    bind_count: FreeCount::ONE,
                    locally_free: bit(2),
                    connective_used: false,
                }],
                news: vec![a::New {
                    bind_count: FreeCount::from_len(3),
                    p: Box::new(par(7)),
                    uri: vec!["rho:io".to_string()],
                    injections: std::collections::BTreeMap::new(),
                    locally_free: bit(3),
                }],
                exprs: vec![a::Expr::GInt(8)],
                matches: vec![a::Match {
                    target: Box::new(name(9)),
                    cases: vec![a::MatchCase {
                        pattern: Box::new(name(10)),
                        source: Box::new(par(11)),
                        free_count: FreeCount::new(1).expect("non-negative"),
                    }],
                    locally_free: bit(4),
                    connective_used: true,
                }],
                unforgeables: vec![a::GUnforgeable::GSysAuthToken],
                bundles: vec![a::Bundle {
                    body: Box::new(par(12)),
                    write_flag: true,
                    read_flag: false,
                }],
                connectives: vec![a::Connective::ConnBool(true)],
                locally_free: bit(5),
                connective_used: true,
                _sort: std::marker::PhantomData,
            };

            let back = par_from_proto(&par_to_proto(&full)).expect("a full par decodes");
            assert_eq!(back, full, "every field survives");

            // The fields that are easy to lose in a conversion are named individually, so a failure
            // says which one moved.
            assert_eq!(back.sends.len(), 1);
            assert!(back.sends[0].persistent);
            assert_eq!(back.receives[0].binds[0].patterns.len(), 1);
            assert!(back.receives[0].peek, "the peek flag is not dropped");
            assert_eq!(back.news[0].uri, vec!["rho:io".to_string()]);
            assert_eq!(back.matches[0].cases.len(), 1);
            assert!(back.bundles[0].write_flag, "the write flag is not dropped");
            assert!(!back.bundles[0].read_flag);
            assert_eq!(back.connectives.len(), 1);
            assert!(back.connective_used);
        }

        /// The runtime payloads — the ones the RSpace keys continuations by — round-trip: a bind
        /// pattern, a list of pars with a random state, and all three tagged continuations.
        #[test]
        fn the_runtime_payloads_round_trip() {
            let pattern = BindPattern {
                patterns: vec![SortedProc::new(par(1))],
                remainder: Some(a::Var::Wildcard),
                free_count: 1,
            };
            assert_eq!(
                bind_pattern_from_proto(&bind_pattern_to_proto(&pattern)).expect("pattern"),
                pattern
            );

            // A negative count is refused here, as `ReceiveBind`/`MatchCase` already refuse it. The
            // count is not inert: `RhoMatch::get` fills `0..free_count` from the match's free map, so
            // a peer's negative count would have applied the continuation with no bound values at all
            // — silently, which is the failure mode the boundary check exists to make loud.
            let mut negative = bind_pattern_to_proto(&pattern);
            negative.free_count = -1;
            assert!(
                bind_pattern_from_proto(&negative).is_err(),
                "a negative free-count must be refused where the message arrives"
            );
            negative.free_count = 0;
            assert!(
                bind_pattern_from_proto(&negative).is_ok(),
                "zero is a count, not a negative one"
            );

            let random = Blake2b512Random::from_init(&[3u8; 8]);
            let list = ListParWithRandom {
                pars: vec![SortedProc::new(par(2))],
                random_state: random.clone(),
            };
            let back = list_par_with_random_from_proto(&list_par_with_random_to_proto(&list))
                .expect("list");
            assert_eq!(back.pars.len(), 1);
            assert_eq!(back.pars[0].as_par(), &par(2));
            assert_eq!(
                back.random_state, random,
                "the split generator state survives, or a replay would diverge"
            );

            let par_with_random = ParWithRandom {
                body: SortedProc::new(par(3)),
                random_state: Blake2b512Random::from_init(&[4u8; 8]),
            };
            let back = par_with_random_from_proto(&par_with_random_to_proto(&par_with_random))
                .expect("par with random");
            assert_eq!(back.body.as_par(), &par(3));

            // `TaggedContinuation` carries a split-generator state, so (like `ParWithRandom`) it
            // has no `PartialEq`; the `Debug` forms are structural, so they are compared instead.
            for continuation in [
                TaggedContinuation::ParBody(ParWithRandom {
                    body: SortedProc::new(par(5)),
                    random_state: Blake2b512Random::from_init(&[6u8; 8]),
                }),
                TaggedContinuation::ScalaBodyRef(7),
            ] {
                let back =
                    tagged_continuation_from_proto(&tagged_continuation_to_proto(&continuation))
                        .expect("tagged continuation");
                assert_eq!(
                    format!("{back:?}"),
                    format!("{continuation:?}"),
                    "a tagged continuation round trip"
                );
            }
        }

        /// A dedicated boundary witness for `BindPattern.free_count`: the broad payload round-trip
        /// asserts it too, but a named test gives the gate a falsifiable witness rather than a
        /// sub-assertion inside another test.
        #[test]
        fn a_negative_bind_pattern_free_count_is_refused_at_the_proto_boundary() {
            let pattern = BindPattern {
                patterns: vec![],
                remainder: None,
                free_count: 0,
            };
            let mut proto = bind_pattern_to_proto(&pattern);
            proto.free_count = -1;
            assert!(
                bind_pattern_from_proto(&proto).is_err(),
                "a negative free-count must be refused at the proto boundary"
            );
            proto.free_count = 0;
            assert!(bind_pattern_from_proto(&proto).is_ok(), "zero is a count");
        }

        /// The `from_proto` half is **partial** where the proto has an optional inner message: a
        /// missing one is `Malformed(<field>)` naming the field, rather than a defaulted value. Two
        /// representative arms are exercised — the shape is the same for all twenty-one binary
        /// operators, and the round-trip tests above already cover their success paths.
        #[test]
        fn a_missing_inner_message_is_malformed_and_names_the_field() {
            use p::expr::ExprInstance as E;

            let unary = p::Expr {
                expr_instance: Some(E::ENotBody(p::ENot { p: None })),
            };
            assert_eq!(
                expr_from_proto(&unary).expect_err("no inner par"),
                ModelsError::Malformed("p")
            );

            let binary = p::Expr {
                expr_instance: Some(E::EMultBody(p::EMult {
                    p1: None,
                    p2: Some(p::Par::default()),
                })),
            };
            assert_eq!(
                expr_from_proto(&binary).expect_err("no p1"),
                ModelsError::Malformed("p1")
            );

            let binary = p::Expr {
                expr_instance: Some(E::EMultBody(p::EMult {
                    p1: Some(p::Par::default()),
                    p2: None,
                })),
            };
            assert_eq!(
                expr_from_proto(&binary).expect_err("no p2"),
                ModelsError::Malformed("p2")
            );

            let var_body = p::Expr {
                expr_instance: Some(E::EVarBody(p::EVar { v: None })),
            };
            assert_eq!(
                expr_from_proto(&var_body).expect_err("no var"),
                ModelsError::Malformed("v")
            );

            // A missing `send`/`receive`/`bundle` body is refused the same way.
            let send = p::Send {
                chan: None,
                data: Vec::new(),
                persistent: false,
                locally_free: Vec::new(),
                connective_used: false,
            };
            assert_eq!(
                send_from_proto(&send).expect_err("no channel"),
                ModelsError::Malformed("chan")
            );

            let receive = p::Receive {
                binds: Vec::new(),
                body: None,
                persistent: false,
                peek: false,
                bind_count: 0,
                locally_free: Vec::new(),
                connective_used: false,
            };
            assert_eq!(
                receive_from_proto(&receive).expect_err("no body"),
                ModelsError::Malformed("body")
            );

            let bundle = p::Bundle {
                body: None,
                write_flag: false,
                read_flag: false,
            };
            assert_eq!(
                bundle_from_proto(&bundle).expect_err("no body"),
                ModelsError::Malformed("body")
            );
        }

        /// A peer's `New`/`Receive` message may carry a **negative** `bind_count` (`RhoTypes.proto`
        /// spells it `int32`, which is signed). The field is not inert: it is part of the term's sort
        /// key (`sorter.rs`'s `leaf_i64(bind_count)`), an input to `well_scoped_par`'s depth
        /// arithmetic, and — once a `New` is normalized — the extent of the levels the body may
        /// reference. The port carried it through unvalidated and clamped it where it was used
        /// (`.max(0)`, `types.rs:436/440`), which both hides the malformed message and keeps a term
        /// whose count cannot mean anything. Both counters are `FreeCount` carriers now, so the sign
        /// is refused *here*, at the declared boundary — the same validate-on-ingress rule the
        /// sibling `free_count` fields follow (AUDIT §11 R12, C52).
        ///
        /// Falsifier: with the pass-through restored (`bind_count: p.bind_count`), this fails — both
        /// decoders answer `Ok` with a negative count in the term.
        #[test]
        fn a_negative_bind_count_is_refused_at_the_wire_boundary() {
            let receive = p::Receive {
                binds: Vec::new(),
                body: Some(par_to_proto(&par(0))),
                persistent: false,
                peek: false,
                bind_count: -1,
                locally_free: Vec::new(),
                connective_used: false,
            };
            assert!(
                receive_from_proto(&receive).is_err(),
                "a receive declaring a negative bind count must be refused"
            );

            let new = p::New {
                bind_count: -2,
                p: Some(par_to_proto(&par(0))),
                uri: Vec::new(),
                injections: std::collections::BTreeMap::new(),
                locally_free: Vec::new(),
            };
            assert!(
                new_from_proto(&new).is_err(),
                "a `new` declaring a negative bind count must be refused"
            );
        }

        /// An Expr must identify a known value, even when it arrives inside a Par.
        /// Refusal here supersedes the former default-to-false characterization.
        #[test]
        fn an_instance_less_expr_is_malformed() {
            let empty = p::Expr::default();
            assert!(empty.expr_instance.is_none());
            assert_eq!(
                expr_from_proto(&empty),
                Err(ModelsError::Malformed("expr_instance"))
            );

            let par = p::Par {
                exprs: vec![p::Expr::default()],
                ..p::Par::default()
            };
            assert_eq!(
                par_from_proto::<a::ProcSort>(&par),
                Err(ModelsError::Malformed("expr_instance"))
            );
        }

        // **The canonical-form decoder test was here and is retired** (issue #527): it pinned the
        // five refusals a noncanonical `g_decimal` payload got — empty, non-minimal, trailing-zero,
        // zero-with-scale, scale-over-range. The whole alternative-encoding surface went with the
        // type; tag 35 is `reserved` and `models/tests/expr_refusal.rs` pins the one refusal that
        // remains.
    }
}
