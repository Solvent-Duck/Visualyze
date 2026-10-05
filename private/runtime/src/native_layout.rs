//! Accepted native layouts retain their solver's authoritative representation.
//! Display geometry is derived once and is never fed back into a solver.
use super::layout_geometry::SceneGeometry;
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
use visualyze_layout::{
    LayoutError, LayoutSolution, Metrics, Point, Projection,
    assembly::AnchorPin,
    composed::{CanonicalPins, ComposedSolution, CompositionContext, validate_composed},
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LayoutData {
    Flat(Box<LayoutSolution>),
    Composed {
        solution: Box<ComposedSolution>,
        pins: CanonicalPins,
        anchors: Vec<AnchorPin>,
        context: CompositionContext,
        /// Full durable constraints; the fields above constrain only this
        /// visible composition. Hidden interiors retain their original pins.
        retained: Option<Box<ComposedConstraints>>,
        /// Constraints created in this view. Effective inherited constraints
        /// must never become new owned records merely through navigation.
        owned: Option<Box<ComposedConstraints>>,
    },
}
/// Constraint coordinates are meaningful only within this explicit context.
/// The viewer stages a copy while solving and restores the accepted copy on
/// cancellation, failure or undo.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ComposedConstraints {
    pub context: CompositionContext,
    pub pins: CanonicalPins,
    pub anchors: Vec<AnchorPin>,
    /// Supplied object -> supplied ownership path (object first). Rechecked
    /// against the model on workers, so removal/reparenting is not collapse.
    pub ownership: BTreeMap<String, Vec<String>>,
    pub presentation: BTreeMap<String, visualyze_layout::composed::persistence::PinPresentation>,
}
impl ComposedConstraints {
    pub fn empty(context: CompositionContext) -> Self {
        Self {
            context,
            pins: CanonicalPins::new(),
            anchors: vec![],
            ownership: BTreeMap::new(),
            presentation: BTreeMap::new(),
        }
    }

    pub fn ancestry(&self, id: &str) -> Option<&Vec<String>> {
        self.presentation
            .get(id)
            .map(|pin| &pin.path)
            .or_else(|| self.ownership.get(id))
    }

    pub fn resolve(
        context: &CompositionContext,
        registry: &BTreeMap<(visualyze_model::provider::SemanticLens, String), Self>,
        cancelled: &AtomicBool,
    ) -> Result<Self, LayoutError> {
        visualyze_layout::composed::resolve_constraints(
            context,
            &registry.values().map(Self::persisted).collect::<Vec<_>>(),
            cancelled,
        )
        .map(Into::into)
    }
    pub fn persisted(&self) -> visualyze_layout::composed::persistence::PersistedConstraintSet {
        visualyze_layout::composed::persistence::PersistedConstraintSet {
            context: self.context.clone(),
            pins: self.pins.clone(),
            anchors: self.anchors.clone(),
            ownership: self.ownership.clone(),
            presentation: self.presentation.clone(),
        }
    }
    pub fn active(&self, projection: &Projection) -> Result<Self, LayoutError> {
        let plan =
            visualyze_layout::assembly_plan::plan_frames(projection, &AtomicBool::new(false))?;
        let visible = |owner: &str, id: &str| {
            plan.frames
                .get(owner)
                .is_some_and(|frame| frame.projection.nodes.iter().any(|node| node.id == id))
        };
        let hidden = |owner: &str, id: &str| {
            self.ancestry(id).is_some_and(|path| {
                path.iter().any(|ancestor| ancestor == owner)
                    // A canonical root pin belongs to its interior, not the
                    // same-ID proxy in a parent frame. Collapsing that interior
                    // makes it dormant even when the root itself is displayed.
                    && path.iter().skip(usize::from(id != owner)).any(|ancestor| {
                        projection.nodes.iter().any(|node| {
                            node.id == *ancestor && node.solid && node.id != projection.root
                        })
                    })
            })
        };
        // The worker has rechecked supplied ancestry before this classification.
        // A valid pin outside bounded detail/pages remains dormant. A displayed
        // object under a different canonical owner is still a conflict.
        let paged_out = |owner: &str, id: &str| {
            self.ancestry(id).is_some_and(|path| {
                path.iter().any(|item| item == owner)
                    && path.iter().any(|item| item == &projection.root)
                    && id != projection.root
                    && !projection.nodes.iter().any(|node| node.id == id)
                    && !projection.semantic_to_display.contains_key(id)
            })
        };
        let mut active = Self {
            context: self.context.clone(),
            pins: CanonicalPins::new(),
            anchors: vec![],
            ownership: self.ownership.clone(),
            presentation: self.presentation.clone(),
        };
        for (owner, pins) in &self.pins {
            for (id, point) in pins {
                if visible(owner, id) {
                    active
                        .pins
                        .entry(owner.clone())
                        .or_default()
                        .insert(id.clone(), *point);
                } else if !hidden(owner, id) && !paged_out(owner, id) {
                    return Err(LayoutError::Invalid(format!(
                        "Pinned component {id} is absent from its canonical owner; constraints retained"
                    )));
                }
            }
        }
        for anchor in &self.anchors {
            if visible(anchor.owner(), anchor.object_id()) {
                active.anchors.push(anchor.clone());
            } else if !hidden(anchor.owner(), anchor.object_id())
                && !paged_out(anchor.owner(), anchor.object_id())
            {
                return Err(LayoutError::Invalid(format!(
                    "Pinned component {} is not hidden by collapsed or paged detail; constraints retained",
                    anchor.object_id()
                )));
            }
        }
        Ok(active)
    }
}
impl From<visualyze_layout::composed::persistence::PersistedConstraintSet> for ComposedConstraints {
    fn from(set: visualyze_layout::composed::persistence::PersistedConstraintSet) -> Self {
        Self {
            context: set.context,
            pins: set.pins,
            anchors: set.anchors,
            ownership: set.ownership,
            presentation: set.presentation,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NativeLayout {
    pub model_id: String,
    pub revision: u64,
    pub projection_key: String,
    pub version: u32,
    pub metrics: Metrics,
    pub geometry: SceneGeometry,
    pub data: LayoutData,
}
impl From<LayoutSolution> for NativeLayout {
    fn from(solution: LayoutSolution) -> Self {
        Self {
            model_id: solution.model_id.clone(),
            revision: solution.revision,
            projection_key: solution.projection_key.clone(),
            version: solution.version,
            metrics: solution.metrics.clone(),
            geometry: SceneGeometry::flat(&solution),
            data: LayoutData::Flat(Box::new(solution)),
        }
    }
}
impl NativeLayout {
    /// A lower canonical cost cannot buy an apparent win by shrinking a
    /// retained interior to a smaller presentation scale.
    pub fn preserves_interior_scale(&self, old: &Self) -> bool {
        match (self.composed_solution(), old.composed_solution()) {
            (Some(new), Some(old)) => old.assembly.frame_ids().all(|id| {
                match (
                    new.assembly.transform_to(id, new.assembly.root()),
                    old.assembly.transform_to(id, old.assembly.root()),
                ) {
                    (Ok(new), Ok(old)) => new.scale_shift() <= old.scale_shift(),
                    _ => false,
                }
            }),
            (None, None) => true,
            _ => false,
        }
    }
    pub fn composed_constraints(&self) -> Option<ComposedConstraints> {
        match &self.data {
            LayoutData::Composed {
                context,
                pins,
                anchors,
                retained,
                ..
            } => Some(
                retained
                    .as_deref()
                    .cloned()
                    .unwrap_or_else(|| ComposedConstraints {
                        context: context.clone(),
                        pins: pins.clone(),
                        anchors: anchors.clone(),
                        ownership: BTreeMap::new(),
                        presentation: BTreeMap::new(),
                    }),
            ),
            LayoutData::Flat(_) => None,
        }
    }

    pub fn owned_constraints(&self) -> Option<ComposedConstraints> {
        match &self.data {
            LayoutData::Composed {
                owned: Some(owned), ..
            } => Some(owned.as_ref().clone()),
            _ => self.composed_constraints(),
        }
    }

    pub fn retain_owned_constraints(
        mut self,
        owned: ComposedConstraints,
    ) -> Result<Self, LayoutError> {
        let effective = self.composed_constraints().ok_or_else(|| {
            LayoutError::Invalid("owned canonical constraints require a composed layout".into())
        })?;
        if owned.pins.iter().any(|(owner, pins)| {
            pins.iter().any(|(id, point)| {
                effective.pins.get(owner).and_then(|pins| pins.get(id)) != Some(point)
            })
        }) || owned
            .anchors
            .iter()
            .any(|pin| !effective.anchors.contains(pin))
        {
            return Err(LayoutError::Invalid(
                "owned constraints are absent from the effective solve".into(),
            ));
        }
        if let LayoutData::Composed {
            context,
            owned: retained,
            ..
        } = &mut self.data
        {
            if *context != owned.context {
                return Err(LayoutError::Invalid(
                    "owned constraints belong to a different view".into(),
                ));
            }
            *retained = Some(Box::new(owned));
        }
        Ok(self)
    }

    pub fn validate_constraints(
        &self,
        projection: &Projection,
        flat_pins: &BTreeMap<String, Point>,
        constraints: Option<&ComposedConstraints>,
    ) -> Result<(), LayoutError> {
        self.validate(projection, flat_pins)?;
        if let Some(actual) = self.composed_constraints() {
            let matches = constraints
                .filter(|expected| !expected.pins.is_empty() || !expected.anchors.is_empty())
                .map_or(
                    actual.pins.is_empty() && actual.anchors.is_empty(),
                    |expected| {
                        expected.context == actual.context
                            && expected.pins == actual.pins
                            && expected.anchors == actual.anchors
                            && expected
                                .ownership
                                .iter()
                                .all(|(id, path)| actual.ownership.get(id) == Some(path))
                            && expected
                                .presentation
                                .iter()
                                .all(|(id, pin)| actual.presentation.get(id) == Some(pin))
                    },
                );
            if !matches {
                return Err(LayoutError::Invalid(
                    "layout does not match the current canonical pins and anchor context".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn composed(
        solution: ComposedSolution,
        pins: CanonicalPins,
        anchors: Vec<AnchorPin>,
        context: CompositionContext,
        cancel: &AtomicBool,
    ) -> Result<Self, LayoutError> {
        if context.model_id != solution.model_id || context.root != solution.assembly.root() {
            return Err(LayoutError::Invalid(
                "canonical composition differs from its typed view context".into(),
            ));
        }
        let geometry = SceneGeometry::composed(&solution, cancel)?;
        geometry.validate_float_geometry(false, geometry.center())?;
        let metrics = Metrics {
            route_length: solution.metrics.canonical_route_length,
            bends: solution.metrics.bends,
            structural_cost: solution.metrics.structural_cost,
            // UI occupied space is the outer assembly's envelope. Canonical
            // interior costs remain in structural_cost and cannot be reduced
            // by presentation scaling.
            volume: solution
                .metrics
                .frames
                .get(solution.assembly.root())
                .map_or(0.0, |m| m.volume),
            aspect_ratio: solution
                .metrics
                .frames
                .values()
                .map(|m| m.aspect_ratio)
                .fold(1.0, f64::max),
            occlusion: solution
                .metrics
                .frames
                .values()
                .map(|m| m.occlusion)
                .fold(0.0, f64::max),
            shared_lane_length: solution
                .metrics
                .frames
                .values()
                .map(|m| m.shared_lane_length)
                .sum(),
            crossing_points: solution
                .metrics
                .frames
                .values()
                .map(|m| m.crossing_points)
                .sum(),
            congestion: solution.metrics.frames.values().map(|m| m.congestion).sum(),
            ..Default::default()
        };
        Ok(Self {
            model_id: solution.model_id.clone(),
            revision: solution.revision,
            projection_key: solution.projection_key.clone(),
            version: solution.version,
            metrics,
            geometry,
            data: LayoutData::Composed {
                solution: Box::new(solution),
                pins,
                anchors,
                context,
                retained: None,
                owned: None,
            },
        })
    }
    pub fn retain_constraints(
        mut self,
        projection: &Projection,
        constraints: ComposedConstraints,
    ) -> Result<Self, LayoutError> {
        let active = constraints.active(projection)?;
        if let LayoutData::Composed {
            pins,
            anchors,
            context,
            retained,
            ..
        } = &mut self.data
        {
            if *context != constraints.context || *pins != active.pins || *anchors != active.anchors
            {
                return Err(LayoutError::Invalid("active/dormant canonical constraint partition differs from the validated solve".into()));
            }
            *retained = Some(Box::new(constraints));
        }
        Ok(self)
    }
    pub fn flat(&self) -> Option<&LayoutSolution> {
        match &self.data {
            LayoutData::Flat(s) => Some(s),
            _ => None,
        }
    }
    pub fn composed_solution(&self) -> Option<&ComposedSolution> {
        match &self.data {
            LayoutData::Composed { solution, .. } => Some(solution),
            _ => None,
        }
    }
    pub fn validate(
        &self,
        projection: &Projection,
        pins: &BTreeMap<String, Point>,
    ) -> Result<(), LayoutError> {
        match &self.data {
            LayoutData::Flat(s) => visualyze_layout::validate(projection, s, pins, 2),
            LayoutData::Composed {
                solution,
                pins,
                anchors,
                retained,
                ..
            } => {
                if let Some(retained) = retained {
                    let active = retained.active(projection)?;
                    if active.pins != *pins || active.anchors != *anchors {
                        return Err(LayoutError::Invalid(
                            "invalid active/dormant canonical constraint partition".into(),
                        ));
                    }
                }
                validate_composed(projection, solution, pins, &AtomicBool::new(false))?;
                for pin in anchors {
                    pin.validate(&solution.assembly)
                        .map_err(|e| LayoutError::Invalid(e.to_string()))?;
                }
                Ok(())
            }
        }
    }
    pub fn refresh_metrics(&mut self, projection: &Projection) {
        if let LayoutData::Flat(s) = &mut self.data {
            s.metrics = visualyze_layout::measure(projection, s, None);
            self.metrics = s.metrics.clone();
        }
    }
}
