//! Renderer geometry shared by flat layouts and exact canonical compositions.
//! Conversions happen only here; rendering, picking and inspection consume the
//! same coordinates. Canonical data remains authoritative for solve/validation.
use bevy::{math::DVec3, prelude::*};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{LayoutError, LayoutSolution, Point, composed::ComposedSolution};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SceneGeometry {
    pub boxes: BTreeMap<String, SceneBox>,
    pub routes: Vec<SceneRoute>,
    /// Canonical routing clearance in scene units, before fragment scaling.
    pub clearance: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SceneBox {
    pub min: DVec3,
    pub max: DVec3,
    pub solid: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SceneRoute {
    pub edge_id: String,
    pub semantic_ids: Vec<String>,
    pub points: Vec<DVec3>,
    /// Clearance is scaled with each canonical fragment. Keep tube widths and
    /// arrowheads inside the same space the canonical router validated.
    pub segment_scales: Vec<f32>,
}
pub(crate) fn point(p: Point) -> DVec3 {
    DVec3::new(p.x as f64, p.y as f64, p.z as f64) / 10.0
}
impl SceneGeometry {
    pub fn flat(solution: &LayoutSolution) -> Self {
        Self {
            // The native flat solver/validator uses two canonical units.
            clearance: 0.2,
            boxes: solution
                .boxes
                .iter()
                .map(|(id, b)| {
                    (
                        id.clone(),
                        SceneBox {
                            min: point(b.bounds.min),
                            max: point(b.bounds.max),
                            solid: b.solid,
                        },
                    )
                })
                .collect(),
            routes: solution
                .routes
                .iter()
                .map(|r| SceneRoute {
                    edge_id: r.edge_id.clone(),
                    semantic_ids: r.semantic_ids.clone(),
                    points: r.points.iter().map(|p| point(*p)).collect(),
                    segment_scales: vec![1.0; r.points.len().saturating_sub(1)],
                })
                .collect(),
        }
    }
    pub fn composed(
        solution: &ComposedSolution,
        cancelled: &AtomicBool,
    ) -> Result<Self, LayoutError> {
        let presentation = solution.present(cancelled)?;
        let convert =
            |p: visualyze_layout::assembly::ExactPoint| DVec3::from_array(p.to_f64()) / 10.0;
        let boxes = presentation
            .boxes
            .into_iter()
            .map(|(id, b)| {
                (
                    id,
                    SceneBox {
                        min: convert(b.bounds.min),
                        max: convert(b.bounds.max),
                        solid: b.solid,
                    },
                )
            })
            .collect();
        let mut routes = Vec::with_capacity(presentation.routes.len());
        for route in presentation.routes {
            let mut segment_scales = Vec::new();
            for fragment in &solution.routes[&route.edge_id].fragments {
                let transform = solution
                    .assembly
                    .transform_to(&fragment.frame_id, solution.assembly.root())
                    .map_err(|e| LayoutError::Invalid(e.to_string()))?;
                let scale = 2.0_f32.powi(-i32::from(transform.scale_shift()));
                segment_scales.extend(std::iter::repeat_n(
                    scale,
                    fragment.route.points.len().saturating_sub(1),
                ));
            }
            if segment_scales.len() != route.points.len().saturating_sub(1) {
                return Err(LayoutError::Invalid(
                    "presentation segment/scale mismatch".into(),
                ));
            }
            routes.push(SceneRoute {
                edge_id: route.edge_id,
                semantic_ids: route.semantic_ids,
                points: route.points.into_iter().map(convert).collect(),
                segment_scales,
            });
        }
        Ok(Self {
            boxes,
            routes,
            clearance: solution.clearance as f64 / 10.0,
        })
    }
    pub fn center(&self) -> Vec3 {
        self.boxes
            .values()
            .map(|b| (b.min, b.max))
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
            .map_or(Vec3::ZERO, |(min, max)| ((min + max) * 0.5).as_vec3())
    }
    /// Floating conversion follows subtraction of the stable view origin. This
    /// protects nearby small interiors from a large absolute model translation.
    pub fn display(&self, mut p: DVec3, explode: bool, origin: Vec3) -> Vec3 {
        p.y += self.inspection_offset(p.y, explode);
        (p - origin.as_dvec3()).as_vec3()
    }
    fn inspection_offset(&self, y: f64, explode: bool) -> f64 {
        if !explode {
            return 0.0;
        }
        let mut intervals: Vec<_> = self
            .boxes
            .values()
            .filter(|b| b.solid)
            .map(|b| (b.min.y, b.max.y))
            .collect();
        intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (a, b) in intervals {
            if let Some(last) = merged.last_mut()
                && a <= last.1
            {
                last.1 = last.1.max(b);
            } else {
                merged.push((a, b));
            }
        }
        merged
            .windows(2)
            .map(|p| {
                let (lo, hi) = (p[0].1, p[1].0);
                if y >= hi {
                    1.5
                } else if y > lo {
                    1.5 * (y - lo) / (hi - lo)
                } else {
                    0.0
                }
            })
            .sum()
    }
    /// Reject unrenderable precision before replacing a valid scene. This is a
    /// numeric check, not a claim about pixel visibility or easy selection.
    pub fn validate_float_geometry(&self, explode: bool, origin: Vec3) -> Result<(), LayoutError> {
        if !self.clearance.is_finite() || self.clearance <= 0.0 {
            return Err(LayoutError::Invalid(
                "invalid scene routing clearance".into(),
            ));
        }
        let mut rendered = Vec::with_capacity(self.boxes.len());
        let mut obstacle_error = 0.0_f64;
        for (id, b) in &self.boxes {
            let (lo, hi) = (
                self.display(b.min, explode, origin),
                self.display(b.max, explode, origin),
            );
            // Match the position/half-size operations used to build box meshes.
            let center = (lo + hi) * 0.5;
            let half = (hi - lo) * 0.5;
            let (drawn_lo, drawn_hi) = (center - half, center + half);
            if !lo.is_finite()
                || !hi.is_finite()
                || !drawn_lo.is_finite()
                || !drawn_hi.is_finite()
                || !(drawn_hi - drawn_lo).cmpgt(Vec3::ZERO).all()
            {
                return Err(LayoutError::Invalid(format!(
                    "{id} is too small or distant to represent in this view; enter its containing region"
                )));
            }
            let error = rendering_error(b.min, lo).max(rendering_error(b.max, hi));
            obstacle_error = obstacle_error.max(error);
            rendered.push((id, b, drawn_lo.as_dvec3(), drawn_hi.as_dvec3(), error));
        }
        // Containing frames intentionally overlap their contents. Only compare
        // pairs separated in the authoritative geometry; their gap must survive
        // rounding and the conservative mesh-transform error allowance.
        for (index, (id, a, lo_a, hi_a, error_a)) in rendered.iter().enumerate() {
            for (other, b, lo_b, hi_b, error_b) in &rendered[index + 1..] {
                let before = (b.min - a.max).max(a.min - b.max).max_element();
                if before < 0.0 {
                    continue;
                }
                let after = (*lo_b - *hi_a).max(*lo_a - *hi_b).max_element();
                if after < 0.0 || (before > 0.0 && after <= error_a + error_b) {
                    return Err(LayoutError::Invalid(format!(
                        "{id} and {other} lose their separation at this scale; enter their containing region"
                    )));
                }
            }
        }
        for route in &self.routes {
            if route.segment_scales.len() != route.points.len().saturating_sub(1)
                || route
                    .segment_scales
                    .iter()
                    .any(|scale| !scale.is_finite() || *scale <= 0.0)
            {
                return Err(LayoutError::Invalid(format!(
                    "{} has invalid canonical segment scales",
                    route.edge_id
                )));
            }
            for (index, pair) in route.points.windows(2).enumerate() {
                let a = self.display(pair[0], explode, origin);
                let b = self.display(pair[1], explode, origin);
                let delta = b - a;
                if !a.is_finite()
                    || !b.is_finite()
                    || a == b
                    || delta
                        .to_array()
                        .iter()
                        .filter(|coordinate| **coordinate != 0.0)
                        .count()
                        != 1
                {
                    return Err(LayoutError::Invalid(format!(
                        "{} has an unrepresentable route segment at this scale",
                        route.edge_id
                    )));
                }
                let scale = f64::from(route.segment_scales[index]);
                // A tube reaches 0.0325 units off its centerline. The widest
                // arrow reaches 0.12 + 0.055/2 = 0.1475 units; reserve 0.16
                // before allowing any numeric error. Fragment scale applies
                // equally to this envelope and to the router's real clearance.
                let available = (self.clearance - 0.16) * scale;
                let route_error = rendering_error(pair[0], a).max(rendering_error(pair[1], b));
                if available <= route_error + obstacle_error {
                    return Err(LayoutError::Invalid(format!(
                        "{} cannot preserve connector clearance at this scale; enter its containing region",
                        route.edge_id
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Bound rounding during exact-to-f64 conversion, origin/inspection arithmetic,
/// and the f32 center/half-size/rotation/translation operations used by meshes.
/// This deliberately favors rejecting deep mixed-scale views over claiming
/// that positive extents alone protect the much smaller connector clearance.
fn rendering_error(source: DVec3, displayed: Vec3) -> f64 {
    16.0 * (f64::EPSILON * source.abs().max_element()
        + f64::from(f32::EPSILON) * displayed.as_dvec3().abs().max_element())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positive_extents_do_not_allow_route_clearance_to_disappear_after_rounding() {
        let scale = 2.0_f64.powi(-28);
        let face = 0.1;
        let outside = face - 0.2 * scale;
        let geometry = SceneGeometry {
            clearance: 0.2,
            boxes: BTreeMap::from([(
                "obstacle".into(),
                SceneBox {
                    min: DVec3::new(face, -2.4 * scale, -2.4 * scale),
                    max: DVec3::new(face + 4.8 * scale, 2.4 * scale, 2.4 * scale),
                    solid: true,
                },
            )]),
            routes: vec![SceneRoute {
                edge_id: "parallel".into(),
                semantic_ids: vec!["claim".into()],
                points: vec![
                    DVec3::new(outside, -scale, 0.0),
                    DVec3::new(outside, scale, 0.0),
                ],
                segment_scales: vec![scale as f32],
            }],
        };
        assert!(outside < face);
        assert_eq!(outside as f32, face as f32);
        let obstacle = &geometry.boxes["obstacle"];
        assert!(
            (obstacle.max.as_vec3() - obstacle.min.as_vec3())
                .cmpgt(Vec3::ZERO)
                .all()
        );
        let error = geometry
            .validate_float_geometry(false, Vec3::ZERO)
            .unwrap_err()
            .to_string();
        assert!(error.contains("connector clearance"), "{error}");
        // Inspecting near this region removes the mixed-scale precision loss.
        geometry
            .validate_float_geometry(false, Vec3::new(face as f32, 0.0, 0.0))
            .unwrap();
    }

    #[test]
    fn formerly_separate_boxes_cannot_acquire_a_shared_face_after_rounding() {
        let geometry = SceneGeometry {
            clearance: 0.2,
            boxes: BTreeMap::from([
                (
                    "left".into(),
                    SceneBox {
                        min: DVec3::new(-1.0, 0.0, 0.0),
                        max: DVec3::new(0.1, 1.0, 1.0),
                        solid: true,
                    },
                ),
                (
                    "right".into(),
                    SceneBox {
                        min: DVec3::new(0.1 + 1e-10, 0.0, 0.0),
                        max: DVec3::new(1.0, 1.0, 1.0),
                        solid: true,
                    },
                ),
            ]),
            routes: vec![],
        };
        let error = geometry
            .validate_float_geometry(false, Vec3::ZERO)
            .unwrap_err()
            .to_string();
        assert!(error.contains("lose their separation"), "{error}");
    }

    #[test]
    fn solved_flat_and_nested_composed_geometry_keep_real_clearance_and_pass_inspection() {
        use visualyze_layout::composed::{CanonicalPins, ComposedRequest, solve_composed};
        use visualyze_layout::{
            Algorithm, Edge, LayoutRequest, Node, NodeKind, Projection, QualityProfile,
        };
        let node = |id: &str, parent: Option<&str>, solid: bool, band| Node {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: parent.map(str::to_owned),
            kind: if solid {
                NodeKind::Service
            } else {
                NodeKind::Group
            },
            solid,
            band,
            min_size: Point::new(12, 8, 12),
            preferred_size: Point::new(24, 16, 24),
            max_size: Point::new(48, 32, 48),
        };
        let projection = Projection {
            key: "geometry-check".into(),
            model_id: "geometry-model".into(),
            revision: 1,
            root: "root".into(),
            nodes: vec![
                node("root", None, false, 0),
                node("auth", Some("root"), true, 3),
                node("db", Some("root"), false, 0),
                node("table", Some("db"), false, 0),
                node("row", Some("table"), true, 0),
            ],
            edges: vec![Edge {
                id: "auth-row".into(),
                source: "auth".into(),
                target: "row".into(),
                semantic_ids: vec!["coverage".into()],
            }],
            relation_metadata: BTreeMap::new(),
            internal_relation_ids: vec![],
            semantic_to_display: BTreeMap::new(),
            suggested_groups: vec![],
            references: BTreeMap::new(),
            diagnostics: vec![],
        };
        let cancel = AtomicBool::new(false);
        let flat = visualyze_layout::solve(LayoutRequest {
            projection: &projection,
            algorithm: Algorithm::Floorplan,
            quality: QualityProfile::default(),
            pins: &BTreeMap::new(),
            previous: None,
            work_budget: 192,
            cancelled: &cancel,
        })
        .unwrap();
        let flat = SceneGeometry::flat(&flat);
        assert_eq!(flat.clearance, 0.2);
        for explode in [false, true] {
            flat.validate_float_geometry(explode, flat.center())
                .unwrap();
        }
        for clearance in [2, 3] {
            let composed = solve_composed(ComposedRequest {
                projection: &projection,
                algorithm: Algorithm::Floorplan,
                quality: QualityProfile {
                    clearance,
                    ..QualityProfile::default()
                },
                pins: &CanonicalPins::new(),
                work_budget: 192,
                cancelled: &cancel,
            })
            .unwrap();
            let geometry = SceneGeometry::composed(&composed, &cancel).unwrap();
            assert_eq!(geometry.clearance, clearance as f64 / 10.0);
            assert!(
                geometry
                    .routes
                    .iter()
                    .flat_map(|route| &route.segment_scales)
                    .any(|scale| *scale < 1.0)
            );
            for explode in [false, true] {
                geometry
                    .validate_float_geometry(explode, geometry.center())
                    .unwrap();
            }
        }
    }

    #[test]
    fn invalid_scales_and_precision_lost_during_inspection_are_rejected() {
        let mut geometry = SceneGeometry {
            clearance: 0.2,
            boxes: BTreeMap::new(),
            routes: vec![SceneRoute {
                edge_id: "path".into(),
                semantic_ids: vec!["claim".into()],
                points: vec![DVec3::ZERO, DVec3::X],
                segment_scales: vec![1.0],
            }],
        };
        geometry.validate_float_geometry(false, Vec3::ZERO).unwrap();
        for scales in [vec![], vec![0.0], vec![f32::NAN], vec![1.0, 1.0]] {
            geometry.routes[0].segment_scales = scales;
            assert!(geometry.validate_float_geometry(false, Vec3::ZERO).is_err());
        }
        geometry.routes.clear();
        geometry.boxes.insert(
            "lower".into(),
            SceneBox {
                min: DVec3::new(0.0, -2.0, 0.0),
                max: DVec3::new(1.0, -1.0, 1.0),
                solid: true,
            },
        );
        geometry.boxes.insert(
            "small".into(),
            SceneBox {
                min: DVec3::ZERO,
                max: DVec3::splat(1e-9),
                solid: true,
            },
        );
        geometry.validate_float_geometry(false, Vec3::ZERO).unwrap();
        assert!(geometry.validate_float_geometry(true, Vec3::ZERO).is_err());
    }
    #[test]
    fn origin_is_subtracted_before_small_fractional_bounds_reach_gpu_precision() {
        let geometry = SceneGeometry {
            clearance: 0.2,
            boxes: BTreeMap::from([(
                "tiny".into(),
                SceneBox {
                    min: DVec3::splat(100_000.0),
                    max: DVec3::splat(100_000.000_1),
                    solid: true,
                },
            )]),
            routes: vec![],
        };
        assert!(geometry.validate_float_geometry(false, Vec3::ZERO).is_err());
        geometry
            .validate_float_geometry(false, Vec3::splat(100_000.0))
            .unwrap();
        let b = &geometry.boxes["tiny"];
        assert!(
            (geometry.display(b.max, false, Vec3::splat(100_000.0))
                - geometry.display(b.min, false, Vec3::splat(100_000.0)))
            .cmpgt(Vec3::ZERO)
            .all()
        );
    }
    #[test]
    fn inspection_uses_the_same_map_for_object_faces_and_fractional_route_vertices() {
        let geometry = SceneGeometry {
            clearance: 0.2,
            boxes: BTreeMap::from([
                (
                    "a".into(),
                    SceneBox {
                        min: DVec3::ZERO,
                        max: DVec3::ONE,
                        solid: true,
                    },
                ),
                (
                    "b".into(),
                    SceneBox {
                        min: DVec3::new(0.0, 2.0, 0.0),
                        max: DVec3::new(1.0, 3.0, 1.0),
                        solid: true,
                    },
                ),
            ]),
            routes: vec![],
        };
        assert_eq!(
            geometry.display(DVec3::new(0.5, 1.0, 0.5), true, Vec3::ZERO),
            Vec3::new(0.5, 1.0, 0.5)
        );
        assert_eq!(
            geometry.display(DVec3::new(0.5, 2.0, 0.5), true, Vec3::ZERO),
            Vec3::new(0.5, 3.5, 0.5)
        );
        assert_eq!(
            geometry.display(DVec3::new(0.5, 1.5, 0.5), true, Vec3::ZERO),
            Vec3::new(0.5, 2.25, 0.5)
        );
        geometry.validate_float_geometry(true, Vec3::ZERO).unwrap();
    }
}
