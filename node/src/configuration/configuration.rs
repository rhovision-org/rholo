//! Configuration assembly (port of `Configuration.scala`).

use std::path::PathBuf;

use hocon::Hocon;
use linked_hash_map::LinkedHashMap;

use super::commandline::config_mapper;
use super::commandline::options::{Commands, Options};
use super::hocon::{check_shard_config_exclusivity, node_conf_from_hocon};
use super::model::NodeConf;
use crate::resources::policy::StartupEnforcement;

/// A named set of defaults (port of `Configuration.Profile`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub data_dir: PathBuf,
    pub description: String,
}

fn default_profile() -> Profile {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    Profile {
        name: "default".to_string(),
        data_dir: home.join(".rnode"),
        description: "Defaults to $HOME/.rnode".to_string(),
    }
}

fn docker_profile() -> Profile {
    Profile {
        name: "docker".to_string(),
        data_dir: PathBuf::from("/var/lib/rnode"),
        description: "Defaults to /var/lib/rnode".to_string(),
    }
}

pub fn profiles() -> Vec<Profile> {
    vec![default_profile(), docker_profile()]
}

/// The config assembly (port of `Configuration` object).
pub struct Configuration;

impl Configuration {
    /// Build a `NodeConf` from CLI options (port of `Configuration.build`). The kamon config (the
    /// 4th return value in Scala) is not carried: the reporter switches are checked here instead —
    /// refused or noted, never left as settings nothing reads (`check_metrics_config`, AUDIT §6).
    pub fn build(options: &Options) -> Result<(NodeConf, Profile, Option<PathBuf>), String> {
        // **An unrecognised `--profile` is refused, not silently replaced by the default** (AUDIT C147).
        // The profile is the whole of the decision about where a node's data lives, and the default's
        // directory is `$HOME/.rnode` — so the docs' own command (`--profile docker`, one keystroke from
        // `docer`) typo'd starts the node on a *fresh, empty* data directory: a restart that looks like
        // data loss or an unexpected re-genesis, with nothing naming the profile. A refusal names both
        // the name it did not recognise and the ones it has, which is the whole of the fix. (Whether the
        // oracle falls back here is not established — typesafe-config's `--profile` selects a *file*, and
        // the Scala's behaviour on a name with no file is unrecorded — so this refuses on the port's own
        // ground: a silent change of data directory is the class this register exists for.)
        let profile = match &options.profile {
            Some(name) => profiles()
                .into_iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| {
                    let known: Vec<String> = profiles()
                        .iter()
                        .map(|p| format!("'{}' ({})", p.name, p.data_dir.display()))
                        .collect();
                    format!(
                        "unknown profile '{name}': this port has {}. Refusing rather than falling back \
                         to '{}', whose data directory is different",
                        known.join(", "),
                        default_profile().name
                    )
                })?,
            None => default_profile(),
        };

        let run = match &options.subcommand {
            Commands::Run(run) => run,
            _ => {
                return Err("`run` subcommand is required to build a node configuration".to_string())
            }
        };

        let data_dir = run
            .data_dir
            .clone()
            .unwrap_or_else(|| profile.data_dir.clone());

        let config_file_path = run
            .config_file
            .clone()
            .unwrap_or_else(|| data_dir.join("rnode.conf"));
        let config_file = if config_file_path.exists() {
            Some(config_file_path)
        } else {
            None
        };

        let options_config = config_mapper::from_options(options);
        let file_config = match &config_file {
            Some(path) => parse_file(path)?,
            None => Hocon::Hash(LinkedHashMap::new()),
        };
        // Checked on the operator-supplied layers only: `defaults.conf` always sets
        // `casper.shard-name`, so its presence after the merge says nothing about intent.
        check_shard_config_exclusivity(&options_config, "the command line")?;
        check_shard_config_exclusivity(&file_config, "the config file")?;

        let default_config = parse_defaults(&data_dir.to_string_lossy())?;
        // The defaults' own `NodeConf`, taken *before* the merge consumes the `Hocon`: this is what
        // `check_inert_config` compares against, so the comparison cannot drift from `defaults.conf`
        // (AUDIT C143).
        let defaults_conf = node_conf_from_hocon(&default_config)?;

        let merged = merge(merge(options_config, file_config), default_config);
        let mut node_conf = node_conf_from_hocon(&merged)?;
        resolve_validator_private_key(&mut node_conf)?;

        for spec in node_conf.casper.shards.iter() {
            let quorum = spec.genesis_block_data.pos_multi_sig_quorum;
            let keys_len = spec.genesis_block_data.pos_multi_sig_public_keys.len();
            if quorum > keys_len as i32 {
                return Err(format!(
                    "shard '{}': The value 'pos-multi-sig-quorum' should be less or equal the length of 'pos-multi-sig-public-keys' (the actual values are '{quorum}' and '{keys_len}' respectively)",
                    spec.shard_id
                ));
            }
        }

        // AUDIT C143: the keys this port parses into `NodeConf` and enforces nowhere. Checked here,
        // with the other operator-facing configuration errors, against the defaults' own `NodeConf`
        // taken above.
        check_inert_config(&node_conf, &defaults_conf)?;
        check_resource_guard(&node_conf)?;

        // The hostlist's own bounds. Checked whether or not the route is mounted, so a configuration
        // that would be refused the moment the route is enabled is refused now rather than at the
        // restart that enables it.
        crate::web::node_list::NodeListConfig {
            probe_interval: node_conf.api_server.node_list_probe_interval,
            probe_timeout: node_conf.api_server.node_list_probe_timeout,
            max_entries: node_conf.api_server.node_list_max_entries,
        }
        .validate()?;

        // AUDIT §6: the Kamon reporter switches. Checked here, where the operator-facing
        // configuration errors already live (`check_shard_config_exclusivity`, the shard quorum
        // check), so a `run` asking for a reporter this port does not have fails at startup instead
        // of running with a setting that reports nothing.
        if let Some(note) = check_metrics_config(&node_conf.metrics)? {
            println!("{note}");
        }

        check_merge_divergence_injection(&node_conf)?;
        check_sync_anchor(&node_conf)?;
        check_self_reset(&node_conf)?;
        // **The production recipient is a repository constant** (`genesis::dev_tax`): an empty
        // configured address resolves to it outside dev mode and to no allocation inside it. Applied
        // before validation, so the genesis builder, the replay descriptors and the checks see one
        // value; an explicit value always wins.
        apply_production_dev_tax_address(&mut node_conf);
        check_dev_tax_address(&node_conf)?;

        Ok((check_dev_mode(node_conf), profile, config_file))
    }
}

/// Resolve `casper.validator-private-key-path` into `casper.validator-private-key`.
///
/// The runtime reads only the hex field, so a key supplied *by path* was parsed into the
/// config and then silently dropped — leaving the node with no validator identity, no
/// diagnostic, and (in standalone mode) a genesis ceremony that cannot be signed.
/// Resolving it here also lets operators keep the secret off the command line:
/// `--validator-private-key <hex>` was visible to any local process via `ps` or
/// `/proc/<pid>/cmdline`, a file path is not — and that flag is now **gone** (W-N11-05),
/// refused at parse time so the two routes left are this path and the config file's own
/// `casper.validator-private-key`.
///
/// Precedence: an explicit `validator-private-key` always wins; the path is consulted
/// only when that is absent. A path that is set but unreadable or not a key is a hard
/// error rather than a silent fallback, for the same reason — silently running without
/// a validator identity is the failure this exists to prevent.
fn resolve_validator_private_key(node_conf: &mut NodeConf) -> Result<(), String> {
    if node_conf.casper.validator_private_key.is_some() {
        return Ok(());
    }
    let Some(path) = node_conf.casper.validator_private_key_path.clone() else {
        return Ok(());
    };
    let key = rchain_crypto::util::key_util::load_validator_private_key(&path)?;
    node_conf.casper.validator_private_key = Some(rchain_shared::base16::encode(key.bytes()));
    Ok(())
}

/// What this port does about the Kamon reporter switches (`metrics.*`, AUDIT §6).
///
/// The oracle's `kamon.conf` gates a Prometheus scrape endpoint (`prometheus { enabled = false }`)
/// and configures InfluxDB, Zipkin and Sigar reporters that *push*. This port carries no Kamon: it
/// always serves Prometheus text at `GET /metrics`, and has no InfluxDB or UDP sender, no Zipkin
/// span reporter and no Sigar collector — so a switch for one of those can only mislead the operator
/// who sets it, whether the node then says nothing or starts with a knob nothing reads.
///
/// - The four reporters with no counterpart are **refused**: an error naming each one that was set,
///   at startup, before anything runs.
/// - `prometheus` is accepted and **noted**: the endpoint it asks for does exist here — always on,
///   not gated by the switch — which is what the note says. A `false` value cannot be told apart from
///   the default once the layers are merged, so that direction is documented rather than reported
///   (the flag's help text and `defaults.conf` both say the endpoint is not gated).
///
/// `Ok(None)` means the operator asked for nothing and there is nothing to say.
fn check_metrics_config(metrics: &super::model::Metrics) -> Result<Option<String>, String> {
    let mut absent: Vec<&str> = Vec::new();
    if metrics.influxdb {
        absent.push("influxdb");
    }
    if metrics.influxdb_udp {
        absent.push("influxdb-udp");
    }
    if metrics.zipkin {
        absent.push("zipkin");
    }
    if metrics.sigar {
        absent.push("sigar");
    }
    if !absent.is_empty() {
        return Err(format!(
            "unimplemented metrics reporter(s) enabled: {}. This port has no InfluxDB sender, no \
             InfluxDB UDP sender, no Zipkin span reporter and no Sigar collector, so these settings \
             would report nothing — refused rather than accepted and ignored. `GET /metrics` serves \
             Prometheus text and is always on; unset the setting to start.",
            absent.join(", ")
        ));
    }
    Ok(metrics.prometheus.then(|| {
        "metrics.prometheus is set: `GET /metrics` serves Prometheus text in this port and is always \
         on, so the setting is already satisfied — it does not gate the endpoint (a `false` value \
         does not disable it either)."
            .to_string()
    }))
}

/// The config keys this port parses into `NodeConf` and **enforces nowhere** (AUDIT C143).
///
/// **Refused when they differ from the shipped default, rather than accepted and ignored** — the
/// disposition this register already chose for the metrics reporters above and for
/// `disable-state-exporter`: a knob that does nothing is worse than no knob, because the operator
/// believes it did something and nothing in the node ever contradicts them.
///
/// The nine are the ones a grep over every crate finds read by nothing outside
/// `node/src/configuration`:
///
/// - `protocol-server.max-message-consumers` — documented in the file's longest comment as the bound
///   to raise for a busier node ("the very minimum should be {number of nodes} * {synchrony
///   constraint}"); the effective bound is the hardcoded `MAX_CONCURRENT_DISPATCH = 1024`.
/// - `protocol-server.use-random-ports` — the node still fails to start when its port is taken.
/// - `protocol-server.dynamic-ip` — read by nothing.
/// - `peers-discovery.init-wait-loop-interval` — read by nothing.
/// - the gRPC keepalive group (`api-server.keep-alive-time`, `keep-alive-timeout`,
///   `permit-keep-alive-time`, `max-connection-age`, `max-connection-age-grace`) — no HTTP/2 or TCP
///   keepalive is set anywhere in the tree, so the only mechanism the oracle had for retiring a
///   half-open peer connection is inert and a dead peer stays "connected" until discovery's 20-minute
///   cleanup.
///
/// **The comparison is derived, not tabulated.** Each field is compared against the `NodeConf` that
/// `defaults.conf` alone produces, so a default cannot drift out from under the refusal — a
/// hand-written table of the nine defaults would be a second definition of them, which is this
/// register's own recurring defect class. An operator who changed none of them — the common case, and
/// every existing test — is unaffected, which is what makes this safe to add to a running series.
pub fn check_inert_config(node_conf: &NodeConf, defaults: &NodeConf) -> Result<(), String> {
    let mut set: Vec<&str> = Vec::new();
    if node_conf.protocol_server.max_message_consumers
        != defaults.protocol_server.max_message_consumers
    {
        set.push("protocol-server.max-message-consumers");
    }
    if node_conf.protocol_server.use_random_ports != defaults.protocol_server.use_random_ports {
        set.push("protocol-server.use-random-ports");
    }
    if node_conf.protocol_server.dynamic_ip != defaults.protocol_server.dynamic_ip {
        set.push("protocol-server.dynamic-ip");
    }
    if node_conf.peers_discovery.init_wait_loop_interval
        != defaults.peers_discovery.init_wait_loop_interval
    {
        set.push("peers-discovery.init-wait-loop-interval");
    }
    if node_conf.api_server.keep_alive_time != defaults.api_server.keep_alive_time {
        set.push("api-server.keep-alive-time");
    }
    if node_conf.api_server.keep_alive_timeout != defaults.api_server.keep_alive_timeout {
        set.push("api-server.keep-alive-timeout");
    }
    if node_conf.api_server.permit_keep_alive_time != defaults.api_server.permit_keep_alive_time {
        set.push("api-server.permit-keep-alive-time");
    }
    if node_conf.api_server.max_connection_age != defaults.api_server.max_connection_age {
        set.push("api-server.max-connection-age");
    }
    if node_conf.api_server.max_connection_age_grace != defaults.api_server.max_connection_age_grace
    {
        set.push("api-server.max-connection-age-grace");
    }

    // **Four the class had not reached** (2026-10-14, the coverage-gap pass). The first two are
    // documented in `defaults.conf` as *proposer* guards — *"Node is allowed to propose a block only
    // when …"* — and the Scala oracle enforced both; a workspace-wide grep finds every occurrence of
    // either in this module's own plumbing, so nothing reads them. The two `fork-choice-*` settings
    // are read by nothing at all. Same disposition as the group above: a setting that does nothing
    // must not look as though it did, and a *safety* setting that silently does nothing is worse
    // than one that is refused.
    if node_conf.casper.synchrony_constraint_threshold
        != defaults.casper.synchrony_constraint_threshold
    {
        set.push("casper.synchrony-constraint-threshold");
    }
    if node_conf.casper.height_constraint_threshold != defaults.casper.height_constraint_threshold {
        set.push("casper.height-constraint-threshold");
    }
    if node_conf.casper.fork_choice_stale_threshold != defaults.casper.fork_choice_stale_threshold {
        set.push("casper.fork-choice-stale-threshold");
    }
    if node_conf.casper.fork_choice_check_if_stale_interval
        != defaults.casper.fork_choice_check_if_stale_interval
    {
        set.push("casper.fork-choice-check-if-stale-interval");
    }

    if !set.is_empty() {
        return Err(format!(
            "unimplemented setting(s) changed: {}. This port parses these and enforces none of them \
             — `max-message-consumers` leaves the inbound dispatch bound at its hardcoded 1024, \
             `use-random-ports`/`dynamic-ip` do not change how a port is chosen, and no HTTP/2 or TCP \
             keepalive is set anywhere, so the keepalive group would retire no half-open peer \
             connection — and the four `casper` settings above are read by no code outside this \
             module: the two thresholds are *documented* as proposer guards and are enforced nowhere, \
             so an operator who set one had a bound that did not exist. Refused rather than accepted \
             and ignored, which is the disposition this register chose for the metrics reporters and \
             for `disable-state-exporter`: a setting that does nothing must not look as though it \
             did. Remove the setting(s) to start.",
            set.join(", ")
        ));
    }
    Ok(())
}

/// The resource-guard block's own refusals (A2): an unknown enforcement word, a negative floor or a
/// non-positive data-dir floor is an operator error, not a setting to guess around.
fn check_resource_guard(node_conf: &NodeConf) -> Result<(), String> {
    let guard = &node_conf.resource_guard;
    if StartupEnforcement::parse(&guard.enforce_startup).is_none() {
        return Err(format!(
            "resource-guard.enforce-startup must be auto, enforce or warn (got {:?})",
            guard.enforce_startup
        ));
    }
    if guard.min_available_mb < 0 {
        return Err(
            "resource-guard.min-available-mb must be >= 0 (0 = the chain-length policy table)"
                .to_string(),
        );
    }
    if guard.min_data_dir_free_mb <= 0 {
        return Err("resource-guard.min-data-dir-free-mb must be greater than 0".to_string());
    }
    Ok(())
}

/// If not in dev mode, strip the deployer private key (port of `Configuration.checkDevMode`).
/// **The divergence injection refuses to arm outside dev-mode**, and says why.
///
/// A node that perturbs its own fringe records diverges from its peers *on purpose* — and from every
/// other node's side that is indistinguishable from a node that diverged by accident: they see its
/// blocks fail against a pre-state they cannot reproduce, which is TE-1's shape, and the chain does not
/// recover. That is the one thing a mis-typed flag must not be able to cause on a network anyone is
/// reading, so the injection requires `--dev-mode` alongside it. The equivocation injection needs no
/// such guard because it is self-harm only — the node's own bond is what its peers take.
/// **A `--sync-anchor` that is not a block hash is refused here, not at boot.**
///
/// The value is the one thing that decides where a wiped node syncs to (C259), so a typo in it is the
/// difference between recovering a chain and syncing to something nobody vouched for. Refusing at
/// startup costs the operator a restart; refusing at boot would too, but *after* the node has read its
/// store and told the peers it is joining. Checked with the other operator-facing configuration errors,
/// which is where this file already puts them.
fn check_sync_anchor(node_conf: &NodeConf) -> Result<(), String> {
    let Some(anchor) = node_conf.casper.sync_anchor.as_deref() else {
        return Ok(());
    };
    rchain_models::block_hash::BlockHash::try_from_hex(anchor)
        .map(|_| ())
        .map_err(|e| {
            format!(
                "casper.sync-anchor `{anchor}` is not a block hash: {e}. It is the anchor a wiped node \
                 syncs to, so it is not a value to guess at"
            )
        })
}

/// **A `dev-tax-address` that is set must be a REV address**, refused at startup rather than at
/// genesis (Law 78, `casper/src/genesis/dev_tax.rs`).
///
/// The allocation's *amount* is derived from the wallets total, so a malformed address cannot make it
/// wrong — it makes the genesis descriptors fail to build, where the error reads as a genesis-parsing
/// problem rather than as the operator typo it is. Checked here, with the other operator-facing
/// configuration errors, which is where this file already puts them.
///
/// **Presence is deliberately not checked here.** Whether the address *must* be set is decided at
/// genesis creation (`node_launch::apply`), because only a node about to build a genesis is making
/// that decision, and only there can a *new* chain be told from a restart onto an existing one — the
/// public testnet's bootstrap is standalone and its genesis predates this key.
/// Resolve every shard's empty `dev-tax-address` to [`rchain_casper::genesis::dev_tax::
/// PRODUCTION_DEV_TAX_ADDRESS`] outside dev mode, and leave it empty inside it. An explicit value is
/// untouched. See `dev_tax::effective_address` for why this is a repository constant rather than an
/// image default (devnets must never inherit a real allocation).
fn apply_production_dev_tax_address(node_conf: &mut NodeConf) {
    let dev_mode = node_conf.dev_mode;
    for spec in node_conf.casper.shards.iter_mut() {
        let configured = spec.genesis_block_data.dev_tax_address.clone();
        spec.genesis_block_data.dev_tax_address =
            rchain_casper::genesis::dev_tax::effective_address(&configured, dev_mode);
    }
}

fn check_dev_tax_address(node_conf: &NodeConf) -> Result<(), String> {
    for spec in node_conf.casper.shards.iter() {
        let address = &spec.genesis_block_data.dev_tax_address;
        if address.is_empty() {
            continue;
        }
        rchain_casper::genesis::dev_tax::parse_address(address)
            .map_err(|e| format!("shard '{}': {e}", spec.shard_id))?;
    }
    Ok(())
}

fn check_merge_divergence_injection(node_conf: &NodeConf) -> Result<(), String> {
    let n = node_conf.casper.merge_divergence_injection;
    if n > 0 && !node_conf.dev_mode {
        return Err(format!(
            "casper.merge-divergence-injection = {n} requires --dev-mode: the injection makes this \
             node's merge disagree with its peers' on purpose, which is indistinguishable from a node \
             that diverged by accident, and a chain in that state does not recover"
        ));
    }
    Ok(())
}

/// **`casper.self-reset` without a bootstrap is refused here, not discovered at the reset** (#320).
///
/// A reset rewinds the node to its last finalised block, which puts it *deliberately behind* its peers:
/// everything above the anchor is dropped and has to be re-requested. A node with no bootstrap has
/// nobody to ask, so the switch would buy it a truncated DAG and no way back — worse than the refusal it
/// replaces, because the refusal at least leaves the node serving the chain it has. The check is here
/// rather than at the reset because it is a property of the operator's configuration: it costs a restart
/// to find out, not a divergence.
///
/// **Not gated on `--dev-mode`**, deliberately — see `CasperConf::self_reset`: the switch is the
/// operator's consent to a change the node has already detected, counted and logged, and a stranded
/// production validator is exactly who needs it.
fn check_self_reset(node_conf: &NodeConf) -> Result<(), String> {
    if !node_conf.casper.self_reset {
        return Ok(());
    }
    // **`standalone` is the predicate, not "is a bootstrap configured".** A non-standalone node always
    // has one (the HOCON `bootstrap` key is required, and `node_runtime` turns it into the peer the
    // retriever asks); a standalone node is its own network — it creates genesis and has nobody to
    // re-sync from, which is the case this check exists for.
    if node_conf.standalone {
        return Err(
            "casper.self-reset = true needs a bootstrap: the reset rewinds this node to its last \
             finalized block and re-requests everything above it, so a standalone node would find \
             itself behind its own chain with nobody to ask"
                .to_string(),
        );
    }
    Ok(())
}

pub fn check_dev_mode(node_conf: NodeConf) -> NodeConf {
    if node_conf.dev_mode {
        node_conf
    } else {
        if node_conf.dev.deployer_private_key.is_some() {
            println!("Node is not in dev mode, ignoring --deployer-private-key");
        }
        NodeConf {
            dev: super::model::DevConf {
                deployer_private_key: None,
            },
            ..node_conf
        }
    }
}

/// Escape a string for embedding inside a quoted HOCON string value: backslash-escape `\` and `"`,
/// and strip newlines/carriage returns so a `data_dir` value cannot break out of the quoted string
/// or inject additional config lines.
fn escape_hocon_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "")
        .replace('\r', "")
}

/// Parse the bundled `defaults.conf` with `default-data-dir` injected (port of the
/// `ConfigSource.resources("defaults.conf").withFallback(...)` default source).
///
/// **`strict()` is load-bearing** (AUDIT C141) — see [`parse_file`] for what it prevents.
pub fn parse_defaults(data_dir: &str) -> Result<Hocon, String> {
    let defaults = include_str!("defaults.conf");
    let escaped = escape_hocon_string(data_dir);
    let combined = format!("default-data-dir = \"{escaped}\"\n{defaults}");
    hocon::HoconLoader::new()
        .strict()
        .load_str(&combined)
        .map_err(|e| e.to_string())?
        .hocon()
        .map_err(|e| e.to_string())
}

/// Parse a user HOCON config file.
///
/// **`strict()` is load-bearing, and this is the operator-facing half of AUDIT C141.** hocon 0.9's
/// loader is **non-strict by default**: its parser stops at the first syntax error and returns `Ok` with
/// everything after that point *discarded* — no error, no warning, not even a log line. So an operator
/// who leaves an unbalanced brace, or who edits the exported `shards = [...]` array and drops a line,
/// gets a node running on **defaults for every key below the error**, and the keys most likely to be
/// below it are the ones the file exists for: `casper.shards` and the per-shard genesis data cannot be
/// addressed from the command line at all (`docs/src/node/operating.md` — "File-configured"), so
/// reverting them means an ordinary single-shard `/root` node with default genesis parameters — a
/// *different chain*, and on a fresh data dir there is no shard-id mismatch to catch it. The asymmetry
/// is the tell: a *type* error in the same file is reported legibly by the same pipeline, so the error
/// path reads as exercised while the syntax path is not.
///
/// With `strict()`, hocon returns `Err("file could not be parsed completely")` instead, which the caller
/// reports as a configuration error.
pub fn parse_file(path: &PathBuf) -> Result<Hocon, String> {
    hocon::HoconLoader::new()
        .strict()
        .load_file(path)
        .map_err(|e| e.to_string())?
        .hocon()
        .map_err(|e| e.to_string())
}

/// Deep merge with fallback semantics (port of typesafe-config `withFallback`): the higher-priority
/// value wins; nested objects merge recursively.
pub fn merge(high: Hocon, low: Hocon) -> Hocon {
    match (high, low) {
        (Hocon::Hash(mut high_map), Hocon::Hash(low_map)) => {
            for (key, low_value) in low_map {
                match high_map.get(&key) {
                    Some(high_value) => {
                        let merged = merge(high_value.clone(), low_value);
                        high_map.insert(key, merged);
                    }
                    None => {
                        high_map.insert(key, low_value);
                    }
                }
            }
            Hocon::Hash(high_map)
        }
        (high, _) => high,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::model::{
        ApiServer, DevConf, Metrics, PeersDiscovery, ProtocolClient, ProtocolServer,
        ResourceGuardConf, Storage,
    };
    use rchain_casper::{CasperConf, GenesisBlockData, ShardMemberships, ShardSpec};
    use rchain_comm::peer_node::PeerNode;
    use rchain_comm::transport::tls_conf::TlsConf;
    use std::time::Duration;

    fn default_pos_multi_sig_public_keys() -> Vec<String> {
        vec![
            "04db91a53a2b72fcdcb201031772da86edad1e4979eb6742928d27731b1771e0bc40c9e9c9fa6554bdec041a87cee423d6f2e09e9dfb408b78e85a4aa611aad20c".to_string(),
            "042a736b30fffcc7d5a58bb9416f7e46180818c82b15542d0a7819d1a437aa7f4b6940c50db73a67bfc5f5ec5b5fa555d24ef8339b03edaa09c096de4ded6eae14".to_string(),
            "047f0f0f5bbe1d6d1a8dac4d88a3957851940f39a57cd89d55fe25b536ab67e6d76fd3f365c83e5bfe11fe7117e549b1ae3dd39bfc867d1c725a4177692c4e7754".to_string(),
        ]
    }

    fn default_pos_vault_pub_key() -> String {
        "0432946f7f91f8f767d7c3d43674faf83586dffbd1b8f9278a5c72820dc20308836299f47575ff27f4a736b72e63d91c3cd853641861f64e08ee5f9204fc708df6".to_string()
    }

    fn default_system_contract_pub_key() -> String {
        "04e2eb6b06058d10b30856043c29076e2d2d7c374d2beedded6ecb8d1df585dfa583bd7949085ac6b0761497b0cfd056eb3d0db97efb3940b14c00fff4e53c85bf".to_string()
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// The default single-shard membership: the `root` shard under the root parent (full id
    /// `/root`), with the given genesis data.
    fn root_shard_spec(
        genesis_block_data: GenesisBlockData,
        autogen_shard_size: i32,
    ) -> ShardMemberships {
        ShardMemberships::new(vec![ShardSpec::new(
            "root".to_string(),
            "/".to_string(),
            genesis_block_data,
            autogen_shard_size,
        )
        .expect("a valid root shard spec")])
        .expect("a non-empty membership set")
    }

    /// **The divergence injection cannot be armed on a node that is not in dev mode.**
    ///
    /// It is the instrument's only guard, and a guard rather than a convention: a node that perturbs its
    /// own fringe records diverges from its peers *on purpose*, and no peer can distinguish that from a
    /// node that diverged by accident — the chain in that state does not recover (TE-1). The assertions
    /// are deliberately a pair, so the test cannot pass by refusing everything: the same configuration is
    /// refused without `dev_mode` and accepted with it, and a node injecting nothing needs no dev-mode.
    #[test]
    fn the_merge_divergence_injection_requires_dev_mode() {
        let mut conf = default_expected();
        conf.casper.merge_divergence_injection = 1;
        conf.dev_mode = false;
        let err = check_merge_divergence_injection(&conf)
            .expect_err("arming the injection outside dev-mode must be refused");
        assert!(
            err.contains("requires --dev-mode"),
            "the refusal has to name what is missing: {err}"
        );

        conf.dev_mode = true;
        assert!(
            check_merge_divergence_injection(&conf).is_ok(),
            "…and dev-mode is the whole of the requirement"
        );

        conf.dev_mode = false;
        conf.casper.merge_divergence_injection = 0;
        assert!(
            check_merge_divergence_injection(&conf).is_ok(),
            "a node that injects nothing needs no dev-mode"
        );
    }

    /// **A `--sync-anchor` that is not a block hash is refused at load, and a real one is accepted.**
    ///
    /// The value decides where a wiped node syncs to (C259), so a typo in it is the difference between
    /// recovering a chain and syncing to something nobody vouched for. The assertions are a set rather
    /// than a single refusal, so the test cannot pass by refusing everything: no anchor is the ordinary
    /// path and a well-formed block hash is the operator's real case.
    #[test]
    fn a_sync_anchor_that_is_not_a_block_hash_is_refused() {
        let mut conf = default_expected();
        assert!(
            check_sync_anchor(&conf).is_ok(),
            "no anchor is the ordinary path and must stay accepted"
        );

        conf.casper.sync_anchor = Some("not-a-hash".to_string());
        let err = check_sync_anchor(&conf).expect_err("a typo must be refused at load");
        assert!(
            err.contains("is not a block hash"),
            "the refusal says what is wrong: {err}"
        );

        conf.casper.sync_anchor =
            Some("c01a6091fbde1a1098e7791d29818e3da81380da87f142cc74c98b2ae5d2face".to_string());
        assert!(
            check_sync_anchor(&conf).is_ok(),
            "a well-formed block hash is the case the switch exists for"
        );
    }

    /// **A `dev-tax-address` that does not parse is refused at load, and an empty one is accepted**
    /// (Law 78).
    ///
    /// The allocation's *amount* is derived from the wallets total, so a malformed address cannot make
    /// it wrong — it makes the genesis descriptors fail to build, where the error reads as a
    /// genesis-parsing problem rather than the operator typo it is. A set is asserted rather than one
    /// refusal so the test cannot pass by refusing everything: empty is the devnet and testnet path and
    /// must stay accepted, and a well-formed REV address is the case the key exists for.
    #[test]
    fn a_dev_tax_address_that_is_not_a_rev_address_is_refused() {
        let mut conf = default_expected();
        assert!(
            check_dev_tax_address(&conf).is_ok(),
            "empty is the path for a network that allocates none"
        );

        conf.casper
            .shards
            .primary_mut()
            .genesis_block_data
            .dev_tax_address = "not-an-address".to_string();
        let err = check_dev_tax_address(&conf).expect_err("a typo must be refused at load");
        assert!(
            err.contains("dev-tax-address") && err.contains("not a REV address"),
            "the refusal says what is wrong: {err}"
        );

        let mut key = vec![0u8; 65];
        key[0] = 0x04;
        let address = rchain_rholang::util::rev_address::RevAddress::from_public_key(
            &rchain_crypto::public_key::PublicKey::new(key),
        )
        .expect("a valid address");
        conf.casper
            .shards
            .primary_mut()
            .genesis_block_data
            .dev_tax_address = address.to_base58();
        assert!(
            check_dev_tax_address(&conf).is_ok(),
            "a well-formed REV address is the case the key exists for"
        );
    }

    /// **`self-reset` is off by default, and refused where it could not work** (#320).
    ///
    /// The switch is the operator's consent to a rewind, so the two ends are what matter: off is the
    /// ordinary path and must stay accepted, and on is refused on a node with nobody to re-sync from —
    /// a standalone node would rewind itself behind its own chain and have no peer to ask for the
    /// blocks above the anchor. The assertions are a set so the test cannot pass by refusing
    /// everything.
    #[test]
    fn self_reset_is_refused_without_a_peer_to_re_sync_from() {
        let mut conf = default_expected();
        assert!(
            check_self_reset(&conf).is_ok(),
            "off is the ordinary path: this is what every build before #320 did"
        );

        conf.casper.self_reset = true;
        assert!(
            check_self_reset(&conf).is_ok(),
            "on a node with a bootstrap the switch is the case it exists for"
        );

        conf.standalone = true;
        let err = check_self_reset(&conf).expect_err("a standalone node has nobody to ask");
        assert!(
            err.contains("needs a bootstrap"),
            "the refusal names what is missing: {err}"
        );
    }

    fn default_expected() -> NodeConf {
        let bootstrap = PeerNode::from_address(
            "rnode://de6eed5d00cf080fc587eeb412cb31a75fd10358@52.119.8.109?protocol=40400&discovery=40404",
        )
        .unwrap();
        NodeConf {
            standalone: false,
            autopropose: false,
            propose_on_deploy: false,
            attest_on_new_blocks: true,
            no_attest_on_new_blocks: false,
            dev_mode: false,
            resource_guard: ResourceGuardConf::default(),
            protocol_server: ProtocolServer {
                network_id: "testnet".to_string(),
                host: None,
                use_random_ports: false,
                dynamic_ip: false,
                no_upnp: false,
                port: 40400,
                grpc_max_recv_message_size: 262144,
                grpc_max_recv_stream_message_size: 268435456,
                max_message_consumers: 400,
                disable_state_exporter: false,
            },
            protocol_client: ProtocolClient {
                network_id: "testnet".to_string(),
                bootstrap,
                disable_lfs: false,
                batch_max_connections: 20,
                network_timeout: secs(5),
                grpc_max_recv_message_size: 262144,
                grpc_stream_chunk_size: 262144,
            },
            peers_discovery: PeersDiscovery {
                port: 40404,
                lookup_interval: secs(20),
                cleanup_interval: secs(20 * 60),
                heartbeat_batch_size: 100,
                init_wait_loop_interval: secs(1),
            },
            api_server: ApiServer {
                host: "0.0.0.0".to_string(),
                port_grpc_external: 40401,
                port_grpc_internal: 40402,
                grpc_max_recv_message_size: 16777216,
                port_http: 40403,
                port_admin_http: 40405,
                max_blocks_limit: 50,
                enable_reporting: false,
                publish_node_list: false,
                node_list_probe_interval: secs(60),
                node_list_probe_timeout: secs(2),
                node_list_max_entries: 32,
                enable_txn_api: false,
                enable_devnet_cors: false,
                enable_devnet_admin_public: false,
                keep_alive_time: secs(2 * 60 * 60),
                keep_alive_timeout: secs(20),
                permit_keep_alive_time: secs(5 * 60),
                max_connection_idle: secs(60 * 60),
                max_connection_age: secs(60 * 60),
                max_connection_age_grace: secs(60 * 60),
                ocapn_listen: None,
                ocapn_listen_unix: None,
                ocapn_listen_noise: None,
                ocapn_listen_websocket: None,
                ocapn_advertised_host: None,
                ocapn_identity_key: None,
                ocapn_deny_local_dial: false,
                enable_ocapn_dial: false,
            },
            storage: Storage {
                data_dir: PathBuf::from("/var/lib/rnode"),
            },
            tls: TlsConf {
                certificate_path: PathBuf::from("/var/lib/rnode/node.certificate.pem"),
                key_path: PathBuf::from("/var/lib/rnode/node.key.pem"),
                secure_random_non_blocking: false,
                custom_certificate_location: false,
                custom_key_location: false,
            },
            casper: CasperConf {
                validator_public_key: None,
                validator_private_key: None,
                validator_private_key_path: None,
                shards: root_shard_spec(
                    GenesisBlockData {
                        executor_share: 2500,
                        absence_slack: 0,
                        participation_grace: 0,
                        min_phlo_price: 1,
                        max_number_of_parents: 255,
                        genesis_data_dir: PathBuf::from("/var/lib/rnode/genesis"),
                        bonds_file: "/var/lib/rnode/genesis/bonds.txt".to_string(),
                        wallets_file: "/var/lib/rnode/genesis/wallets.txt".to_string(),
                        bond_minimum: 1,
                        bond_maximum: 9223372036854775807,
                        epoch_length: 10000,
                        quarantine_length: 50000,
                        genesis_block_number: 0,
                        number_of_active_validators: 100,
                        pos_multi_sig_public_keys: default_pos_multi_sig_public_keys(),
                        pos_multi_sig_quorum: 2,
                        pos_vault_pub_key: String::new(),
                        system_contract_pub_key: String::new(),
                        dev_tax_address: String::new(),
                        dev_tax_vesting_blocks: 18_934_560,

                        liveness_absence_threshold: 0,
                        liveness_jail_after_boundaries: 0,
                        liveness_jail_epochs: 0,
                        liveness_vote_window: 0,
                        liveness_slash_severity: 0,
                    },
                    5,
                ),
                casper_loop_interval: secs(30),
                requested_blocks_timeout: secs(240),
                // Mirrors `defaults.conf`; it moved with it when #153 made the key a consensus bound.
                // The two have to agree, and this fixture is what proves the parsed defaults equal the
                // documented ones.
                max_number_of_parents: 255,
                fork_choice_stale_threshold: secs(10 * 60),
                fork_choice_check_if_stale_interval: secs(11 * 60),
                synchrony_constraint_threshold: 0.67,
                height_constraint_threshold: 1000,
                min_phlo_price: 1,
                equivocation_injection: false,
                merge_divergence_injection: 0,
                sync_anchor: None,
                expected_genesis_hash: None,
                self_reset: false,
                effect_mode: "dfs".to_string(),
            },
            metrics: Metrics {
                prometheus: false,
                influxdb: false,
                influxdb_udp: false,
                zipkin: false,
                sigar: false,
            },
            dev: DevConf {
                deployer_private_key: None,
            },
            default_data_dir: "/var/lib/rnode".to_string(),
        }
    }

    /// Build a node config the way `main.rs` does — a real `Options` parse, then
    /// `Configuration::build` — from a `run` invocation. The data dir is private to `case` so no
    /// stray `/var/lib/rnode/rnode.conf` can reach a test; `config_file`, when given, is written to
    /// `<data-dir>/rnode.conf`, which is where `build` looks by default.
    fn build_run(
        case: &str,
        flags: &[&str],
        config_file: Option<&str>,
    ) -> Result<NodeConf, String> {
        use clap::Parser as _;
        let dir =
            std::env::temp_dir().join(format!("rchain_metrics_{}_{case}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a writable temp data dir");
        if let Some(body) = config_file {
            std::fs::write(dir.join("rnode.conf"), body).expect("write the test config file");
        }
        let mut args: Vec<String> = vec![
            "rchain".to_string(),
            "run".to_string(),
            "--data-dir".to_string(),
            dir.to_string_lossy().into_owned(),
        ];
        args.extend(flags.iter().map(|f| (*f).to_string()));
        let options = Options::parse_from(args);
        Configuration::build(&options).map(|(conf, _, _)| conf)
    }

    /// A `run` that asks for no reporter builds exactly as before: the refusal is the *only* new
    /// gate, and it cannot fire for an operator who asked for nothing.
    #[test]
    fn a_config_without_the_metrics_switches_still_builds() {
        let conf = build_run("no_flags", &[], None).expect("a plain run must still build");
        assert_eq!(
            conf.metrics,
            Metrics {
                prometheus: false,
                influxdb: false,
                influxdb_udp: false,
                zipkin: false,
                sigar: false,
            }
        );
    }

    /// AUDIT §6: the four reporter switches have no counterpart in this port, so asking for one is
    /// an error at startup naming it — rather than a setting that is accepted and reports nothing.
    #[test]
    fn metrics_reporters_absent_from_this_port_are_refused() {
        for (flag, name) in [
            ("--influxdb", "influxdb"),
            ("--influxdb-udp", "influxdb-udp"),
            ("--zipkin", "zipkin"),
            ("--sigar", "sigar"),
        ] {
            let err = build_run(name, &[flag], None)
                .expect_err("a reporter with no implementation must be refused");
            assert!(err.contains("unimplemented"), "{flag}: {err}");
            assert!(
                err.contains(name),
                "{flag} must be named in the error: {err}"
            );
            assert!(
                err.contains("/metrics"),
                "{flag}: the error must say what the port does serve: {err}"
            );
        }
    }

    /// The same refusal reaches an operator who never touched the CLI: a Scala-shaped config file
    /// that enables a reporter is refused too, because the check reads the merged config.
    #[test]
    fn a_config_file_enabling_an_absent_reporter_is_refused_too() {
        let err = build_run(
            "config_file",
            &[],
            Some("metrics {\n  influxdb = true\n}\n"),
        )
        .expect_err("a config file enabling an absent reporter must be refused");
        assert!(err.contains("influxdb"), "{err}");
    }

    /// `--prometheus` is accepted: the endpoint it asks for *does* exist here (always on rather than
    /// gated), so the honest answer is a note, not a refusal — and refusing it would break every
    /// Scala-shaped config that sets it for a feature the port already provides.
    #[test]
    fn prometheus_is_accepted_with_a_note_that_the_endpoint_is_always_on() {
        let conf =
            build_run("prometheus", &["--prometheus"], None).expect("prometheus is accepted");
        assert!(conf.metrics.prometheus, "the setting is still parsed");
        let note = check_metrics_config(&conf.metrics)
            .expect("accepted")
            .expect("a note, not silence");
        assert!(note.contains("/metrics"), "{note}");
        assert!(note.contains("always on"), "{note}");
    }

    #[test]
    fn parse_default_config() {
        let default_config = parse_defaults("/var/lib/rnode").unwrap();
        let config = node_conf_from_hocon(&default_config).unwrap();
        assert_eq!(config, default_expected());
    }

    /// Parse a HOCON fragment, for the shard-membership tests below.
    /// **The regression test for AUDIT C147.** A `--profile` is the only thing that decides where a
    /// node's data directory lives, and an unrecognised name used to be replaced by the default profile
    /// in silence — whose directory is `$HOME/.rnode`. The docs' own command is `--profile docker`, one
    /// keystroke from `docer`, so an operator's typo started the node on a *fresh, empty* data
    /// directory: what looks like data loss or an unexpected re-genesis, with nothing naming the
    /// profile. The control keeps the refusal honest — both names this port has must still build, so
    /// this is a refusal about the *name* and not about profiles in general.
    #[test]
    fn an_unknown_profile_is_refused_rather_than_replaced_by_the_default() {
        use crate::configuration::commandline::options::Options;
        use clap::Parser as _;

        // `--profile` is a *global* option, so it precedes the subcommand — which is how the docs spell
        // it (`rnode --profile docker run -s …`), and why this test builds its argv rather than using
        // `build_run` (whose flags land after `run`).
        let dir = std::env::temp_dir().join(format!("rnode-c147-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let build = |profile: &str| {
            let options = Options::parse_from([
                "rchain",
                "--profile",
                profile,
                "run",
                "--data-dir",
                &dir.to_string_lossy(),
            ]);
            Configuration::build(&options).map(|(conf, _, _)| conf)
        };

        let err = build("docer").expect_err(
            "an unknown profile must be refused — the fallback moves the node's data directory",
        );
        assert!(
            err.contains("unknown profile") && err.contains("docer"),
            "the refusal must name the profile it did not recognise: {err}"
        );

        for name in ["default", "docker"] {
            assert!(
                build(name).is_ok(),
                "the profile '{name}' must still build, so the refusal is about the name and not \
                 about profiles in general"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The regression test for AUDIT C143.** Nine keys are parsed into `NodeConf` and read by
    /// nothing: `max-message-consumers` — whose documented purpose is the inbound dispatch bound, which
    /// is the hardcoded 1024 — `use-random-ports`, `dynamic-ip`, `init-wait-loop-interval`, and the five
    /// keepalive knobs, where no HTTP/2 or TCP keepalive exists anywhere in the tree. The disposition is
    /// the one this register already chose twice (the metrics reporters, `disable-state-exporter`):
    /// refused rather than accepted and ignored.
    ///
    /// The **control** matters as much as the refusals and is here explicitly: a setting this port *does*
    /// enforce must still build, so the check is value-sensitive rather than "the key appeared in a
    /// file". (The other forty-one tests in this module are the same control at larger scale — none of
    /// them changes one of the nine, and all of them still build.)
    #[test]
    fn a_setting_this_port_does_not_enforce_is_refused() {
        // `max-message-consumers` is reachable both ways, and the CLI is checked first because it is
        // the layer that wins: the file spelling below it is then the same refusal through the same
        // function.
        let err = build_run(
            "inert_cli",
            &["--protocol-max-message-consumers", "5000"],
            None,
        )
        .expect_err("a setting this port does not enforce must be refused (AUDIT C143)");
        assert!(
            err.contains("unimplemented setting"),
            "the refusal must say what it is about: {err}"
        );

        // Collected rather than asserted in the loop: the first failure would hide the rest, and which
        // of the nine are reachable *through a config file* is the thing worth knowing.
        let mut not_refused: Vec<&str> = Vec::new();
        let mut wrongly: Vec<(String, String)> = Vec::new();
        for body in [
            "protocol-server { max-message-consumers = 5000 }",
            "protocol-server { use-random-ports = true }",
            "protocol-server { dynamic-ip = true }",
            // `2 seconds`, not `1 second`: the default *is* `1s`, and a fixture set to its own default
            // is a change that did not happen — the comparison sees no difference and is right not to
            // refuse. That mistake is why this test collects its failures instead of asserting in the
            // loop: the whole list in one run is what showed it.
            "peers-discovery { init-wait-loop-interval = 2 seconds }",
            "api-server { keep-alive-time = 5 seconds }",
            "api-server { keep-alive-timeout = 5 seconds }",
            "api-server { permit-keep-alive-time = 5 seconds }",
            "api-server { max-connection-age = 5 seconds }",
            "api-server { max-connection-age-grace = 5 seconds }",
        ] {
            match build_run("inert", &[], Some(body)) {
                Err(e) if e.contains("unimplemented setting") => {}
                Err(e) => wrongly.push((body.to_string(), e)),
                Ok(_) => not_refused.push(body),
            }
        }
        assert!(
            not_refused.is_empty() && wrongly.is_empty(),
            "every one of these must be refused, and for its own reason (AUDIT C143).\n  accepted: \
             {not_refused:#?}\n  refused for some other reason: {wrongly:#?}"
        );

        assert!(
            build_run("enforced", &[], Some("api-server { port-http = 40403 }")).is_ok(),
            "the control: a setting this port does enforce must still build"
        );
    }

    /// **The regression test for AUDIT C141.** A config file whose first line is valid and whose second
    /// is not must be **refused**, not parsed up to the error: the loader is non-strict by default and
    /// answers `Ok` with everything after the syntax error discarded, so the keys below it silently
    /// revert to their defaults — and for `casper.shards` and the per-shard genesis data, addressable
    /// only from this file, "reverts to the default" means a node on a different chain.
    ///
    /// Three cases, because the *shape* of the failure is what the row demonstrated: junk first (the
    /// valid key after it was dropped and the node came up on defaults), junk last (the same defect seen
    /// from the other end, and the one an operator creates by dropping a line from an array), and the
    /// control — so a refusal below cannot be the loader refusing everything.
    #[test]
    fn a_config_file_with_a_syntax_error_is_refused_not_truncated() {
        let dir = std::env::temp_dir().join(format!("rnode-c141-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let write = |name: &str, body: &str| {
            let p = dir.join(name);
            std::fs::write(&p, body).expect("write config");
            p
        };

        let good = write("good.conf", "api-server { port-http = 40403 }\n");
        assert!(
            parse_file(&good).is_ok(),
            "the control: a valid config file must still parse"
        );

        let junk_first = write(
            "junk-first.conf",
            "this is = = not hocon [[[\napi-server { port-http = 1 }\n",
        );
        let err = parse_file(&junk_first).expect_err(
            "a file that is not entirely HOCON must be refused — everything after the syntax error \
             would otherwise be dropped without a word (AUDIT C141)",
        );
        assert!(!err.is_empty(), "and the refusal must say something: {err}");

        let junk_last = write(
            "junk-last.conf",
            "api-server { port-http = 1 }\nshards = [\n",
        );
        assert!(
            parse_file(&junk_last).is_err(),
            "an unterminated array must be refused, not cut off at the bracket"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn parse_hocon_str(s: &str) -> Hocon {
        hocon::HoconLoader::new()
            .load_str(s)
            .expect("parse hocon")
            .hocon()
            .expect("hocon tree")
    }

    /// A node configured with the legacy scalar shard keys and one configured with an equivalent
    /// one-entry `casper.shards` array must be the same node: the array form has to be the identity
    /// case, not merely equivalent-looking.
    #[test]
    fn one_entry_shards_array_equals_the_legacy_scalar_keys() {
        let legacy = node_conf_from_hocon(&merge(
            parse_hocon_str("casper { shard-name = root, parent-shard-id = / }"),
            parse_defaults("/var/lib/rnode").unwrap(),
        ))
        .expect("legacy config");

        let array = node_conf_from_hocon(&merge(
            parse_hocon_str("casper { shards = [ { shard-name = root, parent-shard-id = / } ] }"),
            parse_defaults("/var/lib/rnode").unwrap(),
        ))
        .expect("array config");

        assert_eq!(legacy.casper.shards, array.casper.shards);
        assert_eq!(legacy.casper.shards.len(), 1);
        assert_eq!(legacy.casper.shards.primary().shard_id.to_string(), "/root");
        // The entry inherits the node-level genesis data rather than getting an empty block.
        assert_eq!(
            legacy.casper.shards.primary().genesis_block_data,
            array.casper.shards.primary().genesis_block_data
        );
    }

    /// A `shards` array carries one membership per entry, in order (entry 0 is the primary), and an
    /// entry-level `genesis-block-data` replaces the node-level one for that shard only.
    #[test]
    fn shards_array_parses_each_membership_with_its_own_genesis() {
        let config = node_conf_from_hocon(&merge(
            parse_hocon_str(
                r#"casper { shards = [
                     { shard-name = root, parent-shard-id = / }
                     { shard-name = child, parent-shard-id = /root,
                       genesis-block-data { bonds-file = /var/lib/rnode/child-bonds.txt } }
                   ] }"#,
            ),
            parse_defaults("/var/lib/rnode").unwrap(),
        ))
        .expect("multi-shard config");

        let shards = &config.casper.shards;
        assert_eq!(shards.len(), 2);
        let ids: Vec<String> = shards.iter().map(|s| s.shard_id.to_string()).collect();
        assert_eq!(ids, vec!["/root".to_string(), "/root/child".to_string()]);
        assert_eq!(shards.primary().shard_id.to_string(), "/root");

        // Entry 1 overrode its bonds file wholesale; every other field came from the node default,
        // so nothing was silently zeroed by the override.
        let child = shards
            .get(&rchain_shared::refined::ShardId::try_from("/root/child".to_string()).unwrap());
        let child = child.expect("child membership");
        assert_eq!(
            child.genesis_block_data.bonds_file,
            "/var/lib/rnode/child-bonds.txt"
        );
        assert_eq!(
            child.genesis_block_data.epoch_length,
            shards.primary().genesis_block_data.epoch_length
        );
    }

    /// A nested hierarchy resolves in order, with the first entry the primary — the ordering the
    /// per-shard data directories and the request default both depend on.
    #[test]
    fn three_memberships_resolve_in_order() {
        let config = node_conf_from_hocon(&merge(
            parse_hocon_str(
                r#"casper { shards = [
                     { shard-name = root, parent-shard-id = / }
                     { shard-name = child, parent-shard-id = /root }
                     { shard-name = leaf, parent-shard-id = /root/child }
                   ] }"#,
            ),
            parse_defaults("/var/lib/rnode").unwrap(),
        ))
        .expect("three memberships");

        let ids: Vec<String> = config
            .casper
            .shards
            .iter()
            .map(|s| s.shard_id.to_string())
            .collect();
        assert_eq!(ids, vec!["/root", "/root/child", "/root/child/leaf"]);
        assert_eq!(config.casper.shards.len(), 3);
        assert_eq!(config.casper.shards.primary().shard_id.to_string(), "/root");
    }

    /// **The hostlist is off unless an operator turns it on**, and its bounds are refused where every
    /// other operator-facing configuration error is. The bounds are checked whether or not the route
    /// is mounted, so a configuration that would be refused at the restart that enables it is refused
    /// now.
    #[test]
    fn publish_node_list_defaults_off_parses_on_and_validates_its_bounds() {
        let off = node_conf_from_hocon(&parse_defaults("/var/lib/rnode").unwrap()).unwrap();
        assert!(!off.api_server.publish_node_list);
        assert_eq!(off.api_server.node_list_max_entries, 32);
        assert_eq!(
            off.api_server.node_list_probe_interval,
            std::time::Duration::from_secs(60)
        );

        let on = node_conf_from_hocon(&merge(
            parse_hocon_str("api-server { publish-node-list = true }"),
            parse_defaults("/var/lib/rnode").unwrap(),
        ))
        .unwrap();
        assert!(on.api_server.publish_node_list);

        let bad = |body: &str| {
            let conf = node_conf_from_hocon(&merge(
                parse_hocon_str(body),
                parse_defaults("/var/lib/rnode").unwrap(),
            ))
            .unwrap();
            crate::web::node_list::NodeListConfig {
                probe_interval: conf.api_server.node_list_probe_interval,
                probe_timeout: conf.api_server.node_list_probe_timeout,
                max_entries: conf.api_server.node_list_max_entries,
            }
            .validate()
        };
        for body in [
            "api-server { node-list-max-entries = 0 }",
            "api-server { node-list-max-entries = 129 }",
            // A probe that outlives its cycle starts the next one on top of the last.
            "api-server { node-list-probe-interval = 1s, node-list-probe-timeout = 5s }",
            "api-server { node-list-probe-timeout = 0s }",
        ] {
            assert!(bad(body).is_err(), "{body} must be refused");
        }
    }

    /// The transaction API is off unless an operator turns it on: a gateway node must not serve the
    /// routes by virtue of being a gateway.
    #[test]
    fn enable_txn_api_defaults_off_and_parses_on() {
        let off = node_conf_from_hocon(&parse_defaults("/var/lib/rnode").unwrap()).unwrap();
        assert!(!off.api_server.enable_txn_api);

        let on = node_conf_from_hocon(&merge(
            parse_hocon_str("api-server { enable-txn-api = true }"),
            parse_defaults("/var/lib/rnode").unwrap(),
        ))
        .unwrap();
        assert!(on.api_server.enable_txn_api);
    }

    /// Setting both the array and the scalar keys is an error rather than a silent precedence rule.
    #[test]
    fn shards_and_scalar_keys_are_mutually_exclusive() {
        let both = parse_hocon_str(
            "casper { shard-name = root, parent-shard-id = /, shards = [ { shard-name = root, parent-shard-id = / } ] }",
        );
        let err = check_shard_config_exclusivity(&both, "the config file")
            .expect_err("must reject both forms");
        assert!(err.contains("mutually exclusive"), "{err}");
        // A layer with only one of the two forms is fine.
        assert!(check_shard_config_exclusivity(
            &parse_hocon_str("casper { shard-name = root }"),
            "the config file"
        )
        .is_ok());
    }

    /// A node must belong to at least one shard, and no shard may be listed twice.
    #[test]
    fn memberships_reject_empty_and_duplicate_lists() {
        let base = parse_defaults("/var/lib/rnode").unwrap();
        let empty = node_conf_from_hocon(&merge(
            parse_hocon_str("casper { shards = [] }"),
            base.clone(),
        ));
        assert!(empty.is_err(), "an empty shards array must be rejected");

        let duplicate = node_conf_from_hocon(&merge(
            parse_hocon_str(
                "casper { shards = [ { shard-name = root, parent-shard-id = / }, { shard-name = root, parent-shard-id = / } ] }",
            ),
            base,
        ));
        let err = duplicate.expect_err("duplicate memberships must be rejected");
        assert!(err.contains("duplicate shard membership"), "{err}");
    }

    #[test]
    fn cli_options_override_defaults() {
        use crate::configuration::commandline::options::Options;
        use clap::Parser as _;

        let args = "\
            run --standalone --dev-mode \
            --host localhost \
            --bootstrap rnode://de6eed5d00cf080fc587eeb412cb31a75fd10358@52.119.8.109?protocol=40400&discovery=40404 \
            --network-id testnet --no-upnp --dynamic-ip --autogen-shard-size 111111 --use-random-ports \
            --network-timeout 111111seconds --discovery-port 111111 --discovery-lookup-interval 111111seconds \
            --discovery-cleanup-interval 111111seconds --discovery-heartbeat-batch-size 111111 \
            --discovery-init-wait-loop-interval 111111seconds --protocol-port 111111 \
            --protocol-grpc-max-recv-message-size 111111 --protocol-grpc-max-recv-stream-message-size 111111 \
            --protocol-grpc-stream-chunk-size 111111 --protocol-max-connections 111111 \
            --protocol-max-message-consumers 111111 --disable-state-exporter \
            --tls-certificate-path /var/lib/rnode/node.certificate.pem --tls-key-path /var/lib/rnode/node.key.pem \
            --tls-secure-random-non-blocking --api-host localhost --api-port-grpc-external 111111 \
            --api-port-grpc-internal 111111 --api-port-http 111111 --api-port-admin-http 111111 \
            --api-grpc-max-recv-message-size 111111 --api-max-blocks-limit 111111 --api-enable-reporting \
            --api-keep-alive-time 111111seconds --api-keep-alive-timeout 111111seconds \
            --api-permit-keep-alive-time 111111seconds --api-max-connection-idle 111111seconds \
            --api-max-connection-age 111111seconds --api-max-connection-age-grace 111111seconds \
            --data-dir /var/lib/rnode --shard-name root --validator-public-key 111111 \
            --validator-private-key-path /var/lib/rnode/pem.key \
            --casper-loop-interval 111111seconds --requested-blocks-timeout 111111seconds \
            --max-number-of-parents 111111 --fork-choice-stale-threshold 111111seconds \
            --fork-choice-check-if-stale-interval 111111seconds --synchrony-constraint-threshold 111111 \
            --height-constraint-threshold 111111 --bonds-file /var/lib/rnode/genesis/bonds1.txt \
            --wallets-file /var/lib/rnode/genesis/wallets1.txt --bond-minimum 111111 --bond-maximum 111111 \
            --epoch-length 111111 --quarantine-length 111111 --genesis-block-number 222 \
            --number-of-active-validators 111111 --pos-vault-pub-key 0432946f7f91f8f767d7c3d43674faf83586dffbd1b8f9278a5c72820dc20308836299f47575ff27f4a736b72e63d91c3cd853641861f64e08ee5f9204fc708df6 \
            --system-contract-pub-key 04e2eb6b06058d10b30856043c29076e2d2d7c374d2beedded6ecb8d1df585dfa583bd7949085ac6b0761497b0cfd056eb3d0db97efb3940b14c00fff4e53c85bf \
            --disable-lfs --prometheus --influxdb --influxdb-udp --zipkin --sigar --no-attest-on-new-blocks";

        let options = Options::parse_from(std::iter::once("rchain").chain(args.split_whitespace()));
        let options_config = config_mapper::from_options(&options);
        let default_config = parse_defaults("/var/lib/rnode").unwrap();
        let merged = merge(options_config, default_config);
        let config = node_conf_from_hocon(&merged).unwrap();

        let bootstrap = PeerNode::from_address(
            "rnode://de6eed5d00cf080fc587eeb412cb31a75fd10358@52.119.8.109?protocol=40400&discovery=40404",
        )
        .unwrap();

        let expected = NodeConf {
            standalone: true,
            autopropose: false,
            propose_on_deploy: false,
            attest_on_new_blocks: true,
            no_attest_on_new_blocks: true,
            dev_mode: true,
            resource_guard: ResourceGuardConf::default(),
            protocol_server: ProtocolServer {
                network_id: "testnet".to_string(),
                host: Some("localhost".to_string()),
                use_random_ports: true,
                dynamic_ip: true,
                no_upnp: true,
                port: 111111,
                grpc_max_recv_message_size: 111111,
                grpc_max_recv_stream_message_size: 111111,
                max_message_consumers: 111111,
                disable_state_exporter: true,
            },
            protocol_client: ProtocolClient {
                network_id: "testnet".to_string(),
                bootstrap,
                disable_lfs: true,
                batch_max_connections: 111111,
                network_timeout: secs(111111),
                grpc_max_recv_message_size: 111111,
                grpc_stream_chunk_size: 111111,
            },
            peers_discovery: PeersDiscovery {
                port: 111111,
                lookup_interval: secs(111111),
                cleanup_interval: secs(111111),
                heartbeat_batch_size: 111111,
                init_wait_loop_interval: secs(111111),
            },
            api_server: ApiServer {
                host: "localhost".to_string(),
                port_grpc_external: 111111,
                port_grpc_internal: 111111,
                grpc_max_recv_message_size: 111111,
                port_http: 111111,
                port_admin_http: 111111,
                max_blocks_limit: 111111,
                enable_reporting: true,
                publish_node_list: false,
                node_list_probe_interval: secs(60),
                node_list_probe_timeout: secs(2),
                node_list_max_entries: 32,
                enable_txn_api: false,
                enable_devnet_cors: false,
                enable_devnet_admin_public: false,
                keep_alive_time: secs(111111),
                keep_alive_timeout: secs(111111),
                permit_keep_alive_time: secs(111111),
                max_connection_idle: secs(111111),
                max_connection_age: secs(111111),
                max_connection_age_grace: secs(111111),
                ocapn_listen: None,
                ocapn_listen_unix: None,
                ocapn_listen_noise: None,
                ocapn_listen_websocket: None,
                ocapn_advertised_host: None,
                ocapn_identity_key: None,
                ocapn_deny_local_dial: false,
                enable_ocapn_dial: false,
            },
            storage: Storage {
                data_dir: PathBuf::from("/var/lib/rnode"),
            },
            tls: TlsConf {
                certificate_path: PathBuf::from("/var/lib/rnode/node.certificate.pem"),
                key_path: PathBuf::from("/var/lib/rnode/node.key.pem"),
                secure_random_non_blocking: true,
                custom_certificate_location: false,
                custom_key_location: false,
            },
            casper: CasperConf {
                validator_public_key: Some("111111".to_string()),
                // **`None` from the command line, and that is the change** (W-N11-05): the flag that
                // put the scalar in `/proc/<pid>/cmdline` is gone, so the hex field is empty here and
                // the path route beside it is what `resolve_validator_private_key` resolves at boot.
                validator_private_key: None,
                validator_private_key_path: Some(PathBuf::from("/var/lib/rnode/pem.key")),
                shards: root_shard_spec(
                    GenesisBlockData {
                        executor_share: 2500,
                        absence_slack: 0,
                        participation_grace: 0,
                        min_phlo_price: 1,
                        max_number_of_parents: 111111,
                        genesis_data_dir: PathBuf::from("/var/lib/rnode/genesis"),
                        bonds_file: "/var/lib/rnode/genesis/bonds1.txt".to_string(),
                        wallets_file: "/var/lib/rnode/genesis/wallets1.txt".to_string(),
                        bond_minimum: 111111,
                        bond_maximum: 111111,
                        epoch_length: 111111,
                        quarantine_length: 111111,
                        genesis_block_number: 222,
                        number_of_active_validators: 111111,
                        pos_multi_sig_public_keys: default_pos_multi_sig_public_keys(),
                        pos_multi_sig_quorum: 2,
                        pos_vault_pub_key: default_pos_vault_pub_key(),
                        system_contract_pub_key: default_system_contract_pub_key(),
                        dev_tax_address: String::new(),
                        dev_tax_vesting_blocks: 18_934_560,

                        liveness_absence_threshold: 0,
                        liveness_jail_after_boundaries: 0,
                        liveness_jail_epochs: 0,
                        liveness_vote_window: 0,
                        liveness_slash_severity: 0,
                    },
                    111111,
                ),
                casper_loop_interval: secs(111111),
                requested_blocks_timeout: secs(111111),
                max_number_of_parents: 111111,
                fork_choice_stale_threshold: secs(111111),
                fork_choice_check_if_stale_interval: secs(111111),
                synchrony_constraint_threshold: 111111.0,
                height_constraint_threshold: 111111,
                min_phlo_price: 1,
                equivocation_injection: false,
                merge_divergence_injection: 0,
                sync_anchor: None,
                expected_genesis_hash: None,
                self_reset: false,
                effect_mode: "dfs".to_string(),
            },
            metrics: Metrics {
                prometheus: true,
                influxdb: true,
                influxdb_udp: true,
                zipkin: true,
                sigar: true,
            },
            dev: DevConf {
                deployer_private_key: None,
            },
            default_data_dir: "/var/lib/rnode".to_string(),
        };

        assert_eq!(config, expected);
    }

    /// **The scalar survives in the config as a plain `String`** (W-N5-08).
    ///
    /// The row: the only scrub of the validator scalar is `PrivateKey::drop`
    /// (`crypto/src/private_key.rs`), which zeroizes the key the *crypto* type holds. The runtime does
    /// not hold that one — it holds whatever this resolver put in `casper.validator_private_key`, a
    /// plain `Option<String>` (`casper/src/conf.rs`), so the 64-hex is still in the heap for the life
    /// of the process and nothing overwrites it.
    ///
    /// The reading is taken at the two moments the node has already taken what it needs: after the
    /// resolution, and after the identity has been derived from it. The row's recorded verdict is
    /// `NOT-RE-ESTABLISHED` — the referee could not confirm the deviation — so this is where the tree
    /// decides it rather than an assertion written around the verdict.
    #[test]
    fn the_validator_scalar_survives_in_the_config_string_after_the_identity_is_built() {
        const HEX: &str = "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530";
        let mut conf = default_expected();
        conf.casper.validator_private_key = Some(HEX.to_string());

        // What the runtime will do with it: derive the identity. That is the whole consumption of the
        // scalar, and it leaves the string untouched.
        let identity = rchain_casper::validator_identity::ValidatorIdentity::from_hex(HEX)
            .expect("a valid key");
        assert!(!identity.public_key.bytes().is_empty());

        assert_eq!(
            conf.casper.validator_private_key.as_deref(),
            Some(HEX),
            "**the whole scalar is still in the config field after the identity was built** \
             (W-N5-08): the only scrub is `PrivateKey::drop`, and the config holds a `String`"
        );
        assert_eq!(
            conf.casper.validator_private_key.as_deref().map(str::len),
            Some(64),
            "and it is the scalar itself, not a digest of it"
        );
    }

    /// **Two layers disagree about the validator key, and the node takes one without a word**
    /// (W-N5-01).
    ///
    /// The row: `casper.validator-private-key` in the config file and `--validator-private-key-path`
    /// on the command line are two ways to say the same thing, and the layered config lets them
    /// disagree. The documented precedence is real — one of them wins — but nothing tells the operator
    /// that the other was ignored, so a node that comes up on the *wrong* identity against a config
    /// the operator edited looks exactly like one that came up on the right one.
    ///
    /// **The reading is sharper than the row, and it is the other way round.** The arm was written
    /// expecting the command line to win; measured on 2026-10-14, the *config file* wins — so a
    /// `--validator-private-key-path` an operator typed is ignored by a hex sitting in a file the
    /// operator may not even have edited. The reason is that precedence is decided **per form and not
    /// per layer**: `resolve_validator_private_key` reads "an explicit hex always wins; the path is
    /// consulted only when that is absent", and it asks that of the *merged* config, where a lower
    /// layer's hex and a higher layer's path are both simply present. Nothing warns.
    ///
    /// Falsified by the warning or error the row asks for, or by a resolution that weighs layers
    /// before forms — either reddens the assertions below.
    #[test]
    fn a_config_files_key_overrides_the_command_lines_path_without_a_word() {
        // The key the file layer names, and the key the command line's path holds. Two different
        // identities, so the resolved one says which layer was believed.
        const FILE_HEX: &str = "2222222222222222222222222222222222222222222222222222222222222222";
        const PATH_HEX: &str = "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530";
        let dir = std::env::temp_dir().join(format!("rchain_layered_key_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a writable temp dir");
        let key_path = dir.join("cli.hex");
        rchain_crypto::util::key_util::write_private_key(&key_path, format!("{PATH_HEX}\n"))
            .expect("the command-line key file");

        let conf = build_run(
            "layered_key",
            &[
                "--validator-private-key-path",
                key_path.to_str().expect("a path"),
            ],
            Some(&format!(
                "casper {{ validator-private-key = \"{FILE_HEX}\" }}\n"
            )),
        )
        .expect("the node builds: the two layers disagreeing is not an error");

        let resolved = conf
            .casper
            .validator_private_key
            .as_deref()
            .expect("a key was resolved");
        assert_eq!(
            resolved, FILE_HEX,
            "**the config file's key won, and the command line's flag was ignored** (W-N5-01): the \
             operator typed `--validator-private-key-path`, the node read the file layer's hex \
             instead, and the build said nothing. Precedence here is decided per *form* (an explicit \
             hex beats a path) and not per *layer*, so the higher layer lost"
        );
        assert_ne!(
            resolved, PATH_HEX,
            "and the key the flag named is not the key in use — which is the whole of the row, one \
             layer deeper than it claims"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A validator key given *by path* must end up in the hex field the runtime reads.
    /// Before this, `--validator-private-key-path` was parsed and then dropped, so a
    /// node configured that way ran with no validator identity and said nothing.
    #[test]
    fn validator_private_key_path_is_resolved_into_the_hex_field() {
        let dir = std::env::temp_dir().join(format!("rchain_validator_key_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        const HEX: &str = "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530";
        let key_path = dir.join("validator.hex");
        // Trailing newline: what any editor or `echo` leaves behind. Owner-only, because the loader
        // refuses a key file another uid can reach (W-N5-09) — the accepted fixture has to be written
        // the way the writers write it, which is what makes the refusal below meaningful.
        rchain_crypto::util::key_util::write_private_key(&key_path, format!("{HEX}\n")).unwrap();

        let mut conf = default_expected();
        conf.casper.validator_private_key = None;
        conf.casper.validator_private_key_path = Some(key_path);
        resolve_validator_private_key(&mut conf).unwrap();
        assert_eq!(conf.casper.validator_private_key.as_deref(), Some(HEX));

        // An explicit hex key wins, and the path is not even read (it does not exist).
        let explicit = "11".repeat(32);
        conf.casper.validator_private_key = Some(explicit.clone());
        conf.casper.validator_private_key_path = Some(dir.join("does-not-exist.pem"));
        resolve_validator_private_key(&mut conf).unwrap();
        assert_eq!(
            conf.casper.validator_private_key.as_deref(),
            Some(explicit.as_str())
        );

        // A path that is set but is not a key is an error, not a silent no-op.
        let junk = dir.join("junk.txt");
        rchain_crypto::util::key_util::write_private_key(&junk, "not a key\n").unwrap();
        conf.casper.validator_private_key = None;
        conf.casper.validator_private_key_path = Some(junk);
        assert!(resolve_validator_private_key(&mut conf).is_err());

        // And a key file the whole host can read is an error too (W-N5-09): the node refuses to adopt
        // a signing identity that another local account could have taken a copy of. The mode is set
        // explicitly, because the process umask would otherwise mask the fixture into being readable
        // only by its owner.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let wide = dir.join("wide.hex");
            std::fs::write(&wide, format!("{HEX}\n")).unwrap();
            std::fs::set_permissions(&wide, std::fs::Permissions::from_mode(0o644)).unwrap();
            conf.casper.validator_private_key = None;
            conf.casper.validator_private_key_path = Some(wide.clone());
            let err = resolve_validator_private_key(&mut conf)
                .expect_err("a world-readable validator key must not be adopted");
            assert!(err.contains("wide.hex"), "{err}");
        }

        // No path and no hex key at all is fine (an observer node).
        conf.casper.validator_private_key = None;
        conf.casper.validator_private_key_path = None;
        resolve_validator_private_key(&mut conf).unwrap();
        assert!(conf.casper.validator_private_key.is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// **The four inert settings the C143 class had not reached** (2026-10-14, the coverage-gap
    /// pass). Each is parsed, is read by no code outside this module — the two thresholds are
    /// *documented* in `defaults.conf` as proposer guards, and the Scala oracle enforced both — and
    /// each was accepted and ignored. Changing one is refused now, so the silence is gone.
    ///
    /// The control is the first assertion: the defaults must pass, or the four would be refused for
    /// the wrong reason and this test would be green over a broken comparison.
    ///
    /// Falsifier: red on the pre-pass tree, where all four were accepted and the node started
    /// holding a bound that did not exist.
    #[test]
    fn the_inert_settings_this_pass_found_are_refused() {
        let defaults = node_conf_from_hocon(&parse_defaults("/var/lib/rnode").expect("defaults"))
            .expect("defaults parse");
        check_inert_config(&defaults, &defaults).expect("the defaults are not refused");

        for (name, change) in [
            (
                "casper.synchrony-constraint-threshold",
                Box::new(|c: &mut NodeConf| {
                    c.casper.synchrony_constraint_threshold += 0.01;
                }) as Box<dyn Fn(&mut NodeConf)>,
            ),
            (
                "casper.height-constraint-threshold",
                Box::new(|c: &mut NodeConf| {
                    c.casper.height_constraint_threshold += 1;
                }),
            ),
            (
                "casper.fork-choice-stale-threshold",
                Box::new(|c: &mut NodeConf| {
                    c.casper.fork_choice_stale_threshold += std::time::Duration::from_secs(1);
                }),
            ),
            (
                "casper.fork-choice-check-if-stale-interval",
                Box::new(|c: &mut NodeConf| {
                    c.casper.fork_choice_check_if_stale_interval +=
                        std::time::Duration::from_secs(1);
                }),
            ),
        ] {
            let mut changed = defaults.clone();
            change(&mut changed);
            let err = check_inert_config(&changed, &defaults)
                .expect_err(&format!("{name} must be refused, not accepted and ignored"));
            assert!(err.contains(name), "{name} is not named in: {err}");
        }
    }

    /// **The resource-guard thresholds are refused at build, not guessed around** (A2). A typo in the
    /// enforcement word or a nonsensical floor would otherwise reach the preflight and be read as
    /// "auto" or as a zero floor — the failure this test exists to forbid. The control is the shipped
    /// default: it must pass, or the three refusals would be green over a broken comparison.
    #[test]
    fn resource_guard_thresholds_are_checked_at_build() {
        let defaults = node_conf_from_hocon(&parse_defaults("/var/lib/rnode").expect("defaults"))
            .expect("defaults parse");
        check_resource_guard(&defaults).expect("the shipped resource-guard block is not refused");

        for (name, change) in [
            (
                "resource-guard.enforce-startup",
                Box::new(|c: &mut NodeConf| {
                    c.resource_guard.enforce_startup = "sometimes".to_string();
                }) as Box<dyn Fn(&mut NodeConf)>,
            ),
            (
                "resource-guard.min-available-mb",
                Box::new(|c: &mut NodeConf| {
                    c.resource_guard.min_available_mb = -1;
                }),
            ),
            (
                "resource-guard.min-data-dir-free-mb",
                Box::new(|c: &mut NodeConf| {
                    c.resource_guard.min_data_dir_free_mb = 0;
                }),
            ),
        ] {
            let mut changed = defaults.clone();
            change(&mut changed);
            let err = check_resource_guard(&changed)
                .expect_err(&format!("{name} must be refused, not guessed around"));
            assert!(err.contains(name), "{name} is not named in: {err}");
        }
    }

    /// **The production dev-tax recipient is applied from the repository constant only outside dev
    /// mode.** Empty config → the constant on a production node, empty on a devnet; explicit wins.
    #[test]
    fn an_empty_dev_tax_address_resolves_from_the_repository_outside_dev_mode() {
        let defaults = parse_defaults("/tmp/n467-dev-tax").expect("defaults");
        let mut conf = node_conf_from_hocon(&defaults).expect("a default NodeConf");

        conf.dev_mode = false;
        apply_production_dev_tax_address(&mut conf);
        for spec in conf.casper.shards.iter() {
            assert_eq!(
                spec.genesis_block_data.dev_tax_address,
                rchain_casper::genesis::dev_tax::PRODUCTION_DEV_TAX_ADDRESS
            );
        }

        conf.dev_mode = true;
        for spec in conf.casper.shards.iter_mut() {
            spec.genesis_block_data.dev_tax_address = String::new();
        }
        apply_production_dev_tax_address(&mut conf);
        for spec in conf.casper.shards.iter() {
            assert!(
                spec.genesis_block_data.dev_tax_address.is_empty(),
                "dev mode must not inherit the production allocation"
            );
        }

        for spec in conf.casper.shards.iter_mut() {
            spec.genesis_block_data.dev_tax_address = "an explicit address".to_string();
        }
        apply_production_dev_tax_address(&mut conf);
        for spec in conf.casper.shards.iter() {
            assert_eq!(
                spec.genesis_block_data.dev_tax_address,
                "an explicit address"
            );
        }
    }
}
