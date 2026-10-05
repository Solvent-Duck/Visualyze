//! Exact presentation of independently solved, bounded canonical interiors.
//!
//! This module does not solve, reroute, or establish continuity between routes
//! belonging to different frames. Call the ordinary geometry validator before
//! constructing a canonical frame; the checks here additionally establish safe
//! numeric shape, unique ownership, semantic membership, and presentation fit.
//! Existing `LayoutSolution` coordinates and serialization are unchanged.
//!
//! A local point maps to its parent as `translation + (point - origin) / 2^k`.
//! Uniform positive scales preserve axes and proportions. Every operation stays
//! exact until an explicit `to_f64` call. Rendering may still lose precision or
//! make a deeply scaled object too small to pick; exact geometry does not promise
//! float precision or pixel visibility. Boundary joins require integral terminal
//! coordinates in each participating canonical frame. `fit` guarantees only
//! containment; `fit_envelope` pads the canonical domain so every integer parent
//! boundary terminal has an exact integer inverse on the child boundary.

use crate::{Bounds, LAYOUT_VERSION, LayoutSolution, Point};
use std::{collections::BTreeMap, collections::BTreeSet, error::Error, fmt};

pub const MAX_SHIFT: u8 = 30;
pub const MAX_DEPTH: usize = 32;
pub const MAX_FRAMES: usize = 256;
pub const MAX_COORDINATE: i64 = 1 << 40;
pub const MAX_TOTAL_BOXES: usize = 16_384;
pub const MAX_TOTAL_ROUTE_POINTS: usize = 262_144;
pub const MAX_TOTAL_SEMANTIC_MEMBERS: usize = 262_144;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssemblyError {
    Invalid(String),
    Limit(&'static str),
    Overflow,
    NonIntegral,
    NotBoundary,
    UnknownFrame(String),
    PinConflict(String),
}
impl fmt::Display for AssemblyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "Invalid canonical assembly: {message}"),
            Self::Limit(name) => write!(f, "Canonical assembly limit exceeded: {name}"),
            Self::Overflow => f.write_str("Canonical assembly numeric range exceeded"),
            Self::NonIntegral => f.write_str("Boundary terminal is not an exact integer"),
            Self::NotBoundary => f.write_str("Terminal is not on the canonical frame boundary"),
            Self::UnknownFrame(id) => write!(f, "Unknown canonical frame: {id}"),
            Self::PinConflict(id) => {
                write!(f, "Anchor-frame pin conflicts with presentation: {id}")
            }
        }
    }
}
impl Error for AssemblyError {}
type Result<T> = std::result::Result<T, AssemblyError>;

/// Three exact dyadics sharing a normalized denominator. Absolute coordinates
/// are bounded by `MAX_COORDINATE`; denominators are at most `2^MAX_SHIFT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactPoint {
    numerators: [i128; 3],
    shift: u8,
}
impl ExactPoint {
    pub fn new(numerators: [i128; 3], shift: u8) -> Result<Self> {
        Self::normalized(numerators, shift)
    }
    fn normalized(mut numerators: [i128; 3], mut shift: u8) -> Result<Self> {
        // Public inputs cannot request an unchecked integer shift. Internal
        // arithmetic needs at most twice the supported denominator exponent.
        if shift > MAX_SHIFT * 2 {
            return Err(AssemblyError::Limit("dyadic denominator"));
        }
        while shift > 0 && numerators.iter().all(|n| n % 2 == 0) {
            for n in &mut numerators {
                *n /= 2;
            }
            shift -= 1;
        }
        if shift > MAX_SHIFT {
            return Err(AssemblyError::Limit("dyadic denominator"));
        }
        let limit = i128::from(MAX_COORDINATE) << shift;
        if numerators.iter().any(|n| *n < -limit || *n > limit) {
            return Err(AssemblyError::Overflow);
        }
        Ok(Self { numerators, shift })
    }
    pub fn integer(point: Point) -> Result<Self> {
        Self::new(coords(point).map(i128::from), 0)
    }
    pub fn numerators(self) -> [i128; 3] {
        self.numerators
    }
    pub fn denominator_shift(self) -> u8 {
        self.shift
    }
    pub fn to_integer(self) -> Result<Point> {
        if self.shift != 0 {
            return Err(AssemblyError::NonIntegral);
        }
        Ok(point(self.numerators.map(|n| n as i64)))
    }
    /// Explicit lossy presentation conversion. Never use this for validation,
    /// terminal joins, transform composition, or pin equality.
    pub fn to_f64(self) -> [f64; 3] {
        let denominator = (1u64 << self.shift) as f64;
        self.numerators.map(|n| n as f64 / denominator)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactBounds {
    pub min: ExactPoint,
    pub max: ExactPoint,
}
impl ExactBounds {
    pub fn integer(bounds: Bounds) -> Result<Self> {
        checked_bounds(bounds)?;
        Ok(Self {
            min: ExactPoint::integer(bounds.min)?,
            max: ExactPoint::integer(bounds.max)?,
        })
    }
    pub fn contains(self, other: Self) -> bool {
        le(self.min, other.min) && le(other.max, self.max)
    }
    pub fn contains_point(self, other: ExactPoint) -> bool {
        le(self.min, other) && le(other, self.max)
    }
}

/// Checked, axis-preserving map from a canonical frame into another frame.
/// The origin is retained explicitly, so pin locks also reject a compensated
/// origin rewrite that happens to produce the same affine map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DyadicTransform {
    origin: Point,
    translation: ExactPoint,
    shift: u8,
}
/// A route-ready fit. Enlarge the canonical root/domain to `canonical_bounds`
/// before local routing; its boundary then maps exactly onto the parent host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvelopeFit {
    pub transform: DyadicTransform,
    pub canonical_bounds: Bounds,
}
impl DyadicTransform {
    pub fn new(origin: Point, translation: Point, shift: u8) -> Result<Self> {
        Self::from_exact(origin, ExactPoint::integer(translation)?, shift)
    }
    pub fn from_exact(origin: Point, translation: ExactPoint, shift: u8) -> Result<Self> {
        checked_point(origin)?;
        if shift > MAX_SHIFT {
            return Err(AssemblyError::Limit("transform scale"));
        }
        Ok(Self {
            origin,
            translation,
            shift,
        })
    }
    pub fn identity() -> Self {
        Self {
            origin: Point::ZERO,
            translation: ExactPoint {
                numerators: [0; 3],
                shift: 0,
            },
            shift: 0,
        }
    }
    pub fn origin(self) -> Point {
        self.origin
    }
    pub fn translation(self) -> ExactPoint {
        self.translation
    }
    pub fn scale_shift(self) -> u8 {
        self.shift
    }

    /// The smallest nonnegative shrink exponent fitting all three dimensions.
    /// Aligns minimum corners; any spare envelope space remains explicit padding.
    pub fn fit(local: Bounds, envelope: Bounds) -> Result<Self> {
        checked_bounds(local)?;
        checked_bounds(envelope)?;
        let local_size = sizes(local);
        let envelope_size = sizes(envelope);
        let shift = (0..=MAX_SHIFT)
            .find(|shift| (0..3).all(|axis| local_size[axis] <= envelope_size[axis] << shift))
            .ok_or(AssemblyError::Limit("fit scale"))?;
        Self::new(local.min, envelope.min, shift)
    }
    pub fn fit_envelope(local: Bounds, envelope: Bounds) -> Result<EnvelopeFit> {
        let transform = Self::fit(local, envelope)?;
        let envelope_size = sizes(envelope);
        let min = coords(local.min);
        let mut max = [0i64; 3];
        for axis in 0..3 {
            let end = i128::from(min[axis]) + (envelope_size[axis] << transform.shift);
            if end < -i128::from(MAX_COORDINATE) || end > i128::from(MAX_COORDINATE) {
                return Err(AssemblyError::Overflow);
            }
            max[axis] = end as i64;
        }
        Ok(EnvelopeFit {
            transform,
            canonical_bounds: Bounds {
                min: local.min,
                max: point(max),
            },
        })
    }
    pub fn map_point(self, local: Point) -> Result<ExactPoint> {
        self.map_exact(ExactPoint::integer(local)?)
    }
    pub fn map_exact(self, local: ExactPoint) -> Result<ExactPoint> {
        let scaled_shift = local.shift + self.shift;
        let denominator = scaled_shift.max(self.translation.shift);
        let origin = coords(self.origin).map(i128::from);
        let mut mapped = [0; 3];
        for axis in 0..3 {
            let difference = local.numerators[axis]
                .checked_sub(origin[axis] << local.shift)
                .ok_or(AssemblyError::Overflow)?;
            let scaled = difference
                .checked_mul(1i128 << (denominator - scaled_shift))
                .ok_or(AssemblyError::Overflow)?;
            let translation = self.translation.numerators[axis]
                .checked_mul(1i128 << (denominator - self.translation.shift))
                .ok_or(AssemblyError::Overflow)?;
            mapped[axis] = scaled
                .checked_add(translation)
                .ok_or(AssemblyError::Overflow)?;
        }
        ExactPoint::normalized(mapped, denominator)
    }
    pub fn map_bounds(self, bounds: Bounds) -> Result<ExactBounds> {
        checked_bounds(bounds)?;
        Ok(ExactBounds {
            min: self.map_point(bounds.min)?,
            max: self.map_point(bounds.max)?,
        })
    }
    /// `outer.compose(inner)` maps inner-local coordinates directly to outer's
    /// parent. The total shrink exponent, not just each edge, is bounded.
    pub fn compose(self, inner: Self) -> Result<Self> {
        // Preserve the existing origin for a right identity. Rewriting it to
        // zero can require an out-of-range translation even when this map's
        // entire canonical domain and its image are within the numeric limits.
        if inner == Self::identity() {
            return Ok(self);
        }
        let shift = self
            .shift
            .checked_add(inner.shift)
            .ok_or(AssemblyError::Overflow)?;
        if shift > MAX_SHIFT {
            return Err(AssemblyError::Limit("composed scale"));
        }
        Self::from_exact(inner.origin, self.map_exact(inner.translation)?, shift)
    }
    pub fn inverse_integer(self, parent: ExactPoint) -> Result<Point> {
        let denominator = parent.shift.max(self.translation.shift);
        let origin = coords(self.origin).map(i128::from);
        let mut local = [0; 3];
        for axis in 0..3 {
            let a = parent.numerators[axis] << (denominator - parent.shift);
            let b = self.translation.numerators[axis] << (denominator - self.translation.shift);
            local[axis] = a
                .checked_sub(b)
                .and_then(|n| n.checked_mul(1i128 << self.shift))
                .and_then(|n| n.checked_add(origin[axis] << denominator))
                .ok_or(AssemblyError::Overflow)?;
        }
        ExactPoint::normalized(local, denominator)?.to_integer()
    }
    pub fn inverse_boundary_terminal(self, parent: Point, local_bounds: Bounds) -> Result<Point> {
        checked_bounds(local_bounds)?;
        let local = self.inverse_integer(ExactPoint::integer(parent)?)?;
        if !on_boundary(local_bounds, local) {
            return Err(AssemblyError::NotBoundary);
        }
        Ok(local)
    }
    pub fn map_boundary_terminal(self, local: Point, local_bounds: Bounds) -> Result<Point> {
        checked_bounds(local_bounds)?;
        checked_point(local)?;
        if !on_boundary(local_bounds, local) {
            return Err(AssemblyError::NotBoundary);
        }
        self.map_point(local)?.to_integer()
    }
}

/// One already solved interior; its named root box is the frame's stable ID.
/// Read-only access prevents changes after the assembly's shape validation.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalFrame {
    id: String,
    bounds: Bounds,
    solution: LayoutSolution,
}
impl CanonicalFrame {
    pub fn new(solution: LayoutSolution) -> Result<Self> {
        let roots: Vec<_> = solution
            .boxes
            .values()
            .filter(|object| object.parent.is_none())
            .collect();
        if roots.len() != 1 {
            return Err(invalid("use with_root for a canonical root forest"));
        }
        let id = roots[0].id.clone();
        let bounds = roots[0].bounds;
        Self::with_root(id, bounds, solution)
    }
    /// Explicit view-coordinate domain for a solid focus, reference forest or
    /// protruding interfaces. `bounds` is not a semantic object; object IDs and
    /// parents stay untouched. Attached child interiors additionally require a
    /// single open root whose bounds equal this domain.
    pub fn with_root(
        id: impl Into<String>,
        bounds: Bounds,
        solution: LayoutSolution,
    ) -> Result<Self> {
        let id = id.into();
        validate_frame(&id, bounds, &solution)?;
        Ok(Self {
            id,
            bounds,
            solution,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn solution(&self) -> &LayoutSolution {
        &self.solution
    }
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }
}

/// Exactly one displayed instance per frame. For a child, its parent frame must
/// contain a solid, childless routing host with the same ID and semantic members.
/// That box is the authoritative bounded envelope; no second envelope is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyInstance {
    pub frame_id: String,
    pub parent: Option<String>,
    pub transform: DyadicTransform,
}
impl AssemblyInstance {
    pub fn root(frame_id: impl Into<String>) -> Self {
        Self {
            frame_id: frame_id.into(),
            parent: None,
            transform: DyadicTransform::identity(),
        }
    }
    pub fn child(
        frame_id: impl Into<String>,
        parent: impl Into<String>,
        transform: DyadicTransform,
    ) -> Self {
        Self {
            frame_id: frame_id.into(),
            parent: Some(parent.into()),
            transform,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assembly {
    root: String,
    frames: BTreeMap<String, CanonicalFrame>,
    instances: BTreeMap<String, AssemblyInstance>,
}
impl Assembly {
    pub fn new(frames: Vec<CanonicalFrame>, instances: Vec<AssemblyInstance>) -> Result<Self> {
        if frames.is_empty() || frames.len() > MAX_FRAMES || frames.len() != instances.len() {
            return Err(AssemblyError::Limit("frame/instance count"));
        }
        let mut indexed = BTreeMap::new();
        let mut boxes = 0usize;
        let mut points = 0usize;
        let mut members = 0usize;
        let identity = (
            frames[0].solution.model_id.clone(),
            frames[0].solution.revision,
        );
        for frame in frames {
            if (frame.solution.model_id.as_str(), frame.solution.revision)
                != (identity.0.as_str(), identity.1)
            {
                return Err(invalid("frame model/revision mismatch"));
            }
            boxes += frame.solution.boxes.len();
            points += frame
                .solution
                .routes
                .iter()
                .map(|route| route.points.len() + route.scope_ports.len())
                .sum::<usize>();
            members += member_count(&frame.solution);
            if boxes > MAX_TOTAL_BOXES || points > MAX_TOTAL_ROUTE_POINTS {
                return Err(AssemblyError::Limit("resident canonical geometry"));
            }
            if members > MAX_TOTAL_SEMANTIC_MEMBERS {
                return Err(AssemblyError::Limit("resident semantic membership"));
            }
            if indexed.insert(frame.id.clone(), frame).is_some() {
                return Err(invalid("duplicate canonical frame"));
            }
        }
        let mut by_id = BTreeMap::new();
        for instance in instances {
            if !indexed.contains_key(&instance.frame_id) {
                return Err(AssemblyError::UnknownFrame(instance.frame_id));
            }
            if by_id.insert(instance.frame_id.clone(), instance).is_some() {
                return Err(invalid("duplicate displayed ownership"));
            }
        }
        let roots: Vec<_> = by_id
            .values()
            .filter(|instance| instance.parent.is_none())
            .collect();
        if roots.len() != 1 {
            return Err(invalid("one displayed root is required"));
        }
        if roots[0].transform != DyadicTransform::identity() {
            return Err(invalid("root transform must be identity"));
        }
        let assembly = Self {
            root: roots[0].frame_id.clone(),
            frames: indexed,
            instances: by_id,
        };
        let mut displayed: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for frame in assembly.frames.values() {
            for id in frame.solution.boxes.keys() {
                displayed.entry(id).or_default().push(&frame.id);
            }
        }
        for (id, owners) in displayed {
            if owners.len() == 1 {
                continue;
            }
            if owners.len() != 2
                || !owners.contains(&id)
                || assembly
                    .instances
                    .get(id)
                    .and_then(|instance| instance.parent.as_deref())
                    .is_none_or(|parent| !owners.contains(&parent))
            {
                return Err(invalid("duplicate non-host displayed object"));
            }
        }
        for instance in assembly.instances.values() {
            let frame = &assembly.frames[&instance.frame_id];
            if let Some(parent) = &instance.parent {
                let parent = assembly.frame(parent)?;
                let host = parent
                    .solution
                    .boxes
                    .get(&frame.id)
                    .ok_or_else(|| invalid("parent lacks the child's host box"))?;
                if !host.solid
                    || parent
                        .solution
                        .boxes
                        .values()
                        .any(|object| object.parent.as_ref() == Some(&frame.id))
                {
                    return Err(invalid(
                        "child host must be a solid, childless routing obstacle",
                    ));
                }
                let child_root = &frame.solution.boxes[&frame.id];
                if child_root.solid
                    || child_root.bounds != frame.bounds
                    || frame
                        .solution
                        .boxes
                        .values()
                        .filter(|object| object.parent.is_none())
                        .count()
                        != 1
                {
                    return Err(invalid(
                        "attached interior needs one open root matching its canonical domain",
                    ));
                }
                if semantic_members(&host.semantic_ids)
                    != semantic_members(&frame.solution.boxes[&frame.id].semantic_ids)
                {
                    return Err(invalid("host/canonical root semantic membership mismatch"));
                }
                if ExactBounds::integer(host.bounds)?
                    != instance.transform.map_bounds(frame.bounds())?
                {
                    return Err(invalid(
                        "canonical domain must map exactly onto its parent envelope; use fit_envelope",
                    ));
                }
            }
            // Establish connectivity, acyclicity, bounded depth and composed
            // numeric range now, before presentation or pin capture can succeed.
            let transform = assembly.transform_to(&frame.id, &assembly.root)?;
            transform.map_bounds(frame.bounds())?;
        }
        Ok(assembly)
    }
    pub fn root(&self) -> &str {
        &self.root
    }
    pub fn frame_ids(&self) -> impl Iterator<Item = &str> {
        self.frames.keys().map(String::as_str)
    }
    pub fn frame(&self, id: &str) -> Result<&CanonicalFrame> {
        self.frames
            .get(id)
            .ok_or_else(|| AssemblyError::UnknownFrame(id.into()))
    }
    pub fn instance(&self, id: &str) -> Result<&AssemblyInstance> {
        self.instances
            .get(id)
            .ok_or_else(|| AssemblyError::UnknownFrame(id.into()))
    }
    fn chain(&self, owner: &str, anchor: &str) -> Result<Vec<&AssemblyInstance>> {
        self.frame(anchor)?;
        let mut current = owner;
        let mut visited = BTreeSet::new();
        let mut chain = Vec::new();
        loop {
            let instance = self.instance(current)?;
            if !visited.insert(current) {
                return Err(invalid("cyclic instance ownership"));
            }
            if current == anchor {
                return Ok(chain);
            }
            if chain.len() == MAX_DEPTH {
                return Err(AssemblyError::Limit("instance depth"));
            }
            chain.push(instance);
            current = instance
                .parent
                .as_deref()
                .ok_or_else(|| invalid("pin/presentation anchor is not an ancestor"))?;
        }
    }
    pub fn transform_to(&self, owner: &str, anchor: &str) -> Result<DyadicTransform> {
        let mut chain = self.chain(owner, anchor)?.into_iter();
        let Some(first) = chain.next() else {
            return Ok(DyadicTransform::identity());
        };
        chain.try_fold(first.transform, |map, instance| {
            instance.transform.compose(map)
        })
    }
    pub fn parent_envelope(&self, frame: &str) -> Result<Option<Bounds>> {
        let instance = self.instance(frame)?;
        instance
            .parent
            .as_ref()
            .map(|parent| {
                self.frame(parent)?
                    .solution
                    .boxes
                    .get(frame)
                    .map(|host| host.bounds)
                    .ok_or_else(|| invalid("missing parent envelope"))
            })
            .transpose()
    }
    /// Every box, route vertex and scope port uses the same composed map. A host
    /// with an attached interior is exposed as an open envelope. Cross-frame
    /// route joins are intentionally neither synthesized nor validated here.
    pub fn present(&self, frame: &str, anchor: &str) -> Result<PresentedFrame> {
        let canonical = self.frame(frame)?;
        let transform = self.transform_to(frame, anchor)?;
        let boxes = canonical
            .solution
            .boxes
            .iter()
            .map(|(id, object)| {
                let attached = self
                    .instances
                    .get(id)
                    .is_some_and(|instance| instance.parent.as_deref() == Some(frame));
                Ok((
                    id.clone(),
                    PresentedBox {
                        semantic_ids: object.semantic_ids.clone(),
                        parent: object.parent.clone(),
                        bounds: transform.map_bounds(object.bounds)?,
                        solid: object.solid && !attached,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        let routes = canonical
            .solution
            .routes
            .iter()
            .map(|route| {
                Ok(PresentedRoute {
                    edge_id: route.edge_id.clone(),
                    semantic_ids: route.semantic_ids.clone(),
                    points: route
                        .points
                        .iter()
                        .map(|point| transform.map_point(*point))
                        .collect::<Result<_>>()?,
                    scope_ports: route
                        .scope_ports
                        .iter()
                        .map(|port| {
                            Ok(PresentedScopePort {
                                assembly_id: port.assembly_id.clone(),
                                point: transform.map_point(port.point)?,
                                entering: port.entering,
                            })
                        })
                        .collect::<Result<_>>()?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(PresentedFrame {
            frame_id: frame.into(),
            anchor_id: anchor.into(),
            transform,
            boxes,
            routes,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedFrame {
    pub frame_id: String,
    pub anchor_id: String,
    pub transform: DyadicTransform,
    pub boxes: BTreeMap<String, PresentedBox>,
    pub routes: Vec<PresentedRoute>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedBox {
    pub semantic_ids: Vec<String>,
    /// Parent ID within this canonical frame (not a flattened global tree).
    pub parent: Option<String>,
    pub bounds: ExactBounds,
    pub solid: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedRoute {
    pub edge_id: String,
    pub semantic_ids: Vec<String>,
    pub points: Vec<ExactPoint>,
    pub scope_ports: Vec<PresentedScopePort>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedScopePort {
    pub assembly_id: String,
    pub point: ExactPoint,
    pub entering: bool,
}

/// Locks an object's integer center in its owning canonical frame and every
/// owner-to-anchor transform/envelope. Ancestors above the anchor may move.
/// This is a constraint for future solves, not a solver or an automatic repair.
/// The caller must compare assemblies in the same logical view/lens. Model and
/// object identity alone cannot establish that external context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorPin {
    model_id: String,
    owner: String,
    anchor: String,
    object_id: String,
    members: BTreeSet<String>,
    local_point: Point,
    anchored_point: ExactPoint,
    links: Vec<AnchorPinLink>,
}
/// Durable exact constraints for one owner-to-anchor instance. Public records
/// are untrusted until passed through `AnchorPin::from_record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorPinLink {
    pub frame: String,
    pub parent: String,
    pub transform: DyadicTransform,
    pub envelope: Bounds,
    pub canonical_bounds: Bounds,
}
/// Wire-independent durable pin data. A serializer must preserve exact points
/// and transforms; floating coordinates cannot reconstruct this record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorPinRecord {
    pub model_id: String,
    pub owner: String,
    pub anchor: String,
    pub object_id: String,
    pub members: Vec<String>,
    pub local_point: Point,
    pub anchored_point: ExactPoint,
    pub links: Vec<AnchorPinLink>,
}
impl AnchorPin {
    pub fn to_record(&self) -> AnchorPinRecord {
        AnchorPinRecord {
            model_id: self.model_id.clone(),
            owner: self.owner.clone(),
            anchor: self.anchor.clone(),
            object_id: self.object_id.clone(),
            members: self.members.iter().cloned().collect(),
            local_point: self.local_point,
            anchored_point: self.anchored_point,
            links: self.links.clone(),
        }
    }
    /// Check bounded identity, semantic membership, an acyclic exact chain and
    /// its complete affine signature without requiring an evictable assembly.
    /// The caller separately binds the record to its typed view/lens; solving
    /// still validates every active pin against the supplied model geometry.
    pub fn from_record(record: AnchorPinRecord) -> Result<Self> {
        if record.model_id.is_empty()
            || record.model_id.len() > 128
            || ![&record.owner, &record.anchor, &record.object_id]
                .into_iter()
                .all(|id| valid_id(id))
            || !valid_members(&record.members)
            || record.links.len() > MAX_DEPTH
        {
            return Err(invalid("invalid durable pin identity, membership or depth"));
        }
        let identity_bytes = record.members.iter().map(String::len).sum::<usize>()
            + record
                .links
                .iter()
                .map(|link| link.frame.len() + link.parent.len())
                .sum::<usize>()
            + record.model_id.len()
            + record.owner.len()
            + record.anchor.len()
            + record.object_id.len();
        if identity_bytes > 1024 * 1024 {
            return Err(AssemblyError::Limit("durable pin identity bytes"));
        }
        checked_point(record.local_point)?;
        let mut current = record.owner.as_str();
        let mut visited = BTreeSet::new();
        let mut transform = DyadicTransform::identity();
        for (index, link) in record.links.iter().enumerate() {
            if !valid_id(&link.frame)
                || !valid_id(&link.parent)
                || link.frame != current
                || !visited.insert(current)
                || current == record.anchor
            {
                return Err(invalid("invalid durable pin frame chain"));
            }
            checked_bounds(link.canonical_bounds)?;
            checked_bounds(link.envelope)?;
            if link.transform.map_bounds(link.canonical_bounds)?
                != ExactBounds::integer(link.envelope)?
                || (index == 0 && !link.canonical_bounds.contains_point(record.local_point))
                || record
                    .links
                    .get(index + 1)
                    .is_some_and(|parent| !contains(parent.canonical_bounds, link.envelope))
            {
                return Err(invalid("durable pin domain/envelope mismatch"));
            }
            transform = link.transform.compose(transform)?;
            current = &link.parent;
        }
        if current != record.anchor
            || visited.contains(current)
            || transform.map_point(record.local_point)? != record.anchored_point
        {
            return Err(invalid("durable pin anchor or exact point mismatch"));
        }
        Ok(Self {
            model_id: record.model_id,
            owner: record.owner,
            anchor: record.anchor,
            object_id: record.object_id,
            members: record.members.into_iter().collect(),
            local_point: record.local_point,
            anchored_point: record.anchored_point,
            links: record.links,
        })
    }
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
    pub fn semantic_ids(&self) -> &BTreeSet<String> {
        &self.members
    }
    pub fn links(&self) -> &[AnchorPinLink] {
        &self.links
    }
    pub fn capture(
        assembly: &Assembly,
        owner: &str,
        anchor: &str,
        object_id: &str,
    ) -> Result<Self> {
        let frame = assembly.frame(owner)?;
        let object = frame
            .solution
            .boxes
            .get(object_id)
            .ok_or_else(|| invalid("pin object is absent from its owner frame"))?;
        let local_point = object.bounds.center(); // Canonical construction checked bounds first.
        let links = pin_links(assembly, owner, anchor)?;
        Ok(Self {
            model_id: frame.solution.model_id.clone(),
            owner: owner.into(),
            anchor: anchor.into(),
            object_id: object_id.into(),
            members: semantic_members(&object.semantic_ids),
            local_point,
            anchored_point: assembly
                .transform_to(owner, anchor)?
                .map_point(local_point)?,
            links,
        })
    }
    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn object_id(&self) -> &str {
        &self.object_id
    }
    pub fn anchor(&self) -> &str {
        &self.anchor
    }
    pub fn local_point(&self) -> Point {
        self.local_point
    }
    pub fn anchored_point(&self) -> ExactPoint {
        self.anchored_point
    }
    pub fn validate(&self, assembly: &Assembly) -> Result<()> {
        let frame = assembly.frame(&self.owner)?;
        let matches = frame
            .solution
            .boxes
            .get(&self.object_id)
            .is_some_and(|object| {
                frame.solution.model_id == self.model_id
                    && object.bounds.center() == self.local_point
                    && semantic_members(&object.semantic_ids) == self.members
            });
        if !matches
            || pin_links(assembly, &self.owner, &self.anchor)? != self.links
            || assembly
                .transform_to(&self.owner, &self.anchor)?
                .map_point(self.local_point)?
                != self.anchored_point
        {
            return Err(AssemblyError::PinConflict(self.object_id.clone()));
        }
        Ok(())
    }
}
fn pin_links(assembly: &Assembly, owner: &str, anchor: &str) -> Result<Vec<AnchorPinLink>> {
    assembly
        .chain(owner, anchor)?
        .into_iter()
        .map(|instance| {
            Ok(AnchorPinLink {
                frame: instance.frame_id.clone(),
                parent: instance.parent.clone().unwrap(),
                transform: instance.transform,
                canonical_bounds: assembly.frame(&instance.frame_id)?.bounds(),
                envelope: assembly
                    .parent_envelope(&instance.frame_id)?
                    .ok_or_else(|| invalid("pin link lacks parent envelope"))?,
            })
        })
        .collect()
}

fn validate_frame(root_id: &str, domain: Bounds, solution: &LayoutSolution) -> Result<()> {
    checked_bounds(domain)?;
    if solution.version != LAYOUT_VERSION
        || solution.model_id.is_empty()
        || solution.model_id.len() > 128
        || solution.revision == 0
    {
        return Err(invalid("canonical model/revision/layout identity"));
    }
    if solution.boxes.is_empty() || solution.boxes.len() > 1024 || solution.routes.len() > 4096 {
        return Err(AssemblyError::Limit("canonical frame geometry"));
    }
    if member_count(solution) > MAX_TOTAL_SEMANTIC_MEMBERS {
        return Err(AssemblyError::Limit("canonical semantic membership"));
    }
    let root = solution
        .boxes
        .get(root_id)
        .ok_or_else(|| invalid("missing explicit canonical root"))?;
    if root.parent.is_some() {
        return Err(invalid("explicit canonical root has a parent"));
    }
    for (id, object) in &solution.boxes {
        if id != &object.id || !valid_id(id) || !valid_members(&object.semantic_ids) {
            return Err(invalid("canonical object identity/membership"));
        }
        checked_bounds(object.bounds)?;
        if !contains(domain, object.bounds) {
            return Err(invalid("object leaves the canonical domain"));
        }
        let mut current = object;
        let mut seen = BTreeSet::new();
        while let Some(parent) = &current.parent {
            if !seen.insert(parent) {
                return Err(invalid("cyclic canonical containment"));
            }
            if seen.len() > MAX_DEPTH {
                return Err(AssemblyError::Limit("canonical containment depth"));
            }
            let ancestor = solution
                .boxes
                .get(parent)
                .ok_or_else(|| invalid("missing canonical parent"))?;
            // Safe comparisons only: malformed coordinates are not subtracted.
            if ancestor.solid {
                // Prior typed validation establishes that this is an Interface.
                // Here preserve the existing front-face attachment geometry.
                if current.bounds.min.z != ancestor.bounds.max.z
                    || current.bounds.min.x < ancestor.bounds.min.x
                    || current.bounds.max.x > ancestor.bounds.max.x
                    || current.bounds.min.y < ancestor.bounds.min.y
                    || current.bounds.max.y > ancestor.bounds.max.y
                {
                    return Err(invalid("invalid canonical solid-owner attachment"));
                }
            // Protrusion is allowed only through the solid owner's face. The
            // original object still fits every containing open assembly.
            } else if !contains(ancestor.bounds, object.bounds) {
                return Err(invalid("invalid canonical containment"));
            }
            current = ancestor;
        }
    }
    let mut ids = BTreeSet::new();
    let mut points = 0usize;
    for route in &solution.routes {
        if !valid_id(&route.edge_id)
            || !ids.insert(&route.edge_id)
            || !valid_members(&route.semantic_ids)
        {
            return Err(invalid("canonical route identity/membership"));
        }
        if route.points.len() < 2 || route.points.len() > 4096 || route.scope_ports.len() > 4096 {
            return Err(AssemblyError::Limit("canonical route points"));
        }
        points += route.points.len() + route.scope_ports.len();
        if points > MAX_TOTAL_ROUTE_POINTS {
            return Err(AssemblyError::Limit("canonical route points"));
        }
        for p in &route.points {
            checked_point(*p)?;
            if !domain.contains_point(*p) {
                return Err(invalid("route leaves its canonical frame"));
            }
        }
        if route.points.windows(2).any(|pair| {
            coords(pair[0])
                .into_iter()
                .zip(coords(pair[1]))
                .filter(|(a, b)| a != b)
                .count()
                != 1
        }) {
            return Err(invalid(
                "canonical route is not a nondegenerate orthogonal polyline",
            ));
        }
        for port in &route.scope_ports {
            checked_point(port.point)?;
            let bounds = solution
                .boxes
                .get(&port.assembly_id)
                .ok_or_else(|| invalid("unknown canonical scope port"))?
                .bounds;
            if !on_boundary(bounds, port.point) {
                return Err(AssemblyError::NotBoundary);
            }
        }
    }
    Ok(())
}
fn member_count(solution: &LayoutSolution) -> usize {
    solution
        .boxes
        .values()
        .map(|object| object.semantic_ids.len())
        .sum::<usize>()
        + solution
            .routes
            .iter()
            .map(|route| route.semantic_ids.len())
            .sum::<usize>()
}
fn coords(point: Point) -> [i64; 3] {
    [point.x, point.y, point.z]
}
fn point(p: [i64; 3]) -> Point {
    Point::new(p[0], p[1], p[2])
}
fn checked_point(p: Point) -> Result<()> {
    if coords(p)
        .iter()
        .any(|n| *n < -MAX_COORDINATE || *n > MAX_COORDINATE)
    {
        return Err(AssemblyError::Overflow);
    }
    Ok(())
}
fn checked_bounds(bounds: Bounds) -> Result<()> {
    checked_point(bounds.min)?;
    checked_point(bounds.max)?;
    if coords(bounds.min)
        .iter()
        .zip(coords(bounds.max))
        .any(|(min, max)| *min >= max)
    {
        return Err(invalid("nonpositive canonical bounds"));
    }
    Ok(())
}
fn sizes(bounds: Bounds) -> [i128; 3] {
    let min = coords(bounds.min);
    let max = coords(bounds.max);
    std::array::from_fn(|axis| i128::from(max[axis]) - i128::from(min[axis]))
}
fn le(a: ExactPoint, b: ExactPoint) -> bool {
    let shift = a.shift.max(b.shift);
    (0..3).all(|axis| {
        a.numerators[axis] << (shift - a.shift) <= b.numerators[axis] << (shift - b.shift)
    })
}
fn contains(a: Bounds, b: Bounds) -> bool {
    a.contains_point(b.min) && a.contains_point(b.max)
}
fn on_boundary(bounds: Bounds, p: Point) -> bool {
    bounds.contains_point(p)
        && (0..3).any(|axis| {
            coords(p)[axis] == coords(bounds.min)[axis]
                || coords(p)[axis] == coords(bounds.max)[axis]
        })
}
fn semantic_members(ids: &[String]) -> BTreeSet<String> {
    ids.iter().cloned().collect()
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 4096
}
fn valid_members(ids: &[String]) -> bool {
    !ids.is_empty()
        && ids.len() <= 65_536
        && ids.iter().all(|id| valid_id(id))
        && semantic_members(ids).len() == ids.len()
}
fn invalid(message: impl Into<String>) -> AssemblyError {
    AssemblyError::Invalid(message.into())
}
