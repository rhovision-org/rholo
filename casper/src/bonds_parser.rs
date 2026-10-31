//! Bonds file parser (port of `util/BondsParser.scala`).
//!
//! The genesis ceremony uses a bonds file of `<public_key> <stake>` lines to set the initial
//! validator set.

use std::collections::BTreeMap;
use std::path::Path;

use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_shared::base16;
use rchain_shared::refined::NonNegI64;

/// Parse a bonds file into a validator → stake map (port of `BondsParser.parse`).
pub fn parse(path: &Path) -> Result<BTreeMap<PublicKey, NonNegI64>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("FAILED PARSING BONDS FILE: {}\n{}", path.display(), e))?;
    let mut bonds = BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (pk_str, stake_str) = line.split_once(' ').ok_or_else(|| {
            format!("INVALID LINE FORMAT: `<public_key> <stake>`, actual: `{line}`")
        })?;
        let pk_bytes =
            base16::decode(pk_str).ok_or_else(|| format!("INVALID PUBLIC KEY: `{pk_str}`"))?;
        let stake: i64 = stake_str.parse().map_err(|_| {
            format!("INVALID STAKE `{stake_str}`. Please put a non-negative number.")
        })?;
        let stake = NonNegI64::try_from(stake)
            .map_err(|_| format!("NEGATIVE STAKE `{stake_str}`. Stake must be non-negative."))?;
        bonds.insert(PublicKey::new(pk_bytes), stake);
    }
    Ok(bonds)
}

/// **The genesis ceremony's identity: the first bond in the file** (issue #456, Law 84).
///
/// `parse` returns a `BTreeMap`, which is sorted by public key and therefore cannot answer "which
/// bond is first"; the file's order is the input. The ceremony is deterministic only if this key —
/// not each node's local validator key — signs the blessed deploys and fills the block's `sender`,
/// because both enter the block hash. The caller refuses when its own key is not this one.
pub fn first_bond(path: &Path) -> Result<PublicKey, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("FAILED PARSING BONDS FILE: {}\n{}", path.display(), e))?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (pk_str, _) = line.split_once(' ').ok_or_else(|| {
            format!("INVALID LINE FORMAT: `<public_key> <stake>`, actual: `{line}`")
        })?;
        let pk_bytes =
            base16::decode(pk_str).ok_or_else(|| format!("INVALID PUBLIC KEY: `{pk_str}`"))?;
        return Ok(PublicKey::new(pk_bytes));
    }
    Err(format!(
        "the bonds file {} names no bond; a genesis needs at least one",
        path.display()
    ))
}

/// Parse a bonds file, generating a fresh validator set if the file does not exist (port of
/// `BondsParser.parse(path, autogenShardSize)`).
pub fn parse_or_generate(
    path: &Path,
    autogen_shard_size: i32,
) -> Result<BTreeMap<PublicKey, NonNegI64>, String> {
    match parse(path) {
        Ok(bonds) => Ok(bonds),
        Err(_) => new_validators(autogen_shard_size, path),
    }
}

/// Create the directory an output path lives in, if it is not already there.
///
/// **Found by a CI failure, not by reading** (C253's E4, completed). The ceremony writes each
/// validator's `.sk` beside the bonds file and the bonds file last, and it never created the
/// directory it wrote into — which nobody noticed while both writes were discarded, because the
/// failure was silent. The moment the writes became checked, `node_api::http_surface_without_genesis`
/// went red in CI: a fresh standalone node's default bonds path is `<data_dir>/genesis/bonds.txt`,
/// nothing creates `genesis/`, and the node therefore could not boot at all.
///
/// Creating it is the right repair rather than a test patch, because requiring an operator to
/// pre-create the directory a ceremony is about to fill is not a real precondition — the devnet makes
/// it with `mktemp -d`, which is what hid this. The write is still checked, and a genuinely unwritable
/// location still fails with the same message naming the same file; only the *missing directory* case
/// changed, from "fails loudly" to "works".
fn ensure_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        None => Ok(()),
        Some(parent) => std::fs::create_dir_all(parent).map_err(|e| e.to_string()),
    }
}

/// Generate `autogen_shard_size` validators and write their keys + bonds file (port of
/// `newValidators`).
pub fn new_validators(
    autogen_shard_size: i32,
    bonds_file_path: &Path,
) -> Result<BTreeMap<PublicKey, NonNegI64>, String> {
    let mut bonds = BTreeMap::new();
    for i in 0..autogen_shard_size {
        let (sec, pub_key) = Secp256k1.new_key_pair();
        // Write `<public_key>.sk` file with the private key (owner-only perms: it is secret).
        //
        // **The write is checked** (C253's E4). It used to be `let _ = …`, so a ceremony that could
        // not write (a read-only directory, a full disk, a permissions mistake) produced no key file
        // and **no complaint**: the node came up with a validator key that does not exist on disk, and
        // the *next* boot generated a fresh one. That is the worst kind of ceremony failure — the node
        // looks healthy and its identity silently rotates.
        if let Some(parent) = bonds_file_path.parent() {
            let sk_file = parent.join(format!("{}.sk", base16::encode(pub_key.bytes())));
            // The mkdir is inside this chain rather than before it, so a location that cannot hold
            // the key — a *regular file* where the directory should be, which is the case the unit's
            // own test plants — still reports through the write's message and still names the `.sk`
            // path. Creating the directory and then failing to write it would otherwise split into
            // two different errors for one symptom.
            ensure_parent(&sk_file)
                .and_then(|()| {
                    rchain_crypto::util::key_util::write_private_key(
                        &sk_file,
                        base16::encode(sec.bytes()),
                    )
                })
                .map_err(|e| {
                    format!(
                        "FAILED WRITING VALIDATOR KEY {}, so the key this node would bond with does not \
                         exist on disk and the next boot would generate a different one: {e}",
                        sk_file.display()
                    )
                })?;
        }
        // `i >= 0`, so `i + 1 >= 1` is non-negative by construction.
        let stake = NonNegI64::try_from(i64::from(i) + 1)
            .map_err(|_| "autogenerated stake is non-negative".to_string())?;
        bonds.insert(pub_key, stake);
    }

    let mut content = String::new();
    for (pk, stake) in &bonds {
        content.push_str(&format!(
            "{} {}\n",
            base16::encode(pk.bytes()),
            i64::from(*stake)
        ));
    }
    // **And so is the bonds file itself** (C253's E4, the same site's other half). A discarded write
    // here means the ceremony produced no bonds file: the validator set it just generated exists only
    // in this process's memory, and the chain it starts cannot be rejoined by the node that started it.
    ensure_parent(bonds_file_path)
        .and_then(|()| std::fs::write(bonds_file_path, content).map_err(|e| e.to_string()))
        .map_err(|e| {
            format!(
                "FAILED WRITING BONDS FILE {}, so the validator set this ceremony generated exists only \
                 in memory and the node cannot rejoin the chain it started: {e}",
                bonds_file_path.display()
            )
        })?;
    Ok(bonds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bonds_file() {
        let dir = std::env::temp_dir().join(format!("rchain-bonds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bonds.txt");
        std::fs::write(
            &path,
            "04c591a8ff19ac9c4e4e5793673b83123437e975285e7b442f4ee2654dffca5e2d2103ed494718c697ac9aebcfd19612e224db46661011863ed2fc54e71861e2a6 100\n\
             04c591a8ff19ac9c4e4e5793673b83123437e975285e7b442f4ee2654dffca5e2d2103ed494718c697ac9aebcfd19612e224db46661011863ed2fc54e71861e2a6 200\n",
        )
        .unwrap();

        let bonds = parse(&path).unwrap();
        // Duplicate keys collapse to the last stake.
        assert_eq!(bonds.len(), 1);
        assert_eq!(
            bonds.values().next().copied(),
            Some(200.try_into().unwrap())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_bonds_file_rejects_bad_format() {
        let dir = std::env::temp_dir().join(format!("rchain-bonds-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bonds.txt");
        std::fs::write(&path, "garbage line without spaces\n").unwrap();

        assert!(parse(&path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A ceremony that cannot write its keys, or its bonds file, fails loudly and names the path**
    /// (C253's E4).
    ///
    /// Both writes used to discard their `Result` — `let _ = write_private_key(…)` and
    /// `let _ = std::fs::write(…)`. What that hid is not "a file is missing": it is that the node
    /// **comes up anyway**, bonded to a validator key that does not exist on disk, and the next boot
    /// generates a *different* one. The node looks healthy and its identity silently rotates, which is
    /// the worst shape a ceremony failure can take.
    ///
    /// So the falsifier asserts the loud failure *and* that the message carries the path, because an
    /// operator's next step is to look at it. Two cases, one per write, so the second write is not
    /// covered only by the first one's accident: a parent that is a **regular file** (the key write
    /// fails) and a bonds path that is a **directory** (the key write succeeds, the bonds write fails).
    #[test]
    fn a_ceremony_that_cannot_write_fails_loudly_and_names_the_path() {
        let dir =
            std::env::temp_dir().join(format!("rchain-bonds-unwritable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // (1) The parent is a regular file, so the `.sk` write cannot succeed.
        let blocker = dir.join("not-a-directory");
        std::fs::write(&blocker, b"a file, not a directory").unwrap();
        let err = new_validators(1, &blocker.join("bonds.txt"))
            .expect_err("no key file can be written under a regular file");
        assert!(
            err.contains("FAILED WRITING VALIDATOR KEY") && err.contains(".sk"),
            "the refusal names the step and the file: {err}"
        );

        // (2) The parent is writable, so the key write succeeds — and the bonds path is a directory,
        //     so the second write is the one that fails. This is the half a single case would miss.
        let second = dir.join("second");
        std::fs::create_dir_all(second.join("bonds.txt")).unwrap();
        let err = new_validators(1, &second.join("bonds.txt"))
            .expect_err("a bonds path that is a directory cannot be written");
        assert!(
            err.contains("FAILED WRITING BONDS FILE") && err.contains("bonds.txt"),
            "the refusal names the second write and its path: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The ceremony's key file is owner-only, and it is this project's own key form** (W-N5-04).
    ///
    /// The row: the genesis ceremony writes a plaintext base16 private scalar to `<pubkey>.sk`, so the
    /// secret a validator bonds with lands on disk in the clear. What answers that is the **mode** and the
    /// **convention** — `write_private_key` creates the file owner-only, and a bare base16 scalar in an
    /// owner-only file is this project's key form everywhere else: it is exactly what
    /// `--validator-private-key-path` reads and what `key_util::load_validator_private_key` accepts. The
    /// deviation as written ("plaintext") is therefore not the exposure it reads as; what *would* be one is
    /// a file another user can read, and nothing asserted that this one cannot be.
    ///
    /// Falsified by writing the key with `std::fs::write` instead of `write_private_key`: the mode comes
    /// from the process umask (typically 0644) and the first assertion fails. The mutation is the plausible
    /// mistake rather than a contrivance, which is why the assertion is on the mode and not on the content.
    #[test]
    fn the_ceremonys_key_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("rchain-bonds-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bonds = dir.join("bonds.txt");

        let validators =
            new_validators(2, &bonds).expect("the ceremony writes a fresh validator set");
        assert_eq!(validators.len(), 2, "two validators, two bonds");

        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("sk") {
                continue;
            }
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode,
                0o600,
                "the ceremony's key file {} must be owner-only; a world-readable scalar is the exposure \
                 the row is about",
                path.display()
            );
            let body = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                body.trim().len(),
                64,
                "the file holds a bare base16 scalar — the form the node's own key reader accepts: {}",
                path.display()
            );
            assert!(
                rchain_shared::base16::decode(body.trim()).is_some(),
                "and it decodes as base16: {}",
                path.display()
            );
            checked += 1;
        }
        assert_eq!(checked, 2, "one `.sk` per generated validator");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
