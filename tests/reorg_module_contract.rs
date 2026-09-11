#![cfg(not(target_arch = "wasm32"))]

//! Contract tests for `modules::reorg::handle_reorg_switch`.
//!
//! These pin the invariant that the 2026-09-11 espo-e outage violated: a module
//! that reports an index height MUST roll that height back when the indexer
//! reorgs, and a module that does not is caught loudly rather than wedging the
//! indexer's caller.
//!
//! Deliberately no `init_global_tree_db` in this file. With no global tree,
//! `rewind_tree_to_before` is a no-op, which leaves `handle_reorg_switch`'s
//! module-facing contract as the only thing under test — and keeps every test
//! here parallel-safe. The end-to-end version, with a real versioned tree and
//! the real `explorerextensions` module, lives in
//! `tests/reorg_explorerextensions_regression.rs`.

use anyhow::Result;
use bitcoin::Network;
use espo::alkanes::trace::EspoBlock;
use espo::modules::defs::{EspoModule, RpcNsRegistrar};
use espo::modules::reorg::handle_reorg_switch;
use espo::runtime::mdb::Mdb;
use std::sync::{Arc, RwLock};

/// How the test double behaves when asked to roll back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReorgBehaviour {
    /// What the old `EspoModule::handle_reorg` default did, and what
    /// `explorerextensions` inherited: accept the call, change nothing. The
    /// cached height stays at its pre-reorg value.
    SilentNoOp,
    /// What every correct module does: re-read the height from storage. Here,
    /// "storage" is `persisted_height`, standing in for what the versioned
    /// tree exposes once it has been rewound.
    RefreshFromStorage,
    /// Refuses the rollback outright.
    Fails,
}

struct FakeModule {
    name: &'static str,
    behaviour: ReorgBehaviour,
    /// The height the module reports. Cached in memory, exactly like every
    /// real module's `index_height: RwLock<Option<u32>>`.
    cached_height: RwLock<Option<u32>>,
    /// What a re-read from storage would return after the tree rewind.
    persisted_height: Option<u32>,
    preflight_ok: bool,
}

impl FakeModule {
    fn new(name: &'static str, behaviour: ReorgBehaviour, cached: Option<u32>) -> Self {
        Self {
            name,
            behaviour,
            cached_height: RwLock::new(cached),
            persisted_height: None,
            preflight_ok: true,
        }
    }

    fn with_persisted(mut self, height: Option<u32>) -> Self {
        self.persisted_height = height;
        self
    }

    fn with_failing_preflight(mut self) -> Self {
        self.preflight_ok = false;
        self
    }
}

impl EspoModule for FakeModule {
    fn get_name(&self) -> &'static str {
        self.name
    }
    fn set_mdb(&mut self, _mdb: Arc<Mdb>) {}
    fn get_genesis_block(&self, _network: Network) -> u32 {
        0
    }
    fn index_block(&self, _block: EspoBlock) -> Result<()> {
        Ok(())
    }
    fn get_index_height(&self) -> Option<u32> {
        *self.cached_height.read().unwrap()
    }
    fn handle_reorg(&self, _next_height: u32) -> Result<()> {
        match self.behaviour {
            ReorgBehaviour::SilentNoOp => Ok(()),
            ReorgBehaviour::RefreshFromStorage => {
                *self.cached_height.write().unwrap() = self.persisted_height;
                Ok(())
            }
            ReorgBehaviour::Fails => anyhow::bail!("rollback refused"),
        }
    }
    fn preflight_reorg(&self, _next_height: u32) -> Result<()> {
        if self.preflight_ok { Ok(()) } else { anyhow::bail!("cannot roll back") }
    }
    fn register_rpc(&self, _reg: &RpcNsRegistrar) {}
}

fn modules(list: Vec<FakeModule>) -> Vec<Arc<dyn EspoModule>> {
    list.into_iter().map(|m| Arc::new(m) as Arc<dyn EspoModule>).collect()
}

/// The (B) property: a module that reports a height but does not actually roll
/// it back is rejected, by name, rather than being allowed to desync the index.
///
/// This is the shape of the espo-e wedge. Before the fix, `explorerextensions`
/// *was* this module: it inherited a `handle_reorg` that returned `Ok(())` and
/// went on reporting height 966500 after a reorg to 966500.
#[test]
fn height_reporting_module_that_does_not_roll_back_is_rejected() {
    let mods = modules(vec![
        FakeModule::new("good", ReorgBehaviour::RefreshFromStorage, Some(966500))
            .with_persisted(Some(966499)),
        FakeModule::new("forgetful", ReorgBehaviour::SilentNoOp, Some(966500)),
    ]);

    let err = handle_reorg_switch(&mods, 966500)
        .expect_err("a module still reporting the pre-reorg height must not be accepted");
    let msg = format!("{err:#}");

    assert!(msg.contains("forgetful"), "error must name the offending module: {msg}");
    assert!(msg.contains("still reports index height 966500"), "unexpected error: {msg}");
    assert!(
        msg.contains("did not re-read the index height from storage"),
        "error should tell the author what to implement: {msg}"
    );
    assert!(!msg.contains("\"good\""), "the compliant module must not be blamed: {msg}");
}

/// A module that refreshes its cached height from storage passes.
#[test]
fn module_that_refreshes_from_storage_is_accepted() {
    let mods = modules(vec![
        FakeModule::new("essentialsish", ReorgBehaviour::RefreshFromStorage, Some(966500))
            .with_persisted(Some(966499)),
        FakeModule::new("pizzafunish", ReorgBehaviour::RefreshFromStorage, Some(966500))
            .with_persisted(Some(966499)),
    ]);

    handle_reorg_switch(&mods, 966500).expect("all modules rolled back");
    for m in &mods {
        assert_eq!(m.get_index_height(), Some(966499), "{} did not roll back", m.get_name());
    }
}

/// A module that legitimately has no index height still works: the verification
/// pass skips it, so `oylapi`-style modules (whose height is derived from other
/// modules) and genuinely stateless modules are not forced to invent one.
#[test]
fn module_without_an_index_height_is_not_required_to_roll_back() {
    let mods = modules(vec![
        FakeModule::new("heightless", ReorgBehaviour::SilentNoOp, None),
        FakeModule::new("tracked", ReorgBehaviour::RefreshFromStorage, Some(10))
            .with_persisted(Some(9)),
    ]);

    handle_reorg_switch(&mods, 10).expect("a module with no height must not block a reorg");
    assert_eq!(mods[0].get_index_height(), None);
    assert_eq!(mods[1].get_index_height(), Some(9));
}

/// A module whose cached height is already below `next_height` — it never
/// indexed the orphaned blocks — is fine, rollback or not.
#[test]
fn module_already_behind_the_reorg_point_is_accepted() {
    let mods = modules(vec![FakeModule::new("laggard", ReorgBehaviour::SilentNoOp, Some(966400))]);
    handle_reorg_switch(&mods, 966500).expect("a module behind the reorg point has nothing to do");
}

/// `preflight_reorg` runs before anything is mutated, and its veto aborts the
/// switch. This is why `preflight_reorg` keeps a no-op default while
/// `handle_reorg` does not: a silent "no objection" is a safe default, a silent
/// "nothing to roll back" is not.
#[test]
fn preflight_veto_aborts_before_any_module_rolls_back() {
    let mods = modules(vec![
        FakeModule::new("vetoer", ReorgBehaviour::RefreshFromStorage, Some(10))
            .with_persisted(Some(9))
            .with_failing_preflight(),
        FakeModule::new("other", ReorgBehaviour::RefreshFromStorage, Some(10))
            .with_persisted(Some(9)),
    ]);

    let err = handle_reorg_switch(&mods, 10).expect_err("preflight veto must abort the switch");
    let msg = format!("{err:#}");
    assert!(msg.contains("vetoer"), "error must name the vetoing module: {msg}");
    assert_eq!(
        mods[1].get_index_height(),
        Some(10),
        "no module may be rolled back once preflight has vetoed"
    );
}

/// A `handle_reorg` that errors is surfaced with the module's name attached,
/// rather than being swallowed.
#[test]
fn failing_handle_reorg_is_reported_with_the_module_name() {
    let mods = modules(vec![FakeModule::new("brittle", ReorgBehaviour::Fails, Some(10))]);
    let err = handle_reorg_switch(&mods, 10).expect_err("a failing rollback must abort");
    let msg = format!("{err:#}");
    assert!(msg.contains("brittle"), "error must name the failing module: {msg}");
    assert!(msg.contains("failed to handle reorg"), "unexpected error: {msg}");
}
