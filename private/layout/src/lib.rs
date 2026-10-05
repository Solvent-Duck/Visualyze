//! Deterministic, renderer-independent placement. Coordinates are tenths of a
//! scene unit. Semantic records remain in the public model; these are view data.
pub mod assembly;
pub mod assembly_plan;
pub mod composed;
pub mod metrics;
pub mod oracle;
pub mod persistence;
pub mod projection;
pub mod provider_projection;
pub mod quality;
pub mod routing;
pub mod session;
mod solver;
mod validation;

pub use projection::{Node, NodeKind, Projection};
use serde::{Deserialize, Serialize};
pub use solver::{Algorithm, BudgetedLayout, LayoutRequest, QualityProfile, solve, solve_budgeted};
use std::{collections::BTreeMap, fmt, sync::atomic::AtomicBool};
pub use validation::{measure, validate};

pub const LAYOUT_VERSION: u32 = 2;
/// Candidate/scoring profile identity for caches and reproducible comparisons.
/// Version 2 adds bounded refinement from measured detours/routing failures.
/// Version 3 adds volumetric same-band sequence-pair candidates. Version 4 adds
/// exact-track sharing and perpendicular crossings to final structural cost.
/// Version 5 strengthens the Layered initializer with crossing reduction and
/// rectangular depth packing; other initializers and scoring stay unchanged.
/// Serialized geometry is unchanged; new metric fields default for migration.
pub const QUALITY_VERSION: u32 = 5;
pub const UNITS_PER_SCENE_UNIT: f32 = 10.0;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}
impl Point {
    pub const ZERO: Self = Self::new(0, 0, 0);
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }
    pub fn manhattan(self, other: Self) -> i64 {
        (self.x - other.x).abs() + (self.y - other.y).abs() + (self.z - other.z).abs()
    }
    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y), self.z.min(rhs.z))
    }
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y), self.z.max(rhs.z))
    }
}
impl std::ops::Add for Point {
    type Output = Self;
    fn add(self, p: Self) -> Self {
        Self::new(self.x + p.x, self.y + p.y, self.z + p.z)
    }
}
impl std::ops::Sub for Point {
    type Output = Self;
    fn sub(self, p: Self) -> Self {
        Self::new(self.x - p.x, self.y - p.y, self.z - p.z)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    pub min: Point,
    pub max: Point,
}
impl Bounds {
    pub fn center(self) -> Point {
        // Round relative to the minimum corner so translating an odd-sized box
        // across zero preserves its integer center and exact pin position.
        Point::new(
            self.min.x + (self.max.x - self.min.x) / 2,
            self.min.y + (self.max.y - self.min.y) / 2,
            self.min.z + (self.max.z - self.min.z) / 2,
        )
    }
    pub fn size(self) -> Point {
        self.max - self.min
    }
    pub fn contains_point(self, p: Point) -> bool {
        p.x >= self.min.x
            && p.y >= self.min.y
            && p.z >= self.min.z
            && p.x <= self.max.x
            && p.y <= self.max.y
            && p.z <= self.max.z
    }
    pub fn translated(self, p: Point) -> Self {
        Self {
            min: self.min + p,
            max: self.max + p,
        }
    }
    pub fn inflated(self, d: i64) -> Self {
        let p = Point::new(d, d, d);
        Self {
            min: self.min - p,
            max: self.max + p,
        }
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
            && self.min.z < other.max.z
            && self.max.z > other.min.z
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutBox {
    pub id: String,
    pub semantic_ids: Vec<String>,
    pub parent: Option<String>,
    /// World coordinates within this projection. `relative_bounds` gives the
    /// local bounds used by a scene hierarchy, without rounding through floats.
    pub bounds: Bounds,
    pub solid: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub semantic_ids: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub edge_id: String,
    pub semantic_ids: Vec<String>,
    pub points: Vec<Point>,
    /// Ordered crossings through containing assemblies, from source to target.
    /// These are display ports; semantic membership stays on the relationship.
    #[serde(default)]
    pub scope_ports: Vec<ScopePort>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopePort {
    pub assembly_id: String,
    pub point: Point,
    pub entering: bool,
}
/// Diagnostic retention of each comparable object's nearest three surviving
/// siblings (or every sibling when fewer exist), using squared center distance
/// and stable display-ID ties. Counts disclose exclusions instead of treating
/// added/removed/reparented/changed representations as neighborhood disruption.
/// The caller supplies a predecessor in the same logical coordinate frame;
/// matching containment cannot by itself establish typed lens equivalence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeighborhoodContinuity {
    pub status: NeighborhoodComparisonStatus,
    /// Fraction of directed nearest-neighbor slots retained, in [0, 1]. None
    /// means no comparison was made, never perfect retention or zero retention.
    pub retention: Option<f64>,
    pub compared_objects: usize,
    pub compared_neighbor_slots: usize,
    pub retained_neighbor_slots: usize,
    pub added_objects: usize,
    pub removed_objects: usize,
    pub excluded_root_objects: usize,
    pub excluded_reparented_objects: usize,
    pub excluded_representation_changes: usize,
    pub excluded_frame_changes: usize,
    pub excluded_insufficient_peers: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeighborhoodComparisonStatus {
    #[default]
    NoPrevious,
    NoComparableNeighbors,
    Measured,
    IncompatibleFrame,
    LimitExceeded,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    pub route_length: i64,
    pub bends: usize,
    pub volume: f64,
    pub aspect_ratio: f64,
    pub occlusion: f64,
    /// Extra occupied length where distinct displayed routes share a track.
    #[serde(default)]
    pub shared_lane_length: i64,
    /// Distinct perpendicular conflict positions, excluding common attachments.
    #[serde(default)]
    pub crossing_points: usize,
    /// Geometric congestion normalized against the same fixed projection.
    #[serde(default)]
    pub congestion: f64,
    pub displacement: i64,
    /// Diagnostic only: this does not contribute to structural_cost or the
    /// solver's movement penalty. Old serialized metrics have no comparison.
    #[serde(default)]
    pub neighborhood: NeighborhoodContinuity,
    pub structural_cost: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutSolution {
    pub model_id: String,
    pub revision: u64,
    pub projection_key: String,
    pub version: u32,
    pub boxes: BTreeMap<String, LayoutBox>,
    pub routes: Vec<Route>,
    pub metrics: Metrics,
    pub diagnostics: Vec<String>,
}
impl LayoutSolution {
    pub fn relative_bounds(&self, id: &str) -> Option<Bounds> {
        let b = self.boxes.get(id)?;
        let origin = b
            .parent
            .as_ref()
            .and_then(|p| self.boxes.get(p))
            .map_or(Point::ZERO, |p| p.bounds.center());
        Some(b.bounds.translated(Point::ZERO - origin))
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    Invalid(String),
    Unroutable(String),
    Cancelled,
}
impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "Invalid layout: {s}"),
            Self::Unroutable(s) => write!(f, "No valid route: {s}"),
            Self::Cancelled => f.write_str("Layout cancelled"),
        }
    }
}
impl std::error::Error for LayoutError {}
pub(crate) fn check_cancel(cancelled: &AtomicBool) -> Result<(), LayoutError> {
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        Err(LayoutError::Cancelled)
    } else {
        Ok(())
    }
}
