//! Host resource facts, a sampled snapshot and the watchdog that keeps it fresh.
//!
//! **A1 of the resource-safety programme** (docs/src/spec/resource-safety-case.md, HAZOP U17): the node
//! did not read the host at all, so a host below the replay floor looked like any other until the kernel
//! killed it. This module reads only what the process can own or observe — `/proc/meminfo`, the cgroup
//! files the process is already inside, `statvfs` on its data directory, and a best-effort `/proc` scan
//! for co-located nodes. It never sets a limit: cgroup enforcement is the operator's
//! (docs/src/node/resource-ringfence.md).
//!
//! The watchdog runs on a dedicated `std::thread`, not the tokio runtime, because the signal it carries
//! must not be starved by the executor it is meant to protect. A stale sample is reported as
//! `unavailable`, never as `normal` (unit C turns that into admission behaviour).

pub mod admission;
pub mod budget;
pub mod cgroup;
pub mod colocation;
pub mod disk;
pub mod fallback;
pub mod meminfo;
pub mod policy;
pub mod preflight;
pub mod state;

pub use admission::AdmissionPolicy;
pub use budget::CapacityBudget;
pub use cgroup::CgroupFacts;
pub use fallback::{Fallback, ResourceState};
pub use meminfo::MemInfo;
pub use policy::{PolicyFloor, ResourcePolicy, StartupEnforcement, StartupFacts, StartupVerdict};
pub use state::{ResourceGuard, ResourceSnapshot};
