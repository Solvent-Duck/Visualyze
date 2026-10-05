//! Tiny exhaustive reference problems for development comparisons.
//!
//! This module proves an optimum only within its stated reduced problem:
//! fixed-size, non-rotating rectangles; one horizontal band; uniform separation;
//! minimum bounding rectangle area; all labelled sequence pairs. It does not
//! establish an optimum for 3D placement, routing, or the product quality score.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{LayoutError, Point};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceRectangle {
    pub id: String,
    pub width: i64,
    pub depth: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackingReference {
    pub area: i64,
    pub width: i64,
    pub depth: i64,
    pub positions: BTreeMap<String, Point>,
    pub positive: Vec<String>,
    pub negative: Vec<String>,
    pub evaluated_pairs: usize,
}

/// Exhaust every sequence pair for at most six fixed rectangles. Enumeration
/// limits are explicit, and cancellation never returns an unproven partial best.
pub fn exact_sequence_pair(
    rectangles: &[ReferenceRectangle],
    gap: i64,
    cancelled: &AtomicBool,
) -> Result<PackingReference, LayoutError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(LayoutError::Cancelled);
    }
    if rectangles.is_empty() || rectangles.len() > 6 || !(0..=1_000).contains(&gap) {
        return Err(LayoutError::Invalid(
            "packing oracle requires 1–6 rectangles and a gap in 0–1000".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    if rectangles.iter().any(|r| {
        r.id.is_empty()
            || !ids.insert(&r.id)
            || !(1..=10_000).contains(&r.width)
            || !(1..=10_000).contains(&r.depth)
    }) {
        return Err(LayoutError::Invalid(
            "packing oracle rectangles require unique IDs and dimensions in 1–10000".into(),
        ));
    }
    let mut rectangles: Vec<_> = rectangles.iter().collect();
    rectangles.sort_by(|a, b| a.id.cmp(&b.id));
    let permutations = permutations(rectangles.len());
    let mut best: Option<PackingReference> = None;
    let mut evaluated = 0;
    for positive in &permutations {
        for negative in &permutations {
            if evaluated % 256 == 0 && cancelled.load(Ordering::Relaxed) {
                return Err(LayoutError::Cancelled);
            }
            evaluated += 1;
            let mut rank = [0_usize; 6];
            for (i, &rect) in negative.iter().enumerate() {
                rank[rect] = i;
            }
            let mut coordinates = [Point::ZERO; 6];
            // Longest paths through the two acyclic separation graphs. Every
            // pair receives exactly one horizontal or depth separation.
            for (index, &next) in positive.iter().enumerate() {
                for &before in &positive[..index] {
                    if rank[before] < rank[next] {
                        coordinates[next].x = coordinates[next]
                            .x
                            .max(coordinates[before].x + rectangles[before].width + gap);
                    } else {
                        coordinates[next].z = coordinates[next]
                            .z
                            .max(coordinates[before].z + rectangles[before].depth + gap);
                    }
                }
            }
            let width = rectangles
                .iter()
                .enumerate()
                .map(|(i, r)| coordinates[i].x + r.width)
                .max()
                .unwrap();
            let depth = rectangles
                .iter()
                .enumerate()
                .map(|(i, r)| coordinates[i].z + r.depth)
                .max()
                .unwrap();
            // Equal-area optima prefer compact proportions. Enumeration order
            // then provides a reproducible labelled optimum.
            let score = (width * depth, width.max(depth), width, depth);
            if best.as_ref().is_none_or(|old| {
                score < (old.area, old.width.max(old.depth), old.width, old.depth)
            }) {
                best = Some(PackingReference {
                    area: width * depth,
                    width,
                    depth,
                    positions: rectangles
                        .iter()
                        .enumerate()
                        .map(|(i, r)| (r.id.clone(), coordinates[i]))
                        .collect(),
                    positive: positive.iter().map(|&i| rectangles[i].id.clone()).collect(),
                    negative: negative.iter().map(|&i| rectangles[i].id.clone()).collect(),
                    evaluated_pairs: 0,
                });
            }
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(LayoutError::Cancelled);
    }
    let mut best = best.expect("a non-empty rectangle set has a sequence pair");
    best.evaluated_pairs = evaluated;
    Ok(best)
}

fn permutations(length: usize) -> Vec<Vec<usize>> {
    fn visit(prefix: &mut Vec<usize>, used: &mut [bool], result: &mut Vec<Vec<usize>>) {
        if prefix.len() == used.len() {
            result.push(prefix.clone());
            return;
        }
        for next in 0..used.len() {
            if !used[next] {
                used[next] = true;
                prefix.push(next);
                visit(prefix, used, result);
                prefix.pop();
                used[next] = false;
            }
        }
    }
    let mut result = Vec::new();
    visit(&mut Vec::new(), &mut vec![false; length], &mut result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangle(id: &str, width: i64, depth: i64) -> ReferenceRectangle {
        ReferenceRectangle {
            id: id.into(),
            width,
            depth,
        }
    }

    #[test]
    fn exhaustive_square_reference_reaches_the_area_lower_bound() {
        let rectangles: Vec<_> = (0..4).map(|i| rectangle(&i.to_string(), 2, 2)).collect();
        let result = exact_sequence_pair(&rectangles, 0, &AtomicBool::new(false)).unwrap();
        assert_eq!((result.area, result.width, result.depth), (16, 4, 4));
        assert_eq!(result.evaluated_pairs, 24 * 24);
        assert_separated(&rectangles, &result, 0);
    }

    #[test]
    fn mixed_dimensions_gap_and_input_order_preserve_a_valid_exact_reference() {
        let mut rectangles = vec![
            rectangle("a", 3, 2),
            rectangle("b", 2, 3),
            rectangle("c", 1, 1),
        ];
        let result = exact_sequence_pair(&rectangles, 1, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.evaluated_pairs, 36);
        assert_separated(&rectangles, &result, 1);
        assert!(result.area >= rectangles.iter().map(|r| r.width * r.depth).sum());
        rectangles.reverse();
        assert_eq!(
            result,
            exact_sequence_pair(&rectangles, 1, &AtomicBool::new(false)).unwrap()
        );
    }

    #[test]
    fn six_rectangles_exhaust_all_pairs() {
        let rectangles: Vec<_> = (0..6).map(|i| rectangle(&i.to_string(), 1, 1)).collect();
        let result = exact_sequence_pair(&rectangles, 0, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.area, 6);
        assert_eq!(result.evaluated_pairs, 720 * 720);
        assert_eq!((result.width, result.depth), (2, 3));
    }

    #[test]
    fn cancellation_and_unsupported_problems_never_claim_an_optimum() {
        assert!(matches!(
            exact_sequence_pair(&[rectangle("a", 1, 1)], 0, &AtomicBool::new(true)),
            Err(LayoutError::Cancelled)
        ));
        assert!(exact_sequence_pair(&[], 0, &AtomicBool::new(false)).is_err());
        assert!(
            exact_sequence_pair(
                &[rectangle("a", 1, 1), rectangle("a", 2, 2)],
                0,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        let seven: Vec<_> = (0..7).map(|i| rectangle(&i.to_string(), 1, 1)).collect();
        assert!(exact_sequence_pair(&seven, 0, &AtomicBool::new(false)).is_err());
    }

    fn assert_separated(rectangles: &[ReferenceRectangle], result: &PackingReference, gap: i64) {
        for (i, a) in rectangles.iter().enumerate() {
            let p = result.positions[&a.id];
            assert!(p.x + a.width <= result.width && p.z + a.depth <= result.depth);
            for b in &rectangles[i + 1..] {
                let q = result.positions[&b.id];
                assert!(
                    p.x + a.width + gap <= q.x
                        || q.x + b.width + gap <= p.x
                        || p.z + a.depth + gap <= q.z
                        || q.z + b.depth + gap <= p.z
                );
            }
        }
    }
}
