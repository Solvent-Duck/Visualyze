//! Additive, local semantic bundles with revision-aware, bounded, demand-loaded queries.
//!
//! This module does not change the architecture snapshot contract and contains no
//! rendering state. See `public/schema/model-bundle.md` for bundle/profile semantics.
mod index;
mod storage;
mod types;

pub use storage::{BundleWriter, LocalBundle};
pub use types::*;

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Debug)]
pub enum BundleError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
    Limit(&'static str),
    StaleRevision { expected: u64, actual: u64 },
    SourceChanged,
    NotFound(String),
    InvalidCursor,
    Cancelled,
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "bundle I/O: {e}"),
            Self::Json(e) => write!(f, "bundle JSON: {e}"),
            Self::Invalid(e) => write!(f, "invalid bundle: {e}"),
            Self::Limit(name) => write!(f, "bundle limit exceeded: {name}"),
            Self::StaleRevision { expected, actual } => {
                write!(
                    f,
                    "stale model revision {expected}; current revision is {actual}"
                )
            }
            Self::SourceChanged => f.write_str("bundle source changed; reopen and reindex it"),
            Self::NotFound(id) => write!(f, "unknown semantic entity: {id}"),
            Self::InvalidCursor => {
                f.write_str("page cursor belongs to a different query or revision")
            }
            Self::Cancelled => f.write_str("bundle operation cancelled"),
        }
    }
}

impl std::error::Error for BundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for BundleError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for BundleError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_bundle_bytes: u64,
    pub max_record_bytes: usize,
    pub max_entities: usize,
    pub max_relations: usize,
    pub max_evidence: usize,
    /// Conservative index accounting, distinct from measured process RSS.
    pub max_index_bytes: usize,
    pub max_page_records: usize,
    pub max_page_bytes: usize,
    pub cache_records: usize,
    pub cache_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bundle_bytes: 16 * 1024 * 1024 * 1024,
            max_record_bytes: 64 * 1024,
            max_entities: 2_000_000,
            max_relations: 16_000_000,
            max_evidence: 2_000_000,
            max_index_bytes: 1024 * 1024 * 1024,
            max_page_records: 256,
            max_page_bytes: 1024 * 1024,
            cache_records: 256,
            cache_bytes: 4 * 1024 * 1024,
        }
    }
}

impl Limits {
    pub(super) fn validate(&self) -> Result<(), BundleError> {
        if self.max_record_bytes == 0
            || self.max_record_bytes > 64 * 1024
            || self.max_page_records == 0
            || self.max_page_records > 256
            || self.max_page_bytes < self.max_record_bytes
            || self.max_page_bytes > 16 * 1024 * 1024
            || self.max_bundle_bytes == 0
            || self.max_bundle_bytes > 16 * 1024 * 1024 * 1024
        {
            return Err(index::invalid(
                "invalid limits or limits exceed v1 format maxima",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexProgress {
    pub bytes_read: u64,
    pub total_bytes: u64,
    pub records_indexed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageCursor {
    pub(super) model_id: String,
    pub(super) revision: u64,
    pub(super) query: String,
    pub(super) offset: usize,
}

impl PageCursor {
    /// Bounded shape check for private persisted navigation state. A provider
    /// still validates exact model/revision/query identity before using it.
    pub fn valid_shape(&self) -> bool {
        !self.model_id.is_empty()
            && self.model_id.len() <= 128
            && self.revision > 0
            && self.query.len() <= 128 * 1024
            && self.offset <= 32_000_000
    }
}

#[derive(Debug, Clone)]
pub struct PageRequest {
    pub limit: usize,
    pub after: Option<PageCursor>,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: 64,
            after: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub revision: u64,
    pub items: Vec<T>,
    pub next: Option<PageCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailState {
    /// The producer provided no detail beyond the summary.
    NotSupplied,
    /// Detail exists on disk but is not in this provider's current cache.
    Unloaded,
    Loaded,
}

#[derive(Debug, Clone)]
pub struct DetailResult {
    pub revision: u64,
    pub state: DetailState,
    pub detail: Option<Arc<EntityDetail>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderStats {
    pub counts: Counts,
    pub estimated_index_bytes: usize,
    pub cached_records: usize,
    /// Serialized record bytes plus conservative allocation/bookkeeping overhead.
    pub cached_bytes: usize,
}

/// The visible semantic representatives for a bounded subtree projection.
#[derive(Debug, Clone)]
pub struct RegionRequest {
    /// None preserves the original combined ownership view.
    pub lens: Option<SemanticLens>,
    pub focus: String,
    pub visible: Vec<String>,
    /// Optional disjoint ownership subtrees for a local visual-group focus.
    pub scope_roots: Vec<String>,
    pub page: PageRequest,
    /// Maximum indexed endpoints examined, independent of returned record count.
    /// Fresh queries visit only endpoints in the requested ownership subtrees.
    /// An internal relationship can consume two endpoint visits but is emitted once.
    pub scan_budget: usize,
    /// Maximum distinct outside representatives in the current view page.
    pub reference_budget: usize,
    pub existing_references: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectedRelation {
    pub relation: RelationRecord,
    /// A supplied parent relation in this view; virtual envelope links are never
    /// returned as semantic relations.
    pub structural: bool,
    /// Nearest visible ancestor, or an external containing assembly reference.
    pub source_display: String,
    pub target_display: String,
    pub source_inside: bool,
    pub target_inside: bool,
}

#[derive(Debug, Clone)]
pub struct RegionPage {
    pub revision: u64,
    pub items: Vec<ProjectedRelation>,
    /// Exact counts for the examined portion; pages are disjoint and can be summed.
    pub internal_counts: BTreeMap<String, usize>,
    /// Bounded sample of internal membership IDs; enter that region for full records.
    pub internal_relation_ids: Vec<String>,
    pub scanned: usize,
    /// The next claim was not consumed because it needs another reference node.
    pub reference_limited: bool,
    pub next: Option<PageCursor>,
}

/// Queries are pinned to an immutable supplied revision. Absent entity IDs return
/// None for lookup; traversal of an absent parent is an explicit NotFound error.
pub trait ModelProvider {
    fn manifest(&self) -> &Manifest;
    fn summary(&mut self, revision: u64, id: &str) -> Result<Option<EntitySummary>, BundleError>;
    fn owner_id(&self, revision: u64, id: &str) -> Result<Option<String>, BundleError>;
    /// Whether an existing semantic entity belongs to this view. An unknown ID
    /// returns false. The deployment root scope is a virtual display envelope.
    fn in_lens(&self, revision: u64, lens: SemanticLens, id: &str) -> Result<bool, BundleError>;
    fn owner_id_in(
        &self,
        revision: u64,
        lens: SemanticLens,
        id: &str,
    ) -> Result<Option<String>, BundleError>;
    fn relation(&mut self, revision: u64, id: &str) -> Result<Option<RelationRecord>, BundleError>;
    fn children(
        &mut self,
        revision: u64,
        id: &str,
        page: PageRequest,
    ) -> Result<Page<EntitySummary>, BundleError>;
    fn incident(
        &mut self,
        revision: u64,
        id: &str,
        page: PageRequest,
    ) -> Result<Page<RelationRecord>, BundleError>;
    /// Direct indexed child slice; `offset` is validated against this revision.
    fn children_at(
        &mut self,
        revision: u64,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Page<EntitySummary>, BundleError>;
    fn child_count(&self, revision: u64, id: &str) -> Result<usize, BundleError>;
    /// Filtered index traversal: the offset and cursor count only children in
    /// the chosen semantic forest, never post-filtered pages of another view.
    fn children_in(
        &mut self,
        revision: u64,
        lens: SemanticLens,
        id: &str,
        page: PageRequest,
    ) -> Result<Page<EntitySummary>, BundleError>;
    fn children_at_in(
        &mut self,
        revision: u64,
        lens: SemanticLens,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Page<EntitySummary>, BundleError>;
    fn child_count_in(
        &self,
        revision: u64,
        lens: SemanticLens,
        id: &str,
    ) -> Result<usize, BundleError>;
    /// Queries original relations across hidden descendants, without loading entities.
    fn region_relations(
        &mut self,
        revision: u64,
        request: RegionRequest,
        cancelled: &AtomicBool,
    ) -> Result<RegionPage, BundleError>;
    /// Bounded indexed label/ID search. Empty pages can advance a scan cursor.
    fn search(
        &mut self,
        revision: u64,
        query: &str,
        page: PageRequest,
        cancelled: &AtomicBool,
    ) -> Result<Page<EntitySummary>, BundleError>;
    fn detail_state(&self, revision: u64, id: &str) -> Result<DetailState, BundleError>;
    fn detail(&mut self, revision: u64, id: &str) -> Result<DetailResult, BundleError>;
    fn evidence(&mut self, revision: u64, id: &str) -> Result<Option<EvidenceRecord>, BundleError>;
}
