//! Reorg rollback, shared by the indexer and by the tests that pin its
//! contract.
//!
//! This lives in the library (rather than in `main.rs`, where it used to) for
//! one reason: on 2026-09-11 espo-e wedged in production because a module
//! silently declined to roll back, and the only way to keep that from
//! happening again is to be able to drive this exact code from a test. See
//! `EspoModule::handle_reorg` for the full post-mortem.

use anyhow::{Context, Result};
use std::sync::Arc;

use crate::modules::defs::EspoModule;
use crate::runtime::tree_db::get_global_tree_db;

/// Rewind the shared versioned tree so the active root is the state as of
/// `next_height - 1`. Every versioned module namespace (see
/// `Mdb::should_enable_versioned_namespace`) rides on this tree, so this one
/// call is what actually un-writes the orphaned blocks' rows — individual
/// modules must not, and do not, delete their own rows.
pub fn rewind_tree_to_before(next_height: u32) -> Result<()> {
    let Some(tree) = get_global_tree_db() else {
        return Ok(());
    };

    let target_height = match next_height.checked_sub(1) {
        Some(parent_height) => match tree.indexed_height_bounds()? {
            Some((first_height, _)) if parent_height >= first_height => Some(parent_height),
            _ => None,
        },
        None => None,
    };

    tree.rewind_to_height(target_height)
        .with_context(|| format!("failed to rewind versioned tree before height {next_height}"))?;
    Ok(())
}

/// Roll every module back so indexing can resume at `next_height`.
///
/// Four phases, in order:
///   1. `preflight_reorg` on every module — a module that *cannot* roll back
///      says so here, before anything has been mutated.
///   2. `rewind_tree_to_before` — the shared versioned tree moves its active
///      root back to `next_height - 1`. This is the step that discards the
///      orphaned rows for every versioned namespace.
///   3. `handle_reorg` on every module — modules drop/refresh whatever they
///      cache in memory (index height, price caches, live-outpoint sets) so
///      that it agrees with the storage they can now see.
///   4. Verification — any module still claiming an index height at or beyond
///      `next_height` is out of sync with the tree, and indexing it further
///      would write blocks on top of state that no longer exists. We refuse.
///
/// Phase 4 is deliberately fatal and deliberately strict: the indexer stops
/// rather than corrupting the index. That is the correct behaviour, but it
/// means a module that skips phase 3 takes the whole indexer down (and with
/// it, whatever is served off that pod) until a human restarts it. That is
/// exactly what happened on 2026-09-11 — hence `handle_reorg` being a
/// *required* trait method now, so "I forgot phase 3" is a compile error and
/// never again a production outage.
pub fn handle_reorg_switch(modules: &[Arc<dyn EspoModule>], next_height: u32) -> Result<()> {
    for m in modules {
        m.preflight_reorg(next_height).with_context(|| {
            format!("module {} cannot roll back to height {next_height}", m.get_name())
        })?;
    }
    rewind_tree_to_before(next_height)?;
    for m in modules {
        m.handle_reorg(next_height).with_context(|| {
            format!("module {} failed to handle reorg to height {next_height}", m.get_name())
        })?;
    }
    for m in modules {
        let Some(height) = m.get_index_height() else {
            continue;
        };
        if height >= next_height {
            anyhow::bail!(
                "module {} still reports index height {} after reorg to next_height {}; \
                 its handle_reorg did not re-read the index height from storage \
                 (the versioned tree has already been rewound, so the module only has to \
                 refresh whatever height it caches in memory)",
                m.get_name(),
                height,
                next_height
            );
        }
    }
    Ok(())
}
