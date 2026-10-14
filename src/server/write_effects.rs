//! What a write handler does once its write is through, fed by the change
//! log when it recorded the write (delta note §2.6).
//!
//! The handler mints a [`CommitStamp`] before the write and runs the write
//! under it ([`context`]), so every row the write produces carries the
//! stamp's commit IRI. Afterwards [`Written::read`] reads exactly those rows
//! back, and the derived indexes take them instead of rescanning: the
//! full-text index gets the exact quads, entailment extends instead of
//! rebuilding when the write only added, LDES publishes the entities the
//! rows name, and the commit record points at the rows' sequence range.
//! Without rows — capture off, or a write the log could not bound — every
//! consumer falls back to what it did before.

use oxigraph::model::Quad;

use crate::auth::middleware::AuthenticatedUser;
use crate::commit_log::{CommitKind, CommitStamp};
use crate::server::AppState;
use crate::store::changes::{decode_text, ChangeRow, Extent, WriteContext};

/// The change-log context for a write run under `stamp`: the actor IRI as
/// the commit trail mints it, the commit kind, and the stamp's commit IRI.
/// Set it (`WriteContextGuard::set`) inside the blocking closure that runs
/// the primitive, never across an await.
pub(crate) fn context(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    kind: CommitKind,
    stamp: &CommitStamp,
) -> WriteContext {
    WriteContext {
        actor_iri: user.map(|u| {
            format!(
                "{}/users/{}",
                state.base_url.trim_end_matches('/'),
                u.user_id
            )
        }),
        commit_iri: Some(stamp.commit_iri.clone()),
        kind: Some(kind.as_str().to_string()),
    }
}

/// The rows a stamped write left in the change log.
#[derive(Debug, Default, Clone)]
pub(crate) struct Written {
    /// In commit order; empty when the log did not record the write.
    pub rows: Vec<ChangeRow>,
}

impl Written {
    pub(crate) fn read(state: &AppState, stamp: &CommitStamp) -> Written {
        Written {
            rows: state.store.changes().rows_for_commit(&stamp.commit_iri),
        }
    }

    /// Whether the log recorded the write in full: rows exist, none is
    /// store-scoped, and each carries its quads.
    pub(crate) fn is_exact(&self) -> bool {
        !self.rows.is_empty()
            && self
                .rows
                .iter()
                .all(|r| r.scope == "graph" && r.extent == Extent::Full)
    }

    /// The write's exact `(inserted, deleted)` quads, when [`Self::is_exact`].
    pub(crate) fn quads(&self) -> Option<(Vec<Quad>, Vec<Quad>)> {
        if !self.is_exact() {
            return None;
        }
        let mut inserted = Vec::new();
        let mut deleted = Vec::new();
        for r in &self.rows {
            inserted.extend(decode_text(r.added.as_deref()));
            deleted.extend(decode_text(r.removed.as_deref()));
        }
        Some((inserted, deleted))
    }

    /// The write only added quads (and the log knows it): entailment can
    /// extend what it derived instead of deriving it again.
    pub(crate) fn add_only(&self) -> bool {
        self.is_exact() && self.rows.iter().all(|r| r.removed_n == 0)
    }

    /// Text index maintenance for `graphs` from the rows: `true` when it was
    /// done exactly; `false` when the caller must refresh the way it did
    /// before. Blocking.
    pub(crate) fn sync_text_index(&self, state: &AppState, graphs: &[String]) -> bool {
        let Some((inserted, deleted)) = self.quads() else {
            return false;
        };
        #[cfg(feature = "text-search")]
        state.text_index_apply_delta(&inserted, &deleted, graphs);
        #[cfg(not(feature = "text-search"))]
        let _ = (state, graphs, inserted, deleted);
        true
    }

    /// Entailment for `graphs`: extends when the write only added, rebuilds
    /// otherwise. Blocking.
    pub(crate) fn entailment(&self, state: &AppState, graphs: &[String]) {
        if self.add_only() {
            crate::entailment::after_additive_write(state, graphs);
        } else {
            crate::entailment::after_write(state, graphs);
        }
    }
}
