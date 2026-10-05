//! Named, inspectable coefficients for reproducible development comparisons.
//! These preserve profile 4/5 arithmetic; they are not human-calibrated scores.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoringWeights {
    pub route_length: f64,
    pub bends: f64,
    pub compactness: f64,
    pub aspect_ratio: f64,
    pub occlusion: f64,
    pub distortion: f64,
    pub congestion: f64,
}

pub const SCORING_WEIGHTS: ScoringWeights = ScoringWeights {
    route_length: 1.0,
    bends: 0.3,
    compactness: 2.0,
    aspect_ratio: 1.5,
    occlusion: 2.0,
    distortion: 0.25,
    congestion: 1.0,
};

/// A versioned name for the equations in `validation::measure`, not an
/// assertion that volume/occlusion are interchangeable with comprehension.
pub const NORMALIZATION: &str = "preferred-solid-volume-cuberoot-v1";
