//! CLI options → HOCON config (port of `ConfigMapper.scala`).

use std::path::PathBuf;
use std::time::Duration;

use hocon::Hocon;
use linked_hash_map::LinkedHashMap;
use rchain_comm::peer_node::PeerNode;

use super::options::{Commands, Options};

type Entries = Vec<(String, Hocon)>;

fn flag(e: &mut Entries, key: &str, v: bool) {
    if v {
        e.push((key.to_string(), Hocon::Boolean(true)));
    }
}

fn opt_str(e: &mut Entries, key: &str, v: &Option<String>) {
    if let Some(v) = v {
        e.push((key.to_string(), Hocon::String(v.clone())));
    }
}

fn opt_i32(e: &mut Entries, key: &str, v: Option<i32>) {
    if let Some(v) = v {
        e.push((key.to_string(), Hocon::Integer(v as i64)));
    }
}

fn opt_i64(e: &mut Entries, key: &str, v: Option<i64>) {
    if let Some(v) = v {
        e.push((key.to_string(), Hocon::Integer(v)));
    }
}

fn opt_f64(e: &mut Entries, key: &str, v: Option<f64>) {
    if let Some(v) = v {
        e.push((key.to_string(), Hocon::Real(v)));
    }
}

fn opt_path(e: &mut Entries, key: &str, v: &Option<PathBuf>) {
    if let Some(v) = v {
        e.push((
            key.to_string(),
            Hocon::String(v.to_string_lossy().into_owned()),
        ));
    }
}

fn opt_duration(e: &mut Entries, key: &str, v: Option<Duration>) {
    if let Some(v) = v {
        e.push((key.to_string(), Hocon::Integer(v.as_nanos() as i64)));
    }
}

fn opt_peer_node(e: &mut Entries, key: &str, v: &Option<PeerNode>) {
    if let Some(v) = v {
        e.push((key.to_string(), Hocon::String(v.to_address())));
    }
}

fn opt_str_list(e: &mut Entries, key: &str, v: &Option<Vec<String>>) {
    if let Some(v) = v {
        e.push((key.to_string(), Hocon::String(v.join(" "))));
    }
}

/// Build a nested HOCON object from dotted-key entries (port of `ConfigFactory.parseMap`).
fn nested_hash(entries: Entries) -> Hocon {
    let mut root: LinkedHashMap<String, Hocon> = LinkedHashMap::new();
    for (key, value) in entries {
        insert_dotted(&mut root, &key, value);
    }
    Hocon::Hash(root)
}

fn insert_dotted(root: &mut LinkedHashMap<String, Hocon>, key: &str, value: Hocon) {
    if let Some((head, tail)) = key.split_once('.') {
        let child = root
            .entry(head.to_string())
            .or_insert_with(|| Hocon::Hash(LinkedHashMap::new()));
        if let Hocon::Hash(map) = child {
            insert_dotted(map, tail, value);
        }
    } else {
        root.insert(key.to_string(), value);
    }
}

/// Convert CLI options into a HOCON config (port of `ConfigMapper.fromOptions`).
///
/// Only options that were explicitly supplied are written, so the fallback (file, then defaults)
/// supplies the rest.
pub fn from_options(options: &Options) -> Hocon {
    let mut e: Entries = Vec::new();

    if let Commands::Run(run) = &options.subcommand {
        flag(&mut e, "standalone", run.standalone);
        flag(&mut e, "autopropose", run.autopropose);
        flag(&mut e, "propose-on-deploy", run.propose_on_deploy);
        opt_str(&mut e, "protocol-server.network-id", &run.network_id);
        flag(&mut e, "protocol-server.dynamic-ip", run.dynamic_ip);
        flag(&mut e, "protocol-server.no-upnp", run.no_upnp);
        opt_str(&mut e, "protocol-server.host", &run.host);
        opt_i32(&mut e, "protocol-server.port", run.protocol_port);
        flag(
            &mut e,
            "protocol-server.use-random-ports",
            run.use_random_ports,
        );
        flag(
            &mut e,
            "protocol-server.disable-state-exporter",
            run.disable_state_exporter,
        );
        opt_i64(
            &mut e,
            "protocol-server.grpc-max-recv-message-size",
            run.protocol_grpc_max_recv_message_size,
        );
        opt_i64(
            &mut e,
            "protocol-server.grpc-max-recv-stream-message-size",
            run.protocol_grpc_max_recv_stream_message_size,
        );
        opt_i32(
            &mut e,
            "protocol-server.max-message-consumers",
            run.protocol_max_message_consumers,
        );

        opt_i32(&mut e, "peers-discovery.port", run.discovery_port);
        opt_duration(
            &mut e,
            "peers-discovery.lookup-interval",
            run.discovery_lookup_interval,
        );
        opt_duration(
            &mut e,
            "peers-discovery.cleanup-interval",
            run.discovery_cleanup_interval,
        );
        opt_i32(
            &mut e,
            "peers-discovery.heartbeat-batch-size",
            run.discovery_heartbeat_batch_size,
        );
        opt_duration(
            &mut e,
            "peers-discovery.init-wait-loop-interval",
            run.discovery_init_wait_loop_interval,
        );

        opt_peer_node(&mut e, "protocol-client.bootstrap", &run.bootstrap);
        opt_duration(
            &mut e,
            "protocol-client.network-timeout",
            run.network_timeout,
        );
        opt_i32(
            &mut e,
            "protocol-client.batch-max-connections",
            run.protocol_max_connections,
        );
        opt_i64(
            &mut e,
            "protocol-client.grpc-max-recv-message-size",
            run.protocol_grpc_max_recv_message_size,
        );
        opt_i32(
            &mut e,
            "protocol-client.grpc-stream-chunk-size",
            run.protocol_grpc_stream_chunk_size,
        );
        flag(&mut e, "protocol-client.disable-lfs", run.disable_lfs);

        opt_path(&mut e, "storage.data-dir", &run.data_dir);

        opt_str(&mut e, "casper.shard-name", &run.shard_name);
        opt_str(&mut e, "casper.parent-shard-id", &run.parent_shard_id);
        opt_i32(
            &mut e,
            "casper.max-number-of-parents",
            run.max_number_of_parents,
        );
        opt_f64(
            &mut e,
            "casper.synchrony-constraint-threshold",
            run.synchrony_constraint_threshold,
        );
        opt_i64(
            &mut e,
            "casper.height-constraint-threshold",
            run.height_constraint_threshold,
        );
        opt_str(
            &mut e,
            "casper.validator-public-key",
            &run.validator_public_key,
        );
        // **No `casper.validator-private-key` from here** (W-N11-05): the command-line flag that fed
        // it is gone, so this mapping would write a `None` and the key reaches the config file's own
        // key (`hocon.rs`) or the path route beside it. A secret does not travel on a command line.
        opt_path(
            &mut e,
            "casper.validator-private-key-path",
            &run.validator_private_key_path,
        );
        opt_duration(
            &mut e,
            "casper.casper-loop-interval",
            run.casper_loop_interval,
        );
        opt_duration(
            &mut e,
            "casper.requested-blocks-timeout",
            run.requested_blocks_timeout,
        );
        opt_duration(
            &mut e,
            "casper.fork-choice-stale-threshold",
            run.fork_choice_stale_threshold,
        );
        opt_duration(
            &mut e,
            "casper.fork-choice-check-if-stale-interval",
            run.fork_choice_check_if_stale_interval,
        );

        opt_str(
            &mut e,
            "casper.genesis-block-data.bonds-file",
            &run.bonds_file,
        );
        opt_str(
            &mut e,
            "casper.genesis-block-data.wallets-file",
            &run.wallets_file,
        );
        opt_i64(
            &mut e,
            "casper.genesis-block-data.bond-minimum",
            run.bond_minimum,
        );
        opt_i64(
            &mut e,
            "casper.genesis-block-data.bond-maximum",
            run.bond_maximum,
        );
        opt_i32(
            &mut e,
            "casper.genesis-block-data.epoch-length",
            run.epoch_length,
        );
        opt_i32(
            &mut e,
            "casper.genesis-block-data.quarantine-length",
            run.quarantine_length,
        );
        opt_i32(
            &mut e,
            "casper.genesis-block-data.number-of-active-validators",
            run.number_of_active_validators,
        );
        opt_i32(
            &mut e,
            "casper.genesis-block-data.executor-share",
            run.executor_share,
        );
        opt_i32(
            &mut e,
            "casper.genesis-block-data.absence-slack",
            run.absence_slack,
        );
        opt_i32(
            &mut e,
            "casper.genesis-block-data.participation-grace",
            run.participation_grace,
        );
        opt_str(
            &mut e,
            "casper.genesis-block-data.pos-vault-pub-key",
            &run.pos_vault_pub_key,
        );
        opt_str(
            &mut e,
            "casper.genesis-block-data.system-contract-pub-key",
            &run.system_contract_pub_key,
        );
        opt_i64(
            &mut e,
            "casper.genesis-block-data.genesis-block-number",
            run.genesis_block_number,
        );
        opt_str_list(
            &mut e,
            "casper.genesis-block-data.pos-multi-sig-public-keys",
            &run.pos_multi_sig_public_keys,
        );
        opt_i32(
            &mut e,
            "casper.genesis-block-data.pos-multi-sig-quorum",
            run.pos_multi_sig_quorum,
        );

        opt_i32(&mut e, "casper.autogen-shard-size", run.autogen_shard_size);
        opt_i64(&mut e, "casper.min-phlo-price", run.min_phlo_price);
        if run.equivocation_injection {
            e.push((
                "casper.equivocation-injection".to_string(),
                Hocon::Boolean(true),
            ));
        }
        opt_str(&mut e, "casper.sync-anchor", &run.sync_anchor);
        opt_str(
            &mut e,
            "casper.expected-genesis-hash",
            &run.expected_genesis_hash,
        );
        if run.self_reset {
            e.push(("casper.self-reset".to_string(), Hocon::Boolean(true)));
        }
        if let Some(n) = run.merge_divergence_injection {
            e.push((
                "casper.merge-divergence-injection".to_string(),
                Hocon::Integer(i64::from(n)),
            ));
        }
        if let Some(mode) = &run.effect_scheduler {
            e.push((
                "casper.effect-scheduler".to_string(),
                Hocon::String(mode.as_str().to_string()),
            ));
        }

        opt_i32(
            &mut e,
            "api-server.port-grpc-external",
            run.api_port_grpc_external,
        );
        opt_i32(
            &mut e,
            "api-server.port-grpc-internal",
            run.api_port_grpc_internal,
        );
        opt_i32(
            &mut e,
            "api-server.grpc-max-recv-message-size",
            run.api_grpc_max_recv_message_size,
        );
        opt_str(&mut e, "api-server.host", &run.api_host);
        opt_str(&mut e, "api-server.ocapn-listen", &run.ocapn_listen);
        opt_str(
            &mut e,
            "api-server.ocapn-listen-unix",
            &run.ocapn_listen_unix,
        );
        opt_str(
            &mut e,
            "api-server.ocapn-listen-noise",
            &run.ocapn_listen_noise,
        );
        opt_str(
            &mut e,
            "api-server.ocapn-listen-websocket",
            &run.ocapn_listen_websocket,
        );
        opt_str(
            &mut e,
            "api-server.ocapn-identity-key",
            &run.ocapn_identity_key,
        );
        opt_i32(&mut e, "api-server.port-http", run.api_port_http);
        opt_i32(
            &mut e,
            "api-server.port-admin-http",
            run.api_port_admin_http,
        );
        flag(
            &mut e,
            "api-server.enable-reporting",
            run.api_enable_reporting,
        );
        flag(
            &mut e,
            "api-server.publish-node-list",
            run.api_publish_node_list,
        );
        opt_duration(
            &mut e,
            "api-server.node-list-probe-interval",
            run.api_node_list_probe_interval,
        );
        opt_duration(
            &mut e,
            "api-server.node-list-probe-timeout",
            run.api_node_list_probe_timeout,
        );
        if let Some(v) = run.api_node_list_max_entries {
            e.push((
                "api-server.node-list-max-entries".to_string(),
                Hocon::Integer(i64::try_from(v).unwrap_or(i64::MAX)),
            ));
        }
        flag(
            &mut e,
            "api-server.enable-devnet-cors",
            run.api_enable_devnet_cors,
        );
        flag(
            &mut e,
            "api-server.enable-devnet-admin-public",
            run.api_enable_devnet_admin_public,
        );
        opt_i32(
            &mut e,
            "api-server.max-blocks-limit",
            run.api_max_blocks_limit,
        );
        opt_duration(
            &mut e,
            "api-server.keep-alive-time",
            run.api_keep_alive_time,
        );
        opt_duration(
            &mut e,
            "api-server.keep-alive-timeout",
            run.api_keep_alive_timeout,
        );
        opt_duration(
            &mut e,
            "api-server.permit-keep-alive-time",
            run.api_permit_keep_alive_time,
        );
        opt_duration(
            &mut e,
            "api-server.max-connection-idle",
            run.api_max_connection_idle,
        );
        opt_duration(
            &mut e,
            "api-server.max-connection-age",
            run.api_max_connection_age,
        );
        opt_duration(
            &mut e,
            "api-server.max-connection-age-grace",
            run.api_max_connection_age_grace,
        );

        opt_path(&mut e, "tls.key-path", &run.tls_key_path);
        opt_path(&mut e, "tls.certificate-path", &run.tls_certificate_path);
        flag(
            &mut e,
            "tls.secure-random-non-blocking",
            run.tls_secure_random_non_blocking,
        );

        flag(&mut e, "metrics.prometheus", run.prometheus);
        flag(&mut e, "metrics.influxdb", run.influxdb);
        flag(&mut e, "metrics.influxdb-udp", run.influxdb_udp);
        flag(&mut e, "metrics.zipkin", run.zipkin);
        flag(&mut e, "metrics.sigar", run.sigar);

        flag(&mut e, "dev-mode", run.dev_mode);
        flag(&mut e, "attest-on-new-blocks", run.attest_on_new_blocks);
        flag(
            &mut e,
            "no-attest-on-new-blocks",
            run.no_attest_on_new_blocks,
        );
        flag(
            &mut e,
            "resource-guard.allow-under-minimum",
            run.allow_under_minimum,
        );
        flag(
            &mut e,
            "resource-guard.accept-under-minimum-risk",
            run.accept_under_minimum_risk,
        );
        opt_str(
            &mut e,
            "dev.deployer-private-key",
            &run.deployer_private_key,
        );
    }

    nested_hash(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    /// `Hocon` is an enum, not a map: navigating it means matching the `Hash` variant.
    fn field<'a>(h: &'a Hocon, key: &str) -> Option<&'a Hocon> {
        match h {
            Hocon::Hash(map) => map.get(key),
            _ => None,
        }
    }

    fn options(args: &[&str]) -> Options {
        let mut argv = vec!["rnode"];
        argv.extend_from_slice(args);
        Options::try_parse_from(argv).expect("the CLI options parse")
    }

    /// Only options that were **explicitly supplied** are written. This is what makes the layered
    /// fallback work at all: an unset flag must leave the key absent so the config file (then
    /// `defaults.conf`) supplies it — and it is what the shard-config exclusivity check relies on,
    /// since a mapper that always wrote `casper.shard-name` would make *every* configuration look like
    /// it set both the scalar and the array form.
    #[test]
    fn an_unset_flag_writes_nothing() {
        let mapped = from_options(&options(&["run", "--standalone"]));

        assert_eq!(
            field(&mapped, "standalone"),
            Some(&Hocon::Boolean(true)),
            "a supplied flag is written"
        );
        let casper = field(&mapped, "casper").and_then(|c| field(c, "shard-name"));
        assert_eq!(
            casper, None,
            "an absent flag must not write the key at all: {mapped:?}"
        );
    }

    #[test]
    fn the_below_minimum_acceptance_reaches_the_resource_guard() {
        let mapped = from_options(&options(&["run", "--accept-under-minimum-risk"]));
        let guard = field(&mapped, "resource-guard").expect("resource-guard block");
        assert_eq!(
            field(guard, "accept-under-minimum-risk"),
            Some(&Hocon::Boolean(true)),
            "the explicit acceptance must reach the guard: {mapped:?}"
        );
    }

    #[test]
    fn pinned_genesis_cli_reaches_casper_configuration() {
        let hash = "11".repeat(32);
        let mapped = from_options(&options(&["run", "--expected-genesis-hash", &hash]));
        let casper = field(&mapped, "casper").expect("casper configuration");
        assert_eq!(
            field(casper, "expected-genesis-hash"),
            Some(&Hocon::String(hash))
        );
    }

    /// A dotted flag becomes a **nested object**, not a flat key with a dot in it: the merge and the
    /// readers navigate the tree (`get(h, "casper")`), so a flat `"casper.shard-name"` key would be
    /// invisible to them.
    #[test]
    fn a_dotted_flag_becomes_a_nested_object() {
        let mapped = from_options(&options(&[
            "run",
            "--network-id",
            "testnet",
            "--shard-name",
            "child",
        ]));

        let protocol = field(&mapped, "protocol-server").expect("nested object");
        assert_eq!(
            field(protocol, "network-id"),
            Some(&Hocon::String("testnet".to_string()))
        );

        let casper = field(&mapped, "casper").expect("nested object");
        assert_eq!(
            field(casper, "shard-name"),
            Some(&Hocon::String("child".to_string())),
            "the legacy scalar key path is preserved for the CLI"
        );
        assert!(
            field(&mapped, "casper.shard-name").is_none(),
            "and it is not also written as a flat key"
        );
    }

    /// Supplying the scalar shard flags is exactly the shape the config layer rejects when the
    /// `casper.shards` array is also present — so the mapper's output must contain them when, and
    /// only when, the operator asked.
    #[test]
    fn an_explicit_shard_flag_is_visible_to_the_exclusivity_check() {
        let mapped = from_options(&options(&["run", "--parent-shard-id", "/root"]));
        let casper = field(&mapped, "casper").expect("nested object");
        assert_eq!(
            field(casper, "parent-shard-id"),
            Some(&Hocon::String("/root".to_string()))
        );
    }
}
