# Registry-based contract upgrades (RCHIP #43)

**Status: source-backed description of the existing dictionary primitives and their limits.**
This page does not implement a genesis preflight or certify an upgrade of every blessed contract.
Using the dictionary's existing public verbs is an ordinary deploy; replacing native system-process
behavior or a contract held outside that dictionary requires its own compatibility decision.

## What genesis installs

[`MasterDictionary.rho`](../../../casper/src/genesis/resources/rgov/MasterDictionary.rho) publishes
three facets. The genesis bootstrap looks up the rgov class values, publishes each under the ceremony
identity's rooted path, and aliases short names such as `Directory`, `Inbox` and `Chat` to those paths.
`Echo` and `Log` publish the Directory class under their own paths. `GetMe` and `SendThem` are
published by the member-directory feature.

The class lookup keys substituted by
[`rgov.rs::master_dictionary_source`](../../../casper/src/genesis/rgov.rs) are constants. That is
different from the dictionary's final alias targets: the embedded Rholang bootstrap publishes the
looked-up values at rooted paths and then aliases the short names to those paths. The Rust function's
older comment describing the alias targets themselves as constants is stale; the embedded contract
and the deployed-dictionary tests are the relevant implementation.

A new value published at an existing unsealed path becomes the result of a subsequent `resolve`.
A client holding a previously resolved channel continues to hold that channel. This does not update
existing instances, change a captured name, or migrate state through a pointer change.

## The public interface

The constant facet keys come from `masterdict_resolve_uri`, `masterdict_publish_uri` and
`masterdict_root_uri` in `rgov.rs`. A caller first resolves the appropriate key with
`rho:registry:lookup`. Each facet receives three arguments: the verb, an argument list and a reply
channel. These are message shapes, not a complete deploy template:

| Facet | Verb and argument list | Result or authority boundary |
|---|---|---|
| read | `("resolve", [name], reply)` | Latest stored value; `Nil` for an unknown name. An alias to a constant URI can return that target instead of a versioned value. |
| read | `("resolveAt", [path, index], reply)` | Stored value at that index, or `Nil`. Use the explicit rooted path to pin identity as well as version; an alias can be retargeted. |
| read | `("versionsOf", [path], reply)` | Stored version list. `resolve` does not return a numeric version. |
| read | `("targetOf", [name], reply)` | Alias target or known path, useful for checking whether a short name reaches the intended rooted path. |
| read | `("sealed", [path], reply)` | Whether the path is closed to further publication. |
| publish | `("publish", [id, path, value], reply)` | Appends and returns `("published", path, index)`; the caller identity must own the path prefix and the path must be unsealed. |
| publish | `("seal", [id, path], reply)` | Permanently closes a path owned by the caller. |
| publish | `("grant", [id, path], reply)` / `("revoke", [id, path], reply)` | Grants a path-scoped write capability or advances its revocation epoch. |
| root | `("alias", [id, short, target], reply)` | Changes the short-name target only for the current alias-tier authority. |
| root | `("setRootAuthority", [id, successor], reply)` | Rotates alias-tier authority; it does not rewrite ownership of existing rooted paths. |

`resolveName`, `appendVersion` and `setAlias` are internal helpers, not public verbs. Calling those
names through a published facet reaches its bad-verb refusal. `publish` stores a Rholang value; it
does not load source code, compile a contract, or interpret a URI on the caller's behalf.

The caller's deployer identity is a bearer capability. Follow the contract's disclosure rule: pass it
only to the trusted identity-checking facet that needs it, never publish it as the stored value or in
diagnostics. Rooted-path ownership and alias-tier authority are separate checks.

## Before replacing a dictionary-backed value

Record the alias target, explicit rooted path, current value and version list, seal state, authorized
writer and alias authority. Verify that the replacement has actually been installed and is callable.
Publishing a new version changes future dictionary lookups, while a client pinned to the explicit
path and old index keeps the old stored value.

A stale-upgrade guard must compare the reviewed value or history, not compare `resolve` with a numeric
version. The public `publish` verb has no expected-version or expected-source-hash argument. Separate
read and publish calls do not establish compare-and-append semantics; this page supplies no tested
race-prevention mechanism. Concurrent upgrades require a demonstrated serialization or governance
protocol before operational use.

State migration is a separate program with its own replay, conservation, authorization and failure
checks. Replacing a pointer supplies none of those guarantees. Rotating alias authority alone also
does not transfer the ability to append under a previous owner's rooted namespace.

## Existing evidence and remaining acceptance

[`casper/tests/master_dictionary.rs`](../../../casper/tests/master_dictionary.rs) exercises the
installed dictionary through public facets:

- `the_genesis_alias_tier_resolves`: genesis names have alias targets and an unknown name is absent.
- `publish_is_append_only_and_versions_are_pinned`: publication appends, latest lookup changes and an
  explicit old version remains available.
- `the_four_must_fail_cases`: foreign-namespace publication, unauthorized aliasing, bad verbs and bad
  arity are refused.
- `seal_closes_a_path_and_the_key_still_resolves`: a sealed path refuses another version and retains
  its current value.
- `a_granted_key_works_until_it_is_revoked`: revocation prevents a delegated append.

These are dictionary tests. They do not establish an end-to-end replacement of every blessed
contract, a state migration, root-authority handover, stale-upgrade serialization, or preservation of
clients that captured a channel. An actual upgrade must supply those applicable witnesses on its
source-pinned genesis and client set. Native REV/PoS behavior and consensus-rule changes remain under
[the release gates](https://github.com/rhovision-org/rholo/issues/51).

The remaining RCHIP #43 implementation and acceptance obligations are recorded in
[the release gap matrix](../../../spec/audit/final-gap-matrix.md); this documentation does not close
them or establish that the next genesis can be the last one.
