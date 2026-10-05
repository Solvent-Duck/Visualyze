//! Complete relationships across independently placed canonical interiors.
//!
//! Canonical coordinates remain integer. Presentation scales are exact dyadics;
//! they never discount the quality cost of a long interior route.
use std::{collections::BTreeMap, sync::atomic::AtomicBool};

use crate::{
    Algorithm, Edge, LayoutError, Metrics, Point, Projection, QualityProfile,
    assembly::{Assembly, PresentedBox, PresentedRoute},
    routing::{FragmentRequest, RouteFragment},
};

mod cache;
mod constraints;
mod feedback;
pub mod persistence;
mod presentation;
mod solver;
mod validation;
pub use cache::{
    CanonicalCacheLimits, CanonicalCacheStats, CanonicalPlacementCache, ComposedReuse,
    CompositionContext, cache_accepted_composed, solve_composed_cached, solve_composed_reorganized,
};
pub use constraints::resolve_constraints;
pub use solver::solve_composed;
pub use validation::validate_composed;

pub const COMPOSITION_VERSION: u32 = 1;

/// Pins use the integer coordinate system of their explicitly named frame.
/// Existing flat-view pins cannot be silently interpreted as canonical pins.
pub type CanonicalPins = BTreeMap<String, BTreeMap<String, Point>>;

#[derive(Clone, Copy)]
pub struct ComposedRequest<'a> {
    pub projection: &'a Projection,
    pub algorithm: Algorithm,
    pub quality: QualityProfile,
    pub pins: &'a CanonicalPins,
    /// One total evaluation budget, shared by frame placement and composed feedback.
    pub work_budget: usize,
    pub cancelled: &'a AtomicBool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComposedSolution {
    pub model_id: String,
    pub revision: u64,
    pub projection_key: String,
    pub version: u32,
    pub quality_version: u32,
    pub clearance: i64,
    pub assembly: Assembly,
    /// Exactly one complete, directed route per original visible relationship.
    pub routes: BTreeMap<String, ComposedRoute>,
    pub metrics: ComposedMetrics,
    pub work: ComposedWork,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedRoute {
    pub edge: Edge,
    /// Source-to-target order, including the LCA fragment exactly once.
    pub fragments: Vec<FrameFragment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameFragment {
    pub frame_id: String,
    /// Fixed boundary terminals are geometry, never invented semantic objects.
    pub request: FragmentRequest,
    pub route: RouteFragment,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComposedMetrics {
    /// Each frame uses its fixed preferred-dimension reference scale. Padded
    /// routing domains count toward volume; every fragment is measured once.
    pub frames: BTreeMap<String, Metrics>,
    pub canonical_route_length: i64,
    pub bends: usize,
    pub structural_cost: f64,
    /// Display measurements are diagnostics, never structural-cost discounts.
    pub presented_route_length: f64,
    pub maximum_scale_shift: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposedWork {
    pub budget: usize,
    pub allocated: BTreeMap<String, usize>,
    pub evaluations: BTreeMap<String, usize>,
    /// Complete composed feedback candidates evaluated after initial placement.
    /// These consume the same budget as the sum of per-frame evaluations.
    #[serde(default)]
    pub feedback_evaluations: usize,
    /// Accepted improvements among the evaluated feedback candidates, including
    /// recovery from an initially unroutable composition to complete feasibility.
    #[serde(default)]
    pub feedback_improvements: usize,
    /// A zero-evaluation frame must identify the validated geometry it reused.
    pub reused: BTreeMap<String, PlacementReuse>,
    pub cache: CanonicalCacheStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementReuse {
    Previous,
    Cache,
}

/// One deduplicated geometry source for rendering, picking, labels and camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedPresentation {
    pub boxes: BTreeMap<String, PresentedBox>,
    pub routes: Vec<PresentedRoute>,
}

fn invalid(message: impl Into<String>) -> LayoutError {
    LayoutError::Invalid(format!("composed layout: {}", message.into()))
}

fn assembly_error(error: crate::assembly::AssemblyError) -> LayoutError {
    invalid(error.to_string())
}
