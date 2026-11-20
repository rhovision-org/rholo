//! `rnode audit` builds a replay configuration from the audited chain's genesis.
//!
//! `tools/release-check.py run --claim RC-6` found that `rnode audit` could not build its node
//! configuration at all: `main.rs` handed an `Audit` subcommand to a builder that accepted only
//! `run`. The parse test stopped at clap and the runtime tests were pure helpers, so the shipped
//! command was dead. This test pins the dispatch that was missing: an `Audit` `Options` reaches
//! `Configuration::build_audit`, which builds a `NodeConf` from the chain configuration the
//! operator names. A bare `--data-dir` is refused, because the silent fallback would verify the
//! replay against `defaults.conf`'s genesis instead of the audited chain's.
//!
//! The full replay is exercised by `tools/release-check.py run --claim RC-6 --env local`, which
//! needs a devnet and a served certificate. This test is the CI-reached construction witness.

use std::path::PathBuf;

use clap::Parser;

use rchain_node::configuration::commandline::options::Options;
use rchain_node::configuration::Configuration;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rnode-audit-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn audit_options(args: &[&str]) -> Options {
    let mut full = vec!["rnode"];
    full.extend_from_slice(args);
    Options::try_parse_from(full).expect("the audit options parse")
}

#[test]
fn the_audit_subcommand_builds_the_audited_chains_config() {
    let dir = scratch("build");
    let bonds = dir.join("bonds.txt");
    let wallets = dir.join("wallets.txt");
    std::fs::write(&bonds, "").expect("bonds");
    std::fs::write(&wallets, "").expect("wallets");
    let config = dir.join("chain.conf");
    std::fs::write(
        &config,
        format!(
            "casper {{\n  genesis-block-data {{\n    bonds-file = \"{}\"\n    \
             wallets-file = \"{}\"\n  }}\n}}\n",
            bonds.display(),
            wallets.display()
        ),
    )
    .expect("config");

    let options = audit_options(&[
        "audit",
        "--bootstrap",
        "http://127.0.0.1:40403",
        "--data-dir",
        dir.to_str().unwrap(),
        "--out",
        dir.join("out").to_str().unwrap(),
        "--config-file",
        config.to_str().unwrap(),
    ]);
    let (conf, _, _) =
        Configuration::build_audit(&options).expect("the audit configuration is built");
    let genesis = &conf.casper.shards.primary().genesis_block_data;
    assert_eq!(genesis.bonds_file, bonds.to_string_lossy());
    assert_eq!(genesis.wallets_file, wallets.to_string_lossy());
}

#[test]
fn a_bare_data_dir_is_refused_rather_than_verifying_against_the_defaults() {
    let dir = scratch("refuse");
    let options = audit_options(&[
        "audit",
        "--bootstrap",
        "http://127.0.0.1:40403",
        "--data-dir",
        dir.to_str().unwrap(),
        "--out",
        dir.join("out").to_str().unwrap(),
    ]);
    let error = Configuration::build_audit(&options)
        .expect_err("a bare --data-dir must not choose defaults.conf's genesis");
    assert!(
        error.contains("--config-file"),
        "the refusal names the inputs that fix it: {error}"
    );
}
