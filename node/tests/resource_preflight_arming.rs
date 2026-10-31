//! **The start-up preflight is armed, and its override is explicit** (unit A2, safety case H-RS-01/
//! B31).
//!
//! The policy's verdicts are unit-tested against synthesised facts in `node/src/resources/policy.rs`.
//! What those tests cannot show is that the node **reads a real host and refuses a real start**: that
//! the call site is reached before the chain rebuild, that the persisted block count feeds the floor,
//! and that the operator's override is the only way past a refusal. This file pins that seam.
//!
//! The instrument is an impossible floor — 1 TiB of available memory, far above any host that runs a
//! test — set through the same `resource-guard` block an operator edits. The three arms are:
//!
//! 1. a validator with the floor and **no** override: `setup_node_program` returns the refusal naming
//!    the floor and the override, rather than starting a node that would die in the DAG rebuild;
//! 2. the same validator with `allow-under-minimum`: the node starts and the warning says the
//!    override was applied (a refusal that could not be flipped would be indistinguishable from a
//!    broken config);
//! 3. an **observer** with the floor and no override: the node starts and the warning says the role
//!    is why — observers warn, validators enforce, the approved policy.
//!
//! The three arms run in **one test**, sequentially: arm 1 builds a node environment without the
//! heavyweight lock `common::start` takes for a serving node, so running it beside arm 3 on a parallel
//! test thread would be the LMDB address-space race the lock exists to prevent.
//!
//! Falsifier: remove the `preflight::enforce` call from `setup_shard` and arms 1 and 2 both start
//! silently; force the enforcement to `auto` for an observer and arm 3's start is not warned.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{free_ports, standalone_conf, start, temp_dir, test_runtime, VALIDATOR_PRIV_HEX};
use rchain_node::runtime::node_environment;
use rchain_node::runtime::node_runtime::setup_node_program;
use rchain_shared::log::StderrLog;

/// 1 TiB, expressed in the config's MiB unit. Above any host this suite runs on, so the verdict is
/// decided by the configured floor rather than by the machine's weather.
const IMPOSSIBLE_FLOOR_MB: i64 = 1024 * 1024;

#[test]
fn the_preflight_refuses_a_validator_warns_an_observer_and_honours_the_override() {
    let runtime = test_runtime();
    runtime.block_on(async {
        // Arm 1 — a validator below its floor is refused before it serves.
        eprintln!("preflight arming: arm 1 (validator, impossible floor, no override)");
        let dir = temp_dir("preflight-refuse");
        let ports = free_ports(4);
        let mut conf = standalone_conf(&dir, &ports, Some(VALIDATOR_PRIV_HEX));
        conf.resource_guard.min_available_mb = IMPOSSIBLE_FLOOR_MB;
        let id = node_environment::create(&conf).expect("node environment");
        let setup = tokio::time::timeout(
            Duration::from_secs(300),
            setup_node_program(&conf, &id, Arc::new(StderrLog::default())),
        )
        .await
        .expect("the preflight must decide before the expensive setup, not hang");
        let err = match setup {
            Err(e) => e,
            Ok(_) => panic!(
                "a validator below its memory floor must be refused, not started: the host it runs \
                 on cannot replay the chain"
            ),
        };
        assert!(
            err.contains("below the"),
            "the refusal must name the floor: {err}"
        );
        assert!(
            err.contains("allow-under-minimum"),
            "the refusal must name the explicit override: {err}"
        );
        assert!(
            err.contains("MiB floor"),
            "the refusal must be the preflight's, not an unrelated setup error: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("preflight arming: arm 1 refused with: {err}");

        // Arm 1b — the override without the explicit risk acceptance is still refused.
        eprintln!("preflight arming: arm 1b (validator, override, no acceptance)");
        let dir = temp_dir("preflight-no-acceptance");
        let ports = free_ports(4);
        let mut conf = standalone_conf(&dir, &ports, Some(VALIDATOR_PRIV_HEX));
        conf.resource_guard.min_available_mb = IMPOSSIBLE_FLOOR_MB;
        conf.resource_guard.allow_under_minimum = true;
        let id = node_environment::create(&conf).expect("node environment");
        let setup = tokio::time::timeout(
            Duration::from_secs(300),
            setup_node_program(&conf, &id, Arc::new(StderrLog::default())),
        )
        .await
        .expect("the acceptance check must decide before setup");
        let err = match setup {
            Err(e) => e,
            Ok(_) => panic!(
                "a validator override without accept-under-minimum-risk must be refused"
            ),
        };
        assert!(err.contains("accept-under-minimum-risk"), "{err}");
        assert!(err.contains("TERMS.md"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("preflight arming: arm 1b refused with: {err}");

        // Arm 2 — the same validator, the same floor, override + acceptance: starts, and says so.
        eprintln!("preflight arming: arm 2 (validator, same floor, override)");
        let dir = temp_dir("preflight-override");
        let ports = free_ports(4);
        let mut conf = standalone_conf(&dir, &ports, Some(VALIDATOR_PRIV_HEX));
        conf.resource_guard.min_available_mb = IMPOSSIBLE_FLOOR_MB;
        conf.resource_guard.allow_under_minimum = true;
        conf.resource_guard.accept_under_minimum_risk = true;
        {
            // Its own scope: the node holds the heavyweight lock `start` takes until it is dropped,
            // so letting it live to the end of the async block would stall arm 3 on that lock.
            let node =
                tokio::time::timeout(Duration::from_secs(300), start(&conf, ports[2], ports[0]))
                    .await
                    .expect("the overridden validator must start");
            assert!(
                node.log
                    .messages()
                    .iter()
                    .any(|line| line.contains("overrides it")),
                "an explicit override must be logged, not silent: {:?}",
                node.log.messages()
            );
            assert!(
                node.log.messages().iter().any(|line| {
                    line.contains("operator risk notice") && line.contains("TERMS.md")
                }),
                "a validator must see the operator risk notice: {:?}",
                node.log.messages()
            );
            assert!(
                node.log
                    .messages()
                    .iter()
                    .any(|line| line.contains("risk accepted")),
                "the acceptance must be recorded, not silent: {:?}",
                node.log.messages()
            );
            node.request_stop();
            let served = tokio::time::timeout(Duration::from_secs(30), node.handle)
                .await
                .expect("a node asked to stop must return")
                .expect("the serve task must not panic");
            served.expect("a warned start is not a server error");
        }
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("preflight arming: arm 2 started and stopped");

        // Arm 3 — an observer with the floor and no override: starts, and the warning names the role.
        eprintln!("preflight arming: arm 3 (observer, impossible floor)");
        let dir = temp_dir("preflight-observer");
        let ports = free_ports(4);
        let mut conf = standalone_conf(&dir, &ports, None);
        conf.resource_guard.min_available_mb = IMPOSSIBLE_FLOOR_MB;
        {
            let node =
                tokio::time::timeout(Duration::from_secs(300), start(&conf, ports[2], ports[0]))
                    .await
                    .expect("the observer must start");
            assert!(
                node.log
                    .messages()
                    .iter()
                    .any(|line| line.contains("not a validator")),
                "an observer below the floor must be told the start is allowed because of its role: \
                 {:?}",
                node.log.messages()
            );
            node.request_stop();
            let served = tokio::time::timeout(Duration::from_secs(30), node.handle)
                .await
                .expect("a node asked to stop must return")
                .expect("the serve task must not panic");
            served.expect("a warned start is not a server error");
        }
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("preflight arming: arm 3 started and stopped");
    });
}
