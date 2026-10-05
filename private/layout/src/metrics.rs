//! Final route congestion from validated orthogonal geometry.
//!
//! This measures exact shared tracks and perpendicular intersections in 3D.
//! It does not estimate traffic capacity, connector thickness, near misses, or
//! projected visual crossings. Each displayed route counts once, independently
//! of how many semantic relationships it explicitly aggregates.
//! Structural scoring uses shared length divided by the projection's preferred
//! size reference length, plus crossing locations divided by its fixed display
//! edge count. It only compares candidates for the same projection: changing
//! grouping or visibility is not a permissible way to improve this score.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Point, Route};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteCongestion {
    /// Length integral of `distinct routes on a track - 1`, where positive.
    /// Three routes sharing ten units contribute twenty, not thirty.
    pub shared_lane_length: i64,
    /// Distinct positions where perpendicular routes intersect. A position
    /// used as an endpoint by both routes is an intentional common attachment.
    /// Multiple route pairs crossing at the same position count as one location.
    pub crossing_points: usize,
}

#[derive(Clone, Copy)]
struct Segment {
    route: usize,
    low: i64,
    high: i64,
    fixed: [i64; 3],
}

/// Input routes must already pass the geometry validator. Scope-port splits and
/// repeated collinear segments do not increase occupancy by the same route.
/// Sweeps are indexed by exact axis/line/plane; disjoint planes never compare.
pub fn route_congestion(routes: &[Route]) -> RouteCongestion {
    let mut axes: [Vec<Segment>; 3] = std::array::from_fn(|_| Vec::new());
    for (route, geometry) in routes.iter().enumerate() {
        for pair in geometry.points.windows(2) {
            let first = coordinates(pair[0]);
            let second = coordinates(pair[1]);
            let mut changed = (0..3).filter(|axis| first[*axis] != second[*axis]);
            let Some(axis) = changed.next() else {
                continue;
            };
            if changed.next().is_some() {
                continue;
            }
            axes[axis].push(Segment {
                route,
                low: first[axis].min(second[axis]),
                high: first[axis].max(second[axis]),
                fixed: first,
            });
        }
    }
    let shared_lane_length = shared_length(&axes);
    let endpoints: Vec<_> = routes
        .iter()
        .map(|route| {
            route
                .points
                .first()
                .copied()
                .zip(route.points.last().copied())
        })
        .collect();
    let is_endpoint = |route: usize, point: Point| {
        endpoints[route].is_some_and(|(first, last)| point == first || point == last)
    };
    let mut crossings = BTreeSet::new();
    for horizontal_axis in 0..3 {
        for vertical_axis in horizontal_axis + 1..3 {
            let plane_axis = 3 - horizontal_axis - vertical_axis;
            let mut planes = BTreeMap::<i64, (Vec<&Segment>, Vec<&Segment>)>::new();
            for segment in &axes[horizontal_axis] {
                planes
                    .entry(segment.fixed[plane_axis])
                    .or_default()
                    .0
                    .push(segment);
            }
            for segment in &axes[vertical_axis] {
                planes
                    .entry(segment.fixed[plane_axis])
                    .or_default()
                    .1
                    .push(segment);
            }
            for (plane, (horizontal, vertical)) in planes {
                if horizontal.is_empty() || vertical.is_empty() {
                    continue;
                }
                // Start < query < end makes intersection at a bend inclusive.
                // The global position set removes duplicates across bends and
                // across the three axis pairs, without multiplying route pairs.
                let mut events = Vec::with_capacity(horizontal.len() * 2 + vertical.len());
                for (i, segment) in horizontal.iter().enumerate() {
                    events.push((segment.low, 0, i));
                    events.push((segment.high, 2, i));
                }
                for (i, segment) in vertical.iter().enumerate() {
                    events.push((segment.fixed[horizontal_axis], 1, i));
                }
                events.sort_unstable();
                let mut active = BTreeMap::<i64, BTreeMap<usize, usize>>::new();
                for (position, event, index) in events {
                    if event == 1 {
                        let segment = vertical[index];
                        for (offset, horizontal_routes) in active.range(segment.low..=segment.high)
                        {
                            let mut point = [0; 3];
                            point[horizontal_axis] = position;
                            point[vertical_axis] = *offset;
                            point[plane_axis] = plane;
                            let point = Point::new(point[0], point[1], point[2]);
                            if crossings.contains(&point) {
                                continue;
                            }
                            let vertical_endpoint = is_endpoint(segment.route, point);
                            if horizontal_routes.keys().any(|route| {
                                *route != segment.route
                                    && (!vertical_endpoint || !is_endpoint(*route, point))
                            }) {
                                crossings.insert(point);
                            }
                        }
                    } else {
                        let segment = horizontal[index];
                        let offset = segment.fixed[vertical_axis];
                        if event == 0 {
                            *active
                                .entry(offset)
                                .or_default()
                                .entry(segment.route)
                                .or_default() += 1;
                        } else {
                            let routes = active.get_mut(&offset).expect("active line");
                            remove_route(routes, segment.route);
                            if routes.is_empty() {
                                active.remove(&offset);
                            }
                        }
                    }
                }
            }
        }
    }
    RouteCongestion {
        shared_lane_length,
        crossing_points: crossings.len(),
    }
}

fn shared_length(axes: &[Vec<Segment>; 3]) -> i64 {
    let mut total = 0;
    for (axis, segments) in axes.iter().enumerate() {
        let mut lines = BTreeMap::<[i64; 3], Vec<(i64, usize, bool)>>::new();
        for segment in segments {
            let mut line = segment.fixed;
            line[axis] = 0;
            let events = lines.entry(line).or_default();
            events.push((segment.low, segment.route, true));
            events.push((segment.high, segment.route, false));
        }
        for (_, mut events) in lines {
            events.sort_unstable();
            let mut active = BTreeMap::<usize, usize>::new();
            let mut previous = events[0].0;
            for (position, route, entering) in events {
                total += (position - previous) * active.len().saturating_sub(1) as i64;
                previous = position;
                if entering {
                    *active.entry(route).or_default() += 1;
                } else {
                    remove_route(&mut active, route);
                }
            }
        }
    }
    total
}

fn remove_route(active: &mut BTreeMap<usize, usize>, route: usize) {
    let count = active.get_mut(&route).expect("active route segment");
    *count -= 1;
    if *count == 0 {
        active.remove(&route);
    }
}

fn coordinates(point: Point) -> [i64; 3] {
    [point.x, point.y, point.z]
}
