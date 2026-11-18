//! Hand-written lexer + recursive-descent parser for rholang (port of the BNFC-generated
//! `Yylex`/`parser` from `rholang_mercury.cf`).

use crate::errors::RholangError;
use crate::proc_ast::*;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Long(i64),
    // **A `Decimal(String)` token was here and is retired** (issue #527): a `d`-suffixed literal no
    // longer lexes, so `1.5d` / `2d` are a syntax error. See `docs/src/formal/decimal-removed.md`.
    Str(String),
    Uri(String),
    Ident(String),
    Tilde,
    Conj, // /\
    Disj, // \/
    Star,
    Dot,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Pipe,
    Percent,
    PercentPercent,
    Slash,
    Plus,
    PlusPlus,
    Minus,
    MinusMinus,
    Lt,
    Lte,
    Gt,
    Gte,
    EqEq,
    Neq,
    AndAnd,
    OrOr,
    Bang,
    BangBang,
    BangQ,
    Semicolon,
    Amp,
    Arrow,   // =>
    LArrow,  // <-
    LLArrow, // <<-
    QMark,
    Colon,
    Comma,
    Ellipsis,
    Underscore,
    At,
    Eq,
    Eof,
}

fn is_reserved(s: &str) -> bool {
    matches!(
        s,
        "Nil"
            | "true"
            | "false"
            | "not"
            | "and"
            | "or"
            | "matches"
            | "contract"
            | "for"
            | "select"
            | "match"
            | "bundle"
            | "let"
            | "if"
            | "else"
            | "new"
            | "in"
            | "Set"
            | "Bool"
            | "Int"
            | "BigInt"
            | "String"
            | "Uri"
            | "ByteArray"
    )
}

fn lex(src: &str) -> Result<Vec<Tok>, RholangError> {
    let mut toks = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // comments
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            let start = i;
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 >= chars.len() {
                // An unterminated comment used to swallow the rest of the source and lex cleanly, so
                // `Nil /* oops` parsed as `Nil` (AUDIT C36).
                return Err(RholangError::LexerError(format!(
                    "unterminated block comment at position {start}"
                )));
            }
            i += 2;
            continue;
        }
        // string literal
        if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            if i >= chars.len() {
                // `chars[start..i]` with `i == len + 1` **panicked** here: an unterminated literal was
                // a crash rather than a diagnostic (AUDIT C36).
                return Err(RholangError::LexerError(format!(
                    "unterminated string literal at position {start}"
                )));
            }
            i += 1; // closing quote
            toks.push(Tok::Str(chars[start..i].iter().collect()));
            continue;
        }
        // uri literal
        if c == '`' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '`' {
                i += 1;
            }
            if i >= chars.len() {
                return Err(RholangError::LexerError(format!(
                    "unterminated uri literal at position {start}"
                )));
            }
            i += 1;
            toks.push(Tok::Uri(chars[start..i].iter().collect()));
            continue;
        }
        // number. **A `d`-suffixed decimal literal was lexed here and is retired** (issue #527): the
        // lookahead consumed a fraction only when `d` followed it (so a bare `1.5` stayed
        // `Long Dot Long` and `1.foo()` remained a method call). With the type gone, `1.5d` is a
        // `Long Dot Ident` sequence that the parser refuses.
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let n: i64 = chars[start..i]
                .iter()
                .collect::<String>()
                .parse::<i64>()
                .map_err(|e| RholangError::LexerError(e.to_string()))?;
            toks.push(Tok::Long(n));
            continue;
        }
        // identifier / keyword. A standalone `_` is the wildcard token (not a `Var`), so a leading
        // `_` only starts an identifier when followed by another identifier-continuation char
        // (mirrors the BNFC `Var` token: `'_' (letter | digit | '_' | '\'')+`).
        let ident_continue = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '\'';
        if c.is_ascii_alphabetic()
            || (c == '_'
                && chars
                    .get(i + 1)
                    .map(|&n| ident_continue(n))
                    .unwrap_or(false))
        {
            let start = i;
            while i < chars.len() && ident_continue(chars[i]) {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            // `bundle0` is the bundle-equivalence keyword; tokenize it as `bundle` + `0` so
            // `parse_bundle` can read the suffix (see the comment there).
            if ident == "bundle0" {
                toks.push(Tok::Ident("bundle".to_string()));
                toks.push(Tok::Long(0));
            } else {
                toks.push(Tok::Ident(ident));
            }
            continue;
        }
        // operators
        let peek = |n: usize| chars.get(i + n).copied();
        let (tok, advance): (Tok, usize) = match c {
            '~' => (Tok::Tilde, 1),
            // The two logical connectives, in the grammar's own spelling:
            //     PConjunction.  Proc14 ::= Proc14 "/\\" Proc15 ;
            //     PDisjunction.  Proc13 ::= Proc13 "\\/" Proc14 ;
            // i.e. conjunction is `/` then `\`, disjunction is `\` then `/`. These were **swapped**
            // here (and `\\` — not an operator at all — was accepted as the disjunction), so `a \/ b`
            // lexed as conjunction and `a /\ b` did not lex at all (`/` was taken as division and the
            // `\` was then an illegal character). Longest match wins, as in the Scala lexer.
            '/' if peek(1) == Some('\\') => (Tok::Conj, 2),
            '\\' if peek(1) == Some('/') => (Tok::Disj, 2),
            '*' => (Tok::Star, 1),
            '.' if peek(1) == Some('.') && peek(2) == Some('.') => (Tok::Ellipsis, 3),
            '.' => (Tok::Dot, 1),
            '(' => (Tok::LParen, 1),
            ')' => (Tok::RParen, 1),
            '{' => (Tok::LBrace, 1),
            '}' => (Tok::RBrace, 1),
            '[' => (Tok::LBracket, 1),
            ']' => (Tok::RBracket, 1),
            '|' if peek(1) == Some('|') => (Tok::OrOr, 2),
            '|' => (Tok::Pipe, 1),
            '%' if peek(1) == Some('%') => (Tok::PercentPercent, 2),
            '%' => (Tok::Percent, 1),
            '/' => (Tok::Slash, 1),
            '+' if peek(1) == Some('+') => (Tok::PlusPlus, 2),
            '+' => (Tok::Plus, 1),
            '-' if peek(1) == Some('-') => (Tok::MinusMinus, 2),
            '-' if peek(1) == Some('>') => (Tok::Arrow, 2),
            '-' => (Tok::Minus, 1),
            '<' if peek(1) == Some('=') => (Tok::Lte, 2),
            '<' if peek(1) == Some('-') => (Tok::LArrow, 2),
            '<' if peek(1) == Some('<') && peek(2) == Some('-') => (Tok::LLArrow, 3),
            '<' => (Tok::Lt, 1),
            '>' if peek(1) == Some('=') => (Tok::Gte, 2),
            '>' => (Tok::Gt, 1),
            '=' if peek(1) == Some('=') => (Tok::EqEq, 2),
            '=' if peek(1) == Some('>') => (Tok::Arrow, 2),
            '=' => (Tok::Eq, 1),
            '!' if peek(1) == Some('=') => (Tok::Neq, 2),
            '!' if peek(1) == Some('!') => (Tok::BangBang, 2),
            '!' if peek(1) == Some('?') => (Tok::BangQ, 2),
            '!' => (Tok::Bang, 1),
            '&' if peek(1) == Some('&') => (Tok::AndAnd, 2),
            '&' => (Tok::Amp, 1),
            ';' => (Tok::Semicolon, 1),
            '?' => (Tok::QMark, 1),
            ':' => (Tok::Colon, 1),
            ',' => (Tok::Comma, 1),
            '_' => (Tok::Underscore, 1),
            '@' => (Tok::At, 1),
            _ => {
                return Err(RholangError::LexerError(format!(
                    "Illegal character {c} at {i}"
                )))
            }
        };
        i += advance;
        toks.push(tok);
    }
    toks.push(Tok::Eof);
    Ok(toks)
}

/// Maximum recursive-descent nesting depth. Reached by the depth guard in `with_depth` so a
/// maliciously deep term (up to the gRPC message-size limit) cannot overflow the task stack
/// (documented Scala deviation: the BNFC parser has no depth guard). 128 levels is far deeper than
/// any real rholang term.
///
/// **This does not bound the AST, and the comment here used to imply it did** — see `MAX_AST_DEPTH`.
const MAX_PARSE_DEPTH: usize = 128;

/// Maximum number of terms folded into a single left-leaning operator chain (`a+b+c+…`). The
/// recursion-depth guard (`with_depth`) never triggers on such a flat chain, so a separate bound is
/// needed.
///
/// **It does not bound the resulting AST depth either**, which is what this comment claimed until
/// 2026-09-26: a chain is charged per level and the counters are per level, so `depth` nesting levels
/// of `chain` operators compose into an AST of depth `depth × chain`. See `MAX_AST_DEPTH`.
const MAX_CHAIN_LENGTH: usize = 512;

/// Maximum **AST depth** of a parsed term — the depth of the *value*, not of the parser's recursion,
/// and therefore the quantity every consumer recurses over: the normalizer (`normalize_proc`), the
/// sorter (`sort_par`/`sort_expr`), `well_scoped`, the reducer's expression evaluator, the spatial
/// matcher and the pretty printer. `parse` refuses a tree deeper than this
/// ([`exceeds_ast_depth`]), so the two guards above compose into a bounded AST instead of bounding two
/// independent shapes whose *product* is free.
///
/// **The constant is measured, not chosen** (2026-09-26, AUDIT C99). On this tree and the node's own
/// 32 MiB worker stack (`node/src/main.rs`): a debug build aborts between AST depth 732 (normalizes
/// in 739 ms) and 994 (`fatal runtime error: stack overflow, aborting`, SIGABRT — the **process**,
/// not the task); a release build survives 3,000 in 3.2 s and aborts between 8,040 (which costs
/// **64 s** of CPU) and 50,100. The cost is super-linear — 157 ms at 732, 292 at 994, 537 at 1,296,
/// 927 at 1,638, 3.2 s at 3,000 — so the cheap half of this bound's value is the CPU cap.
///
/// 768 is below the *debug* abort with margin, and that is the point: the weakest configuration that
/// ships is not the node's, it is CI's and a developer's. It is also 1.5× the largest single chain
/// (`MAX_CHAIN_LENGTH + 1 = 513` is the deepest a legitimate flat chain reaches), so nothing that
/// parsed before stops parsing — verified by sweeping every `.rho`/`.rhox` in the tree (36 of 37
/// parse; the 37th is `Pos.rhox`, a `$`-substituted template that is not meant to be parsed raw).
///
/// **It does not protect a 2 MiB stack**, where the consumers abort at a few dozen levels — which is
/// why the deep tests here build an explicit 8–32 MiB stack (`rho_examples.rs`, `legacy_contracts.rs`,
/// `node/tests/common/mod.rs`).
///
/// **The runtime-built term is no longer a residual**: a program can fold its way to a deep value
/// without the parser seeing it, and the space refuses one past `storage.rs::MAX_VALUE_DEPTH` (256) at
/// both produce paths — law 50 clause b, AUDIT C100. Two things this constant therefore does *not*
/// cover, both live and both registered rather than implied: a **parsed** term is walked by the
/// evaluator before the space sees the datum, and that walk aborts the process at ~350 links — below
/// this bound, because this bound was measured against the *normalizer*'s frames (AUDIT C101) — and
/// nested tuples make the **parser** itself exponential, which no depth bound can reach (AUDIT C102).
pub const MAX_AST_DEPTH: usize = 768;

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        self.pos += 1;
        t
    }
    /// Run `f` under the recursion-depth guard: increment on entry (rejecting once
    /// `MAX_PARSE_DEPTH` is exceeded) and decrement on every exit path, including `?` early
    /// returns inside `f`.
    fn with_depth<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, RholangError>,
    ) -> Result<T, RholangError> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            self.depth -= 1;
            return Err(RholangError::SyntaxError(
                "parse depth exceeded".to_string(),
            ));
        }
        let result = f(self);
        self.depth -= 1;
        result
    }
    /// Count one operator in a flat chain, rejecting once `MAX_CHAIN_LENGTH` is exceeded.
    fn chain_link(&mut self, count: &mut usize) -> Result<(), RholangError> {
        *count += 1;
        if *count > MAX_CHAIN_LENGTH {
            return Err(RholangError::SyntaxError(
                "operator chain too long".to_string(),
            ));
        }
        Ok(())
    }
    fn eat_ident(&mut self, kw: &str) -> bool {
        if let Tok::Ident(id) = self.peek() {
            if id == kw {
                self.pos += 1;
                return true;
            }
        }
        false
    }
    fn expect(&mut self, t: Tok) -> Result<(), RholangError> {
        if self.peek() == &t {
            self.pos += 1;
            Ok(())
        } else {
            Err(RholangError::SyntaxError(format!(
                "expected {t:?}, got {:?} (pos={})",
                self.peek(),
                self.pos
            )))
        }
    }

    /// Require a keyword rather than consuming it if present. Every keyword the grammar makes
    /// mandatory is checked with this: `eat_ident`'s `false` is a boolean nobody reads, and an
    /// ignored keyword is a term the grammar does not derive (`new x Nil`, AUDIT C32).
    fn expect_ident(&mut self, kw: &str) -> Result<(), RholangError> {
        if self.eat_ident(kw) {
            Ok(())
        } else {
            Err(RholangError::SyntaxError(format!(
                "expected `{kw}`, got {:?} (pos={})",
                self.peek(),
                self.pos
            )))
        }
    }
}

/// Parse a source string into a `Proc` (port of `Compiler.sourceToAST`).
///
/// The whole token stream must be the term: `Tok::Eof` is pushed by the lexer and required here. The
/// grammar derives *terms*, not "a term followed by anything", so trailing input has no derivation —
/// and the port used to return `Ok` for it, which made `parse("Nil )")` and `parse("c!(1) garbage")`
/// **succeed silently** (AUDIT C30). That matters because `source_to_adt_with_env` runs on
/// `deploy.data.term`: a deploy whose term is a valid prefix of what the client wrote would have run,
/// reporting success, having executed something the client did not write.
/// Whether the tree is deeper than `limit`.
///
/// **Iterative on purpose.** The depth of this tree is exactly what the parser does not bound: a flat
/// operator chain is built by a loop, so it costs a bounded number of parser frames and produces an
/// AST of depth `n`. A recursive walk here would therefore overflow the very stack it exists to
/// protect — the failure it is measuring. An explicit worklist has no such mode, and the early return
/// keeps the cost proportional to `limit` rather than to the term.
///
/// The depth counted is the *source* tree's, which is what this module has; the de Bruijn `Par` the
/// normalizer builds from it adds a small constant per construct, so this is a proxy — and a
/// deliberate one, because it is a proxy whose measurement is exact rather than an accounting that
/// must be right about where a chain's depth is charged.
pub fn exceeds_ast_depth(root: &Proc, limit: usize) -> bool {
    let mut work: Vec<(&Proc, usize)> = vec![(root, 1)];
    while let Some((p, d)) = work.pop() {
        if d > limit {
            return true;
        }
        push_sub_procs(p, d + 1, &mut work);
    }
    false
}

/// Push every `Proc` immediately inside `p` onto `out`, each at the given depth.
fn push_sub_procs<'a>(p: &'a Proc, d: usize, out: &mut Vec<(&'a Proc, usize)>) {
    use Proc::*;
    let push_name = |n: &'a Name, out: &mut Vec<(&'a Proc, usize)>| {
        if let Name::NameQuote(inner) = n {
            out.push((inner, d));
        }
    };
    match p {
        PGround(_) | PVar(_) | PVarRef(_, _) | PNil | PSimpleType(_) => {}
        PNegation(a) | PNot(a) | PNeg(a) | PExprs(a) => out.push((a, d)),
        PBundle(_, a) => out.push((a, d)),
        PConjunction(a, b)
        | PDisjunction(a, b)
        | PMult(a, b)
        | PDiv(a, b)
        | PMod(a, b)
        | PPercentPercent(a, b)
        | PAdd(a, b)
        | PMinus(a, b)
        | PPlusPlus(a, b)
        | PMinusMinus(a, b)
        | PLt(a, b)
        | PLte(a, b)
        | PGt(a, b)
        | PGte(a, b)
        | PMatches(a, b)
        | PEq(a, b)
        | PNeq(a, b)
        | PAnd(a, b)
        | PShortAnd(a, b)
        | POr(a, b)
        | PShortOr(a, b)
        | PPar(a, b) => {
            out.push((a, d));
            out.push((b, d));
        }
        PMethod(target, _, args) => {
            out.push((target, d));
            for a in args {
                out.push((a, d));
            }
        }
        PEval(n) => push_name(n, out),
        PCollect(c) => match c {
            Collection::CollectList(items, _) | Collection::CollectSet(items, _) => {
                for i in items {
                    out.push((i, d));
                }
            }
            Collection::CollectTuple(t) => match t {
                Tuple::TupleSingle(a) => out.push((a, d)),
                Tuple::TupleMultiple(a, rest) => {
                    out.push((a, d));
                    for r in rest {
                        out.push((r, d));
                    }
                }
            },
            Collection::CollectMap(kvs, _) => {
                for KeyValuePair(k, v) in kvs {
                    out.push((k, d));
                    out.push((v, d));
                }
            }
        },
        PSend(n, _, procs) => {
            push_name(n, out);
            for x in procs {
                out.push((x, d));
            }
        }
        PContr(n, names, _, body) => {
            push_name(n, out);
            for x in names {
                push_name(x, out);
            }
            out.push((body, d));
        }
        PInput(receipts, body) => {
            for r in receipts {
                push_receipt(r, d, out);
            }
            out.push((body, d));
        }
        PChoice(branches) => {
            for Branch(recv, body) in branches {
                push_linear(recv, d, out);
                out.push((body, d));
            }
        }
        PMatch(target, cases) => {
            out.push((target, d));
            for Case(pat, body) in cases {
                out.push((pat, d));
                out.push((body, d));
            }
        }
        PLet(decl, decls, body) => {
            push_decl(decl, d, out);
            if let Decls::LinearDeclsImpl(ds) = decls {
                for LinearDecl(x) in ds {
                    push_decl(x, d, out);
                }
            }
            if let Decls::ConcDeclsImpl(ds) = decls {
                for ConcDecl(x) in ds {
                    push_decl(x, d, out);
                }
            }
            out.push((body, d));
        }
        PIf(c, t) => {
            out.push((c, d));
            out.push((t, d));
        }
        PIfElse(c, t, e) => {
            out.push((c, d));
            out.push((t, d));
            out.push((e, d));
        }
        PNew(_, body) => out.push((body, d)),
        PSendSynch(n, procs, cont) => {
            push_name(n, out);
            for x in procs {
                out.push((x, d));
            }
            if let SynchSendCont::NonEmptyCont(b) = cont {
                out.push((b, d));
            }
        }
    }
}

fn push_decl<'a>(decl: &'a Decl, d: usize, out: &mut Vec<(&'a Proc, usize)>) {
    let Decl(names, _, procs) = decl;
    for n in names {
        if let Name::NameQuote(inner) = n {
            out.push((inner, d));
        }
    }
    for p in procs {
        out.push((p, d));
    }
}

fn push_receipt<'a>(r: &'a Receipt, d: usize, out: &mut Vec<(&'a Proc, usize)>) {
    let quote = |n: &'a Name, out: &mut Vec<(&'a Proc, usize)>| {
        if let Name::NameQuote(inner) = n {
            out.push((inner, d));
        }
    };
    match r {
        Receipt::ReceiptLinear(l) => push_linear(l, d, out),
        Receipt::ReceiptRepeated(ReceiptRepeatedImpl::RepeatedSimple(binds)) => {
            for RepeatedBind(names, _, src) in binds {
                for n in names {
                    quote(n, out);
                }
                quote(src, out);
            }
        }
        Receipt::ReceiptPeek(ReceiptPeekImpl::PeekSimple(binds)) => {
            for PeekBind(names, _, src) in binds {
                for n in names {
                    quote(n, out);
                }
                quote(src, out);
            }
        }
    }
}

fn push_linear<'a>(l: &'a ReceiptLinearImpl, d: usize, out: &mut Vec<(&'a Proc, usize)>) {
    let ReceiptLinearImpl::LinearSimple(binds) = l;
    for LinearBind(names, _, src) in binds {
        for n in names {
            if let Name::NameQuote(inner) = n {
                out.push((inner, d));
            }
        }
        push_receipt_source(src, d, out);
    }
}

fn push_receipt_source<'a>(src: &'a NameSource, d: usize, out: &mut Vec<(&'a Proc, usize)>) {
    let name = |n: &'a Name, out: &mut Vec<(&'a Proc, usize)>| {
        if let Name::NameQuote(inner) = n {
            out.push((inner, d));
        }
    };
    match src {
        NameSource::SimpleSource(n) => name(n, out),
        NameSource::ReceiveSendSource(n) => name(n, out),
        NameSource::SendReceiveSource(n, procs) => {
            name(n, out);
            for p in procs {
                out.push((p, d));
            }
        }
    }
}

pub fn parse(source: &str) -> Result<Proc, RholangError> {
    let toks = lex(source)?;
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
    };
    let proc = p.parse_proc()?;
    p.expect(Tok::Eof)?;
    if exceeds_ast_depth(&proc, MAX_AST_DEPTH) {
        return Err(RholangError::SyntaxError(
            "term nesting too deep".to_string(),
        ));
    }
    Ok(proc)
}

impl Parser {
    fn parse_proc(&mut self) -> Result<Proc, RholangError> {
        self.with_depth(|p| {
            let mut left = p.parse_proc1()?;
            let mut chain = 0;
            while p.peek() == &Tok::Pipe {
                p.chain_link(&mut chain)?;
                p.next();
                let right = p.parse_proc1()?;
                left = Proc::PPar(Box::new(left), Box::new(right));
            }
            Ok(left)
        })
    }

    fn parse_proc1(&mut self) -> Result<Proc, RholangError> {
        self.with_depth(|p| {
            if p.eat_ident("if") {
                p.expect(Tok::LParen)?;
                let cond = p.parse_proc()?;
                p.expect(Tok::RParen)?;
                let then = p.parse_proc2()?;
                if p.eat_ident("else") {
                    let els = p.parse_proc1()?;
                    Ok(Proc::PIfElse(Box::new(cond), Box::new(then), Box::new(els)))
                } else {
                    Ok(Proc::PIf(Box::new(cond), Box::new(then)))
                }
            } else if p.eat_ident("new") {
                let mut decls = Vec::new();
                while p.peek() != &Tok::Ident("in".to_string()) {
                    decls.push(p.parse_name_decl()?);
                    if p.peek() == &Tok::Comma {
                        p.next();
                    } else {
                        break;
                    }
                }
                // `PNew ::= "new" [NameDecl] "in" Proc1` (`rholang_mercury.cf:74`): `in` is not
                // optional, and ignoring `eat_ident`'s result made `new x Nil` a term — a declaration
                // list that ends where the grammar says a keyword must be (AUDIT C32).
                p.expect_ident("in")?;
                let body = p.parse_proc1()?;
                Ok(Proc::PNew(decls, Box::new(body)))
            } else {
                p.parse_proc2()
            }
        })
    }

    fn parse_proc2(&mut self) -> Result<Proc, RholangError> {
        if self.eat_ident("contract") {
            let name = self.parse_name()?;
            self.expect(Tok::LParen)?;
            let mut names = Vec::new();
            while self.peek() != &Tok::RParen {
                names.push(self.parse_name()?);
                if self.peek() == &Tok::Comma {
                    self.next();
                } else {
                    break;
                }
            }
            let remainder = if self.peek() == &Tok::Ellipsis {
                self.next();
                NameRemainder::NameRemainderVar(self.parse_proc_var()?)
            } else {
                NameRemainder::NameRemainderEmpty
            };
            self.expect(Tok::RParen)?;
            self.expect(Tok::Eq)?;
            self.expect(Tok::LBrace)?;
            let body = self.parse_proc()?;
            self.expect(Tok::RBrace)?;
            Ok(Proc::PContr(name, names, remainder, Box::new(body)))
        } else if self.eat_ident("for") {
            self.expect(Tok::LParen)?;
            let mut receipts = Vec::new();
            while self.peek() != &Tok::RParen {
                receipts.push(self.parse_receipt()?);
                if self.peek() == &Tok::Semicolon {
                    self.next();
                } else {
                    break;
                }
            }
            // A `for()` with zero receipts would desugar to `PInput(vec![], …)`, which the
            // normalizer indexes at `receipts[0]` (panic). Reject it here instead.
            if receipts.is_empty() {
                return Err(RholangError::SyntaxError(
                    "for(...) requires at least one receive".to_string(),
                ));
            }
            self.expect(Tok::RParen)?;
            self.expect(Tok::LBrace)?;
            let body = self.parse_proc()?;
            self.expect(Tok::RBrace)?;
            Ok(Proc::PInput(receipts, Box::new(body)))
        } else if self.eat_ident("select") {
            self.expect(Tok::LBrace)?;
            let mut branches = Vec::new();
            while self.peek() != &Tok::RBrace {
                branches.push(self.parse_branch()?);
            }
            self.expect(Tok::RBrace)?;
            Ok(Proc::PChoice(branches))
        } else if self.eat_ident("match") {
            let target = self.parse_proc4()?;
            self.expect(Tok::LBrace)?;
            let mut cases = Vec::new();
            while self.peek() != &Tok::RBrace {
                cases.push(self.parse_case()?);
            }
            self.expect(Tok::RBrace)?;
            Ok(Proc::PMatch(Box::new(target), cases))
        } else if self.is_bundle() {
            let bundle = self.parse_bundle()?;
            self.expect(Tok::LBrace)?;
            let body = self.parse_proc()?;
            self.expect(Tok::RBrace)?;
            Ok(Proc::PBundle(bundle, Box::new(body)))
        } else if self.eat_ident("let") {
            let decl = self.parse_decl()?;
            let decls = self.parse_decls()?;
            // `PLet ::= "let" Decl Decls "in" "{" Proc "}"` (`rholang_mercury.cf:70`) — `in` is
            // mandatory here too (AUDIT C32).
            self.expect_ident("in")?;
            self.expect(Tok::LBrace)?;
            let body = self.parse_proc()?;
            self.expect(Tok::RBrace)?;
            Ok(Proc::PLet(decl, decls, Box::new(body)))
        } else {
            self.parse_proc3()
        }
    }

    fn parse_proc3(&mut self) -> Result<Proc, RholangError> {
        // A send is `Name Send "(" [Proc] ")"`; otherwise fall through to Proc4.
        let is_name_start = match self.peek() {
            Tok::At | Tok::Underscore => true,
            Tok::Ident(s) => !is_reserved(s),
            _ => false,
        };
        if is_name_start {
            let save = self.pos;
            let name = self.parse_name()?;
            if matches!(self.peek(), Tok::Bang | Tok::BangBang) {
                let send = self.parse_send()?;
                self.expect(Tok::LParen)?;
                let mut data = Vec::new();
                while self.peek() != &Tok::RParen {
                    data.push(self.parse_proc()?);
                    if self.peek() == &Tok::Comma {
                        self.next();
                    } else {
                        break;
                    }
                }
                self.expect(Tok::RParen)?;
                return Ok(Proc::PSend(name, send, data));
            }
            // `PSendSynch. Proc1 ::= Name "!?" "(" [Proc] ")" SynchSendCont ;` with
            // `EmptyCont ::= "."` / `NonEmptyCont ::= ";" Proc1` (`rholang_mercury.cf:75,95,96`).
            // The `Tok::BangQ` was lexed and only ever read by `parse_name_source`, so a synchronous
            // send in **process** position had no parser path at all: `PSendSynch` has a `proc_ast`
            // variant and a normalizer arm (`normalizer.rs:220`) and could not be produced, so
            // `x!?(1); P` parsed as the bare variable `x` with `!?(1); P` discarded — silently, until
            // the `Eof` check above turned it into an error (AUDIT C35).
            if matches!(self.peek(), Tok::BangQ) {
                self.next();
                self.expect(Tok::LParen)?;
                let mut data = Vec::new();
                while self.peek() != &Tok::RParen {
                    data.push(self.parse_proc()?);
                    if self.peek() == &Tok::Comma {
                        self.next();
                    } else {
                        break;
                    }
                }
                self.expect(Tok::RParen)?;
                let cont = if self.peek() == &Tok::Dot {
                    self.next();
                    SynchSendCont::EmptyCont
                } else if self.peek() == &Tok::Semicolon {
                    self.next();
                    SynchSendCont::NonEmptyCont(Box::new(self.parse_proc1()?))
                } else {
                    return Err(RholangError::SyntaxError(format!(
                        "a synchronous send must be followed by `.` or `;`, got {:?} (pos={})",
                        self.peek(),
                        self.pos
                    )));
                };
                return Ok(Proc::PSendSynch(name, data, cont));
            }
            // A bare `Var`/`_` in process position is a process-variable reference (`PVar`/
            // `PVarWildcard`), handled by `parse_proc16`; backtrack and fall through. Only `@`
            // (a quoted name) remains a bare-name error.
            if matches!(name, Name::NameVar(_) | Name::NameWildcard) {
                self.pos = save;
            } else {
                return Err(RholangError::SyntaxError(
                    "bare name in process position".into(),
                ));
            }
        }
        self.parse_proc4()
    }

    fn parse_proc4(&mut self) -> Result<Proc, RholangError> {
        let mut left = self.parse_proc5()?;
        let mut chain = 0;
        loop {
            if self.eat_ident("or") {
                self.chain_link(&mut chain)?;
                let right = self.parse_proc5()?;
                left = Proc::POr(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::OrOr {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc5()?;
                left = Proc::PShortOr(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_proc5(&mut self) -> Result<Proc, RholangError> {
        let mut left = self.parse_proc6()?;
        let mut chain = 0;
        loop {
            if self.eat_ident("and") {
                self.chain_link(&mut chain)?;
                let right = self.parse_proc6()?;
                left = Proc::PAnd(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::AndAnd {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc6()?;
                left = Proc::PShortAnd(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_proc6(&mut self) -> Result<Proc, RholangError> {
        let mut left = self.parse_proc7()?;
        let mut chain = 0;
        loop {
            if self.peek() == &Tok::EqEq {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc7()?;
                left = Proc::PEq(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::Neq {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc7()?;
                left = Proc::PNeq(Box::new(left), Box::new(right));
            } else if self.eat_ident("matches") {
                self.chain_link(&mut chain)?;
                let right = self.parse_proc7()?;
                left = Proc::PMatches(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_proc7(&mut self) -> Result<Proc, RholangError> {
        let mut left = self.parse_proc8()?;
        let mut chain = 0;
        loop {
            if self.peek() == &Tok::Lt {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc8()?;
                left = Proc::PLt(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::Lte {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc8()?;
                left = Proc::PLte(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::Gt {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc8()?;
                left = Proc::PGt(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::Gte {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc8()?;
                left = Proc::PGte(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_proc8(&mut self) -> Result<Proc, RholangError> {
        let mut left = self.parse_proc9()?;
        let mut chain = 0;
        loop {
            if self.peek() == &Tok::Plus {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc9()?;
                left = Proc::PAdd(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::Minus {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc9()?;
                left = Proc::PMinus(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::PlusPlus {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc9()?;
                left = Proc::PPlusPlus(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::MinusMinus {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc9()?;
                left = Proc::PMinusMinus(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_proc9(&mut self) -> Result<Proc, RholangError> {
        let mut left = self.parse_proc10()?;
        let mut chain = 0;
        loop {
            if self.peek() == &Tok::Star {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc10()?;
                left = Proc::PMult(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::Percent {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc10()?;
                left = Proc::PMod(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::PercentPercent {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc10()?;
                left = Proc::PPercentPercent(Box::new(left), Box::new(right));
            } else if self.peek() == &Tok::Slash {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc10()?;
                left = Proc::PDiv(Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_proc10(&mut self) -> Result<Proc, RholangError> {
        self.with_depth(|p| {
            if p.eat_ident("not") {
                let q = p.parse_proc10()?;
                Ok(Proc::PNot(Box::new(q)))
            } else if p.peek() == &Tok::Minus {
                p.next();
                let q = p.parse_proc10()?;
                Ok(Proc::PNeg(Box::new(q)))
            } else {
                p.parse_proc11()
            }
        })
    }

    fn parse_proc11(&mut self) -> Result<Proc, RholangError> {
        let mut left = self.parse_proc11_head()?;
        let mut chain = 0;
        loop {
            if self.peek() == &Tok::Dot {
                self.chain_link(&mut chain)?;
                self.next();
                if let Tok::Ident(method) = self.next() {
                    self.expect(Tok::LParen)?;
                    let mut args = Vec::new();
                    while self.peek() != &Tok::RParen {
                        args.push(self.parse_proc()?);
                        if self.peek() == &Tok::Comma {
                            self.next();
                        } else {
                            break;
                        }
                    }
                    self.expect(Tok::RParen)?;
                    left = Proc::PMethod(Box::new(left), method, args);
                } else {
                    return Err(RholangError::SyntaxError("expected method name".into()));
                }
            } else {
                break;
            }
        }
        Ok(left)
    }

    /// Whether the parenthesised interior at `self.pos` — which must hold an `LParen` — contains a
    /// **top-level comma**, i.e. whether the `(` opens a tuple rather than a group.
    ///
    /// A **scan of the tokens, not a speculative parse**, and that distinction is the second half of
    /// AUDIT C102. The first fix built the tuple from the group attempt's own partial parse, on the
    /// reasoning that an interior the group's `Proc4` can read is the same interior the full grammar
    /// would read. **That reasoning is false**, and `MultiSigRevVault.rho` is the counterexample:
    /// `parse_proc4` reads `bundle+{*m}` as the *arithmetic* `bundle + {*m}` — the `bundle` keyword
    /// production lives at a higher level — so a tuple whose first element is a bundle got a wrong
    /// tree, and the normalizer rejected the file with "Free variable bundle is used twice as a
    /// binder". `parse_proc4` is not a cheap oracle for "what does the full grammar see here".
    ///
    /// Deciding on the tokens means the interior is parsed **exactly once**, by the production that is
    /// right for it, so there is no re-parse to be exponential (each level's scan is bounded by its own
    /// interior, as the old speculative parse already was) and no partial parse to be wrong.
    fn lparen_opens_a_tuple(&self) -> bool {
        let mut depth = 0i32;
        // `get` rather than a slice range (`&self.toks[pos..]`): a variable slice range is an
        // `index`-class site in the type-system gate's books, and this needs no review, so it adds no
        // site — the same reasoning as the value walk's `saturating_add` (AUDIT C100).
        let mut i = self.pos.saturating_add(1);
        while let Some(t) = self.toks.get(i) {
            match t {
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth = depth.saturating_add(1),
                Tok::RParen | Tok::RBracket | Tok::RBrace => {
                    // The interior closed before any top-level comma: this is a group (or an error the
                    // group attempt and the collection path will sort out between them).
                    if depth == 0 {
                        return false;
                    }
                    depth = depth.saturating_sub(1);
                }
                Tok::Comma if depth == 0 => return true,
                _ => {}
            }
            i = i.saturating_add(1);
        }
        false
    }

    /// The head of a `Proc11`: a parenthesised expression (`PExprs ::= "(" Proc4 ")"`) or whatever
    /// the deeper levels produce.
    ///
    /// `(` is ambiguous between a **group** (`PExprs`, at this level) and a **tuple** (`TupleSingle
    /// ::= "(" Proc ",)"` / `TupleMultiple ::= "(" Proc "," [Proc] ")"`, a collection at the deepest
    /// level), and the grammar distinguishes them by *content*: a group's interior must be a `Proc4`
    /// (arithmetic and below — no sends, no `|`, no `new`), while a tuple requires a comma after its
    /// first element. So the tuple case is decided by `lparen_opens_a_tuple` **on the tokens** and
    /// handed straight to the collection path — the interior is read once, by the full grammar — and
    /// only the group case is tried speculatively here.
    ///
    /// The speculation rewinds for an interior this attempt could not read as a `Proc4` (a comma-less
    /// `(a!(b))`, a `(a | b)`, an unclosed paren), and that is cheap: the failure is detected at the
    /// first token the group's grammar cannot continue (the `!` of a send), not after a complete
    /// re-parse of a nested interior. **No successful parse is ever re-read** — which is what made
    /// nested tuples cost 2^n before this fix (parse only, debug, 32 MiB: 10 levels 7 ms, 20 levels
    /// 4.8 s, 30 levels > 45 s, per AUDIT C102).
    fn parse_proc11_head(&mut self) -> Result<Proc, RholangError> {
        if self.peek() != &Tok::LParen {
            return self.parse_proc12();
        }
        if self.lparen_opens_a_tuple() {
            // A tuple: let the collection path parse the interior once, with the full grammar.
            return self.parse_proc12();
        }
        let save = self.pos;
        self.next();
        match self.parse_proc4() {
            Ok(inner) if self.peek() == &Tok::RParen => {
                self.next();
                Ok(inner)
            }
            // The interior did not fit `Proc4`, or the `)` never came: hand the text back to the
            // collection path, which requires the comma too.
            _ => {
                self.pos = save;
                self.parse_proc12()
            }
        }
    }

    fn parse_proc12(&mut self) -> Result<Proc, RholangError> {
        if self.peek() == &Tok::Star {
            self.next();
            let name = self.parse_name()?;
            Ok(Proc::PEval(name))
        } else {
            self.parse_proc13()
        }
    }

    fn parse_proc13(&mut self) -> Result<Proc, RholangError> {
        if self.peek() == &Tok::Eq {
            // VarRefKindProc Var  or  VarRefKindName Var ("=" "*" Var)
            self.next();
            let kind = if self.peek() == &Tok::Star {
                self.next();
                VarRefKind::VarRefKindName
            } else {
                VarRefKind::VarRefKindProc
            };
            let var = self.parse_source_var()?;
            Ok(Proc::PVarRef(kind, var))
        } else {
            // `PDisjunction ::= Proc13 "\\/" Proc14` — the disjunction level, one step looser than
            // the conjunction level below it. There was no such loop, so the token could never be
            // consumed even once the lexer spelled it right: the parser side of `\/` was missing
            // entirely, while the normalizer's `normalize_disjunction` was already there.
            let mut left = self.parse_proc14()?;
            let mut chain = 0;
            while self.peek() == &Tok::Disj {
                self.chain_link(&mut chain)?;
                self.next();
                let right = self.parse_proc14()?;
                left = Proc::PDisjunction(Box::new(left), Box::new(right));
            }
            Ok(left)
        }
    }

    fn parse_proc14(&mut self) -> Result<Proc, RholangError> {
        let mut left = self.parse_proc15()?;
        let mut chain = 0;
        while self.peek() == &Tok::Conj {
            self.chain_link(&mut chain)?;
            self.next();
            let right = self.parse_proc15()?;
            left = Proc::PConjunction(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_proc15(&mut self) -> Result<Proc, RholangError> {
        self.with_depth(|p| {
            if p.peek() == &Tok::Tilde {
                p.next();
                let q = p.parse_proc15()?;
                Ok(Proc::PNegation(Box::new(q)))
            } else {
                p.parse_proc16()
            }
        })
    }

    fn parse_proc16(&mut self) -> Result<Proc, RholangError> {
        let mut target = if self.peek() == &Tok::LBrace {
            self.parse_braced_or_map()?
        } else if self.eat_ident("Nil") {
            Proc::PNil
        } else if let Some(t) = self.parse_simple_type()? {
            Proc::PSimpleType(t)
        } else if self.is_ground() {
            Proc::PGround(self.parse_ground()?)
        } else if self.is_collection() {
            Proc::PCollect(self.parse_collection()?)
        } else {
            Proc::PVar(self.parse_proc_var()?)
        };
        // Method calls (`receiver.method(args...)`) bind tighter than the operators above and chain
        // left-to-right. The argument list is **required**: `PMethod ::= Proc11 "." Var "(" [Proc]
        // ")"` (`rholang_mercury.cf:41`) has no paren-less form, so `x.m` used to parse as a method
        // call with no arguments — a term the grammar does not derive, and one the normalizer then
        // handed to a native method that would wait for an argument nothing would supply (AUDIT C33).
        let mut chain = 0;
        while self.peek() == &Tok::Dot {
            self.chain_link(&mut chain)?;
            self.next();
            let method = self.parse_source_var()?;
            self.expect(Tok::LParen)?;
            let mut args = Vec::new();
            while self.peek() != &Tok::RParen {
                args.push(self.parse_proc()?);
                if self.peek() == &Tok::Comma {
                    self.next();
                } else {
                    break;
                }
            }
            self.expect(Tok::RParen)?;
            target = Proc::PMethod(Box::new(target), method, args);
        }
        Ok(target)
    }

    /// Disambiguate `{ Proc }` (a braced process) from `{ KeyValuePair, ... }` (a map collection).
    /// Both productions start with `{` at the same precedence (`Proc16`), so the parser decides
    /// after reading the first process: a following `:` marks a map; otherwise it is a braced
    /// process. Empty braces are an empty map (there is no empty braced process).
    fn parse_braced_or_map(&mut self) -> Result<Proc, RholangError> {
        self.expect(Tok::LBrace)?;
        if self.peek() == &Tok::RBrace || self.peek() == &Tok::Ellipsis {
            // Empty braces are an empty map, and `{..._}` is an empty map *with a remainder* —
            // `CollectMap ::= "{" [KeyValuePair] ProcRemainder "}"` (`rholang_mercury.cf:183`) allows
            // no pairs. Without this arm the ellipsis was read as the first key's process and the
            // parser said `expected variable, got Ellipsis` (AUDIT C24; found by the corpus case
            // `@{..._}`, `rholang/tests/lean_normalize_corpus.rs`).
            let remainder = self.parse_proc_remainder()?;
            self.expect(Tok::RBrace)?;
            return Ok(Proc::PCollect(Collection::CollectMap(
                Vec::new(),
                remainder,
            )));
        }
        let first = self.parse_proc()?;
        if self.peek() != &Tok::Colon {
            self.expect(Tok::RBrace)?;
            return Ok(first);
        }
        let mut kvs = Vec::new();
        let mut key = first;
        loop {
            self.expect(Tok::Colon)?;
            let value = self.parse_proc()?;
            kvs.push(KeyValuePair(key, value));
            if self.peek() == &Tok::Comma {
                self.next();
                // A trailing separator closes the map (`{a: 1,}`) — accepted, see
                // `a_trailing_separator_is_accepted_as_a_deviation` — and a remainder is not a key,
                // so stop before `parse_proc` meets the ellipsis.
                if self.peek() == &Tok::RBrace || self.peek() == &Tok::Ellipsis {
                    break;
                }
                key = self.parse_proc()?;
            } else {
                break;
            }
        }
        let remainder = self.parse_proc_remainder()?;
        self.expect(Tok::RBrace)?;
        Ok(Proc::PCollect(Collection::CollectMap(kvs, remainder)))
    }

    fn parse_source_var(&mut self) -> Result<String, RholangError> {
        match self.next() {
            Tok::Ident(s) => Ok(s),
            Tok::Underscore => Ok("_".to_string()),
            t => Err(RholangError::SyntaxError(format!(
                "expected variable, got {t:?}"
            ))),
        }
    }

    fn parse_proc_var(&mut self) -> Result<ProcVar, RholangError> {
        if self.peek() == &Tok::Underscore {
            self.next();
            Ok(ProcVar::ProcVarWildcard)
        } else {
            Ok(ProcVar::ProcVarVar(self.parse_source_var()?))
        }
    }

    fn parse_name(&mut self) -> Result<Name, RholangError> {
        self.with_depth(|p| {
            if p.peek() == &Tok::Underscore {
                p.next();
                Ok(Name::NameWildcard)
            } else if p.peek() == &Tok::At {
                p.next();
                let q = p.parse_proc12()?;
                Ok(Name::NameQuote(Box::new(q)))
            } else {
                Ok(Name::NameVar(p.parse_source_var()?))
            }
        })
    }

    fn parse_send(&mut self) -> Result<Send, RholangError> {
        match self.next() {
            Tok::Bang => Ok(Send::SendSingle),
            Tok::BangBang => Ok(Send::SendMultiple),
            t => Err(RholangError::SyntaxError(format!(
                "expected send, got {t:?}"
            ))),
        }
    }

    fn parse_simple_type(&mut self) -> Result<Option<SimpleType>, RholangError> {
        let t = match self.peek() {
            Tok::Ident(s) if s == "Bool" => Some(SimpleType::SimpleTypeBool),
            Tok::Ident(s) if s == "Int" => Some(SimpleType::SimpleTypeInt),
            // `BigInt` is both a simple type and the head of the `GroundBigInt` production
            // (`"BigInt(" LongLiteral ")"`, `rholang_mercury.cf:172`), so the type form is the one
            // *not* followed by `(` — otherwise `BigInt(42)` read as the type and left `(42)` over
            // (AUDIT C34).
            Tok::Ident(s) if s == "BigInt" && !self.at_bigint_ground() => {
                Some(SimpleType::SimpleTypeBigInt)
            }
            Tok::Ident(s) if s == "String" => Some(SimpleType::SimpleTypeString),
            Tok::Ident(s) if s == "Uri" => Some(SimpleType::SimpleTypeUri),
            Tok::Ident(s) if s == "ByteArray" => Some(SimpleType::SimpleTypeByteArray),
            _ => None,
        };
        if t.is_some() {
            self.next();
        }
        Ok(t)
    }

    /// Is the cursor at `"BigInt" "(" Long ")"` — the grammar's `GroundBigInt`?
    fn at_bigint_ground(&self) -> bool {
        matches!(self.toks.get(self.pos), Some(Tok::Ident(s)) if s == "BigInt")
            && matches!(self.toks.get(self.pos + 1), Some(Tok::LParen))
            && matches!(self.toks.get(self.pos + 2), Some(Tok::Long(_)))
            && matches!(self.toks.get(self.pos + 3), Some(Tok::RParen))
    }

    fn is_ground(&self) -> bool {
        if self.at_bigint_ground() {
            return true;
        }
        match self.peek() {
            Tok::Long(_) | Tok::Str(_) | Tok::Uri(_) => true,
            Tok::Ident(s) => s == "true" || s == "false",
            _ => false,
        }
    }

    fn parse_ground(&mut self) -> Result<Ground, RholangError> {
        if self.at_bigint_ground() {
            self.next(); // `BigInt`
            self.next(); // `(`
            let digits = match self.next() {
                Tok::Long(n) => n.to_string(),
                t => {
                    return Err(RholangError::SyntaxError(format!(
                        "expected a LongLiteral in `BigInt(…)`, got {t:?}"
                    )))
                }
            };
            self.expect(Tok::RParen)?;
            return Ok(Ground::GroundBigInt(digits));
        }
        match self.next() {
            Tok::Long(n) => Ok(Ground::GroundInt(n.to_string())),
            Tok::Str(s) => Ok(Ground::GroundString(s)),
            Tok::Uri(u) => Ok(Ground::GroundUri(u)),
            Tok::Ident(s) if s == "true" => Ok(Ground::GroundBool(BoolLiteral::BoolTrue)),
            Tok::Ident(s) if s == "false" => Ok(Ground::GroundBool(BoolLiteral::BoolFalse)),
            t => Err(RholangError::SyntaxError(format!(
                "unexpected ground {t:?}"
            ))),
        }
    }

    fn is_collection(&self) -> bool {
        match self.peek() {
            Tok::LBracket | Tok::LParen | Tok::LBrace => true,
            Tok::Ident(s) => s == "Set",
            _ => false,
        }
    }

    /// A tuple's tail, given its **first element** and a current token of `Comma`: `TupleSingle` for
    /// `(x,)` and `TupleMultiple` for `(x, y, …)`.
    ///
    /// Named rather than inlined because it *is* the tuple production, and because the second attempt
    /// at AUDIT C102 wanted to call it from a second place — see `lparen_opens_a_tuple` for why that
    /// turned out to be the wrong shape and this is back to one caller.
    fn parse_tuple_tail(&mut self, first: Proc) -> Result<Collection, RholangError> {
        self.next(); // the comma
        if self.peek() == &Tok::RParen {
            self.next();
            Ok(Collection::CollectTuple(Tuple::TupleSingle(Box::new(
                first,
            ))))
        } else {
            let mut rest = Vec::new();
            while self.peek() != &Tok::RParen {
                rest.push(self.parse_proc()?);
                if self.peek() == &Tok::Comma {
                    self.next();
                    // `u ::= X | X "," [X]`, and `Tuple ::= "(" Proc "," [Proc] ")"`
                    // has no remainder, so `(1, 2,)` has no derivation. `(1,)` **does**:
                    // it is `TupleSingle ::= "(" Proc ",)"` and is handled above, which
                    // is why this check is here and not on the first comma.
                } else {
                    break;
                }
            }
            self.expect(Tok::RParen)?;
            Ok(Collection::CollectTuple(Tuple::TupleMultiple(
                Box::new(first),
                rest,
            )))
        }
    }

    fn parse_collection(&mut self) -> Result<Collection, RholangError> {
        match self.peek().clone() {
            Tok::LBracket => {
                self.next();
                let mut procs = Vec::new();
                // An *empty* element list may be followed by a remainder: the grammar is
                // `CollectList ::= "[" [Proc] ProcRemainder "]"`
                // (`rholang_mercury.cf:179`), so `[..._]` is a legal pattern and the loop must not
                // try to parse a process where the ellipsis starts. Found by the corpus
                // (`rholang/tests/lean_normalize_corpus.rs`, case `@{..._}`'s sibling) — AUDIT C24.
                while self.peek() != &Tok::RBracket && self.peek() != &Tok::Ellipsis {
                    procs.push(self.parse_proc()?);
                    if self.peek() == &Tok::Comma {
                        self.next();
                        // `[a, b, ...rest]`: the remainder is not a process, so stop before
                        // `parse_proc` sees the ellipsis and let `parse_proc_remainder` take it.
                        if self.peek() == &Tok::Ellipsis {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let remainder = self.parse_proc_remainder()?;
                self.expect(Tok::RBracket)?;
                Ok(Collection::CollectList(procs, remainder))
            }
            Tok::LParen => {
                self.next();
                let first = self.parse_proc()?;
                if self.peek() == &Tok::Comma {
                    self.parse_tuple_tail(first)
                } else {
                    // Reached only through `parse_proc11_head`'s fallback: the interior was not a
                    // `Proc4` (so it is not the `PExprs` group) *and* there is no comma (so it is
                    // not a tuple, whose productions — `TupleSingle ::= "(" Proc ",)"` and
                    // `TupleMultiple ::= "(" Proc "," [Proc] ")"` — both require one). `(a!(b))`
                    // and `(a | b)` land here, and both are syntax errors in the grammar: this
                    // parser used to accept them as one-element tuples, which silently changed the
                    // meaning of any expression feeding an operator (`(a + b) % c` reduced as
                    // `Tuple([a + b]) % c`).
                    return Err(RholangError::SyntaxError(format!(
                        "a tuple needs a comma: `(x,)` for one element, `(x, y)` for more, and a \
                         parenthesised expression may not contain a send, `|`, `new` or `if` \
                         (pos={})",
                        self.pos
                    )));
                }
            }
            Tok::LBrace => {
                self.next();
                let mut kvs = Vec::new();
                // As for lists: an empty map with only a remainder is `CollectMap ::= "{"
                // [KeyValuePair] ProcRemainder "}"` (`rholang_mercury.cf:183`).
                while self.peek() != &Tok::RBrace && self.peek() != &Tok::Ellipsis {
                    let k = self.parse_proc()?;
                    self.expect(Tok::Colon)?;
                    let v = self.parse_proc()?;
                    kvs.push(KeyValuePair(k, v));
                    if self.peek() == &Tok::Comma {
                        self.next();
                        // `{a: 1, ...rest}`: stop before `parse_proc` meets the ellipsis.
                        if self.peek() == &Tok::Ellipsis {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let remainder = self.parse_proc_remainder()?;
                self.expect(Tok::RBrace)?;
                Ok(Collection::CollectMap(kvs, remainder))
            }
            Tok::Ident(s) if s == "Set" => {
                self.next();
                self.expect(Tok::LParen)?;
                let mut procs = Vec::new();
                // As for lists: `CollectSet ::= "Set" "(" [Proc] ProcRemainder ")"`
                // (`rholang_mercury.cf:181`).
                while self.peek() != &Tok::RParen && self.peek() != &Tok::Ellipsis {
                    procs.push(self.parse_proc()?);
                    if self.peek() == &Tok::Comma {
                        self.next();
                        // `Set(a, b, ...rest)`: stop before `parse_proc` meets the ellipsis.
                        if self.peek() == &Tok::Ellipsis {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let remainder = self.parse_proc_remainder()?;
                self.expect(Tok::RParen)?;
                Ok(Collection::CollectSet(procs, remainder))
            }
            _ => Err(RholangError::SyntaxError("expected collection".into())),
        }
    }

    fn parse_proc_remainder(&mut self) -> Result<ProcRemainder, RholangError> {
        if self.peek() == &Tok::Ellipsis {
            self.next();
            Ok(ProcRemainder::ProcRemainderVar(self.parse_proc_var()?))
        } else {
            Ok(ProcRemainder::ProcRemainderEmpty)
        }
    }

    fn is_bundle(&self) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == "bundle")
    }

    fn parse_bundle(&mut self) -> Result<Bundle, RholangError> {
        // The lexer emits `bundle+`/`bundle-`/`bundle0` as `Ident("bundle")` followed by the suffix
        // token, so read the optional suffix to pick the bundle kind.
        match self.next() {
            Tok::Ident(s) if s == "bundle" => match self.peek() {
                Tok::Plus => {
                    self.next();
                    Ok(Bundle::BundleWrite)
                }
                Tok::Minus => {
                    self.next();
                    Ok(Bundle::BundleRead)
                }
                Tok::Long(0) => {
                    self.next();
                    Ok(Bundle::BundleEquiv)
                }
                _ => Ok(Bundle::BundleReadWrite),
            },
            t => Err(RholangError::SyntaxError(format!(
                "expected bundle, got {t:?}"
            ))),
        }
    }

    fn parse_name_decl(&mut self) -> Result<NameDecl, RholangError> {
        let var = self.parse_source_var()?;
        if self.peek() == &Tok::LParen {
            self.next();
            if let Tok::Uri(u) = self.next() {
                self.expect(Tok::RParen)?;
                Ok(NameDecl::NameDeclUrn(var, u))
            } else {
                Err(RholangError::SyntaxError(
                    "expected uri in name decl".into(),
                ))
            }
        } else {
            Ok(NameDecl::NameDeclSimpl(var))
        }
    }

    fn parse_receipt(&mut self) -> Result<Receipt, RholangError> {
        // Distinguish the arrow: linear "<-", peek "<<-", repeated "<=".
        let (names, remainder) = self.parse_bind_head()?;
        match self.peek() {
            Tok::LArrow => {
                self.next();
                let source = self.parse_name_source()?;
                let mut binds = vec![LinearBind(names, remainder, source)];
                while self.peek() == &Tok::Amp {
                    self.next();
                    let (n, r) = self.parse_bind_head()?;
                    self.expect(Tok::LArrow)?;
                    let s = self.parse_name_source()?;
                    binds.push(LinearBind(n, r, s));
                }
                Ok(Receipt::ReceiptLinear(ReceiptLinearImpl::LinearSimple(
                    binds,
                )))
            }
            Tok::LLArrow => {
                self.next();
                let source = self.parse_name()?;
                let mut binds = vec![PeekBind(names, remainder, source)];
                while self.peek() == &Tok::Amp {
                    self.next();
                    let (n, r) = self.parse_bind_head()?;
                    self.expect(Tok::LLArrow)?;
                    let s = self.parse_name()?;
                    binds.push(PeekBind(n, r, s));
                }
                Ok(Receipt::ReceiptPeek(ReceiptPeekImpl::PeekSimple(binds)))
            }
            Tok::Lte => {
                self.next();
                let source = self.parse_name()?;
                let mut binds = vec![RepeatedBind(names, remainder, source)];
                while self.peek() == &Tok::Amp {
                    self.next();
                    let (n, r) = self.parse_bind_head()?;
                    self.expect(Tok::Lte)?;
                    let s = self.parse_name()?;
                    binds.push(RepeatedBind(n, r, s));
                }
                Ok(Receipt::ReceiptRepeated(
                    ReceiptRepeatedImpl::RepeatedSimple(binds),
                ))
            }
            t => Err(RholangError::SyntaxError(format!(
                "expected <-, <<-, or <=, got {t:?}"
            ))),
        }
    }

    fn parse_bind_head(&mut self) -> Result<(Vec<Name>, NameRemainder), RholangError> {
        let mut names = Vec::new();
        while !matches!(
            self.peek(),
            Tok::LArrow | Tok::LLArrow | Tok::Lte | Tok::Ellipsis
        ) {
            names.push(self.parse_name()?);
            if self.peek() == &Tok::Comma {
                self.next();
            } else {
                break;
            }
        }
        let remainder = if self.peek() == &Tok::Ellipsis {
            self.next();
            NameRemainder::NameRemainderVar(self.parse_proc_var()?)
        } else {
            NameRemainder::NameRemainderEmpty
        };
        Ok((names, remainder))
    }

    fn parse_name_source(&mut self) -> Result<NameSource, RholangError> {
        let name = self.parse_name()?;
        if self.peek() == &Tok::BangQ {
            self.next();
            self.expect(Tok::LParen)?;
            let mut procs = Vec::new();
            while self.peek() != &Tok::RParen {
                procs.push(self.parse_proc()?);
                if self.peek() == &Tok::Comma {
                    self.next();
                } else {
                    break;
                }
            }
            self.expect(Tok::RParen)?;
            Ok(NameSource::SendReceiveSource(name, procs))
        } else if self.peek() == &Tok::QMark {
            self.next();
            Ok(NameSource::ReceiveSendSource(name))
        } else {
            Ok(NameSource::SimpleSource(name))
        }
    }

    fn parse_branch(&mut self) -> Result<Branch, RholangError> {
        let (names, remainder) = self.parse_bind_head()?;
        self.expect(Tok::LArrow)?;
        let source = self.parse_name_source()?;
        self.expect(Tok::Arrow)?;
        let body = self.parse_proc3()?;
        Ok(Branch(
            ReceiptLinearImpl::LinearSimple(vec![LinearBind(names, remainder, source)]),
            Box::new(body),
        ))
    }

    fn parse_case(&mut self) -> Result<Case, RholangError> {
        let pattern = self.parse_proc13()?;
        self.expect(Tok::Arrow)?;
        let body = self.parse_proc3()?;
        Ok(Case(Box::new(pattern), Box::new(body)))
    }

    fn parse_decl(&mut self) -> Result<Decl, RholangError> {
        let mut names = Vec::new();
        while self.peek() != &Tok::LArrow && self.peek() != &Tok::Ellipsis {
            names.push(self.parse_name()?);
            if self.peek() == &Tok::Comma {
                self.next();
            } else {
                break;
            }
        }
        let remainder = if self.peek() == &Tok::Ellipsis {
            self.next();
            NameRemainder::NameRemainderVar(self.parse_proc_var()?)
        } else {
            NameRemainder::NameRemainderEmpty
        };
        self.expect(Tok::LArrow)?;
        let mut procs = Vec::new();
        while self.peek() != &Tok::Semicolon
            && self.peek() != &Tok::Amp
            && self.peek() != &Tok::Ident("in".to_string())
        {
            procs.push(self.parse_proc()?);
            if self.peek() == &Tok::Comma {
                self.next();
            } else {
                break;
            }
        }
        Ok(Decl(names, remainder, procs))
    }

    fn parse_decls(&mut self) -> Result<Decls, RholangError> {
        if self.peek() == &Tok::Semicolon {
            self.next();
            let mut decls = Vec::new();
            while self.peek() != &Tok::Ident("in".to_string()) {
                decls.push(LinearDecl(self.parse_decl()?));
                if self.peek() == &Tok::Semicolon {
                    self.next();
                } else {
                    break;
                }
            }
            Ok(Decls::LinearDeclsImpl(decls))
        } else if self.peek() == &Tok::Amp {
            self.next();
            let mut decls = Vec::new();
            while self.peek() != &Tok::Ident("in".to_string()) {
                decls.push(ConcDecl(self.parse_decl()?));
                if self.peek() == &Tok::Amp {
                    self.next();
                } else {
                    break;
                }
            }
            Ok(Decls::ConcDeclsImpl(decls))
        } else {
            Ok(Decls::EmptyDeclImpl)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **AUDIT C30.** `parse` used to return `Ok` for a valid *prefix* of the input: `Tok::Eof` was
    /// pushed by the lexer and matched nowhere, so trailing input was discarded. `source_to_adt`
    /// runs on `deploy.data.term`, so a deploy whose term was a prefix of what the client wrote ran
    /// and reported success having executed something the client did not write.
    #[test]
    fn trailing_input_is_not_a_term() {
        for src in [
            "Nil )",
            "c!(1) garbage",
            "Nil Nil",
            "1 2",
            "new x in { Nil } extra",
            "@10!(10) /\\ @20!(20)",
            "@10!(10) \\/ @20!(20)",
        ] {
            assert!(
                parse(src).is_err(),
                "{src:?} is a term followed by more input, which is not a term"
            );
        }
        // The controls: each of those is a term on its own.
        for src in ["Nil", "c!(1)", "1", "new x in { Nil }"] {
            assert!(parse(src).is_ok(), "{src:?} is a term");
        }
    }

    /// **A trailing separator is accepted, as a registered deviation from the grammar.** The grammar's
    /// lists are `[X] ::= X | X "," [X]`, so `[1, 2,]` has no derivation and the port refused it — until
    /// this was checked against the code that was written for the *Scala* node. That code uses the
    /// spelling: `rhovision-org/rgov`'s `rholang/core/CrowdFund.rho` ends each parameter of its
    /// `contract CrowdFund(…)` head with a comma before a comment, and nine of the Scala's own test
    /// fixtures (`legacy/casper/src/test/resources/{ListOps,NonNegativeNumber,Mint,RegistryOps,…}Test.rho`)
    /// end list literals the same way — files that *ran* in the Scala suite, which is the evidence that
    /// the reference node accepted them. A node that refuses them cannot run the contracts it exists to
    /// run, so the acceptance is deliberate and this test is what states it. (The comma form of a
    /// *remainder*, `[head, ...tail]`, is the same class of deviation and is accepted for the same reason.)
    ///
    /// What is **not** relaxed: every other check from that pass stands — trailing *input* is refused, an
    /// omitted `in` is refused, a paren-less method call is refused (`a_method_call_needs_its_argument_list`).
    #[test]
    fn a_trailing_separator_is_accepted_as_a_deviation() {
        for src in [
            "[1,]",
            "Set(1,)",
            "{a: 1,}",
            "c!(1,)",
            "contract c(@x,) = { Nil }",
            "new x, in { Nil }",
            "(1, 2,)",
            "a.b(1,)",
            "for (x <- @\"c\";) { Nil }",
            // the shape that made this a compatibility bug, comment and all:
            "contract c(@x, // a comment\n @y,) = { Nil }",
        ] {
            assert!(
                parse(src).is_ok(),
                "{src:?} is what the contracts written for the Scala node look like"
            );
        }
        // The controls, unchanged: the derivable forms and the single-element tuple.
        for src in [
            "[1]", "[1, 2]", "(1,)", "Set(1)", "{a: 1}", "c!(1)", "a.b(1)", "a.b()",
        ] {
            assert!(parse(src).is_ok(), "{src:?} is derivable");
        }
    }

    /// A separator followed by a **remainder** stays accepted, and that is a decision with evidence
    /// rather than slack: the grammar separates a remainder from the list by no terminal
    /// (`CollectList ::= "[" [Proc] ProcRemainder "]"`), so `{name: *voter, ...tail}` is equally
    /// underivable — and it is how `Issue.rho:110` and `Ballot.rho:106` are written. Rejecting it
    /// would stop the vendored contracts from loading, so it is a recorded deviation (law 31's data
    /// list), not a rejection.
    #[test]
    fn a_comma_before_a_remainder_is_the_deviation_the_contracts_use() {
        for src in [
            "[a, ...rest]",
            "{name: *voter, ...tail}",
            "Set(a, ...rest)",
            "[a ...rest]",
        ] {
            assert!(parse(src).is_ok(), "{src:?} must stay accepted");
        }
    }

    /// **AUDIT C32.** `PNew ::= "new" [NameDecl] "in" Proc1` and `PLet ::= "let" Decl Decls "in"
    /// "{" Proc "}"` both make `in` mandatory, and both dropped `eat_ident`'s boolean — so `new x
    /// Nil` parsed as a `new` whose body was `Nil`, with the grammar's keyword missing.
    #[test]
    fn in_is_required_by_new_and_let() {
        assert!(parse("new x Nil").is_err());
        assert!(parse("new x, y in { Nil }").is_ok());
        assert!(parse("let x <- 1 in { Nil }").is_ok());
    }

    /// **AUDIT C33.** `PMethod ::= Proc11 "." Var "(" [Proc] ")"` has no paren-less form. The port
    /// accepted `x.m` and produced a call with **no arguments**, which the normalizer then handed to
    /// a native method that waits for an argument nothing will send. The control is `x.m()` — the
    /// empty argument list the grammar does have.
    #[test]
    fn a_method_call_needs_its_argument_list() {
        assert!(parse("x.m").is_err(), "`x.m` is not a term of the grammar");
        assert!(parse("x.m()").is_ok());
        assert!(parse("x.m(1)").is_ok());
        assert!(parse("x.m(1, 2)").is_ok());
    }

    /// **AUDIT C34.** `GroundBigInt ::= "BigInt(" LongLiteral ")"` had no parser arm: `BigInt(42)`
    /// parsed as the *simple type* `BigInt` followed by a discarded `(42)`, while the normalizer
    /// already supported the ground (`normalizer.rs:59`). `BigInt` alone stays the simple type.
    #[test]
    fn bigint_is_a_ground_and_bigint_alone_is_a_type() {
        assert!(matches!(
            parse("BigInt(42)").unwrap(),
            Proc::PGround(Ground::GroundBigInt(_))
        ));
        assert!(matches!(
            parse("BigInt").unwrap(),
            Proc::PSimpleType(SimpleType::SimpleTypeBigInt)
        ));
    }

    /// **AUDIT C35.** `PSendSynch ::= Name "!?" "(" [Proc] ")" SynchSendCont` with
    /// `EmptyCont ::= "."` / `NonEmptyCont ::= "; " Proc1`: the token was lexed and read only by
    /// `parse_name_source`, so a synchronous send in **process** position had no parser path —
    /// `x!?(1); P` parsed as the bare variable `x` with the rest discarded. That its `SynchSendCont`
    /// continuation is present is the assertion: a `PSendSynch` whose tail was dropped would not be
    /// distinguishable from one whose tail was parsed.
    #[test]
    fn a_synchronous_send_parses_with_its_continuation() {
        match parse("x!?(1); Nil").unwrap() {
            Proc::PSendSynch(_, data, SynchSendCont::NonEmptyCont(_)) => assert_eq!(data.len(), 1),
            other => panic!("expected a synchronous send with a continuation, got {other:?}"),
        }
        assert!(matches!(
            parse("x!?(1).").unwrap(),
            Proc::PSendSynch(_, _, SynchSendCont::EmptyCont)
        ));
        // The grammar requires one of the two continuations.
        assert!(parse("x!?(1)").is_err());
    }

    /// **AUDIT C36.** The lexer sliced `chars[start..i]` with `i == len + 1` for an unterminated
    /// literal — a **panic**, not a diagnostic — and an unterminated block comment swallowed the
    /// rest of the source, so `Nil /* oops` lexed cleanly and parsed as `Nil`.
    #[test]
    fn unterminated_lexical_forms_are_errors_not_crashes() {
        for src in ["\"abc", "`abc", "Nil /* oops", "Nil /*"] {
            assert!(parse(src).is_err(), "{src:?} is unterminated");
        }
        assert!(parse("\"abc\"").is_ok());
        assert!(parse("`abc`").is_ok());
        assert!(parse("Nil /* ok */").is_ok());
    }

    #[test]
    fn parses_nil() {
        let p = parse("Nil").unwrap();
        assert_eq!(p, Proc::PNil);
    }

    /// A `ProcRemainder` applies to **every** collection form, but only the map branch broke on
    /// the ellipsis: the list and set branches consumed the comma unconditionally and then called
    /// `parse_proc` on `...`, which is not a process. So `[=*type, ...item]` — how `Inbox.rho`
    /// destructures a message queue, and therefore how the whole rgov governance set that imports
    /// it destructures one — failed to parse with `expected variable, got Ellipsis`, while
    /// `{a: 1, ...rest}` was accepted. The remainder must also be *carried*, not dropped.
    #[test]
    fn collection_remainders_parse_for_lists_and_sets_not_only_maps() {
        for src in [
            "[a, ...rest]",
            "[=*type, ...item]",
            "[=*type, =*subtype, ...item]",
            "[a, ..._]",
            "Set(a, ...rest)",
            "{a: 1, ...rest}",
            "{a: 1, ..._}",
            // A collection with *no* elements and only a remainder is in the grammar too
            // (`[Proc]`/`[KeyValuePair]` may be empty), and neither parser arm accepted it: the
            // element loops could not start on an ellipsis (AUDIT C24).
            "[..._]",
            "[...rest]",
            "Set(..._)",
            "{..._}",
        ] {
            let parsed = parse(src).unwrap_or_else(|e| panic!("`{src}` must parse: {e}"));
            let remainder = match &parsed {
                Proc::PCollect(Collection::CollectList(_, r))
                | Proc::PCollect(Collection::CollectSet(_, r))
                | Proc::PCollect(Collection::CollectMap(_, r)) => r,
                other => panic!("`{src}` parsed as {other:?}, expected a collection"),
            };
            assert!(
                matches!(remainder, ProcRemainder::ProcRemainderVar(_)),
                "`{src}`: the remainder should be bound to a var, got {remainder:?}"
            );
        }

        // A comma-separated collection with no ellipsis still has an empty remainder.
        let Proc::PCollect(Collection::CollectList(_, remainder)) = parse("[a, b]").unwrap() else {
            panic!("expected a list")
        };
        assert!(matches!(remainder, ProcRemainder::ProcRemainderEmpty));

        // The whole shape that failed in the wild, verbatim from `Inbox.rho`: a match arm whose
        // pattern is a par containing a map containing a list-with-remainder.
        parse(
            "match (*items) {
               {[=*type, ...item] | rest} => { ret!(item) | box!(rest) }
               _ => { box!(*items) | ret!(Nil) }
             }",
        )
        .expect("the Inbox.rho read-pattern must parse");
    }

    #[test]
    fn rejects_excessive_nesting_depth() {
        // Deeply nested `new`/`not`/`~` terms must error rather than overflow the stack.
        let deep_new = format!("{}Nil", "new x in ".repeat(MAX_PARSE_DEPTH + 10));
        assert!(parse(&deep_new).is_err());

        let deep_not = format!("{}Nil", "not ".repeat(MAX_PARSE_DEPTH + 10));
        assert!(parse(&deep_not).is_err());
    }

    /// The three guards are separate claims: parser nesting, flat-chain width, and the composed
    /// AST depth. Each has a refusal, and the control just under the composed bound still parses.
    #[test]
    fn the_three_depth_guards_refuse_an_over_deep_term() {
        let deep_new = format!("{}Nil", "new x in ".repeat(MAX_PARSE_DEPTH + 10));
        assert!(
            parse(&deep_new)
                .unwrap_err()
                .to_string()
                .contains("parse depth exceeded"),
            "the parser-recursion guard must name its refusal"
        );
        let over_chain = format!("1{}", " + 1".repeat(MAX_CHAIN_LENGTH + 1));
        assert!(
            parse(&over_chain)
                .unwrap_err()
                .to_string()
                .contains("operator chain too long"),
            "the flat-chain guard must name its refusal"
        );
        assert!(
            parse(&nested_chain(4, 200))
                .unwrap_err()
                .to_string()
                .contains("term nesting too deep"),
            "the composed AST guard must refuse 4 levels x 200 links"
        );
        assert!(
            parse(&nested_chain(3, 100)).is_ok(),
            "the control just under the composed bound must parse"
        );
    }

    /// The **product** of the two guards is what `MAX_AST_DEPTH` exists for, and the shape below is
    /// inside both of them: 4 levels against `MAX_PARSE_DEPTH`'s 128, and 200 operators against
    /// `MAX_CHAIN_LENGTH`'s 512 — while the AST is ~800 deep. Before the AST budget this parsed and the
    /// normalizer then recursed 800 levels; the measured cost is in `MAX_AST_DEPTH`'s comment, and the
    /// abort it prevents is AUDIT C99.
    #[test]
    fn rejects_a_deep_ast_that_stays_inside_both_component_guards() {
        let inner = nested_chain(4, 200);
        assert!(
            parse(&inner).is_err(),
            "4 levels x 200 links is inside both guards and ~800 deep: the AST budget must refuse it"
        );
    }

    /// The control for the test above: the same *shape*, just under the budget, still parses. Without
    /// it the refusal could be a generator that emits nonsense rather than a bound doing its job.
    #[test]
    fn accepts_a_deep_ast_under_the_budget() {
        let inner = nested_chain(3, 100);
        assert!(
            parse(&inner).is_ok(),
            "3 levels x 100 links is ~303 deep, well under the budget"
        );
    }

    /// A maximal single chain is still legal: the AST budget must not quietly lower
    /// `MAX_CHAIN_LENGTH`. `MAX_CHAIN_LENGTH + 1 = 513` is the deepest a legitimate flat chain reaches,
    /// which is why the budget is above it.
    #[test]
    fn a_maximal_chain_still_parses() {
        let src = format!("1{}", " + 1".repeat(MAX_CHAIN_LENGTH));
        assert!(parse(&src).is_ok());
        let over = format!("1{}", " + 1".repeat(MAX_CHAIN_LENGTH + 1));
        assert!(parse(&over).is_err(), "the chain guard still fires first");
    }

    /// `depth` nested parens, each holding a flat chain of `chain` additions — the shape that composes
    /// the two guards into an AST of depth `depth * (chain + 1)`.
    fn nested_chain(depth: usize, chain: usize) -> String {
        let mut inner = String::from("1");
        let link = " + 1".repeat(chain);
        for _ in 0..depth {
            inner = format!("({inner}{link})");
        }
        inner
    }

    /// `depth`-deep nested 2-tuples: `(((1, 1), 1), 1)` — the shape whose parse cost was exponential
    /// (AUDIT C102).
    fn nested_tuple(depth: usize) -> String {
        let mut inner = String::from("1");
        for _ in 0..depth {
            inner = format!("({inner}, 1)");
        }
        inner
    }

    /// **Nested tuples parse in time linear in their nesting** (AUDIT C102).
    ///
    /// `(` is ambiguous between a group and a tuple, and the parser decides on the tokens
    /// (`lparen_opens_a_tuple`). Until this unit's fix the comma case *rewound*, letting the collection
    /// path read the interior a second time — and that re-read re-entered the next level's decision, so
    /// the cost was 2^n. Measured before the fix, parse only, on a 32 MiB stack, in a debug build: 10
    /// levels 7 ms, **20 levels 4.8 s**, and 30 levels did not finish in 45 s. The assertion is loose on
    /// purpose — it guards an asymptote, and the gap is orders of magnitude (microseconds against
    /// seconds), so a loaded CI machine cannot make it flap.
    ///
    /// The depths are the ones **both** parsers accept: deciding on the tokens costs the recursive
    /// descent a few units of `MAX_PARSE_DEPTH` per tuple level, so ~28 levels is this shape's ceiling
    /// under a budget of 128 where the old route reached deeper *in theory only* — 32 levels now report
    /// `parse depth exceeded` in microseconds, and the old parser did not complete 40 levels in 900 s.
    /// Every depth at which the old parser returned `Ok` in measurable time (20 levels, 4.8 s) is still
    /// accepted, which is the property this pins.
    #[test]
    fn nested_tuples_parse_in_linear_time() {
        // The parser is recursive and frame-heavy per level — 10 levels overflows a default test stack
        // — so this runs on the node's own 32 MiB worker size (the `rho_examples.rs` pattern).
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let mut worst = std::time::Duration::ZERO;
                for depth in [10usize, 20, 28] {
                    let started = std::time::Instant::now();
                    let parsed = parse(&nested_tuple(depth))
                        .unwrap_or_else(|e| panic!("{depth} nested tuples is a legal term: {e:?}"));
                    worst = worst.max(started.elapsed());
                    assert!(
                        matches!(
                            parsed,
                            Proc::PCollect(Collection::CollectTuple(Tuple::TupleMultiple(..)))
                        ),
                        "depth {depth} must parse as a tuple, not a group"
                    );
                }
                assert!(
                    worst < std::time::Duration::from_secs(2),
                    "the slowest nesting took {worst:?} — the speculative re-parse is back (AUDIT \
                     C102): before the fix 20 levels took 4.8 s and 30 did not finish in 45 s"
                );
            })
            .expect("spawn")
            .join()
            .expect("the parser must not overflow its stack on 28 levels");
    }

    /// `(x)` is a **group** (`PExprs ::= "(" Proc4 ")"`), not a tuple: the tuple productions
    /// (`TupleSingle ::= "(" Proc ",)"`, `TupleMultiple ::= "(" Proc "," [Proc] ")"`) both require
    /// a comma. This parser built a one-element tuple out of the bare form, so `(a + b) % c`
    /// became `Tuple([a + b]) % c` — a reduce-time type error instead of the arithmetic it is, and
    /// the reason `Registry.rho` and `MakeMint.rho` could not be reduced at all.
    /// The two logical connectives, which the port had **swapped** at the lexer and left
    /// unparseable at the parser: `\/` (disjunction) lexed as the conjunction token and reduced as
    /// `ConnAnd`, while `/\` (conjunction) did not lex at all — `/` was taken as division and the
    /// backslash was then an illegal character. `Proc::PDisjunction` and the normalizer's
    /// `normalize_disjunction` were already written, so nothing could ever reach them.
    #[test]
    fn the_logical_connectives_lex_and_parse_in_their_grammar_spelling() {
        assert!(
            matches!(parse("a /\\ b").unwrap(), Proc::PConjunction(..)),
            "`/\\` is conjunction"
        );
        assert!(
            matches!(parse("a \\/ b").unwrap(), Proc::PDisjunction(..)),
            "`\\/` is disjunction"
        );
        // Precedence: `/\` (Proc14) binds tighter than `\/` (Proc13), so the right operand of the
        // disjunction is the whole conjunction.
        let mixed = parse("a \\/ b /\\ c").unwrap();
        let Proc::PDisjunction(_, right) = &mixed else {
            panic!("expected a disjunction at the top, got {mixed:?}")
        };
        assert!(
            matches!(**right, Proc::PConjunction(..)),
            "`/\\` must bind tighter: {right:?}"
        );
        // Both are left-associative, like the grammar's `Proc13 ::= Proc13 "\\/" Proc14`.
        assert!(matches!(
            parse("a \\/ b \\/ c").unwrap(),
            Proc::PDisjunction(..)
        ));
        // Division is untouched by the longer token.
        assert!(parse("6 / 2").is_ok());
        // `\\` is not an operator in the grammar; it is not a token here either.
        assert!(parse("a \\\\ b").is_err(), "`\\\\` must not lex");
    }

    #[test]
    fn a_parenthesised_expression_is_a_group_not_a_one_element_tuple() {
        // A group contributes no AST node: it is its interior.
        assert_eq!(parse("(3 + 5)").unwrap(), parse("3 + 5").unwrap());
        assert_eq!(parse("((a))").unwrap(), parse("a").unwrap());
        // So it composes with the enclosing operator, which is the whole point.
        assert!(matches!(parse("2 * (3 + 5)").unwrap(), Proc::PMult(..)));
        // And a *tuple* still needs its comma, at one element and at many.
        assert!(matches!(
            parse("(3,)").unwrap(),
            Proc::PCollect(Collection::CollectTuple(Tuple::TupleSingle(_)))
        ));
        assert!(matches!(
            parse("(3, 4)").unwrap(),
            Proc::PCollect(Collection::CollectTuple(Tuple::TupleMultiple(..)))
        ));
    }

    /// A parenthesised expression may not contain a send, `|`, `new` or `if`: `PExprs`'s interior
    /// is a `Proc4`, which is looser than a tuple's `Proc` but tighter than a full `Proc`. Both
    /// sides of that boundary are asserted, because accepting too much here is the same class of
    /// laxness as the one-element tuple was.
    #[test]
    fn a_group_may_not_contain_a_send_or_a_parallel() {
        for src in ["(a!(b))", "(a | b)", "(new x in { Nil })"] {
            let err = parse(src).expect_err("must not parse: {src}");
            assert!(
                format!("{err}").contains("a tuple needs a comma"),
                "{src}: unexpected error {err}"
            );
        }
        // The boundary holds from the other side: `Proc4` is `or`/`==`/`+`/`*` and below, so those
        // are legal in a group...
        for src in [
            "(a or b)",
            "(a == b)",
            "(a + b)",
            "(a * b)",
            "([1, 2].nth(0) + 1)",
        ] {
            assert!(parse(src).is_ok(), "{src} must parse");
        }
        // ...and a *tuple* may hold a send, because `TupleSingle`/`TupleMultiple` take a full
        // `Proc` (which is why the group is tried before the tuple, not after).
        assert!(matches!(
            parse("(a!(b), c)").unwrap(),
            Proc::PCollect(Collection::CollectTuple(Tuple::TupleMultiple(..)))
        ));
    }

    /// The forms that use parentheses for their *own* syntax are unaffected: the `if` condition
    /// (a full `Proc`), `for` binders, `contract` parameters and method-call arguments.
    #[test]
    fn the_parenthesised_forms_the_grammar_allows_still_parse() {
        assert!(parse("if (1 == 1) { Nil }").is_ok(), "if condition");
        assert!(parse("if (true) { Nil } else { Nil }").is_ok(), "if/else");
        assert!(
            parse("if (a!(b)) { Nil }").is_ok(),
            "an if condition is a full Proc, send included"
        );
        assert!(parse("for (x <- y) { Nil }").is_ok(), "for binder");
        assert!(
            parse("contract c(@x, y) = { Nil }").is_ok(),
            "contract params"
        );
        assert!(parse("[1, 2].nth(0)").is_ok(), "method call");
    }

    #[test]
    fn parses_int() {
        let p = parse("42").unwrap();
        assert_eq!(p, Proc::PGround(Ground::GroundInt("42".to_string())));
    }

    /// **A `d`-suffixed decimal literal no longer parses** (issue #527). The two tests that were
    /// here pinned the literal (`2d`, `1.5d`, `007.50d`, `1.5d.toString()`) and its malformed
    /// forms; the type is withdrawn and the lexer no longer has the token. What matters now is the
    /// refusal, and that a bare `1.5` and `1.toString()` still behave as they did — the lookahead
    /// that used to consume a fraction only before `d` is gone, so `1.5d` is `Long Dot Ident` and
    /// the parser refuses it.
    #[test]
    fn decimal_literals_no_longer_parse() {
        for src in [
            "2d",
            "1.5d",
            "-1.5d",
            "0.5d",
            "1.5d.toString()",
            "BigInt(1.5d)",
        ] {
            assert!(
                parse(src).is_err(),
                "{src:?} must not parse now that the decimal type is withdrawn"
            );
        }
        assert!(parse("1.toString()").is_ok());
    }

    #[test]
    fn parses_send() {
        let p = parse("x!(1)").unwrap();
        assert_eq!(
            p,
            Proc::PSend(
                Name::NameVar("x".to_string()),
                Send::SendSingle,
                vec![Proc::PGround(Ground::GroundInt("1".to_string()))],
            )
        );
    }

    #[test]
    fn parses_arith_precedence() {
        let p = parse("1 + 2 * 3").unwrap();
        assert_eq!(
            p,
            Proc::PAdd(
                Box::new(Proc::PGround(Ground::GroundInt("1".to_string()))),
                Box::new(Proc::PMult(
                    Box::new(Proc::PGround(Ground::GroundInt("2".to_string()))),
                    Box::new(Proc::PGround(Ground::GroundInt("3".to_string()))),
                )),
            )
        );
    }

    /// A `Proc` whose deepest node sits at depth `n` in `exceeds_ast_depth`'s accounting.
    ///
    /// The convention is the walk's own, read off `parser.rs:413-418`: the root is seeded at depth 1
    /// and a node popped at `d` hands its children `d + 1`, while `PNegation`'s arm pushes its child
    /// at exactly the depth it was handed (`parser.rs:433`). So every `PNegation` costs exactly one
    /// level and `n - 1` of them around `PNil` is depth `n`. The test below re-derives that boundary
    /// in both directions before it leans on it.
    fn proc_of_depth(n: usize) -> Proc {
        let mut p = Proc::PNil;
        for _ in 1..n {
            p = Proc::PNegation(Box::new(p));
        }
        p
    }

    /// One case per position `push_sub_procs` (`parser.rs:424`) descends into, with `child` parked in
    /// that position.
    ///
    /// The parameter is the point: the list is built twice by the caller, once around a deep child
    /// and once around a depth-1 one, so a case's "too deep" verdict is attributable to the child
    /// having been reached rather than to the shape of the case around it.
    fn walk_cases(child: &Proc) -> Vec<(String, Proc)> {
        use Proc::*;
        let mut cases: Vec<(String, Proc)> = Vec::new();
        let deep = || Box::new(child.clone());
        let nil = || Box::new(Proc::PNil);
        let quote = || Name::NameQuote(Box::new(child.clone()));

        // The four unary variants share one arm, so each gets its own case: one missing
        // `out.push` in that arm costs all four at once.
        cases.push(("PNegation.0".to_string(), PNegation(deep())));
        cases.push(("PNot.0".to_string(), PNot(deep())));
        cases.push(("PNeg.0".to_string(), PNeg(deep())));
        cases.push(("PExprs.0".to_string(), PExprs(deep())));
        cases.push((
            "PBundle.body".to_string(),
            PBundle(Bundle::BundleRead, deep()),
        ));

        // Every binary arm pushes both operands, and the variants share that one arm — so the left
        // and the right position of each variant are separate cases: a dropped `out.push((b, d))`
        // is invisible to a case that parks its child on the left.
        let binary: [(&str, fn(Box<Proc>, Box<Proc>) -> Proc); 22] = [
            ("PConjunction", PConjunction),
            ("PDisjunction", PDisjunction),
            ("PMult", PMult),
            ("PDiv", PDiv),
            ("PMod", PMod),
            ("PPercentPercent", PPercentPercent),
            ("PAdd", PAdd),
            ("PMinus", PMinus),
            ("PPlusPlus", PPlusPlus),
            ("PMinusMinus", PMinusMinus),
            ("PLt", PLt),
            ("PLte", PLte),
            ("PGt", PGt),
            ("PGte", PGte),
            ("PMatches", PMatches),
            ("PEq", PEq),
            ("PNeq", PNeq),
            ("PAnd", PAnd),
            ("PShortAnd", PShortAnd),
            ("POr", POr),
            ("PShortOr", PShortOr),
            ("PPar", PPar),
        ];
        for (name, ctor) in binary {
            cases.push((format!("{name}.left"), ctor(deep(), nil())));
            cases.push((format!("{name}.right"), ctor(nil(), deep())));
        }

        cases.push((
            "PMethod.target".to_string(),
            PMethod(deep(), "m".to_string(), vec![]),
        ));
        cases.push((
            "PMethod.arguments".to_string(),
            PMethod(nil(), "m".to_string(), vec![child.clone()]),
        ));
        // `PEval` reaches its child only through `push_name`, which quotes a `NameQuote` and passes
        // over the other two `Name` shapes: this is the one position where a `Name` is the edge.
        cases.push(("PEval.NameQuote".to_string(), PEval(quote())));

        cases.push((
            "PCollect.CollectList.items".to_string(),
            PCollect(Collection::CollectList(
                vec![child.clone()],
                ProcRemainder::ProcRemainderEmpty,
            )),
        ));
        cases.push((
            "PCollect.CollectSet.items".to_string(),
            PCollect(Collection::CollectSet(
                vec![child.clone()],
                ProcRemainder::ProcRemainderEmpty,
            )),
        ));
        cases.push((
            "PCollect.CollectTuple.TupleSingle".to_string(),
            PCollect(Collection::CollectTuple(Tuple::TupleSingle(deep()))),
        ));
        cases.push((
            "PCollect.CollectTuple.TupleMultiple.head".to_string(),
            PCollect(Collection::CollectTuple(Tuple::TupleMultiple(
                deep(),
                vec![],
            ))),
        ));
        cases.push((
            "PCollect.CollectTuple.TupleMultiple.tail".to_string(),
            PCollect(Collection::CollectTuple(Tuple::TupleMultiple(
                nil(),
                vec![child.clone()],
            ))),
        ));
        cases.push((
            "PCollect.CollectMap.key".to_string(),
            PCollect(Collection::CollectMap(
                vec![KeyValuePair(child.clone(), Proc::PNil)],
                ProcRemainder::ProcRemainderEmpty,
            )),
        ));
        cases.push((
            "PCollect.CollectMap.value".to_string(),
            PCollect(Collection::CollectMap(
                vec![KeyValuePair(Proc::PNil, child.clone())],
                ProcRemainder::ProcRemainderEmpty,
            )),
        ));

        cases.push((
            "PSend.chan".to_string(),
            PSend(quote(), Send::SendSingle, vec![]),
        ));
        cases.push((
            "PSend.data".to_string(),
            PSend(
                Name::NameVar("x".to_string()),
                Send::SendSingle,
                vec![child.clone()],
            ),
        ));
        cases.push((
            "PContr.name".to_string(),
            PContr(quote(), vec![], NameRemainder::NameRemainderEmpty, nil()),
        ));
        cases.push((
            "PContr.names".to_string(),
            PContr(
                Name::NameVar("c".to_string()),
                vec![quote()],
                NameRemainder::NameRemainderEmpty,
                nil(),
            ),
        ));
        cases.push((
            "PContr.body".to_string(),
            PContr(
                Name::NameVar("c".to_string()),
                vec![],
                NameRemainder::NameRemainderEmpty,
                deep(),
            ),
        ));

        // The receipt helpers: `push_receipt` reaches three shapes, `push_linear` two positions
        // inside a `LinearBind`, and `push_receipt_source` three `NameSource` shapes — one of which,
        // `SendReceiveSource`, carries procs of its own.
        let linear =
            |binds: Vec<LinearBind>| Receipt::ReceiptLinear(ReceiptLinearImpl::LinearSimple(binds));
        let bind = |names: Vec<Name>, src: NameSource| {
            LinearBind(names, NameRemainder::NameRemainderEmpty, src)
        };
        let source = |n: Name| NameSource::SimpleSource(n);
        cases.push((
            "PInput.receipts[].ReceiptLinear.bind.names".to_string(),
            PInput(
                vec![linear(vec![bind(
                    vec![quote()],
                    source(Name::NameVar("x".to_string())),
                )])],
                nil(),
            ),
        ));
        cases.push((
            "PInput.receipts[].ReceiptLinear.bind.source.SimpleSource".to_string(),
            PInput(vec![linear(vec![bind(vec![], source(quote()))])], nil()),
        ));
        cases.push((
            "PInput.receipts[].ReceiptLinear.bind.source.ReceiveSendSource".to_string(),
            PInput(
                vec![linear(vec![bind(
                    vec![],
                    NameSource::ReceiveSendSource(quote()),
                )])],
                nil(),
            ),
        ));
        cases.push((
            "PInput.receipts[].ReceiptLinear.bind.source.SendReceiveSource.chan".to_string(),
            PInput(
                vec![linear(vec![bind(
                    vec![],
                    NameSource::SendReceiveSource(quote(), vec![]),
                )])],
                nil(),
            ),
        ));
        cases.push((
            "PInput.receipts[].ReceiptLinear.bind.source.SendReceiveSource.procs".to_string(),
            PInput(
                vec![linear(vec![bind(
                    vec![],
                    NameSource::SendReceiveSource(
                        Name::NameVar("x".to_string()),
                        vec![child.clone()],
                    ),
                )])],
                nil(),
            ),
        ));
        cases.push((
            "PInput.receipts[].ReceiptRepeated.bind.names".to_string(),
            PInput(
                vec![Receipt::ReceiptRepeated(
                    ReceiptRepeatedImpl::RepeatedSimple(vec![RepeatedBind(
                        vec![quote()],
                        NameRemainder::NameRemainderEmpty,
                        Name::NameVar("x".to_string()),
                    )]),
                )],
                nil(),
            ),
        ));
        cases.push((
            "PInput.receipts[].ReceiptRepeated.bind.source".to_string(),
            PInput(
                vec![Receipt::ReceiptRepeated(
                    ReceiptRepeatedImpl::RepeatedSimple(vec![RepeatedBind(
                        vec![],
                        NameRemainder::NameRemainderEmpty,
                        quote(),
                    )]),
                )],
                nil(),
            ),
        ));
        cases.push((
            "PInput.receipts[].ReceiptPeek.bind.names".to_string(),
            PInput(
                vec![Receipt::ReceiptPeek(ReceiptPeekImpl::PeekSimple(vec![
                    PeekBind(
                        vec![quote()],
                        NameRemainder::NameRemainderEmpty,
                        Name::NameVar("x".to_string()),
                    ),
                ]))],
                nil(),
            ),
        ));
        cases.push((
            "PInput.receipts[].ReceiptPeek.bind.source".to_string(),
            PInput(
                vec![Receipt::ReceiptPeek(ReceiptPeekImpl::PeekSimple(vec![
                    PeekBind(vec![], NameRemainder::NameRemainderEmpty, quote()),
                ]))],
                nil(),
            ),
        ));
        cases.push(("PInput.body".to_string(), PInput(vec![], deep())));

        cases.push((
            "PChoice.branches[].bind.names".to_string(),
            PChoice(vec![Branch(
                ReceiptLinearImpl::LinearSimple(vec![bind(
                    vec![quote()],
                    source(Name::NameVar("x".to_string())),
                )]),
                nil(),
            )]),
        ));
        cases.push((
            "PChoice.branches[].body".to_string(),
            PChoice(vec![Branch(
                ReceiptLinearImpl::LinearSimple(vec![]),
                deep(),
            )]),
        ));

        cases.push(("PMatch.target".to_string(), PMatch(deep(), vec![])));
        cases.push((
            "PMatch.cases[].pattern".to_string(),
            PMatch(nil(), vec![Case(deep(), nil())]),
        ));
        cases.push((
            "PMatch.cases[].body".to_string(),
            PMatch(nil(), vec![Case(nil(), deep())]),
        ));

        // `PLet`'s `Decls` arm is an `if let`, not a `match`: `EmptyDeclImpl` carries no decl and so
        // has no position, and the `decl` position is reachable through all three shapes (the third
        // by leaving `decls` empty).
        let decl = |names: Vec<Name>, procs: Vec<Proc>| {
            Decl(names, NameRemainder::NameRemainderEmpty, procs)
        };
        cases.push((
            "PLet.decl.names".to_string(),
            PLet(decl(vec![quote()], vec![]), Decls::EmptyDeclImpl, nil()),
        ));
        cases.push((
            "PLet.decl.procs".to_string(),
            PLet(
                decl(vec![], vec![child.clone()]),
                Decls::EmptyDeclImpl,
                nil(),
            ),
        ));
        cases.push((
            "PLet.decls[].LinearDecl.names".to_string(),
            PLet(
                decl(vec![], vec![]),
                Decls::LinearDeclsImpl(vec![LinearDecl(decl(vec![quote()], vec![]))]),
                nil(),
            ),
        ));
        cases.push((
            "PLet.decls[].LinearDecl.procs".to_string(),
            PLet(
                decl(vec![], vec![]),
                Decls::LinearDeclsImpl(vec![LinearDecl(decl(vec![], vec![child.clone()]))]),
                nil(),
            ),
        ));
        cases.push((
            "PLet.decls[].ConcDecl.names".to_string(),
            PLet(
                decl(vec![], vec![]),
                Decls::ConcDeclsImpl(vec![ConcDecl(decl(vec![quote()], vec![]))]),
                nil(),
            ),
        ));
        cases.push((
            "PLet.decls[].ConcDecl.procs".to_string(),
            PLet(
                decl(vec![], vec![]),
                Decls::ConcDeclsImpl(vec![ConcDecl(decl(vec![], vec![child.clone()]))]),
                nil(),
            ),
        ));
        cases.push((
            "PLet.body".to_string(),
            PLet(decl(vec![], vec![]), Decls::EmptyDeclImpl, deep()),
        ));

        cases.push(("PIf.condition".to_string(), PIf(deep(), nil())));
        cases.push(("PIf.then".to_string(), PIf(nil(), deep())));
        cases.push((
            "PIfElse.condition".to_string(),
            PIfElse(deep(), nil(), nil()),
        ));
        cases.push(("PIfElse.then".to_string(), PIfElse(nil(), deep(), nil())));
        cases.push(("PIfElse.else".to_string(), PIfElse(nil(), nil(), deep())));
        // `PNew`'s other field is a `Vec<NameDecl>`, and a `NameDecl` is a pair of strings: the body
        // is its only position.
        cases.push(("PNew.body".to_string(), PNew(vec![], deep())));
        cases.push((
            "PSendSynch.chan".to_string(),
            PSendSynch(quote(), vec![], SynchSendCont::EmptyCont),
        ));
        cases.push((
            "PSendSynch.data".to_string(),
            PSendSynch(
                Name::NameVar("x".to_string()),
                vec![child.clone()],
                SynchSendCont::EmptyCont,
            ),
        ));
        cases.push((
            "PSendSynch.cont".to_string(),
            PSendSynch(
                Name::NameVar("x".to_string()),
                vec![],
                SynchSendCont::NonEmptyCont(deep()),
            ),
        ));

        cases
    }

    /// **Every position the parser's depth walk descends into is descended into.**
    ///
    /// `push_sub_procs` `match`es `Proc` **exhaustively**, so a *new* variant cannot be forgotten:
    /// the file does not compile until the arm exists. What an exhaustive match does not check is
    /// that an arm already written reaches every child it should — dropping one `out.push((.., d))`
    /// is a silent under-count, and the guard then admits a term of unbounded AST depth for exactly
    /// the constructs that arm covers. That is the failure `MAX_AST_DEPTH` exists to prevent (AUDIT
    /// C99: the normalizer recursed ~800 levels and overflowed the stack), so this test is the other
    /// half of the exhaustive match: one case per position `push_sub_procs`, `push_decl`,
    /// `push_receipt`, `push_linear` and `push_receipt_source` descend into, each parking a depth-10
    /// child in that position and asking for a limit of 5.
    ///
    /// The limit/child pair is the value route's (`models/src/types.rs`'s
    /// `every_construct_that_carries_a_par_is_walked`): the child's deepest node lands at depth 11
    /// (a case root is depth 1 and a parked child always depth 2 — every position here is one edge
    /// from the root), while the *skeleton* of every case is at most depth 2, so at a limit of 5 the
    /// verdict is decided by whether the walk reached the child at all. A dropped push leaves the
    /// walk at depth 2 and the case fails. Every case is built a second time around a depth-1 child
    /// and asserted **not** to exceed the same limit, so the first loop cannot be satisfied by a walk
    /// that is simply always `true`.
    ///
    /// **Verified by mutation** two ways: commenting out `PMethod`'s `out.push((target, d))` fails
    /// `PMethod.target` alone, and commenting out the binary arm's `out.push((b, d))` fails
    /// `PConjunction.right` — the first right-hand case — while the left-hand case beside it, same
    /// arm and same variant, passes. Those two runs are why an arm with more than one variant, or
    /// more than one position, gets a case for every one of them: an arm's pushes are shared, so one
    /// dropped push takes out each case that leans on it.
    ///
    /// The leaves (`PGround`, `PVar`, `PVarRef`, `PNil`, `PSimpleType`) have no case because they
    /// carry no `Proc`; neither does `PNew`'s `Vec<NameDecl>`, nor the `ProcRemainder`/`NameRemainder`
    /// /`VarRefKind` fields of the arms above. `Decls::EmptyDeclImpl` and `SynchSendCont::EmptyCont`
    /// likewise have no position to park a child in. Two arms are `if let` rather than `match` —
    /// `PLet`'s `Decls` and `PSendSynch`'s `SynchSendCont` — so a *new* variant of either would
    /// compile and be silently unwalked; these cases pin the variants that exist, and nothing can pin
    /// ones that do not.
    #[test]
    fn every_construct_in_the_parser_walk_is_descended() {
        // Establish the depth convention `proc_of_depth` claims before leaning on it: depth `n` is
        // admitted at a limit of `n` and refused at `n - 1`, in both directions.
        for n in [1usize, 2, 5, 10] {
            assert!(
                !exceeds_ast_depth(&proc_of_depth(n), n),
                "depth {n} is *at* the limit, not past it"
            );
            assert!(
                exceeds_ast_depth(&proc_of_depth(n + 1), n),
                "depth {} is one past the limit of {n}",
                n + 1
            );
        }

        const LIMIT: usize = 5;
        for (label, p) in walk_cases(&proc_of_depth(10)) {
            assert!(
                exceeds_ast_depth(&p, LIMIT),
                "`{label}` parks a depth-10 child and the limit is {LIMIT}, so the walk must reach \
                 it — a position of `push_sub_procs` is not descended into"
            );
        }
        // The control: the very same cases around a depth-1 child are all inside the limit, so the
        // loop above is a verdict on the child being reached and not on the shape of the case.
        for (label, p) in walk_cases(&proc_of_depth(1)) {
            assert!(
                !exceeds_ast_depth(&p, LIMIT),
                "`{label}` with a depth-1 child is at most 2 deep: nothing in it is past {LIMIT}"
            );
        }
    }
}
