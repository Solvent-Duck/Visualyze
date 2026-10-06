//! Private, explicitly located view state. Never part of the semantic contract.
//!
//! Loading checks bounded shape and identity only. The caller must rebuild the
//! current projection and run the full layout validator before showing a cache.
use std::{
    collections::BTreeSet,
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use serde::{Deserialize, Serialize};

use crate::{LAYOUT_VERSION, LayoutSolution, Point, projection::ViewState};

pub const STATE_VERSION: u32 = 1;
pub const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
pub const DISMISSAL_ENCODING_VERSION: u32 = 1;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedView {
    pub version: u32,
    pub model_id: String,
    pub revision: u64,
    pub view: ViewState,
    pub accepted: LayoutSolution,
    /// Optional in older state files. Suppression metadata never validates a
    /// semantic model or layout; the runtime still validates every candidate.
    #[serde(default)]
    pub dismissed_proposals: Vec<DismissedProposal>,
}

/// Compact, non-security identity of a dismissed proposal. Revision and quality
/// score are deliberately excluded: harmless reloads must not repeat a prompt.
/// Encoding v1 includes box geometry/membership and original directed claims,
/// but excludes route point coordinates, so a reroute alone stays dismissed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DismissedProposal {
    pub encoding_version: u32,
    pub layout_version: u32,
    pub model_id: String,
    pub root: String,
    pub lens: visualyze_model::provider::SemanticLens,
    pub signature: [u64; 2],
}

#[derive(Debug)]
pub enum PersistenceError {
    Io(std::io::Error),
    Json(serde_json::Error),
    UnsupportedVersion(u32),
    TooLarge,
    Invalid(String),
    SourceChanged,
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "view-state I/O: {e}"),
            Self::Json(e) => write!(f, "view-state JSON: {e}"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "unsupported private view-state version {version}; original file preserved"
            ),
            Self::TooLarge => f.write_str("private view state exceeds 8 MiB"),
            Self::Invalid(message) => write!(f, "invalid private view state: {message}"),
            Self::SourceChanged => {
                f.write_str("view-state file changed while saving; existing file preserved")
            }
        }
    }
}

impl std::error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for PersistenceError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for PersistenceError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

/// Read an explicit regular file without changing it, including on version errors.
pub fn load(path: impl AsRef<Path>) -> Result<PersistedView, PersistenceError> {
    let path = path.as_ref();
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(invalid(
            "state path must be a regular file, not a link or directory",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x0004 | 0x0100); // O_NONBLOCK | O_NOFOLLOW
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x0800 | 0x20000); // O_NONBLOCK | O_NOFOLLOW
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("state path is not a regular file"));
    }
    if metadata.len() > MAX_STATE_BYTES as u64 {
        return Err(PersistenceError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(PersistenceError::TooLarge);
    }
    // Strict decoding detects duplicate map keys too, including pins and boxes.
    let value = strict_json::parse(&bytes)?;
    if let Some(version) = value.get("version").and_then(serde_json::Value::as_u64)
        && version != u64::from(STATE_VERSION)
    {
        return Err(PersistenceError::UnsupportedVersion(
            version.try_into().unwrap_or(u32::MAX),
        ));
    }
    let state: PersistedView = serde_json::from_value(value)?;
    validate_shape(&state)?;
    Ok(state)
}

/// Serialize completely before touching the destination, then atomically rename
/// a flushed temporary sibling. Existing corrupt/unsupported/other-model files
/// are preserved. Parent directories and OS application-data policy belong to the caller.
pub fn save(path: impl AsRef<Path>, state: &PersistedView) -> Result<(), PersistenceError> {
    let path = path.as_ref();
    validate_shape(state)?;
    let mut encoded = BoundedBuffer(Vec::new());
    if let Err(error) = serde_json::to_writer(&mut encoded, state) {
        return if error.is_io() {
            Err(PersistenceError::TooLarge)
        } else {
            Err(error.into())
        };
    }
    // Apply the same depth/value budget as loading before creating any file, so
    // unusually large override collections cannot produce an unreadable cache.
    strict_json::parse(&encoded.0)?;
    let original = destination_stamp(path)?;
    if original.is_some() {
        let existing = load(path)?;
        if existing.model_id != state.model_id {
            return Err(invalid("destination belongs to a different model"));
        }
        if existing.revision > state.revision {
            return Err(invalid("refusing to replace a newer saved model revision"));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if path.file_name().is_none() {
        return Err(invalid("destination has no file name"));
    }
    let temporary = parent.join(format!(
        ".visualyze-state-{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Create-new ensures that a name collision never opens or deletes someone
    // else's file. The guard is created only after successful exclusive creation.
    let mut file = options.open(&temporary)?;
    let mut guard = Temporary {
        path: temporary,
        armed: true,
    };
    file.write_all(&encoded.0)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    if destination_stamp(path)? != original {
        return Err(PersistenceError::SourceChanged);
    }
    std::fs::rename(&guard.path, path)?;
    guard.armed = false;
    // Sync the directory entry on supported Unix hosts after the atomic rename.
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

struct Temporary {
    path: PathBuf,
    armed: bool,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn destination_stamp(path: &Path) -> Result<Option<(u64, SystemTime)>, PersistenceError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some((metadata.len(), metadata.modified()?))),
        Ok(_) => Err(invalid("destination is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

struct BoundedBuffer(Vec<u8>);
impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_STATE_BYTES - self.0.len() {
            return Err(std::io::Error::other("state byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_shape(state: &PersistedView) -> Result<(), PersistenceError> {
    if state.version != STATE_VERSION {
        return Err(PersistenceError::UnsupportedVersion(state.version));
    }
    let accepted = &state.accepted;
    if state.model_id.trim().is_empty()
        || state.model_id.len() > 128
        || state.revision == 0
        || state.model_id != accepted.model_id
        || state.revision != accepted.revision
        || accepted.version != LAYOUT_VERSION
        || accepted.projection_key.is_empty()
        || accepted.projection_key.len() > 512 * 1024
    {
        return Err(invalid(
            "model, revision, layout version or projection identity mismatch",
        ));
    }
    if accepted.boxes.is_empty()
        || accepted.boxes.len() > 1024
        || accepted.routes.len() > 4096
        || accepted.diagnostics.len() > 1024
    {
        return Err(invalid("accepted layout exceeds visible shape budgets"));
    }
    if state.dismissed_proposals.len() > 32 {
        return Err(invalid("more than 32 dismissed reorganization proposals"));
    }
    let mut dismissed = BTreeSet::new();
    for proposal in &state.dismissed_proposals {
        if proposal.encoding_version != DISMISSAL_ENCODING_VERSION
            || proposal.layout_version != LAYOUT_VERSION
            || proposal.model_id != state.model_id
            || proposal.root.is_empty()
            || proposal.root.len() > 4096
            || !dismissed.insert(proposal)
        {
            return Err(invalid(
                "invalid, incompatible or duplicate dismissed proposal",
            ));
        }
    }
    let view = &state.view;
    // `peer_ceiling` and `group_target` are legacy and ignored (ADR 0007), so
    // whatever an old file stored is accepted.
    if view.visible_budget == 0
        || view.visible_budget > 1024
        || view.pins.len() > 4096
        || view.frame_pins.len() > 64
        || view.layer_preferences.len() > 4096
        || view.expanded.len() > 4096
        || view.collapsed.len() > 4096
        || view.group_overrides.len() > 1024
        || view.dismissed_groups.len() > 1024
        || view.retained_suggestions().len() > 64
        || view.breadcrumbs().len() > 256
        || view.child_page > 1_000_000
        || view.relation_history.len() > 64
        || view
            .relation_cursor
            .iter()
            .chain(view.relation_history.iter().flatten())
            .any(|cursor| !cursor.valid_shape())
        || view
            .layer_preferences
            .values()
            .any(|band| !(-16..=16).contains(band))
        || view.expanded.iter().any(|id| view.collapsed.contains(id))
    {
        return Err(invalid("view overrides exceed supported bounds"));
    }
    for point in view.pins.values() {
        check_point(*point)?;
    }
    let mut total_frame_pins = 0usize;
    for (frame, pins) in &view.frame_pins {
        total_frame_pins += pins.len();
        let local_frame = ["l:logical|", "l:code|", "l:deployment|"]
            .into_iter()
            .find_map(|prefix| frame.strip_prefix(prefix))
            .unwrap_or(frame);
        if (local_frame != "root"
            && local_frame
                .strip_prefix("f:")
                .is_none_or(|id| id.is_empty()))
            || frame.len() > 4128
            || pins.len() > 4096
            || total_frame_pins > 16384
            || pins.keys().any(|id| id.is_empty() || id.len() > 4096)
        {
            return Err(invalid("invalid or excessive frame-scoped pins"));
        }
        for point in pins.values() {
            check_point(*point)?;
        }
    }
    for (id, bounds) in &accepted.boxes {
        if id.is_empty()
            || id.len() > 4096
            || bounds.id != *id
            || bounds.semantic_ids.is_empty()
            || bounds.semantic_ids.len() > 65536
        {
            return Err(invalid("invalid box identity or membership"));
        }
        check_point(bounds.bounds.min)?;
        check_point(bounds.bounds.max)?;
        let size = bounds.bounds.size();
        if size.x <= 0 || size.y <= 0 || size.z <= 0 {
            return Err(invalid("non-positive saved box bounds"));
        }
        let mut seen = BTreeSet::new();
        let mut parent = bounds.parent.as_deref();
        while let Some(parent_id) = parent {
            if parent_id == id || !seen.insert(parent_id) {
                return Err(invalid("cyclic saved box hierarchy"));
            }
            parent = accepted
                .boxes
                .get(parent_id)
                .ok_or_else(|| invalid("missing saved parent"))?
                .parent
                .as_deref();
        }
    }
    let mut route_ids = BTreeSet::new();
    let mut points = 0usize;
    for route in &accepted.routes {
        points += route.points.len();
        if points > 200_000
            || route.points.len() < 2
            || route.edge_id.is_empty()
            || !route_ids.insert(&route.edge_id)
            || route.semantic_ids.is_empty()
        {
            return Err(invalid("invalid saved route shape"));
        }
        for point in &route.points {
            check_point(*point)?;
        }
    }
    let m = &accepted.metrics;
    if [
        m.volume,
        m.aspect_ratio,
        m.occlusion,
        m.congestion,
        m.structural_cost,
    ]
    .iter()
    .any(|value| !value.is_finite() || *value < 0.0)
        || m.route_length < 0
        || m.displacement < 0
        || m.occlusion > 1.0
        || m.shared_lane_length < 0
    {
        return Err(invalid("invalid saved measurements"));
    }
    for group in &view.group_overrides {
        if group.id.is_empty()
            || group.parent.is_empty()
            || group.members.is_empty()
            || group.members.len() > 4096
            || group.members.iter().collect::<BTreeSet<_>>().len() != group.members.len()
        {
            return Err(invalid("invalid visual group override"));
        }
    }
    Ok(())
}

fn check_point(point: Point) -> Result<(), PersistenceError> {
    if [point.x, point.y, point.z]
        .iter()
        .any(|v| !(-10_000_000..=10_000_000).contains(v))
    {
        Err(invalid("coordinate is outside supported bounds"))
    } else {
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> PersistenceError {
    PersistenceError::Invalid(message.into())
}

mod strict_json {
    use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
    use serde_json::{Map, Number, Value};
    use std::fmt;

    pub(super) fn parse(bytes: &[u8]) -> Result<Value, serde_json::Error> {
        let mut parser = serde_json::Deserializer::from_slice(bytes);
        let mut remaining = 1_000_000;
        let value = Seed {
            depth: 0,
            remaining: &mut remaining,
        }
        .deserialize(&mut parser)?;
        parser.end()?;
        Ok(value)
    }
    struct Seed<'a> {
        depth: usize,
        remaining: &'a mut usize,
    }
    impl<'de> DeserializeSeed<'de> for Seed<'_> {
        type Value = Value;
        fn deserialize<D: serde::Deserializer<'de>>(self, parser: D) -> Result<Value, D::Error> {
            if *self.remaining == 0 {
                return Err(D::Error::custom("view state exceeds JSON value budget"));
            }
            *self.remaining -= 1;
            parser.deserialize_any(self)
        }
    }
    impl<'de> Visitor<'de> for Seed<'_> {
        type Value = Value;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("bounded private view-state JSON")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
            if self.depth >= 16 {
                return Err(A::Error::custom("view-state nesting exceeds 16"));
            }
            let mut result = Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if result.contains_key(&key) {
                    return Err(A::Error::custom("duplicate view-state key"));
                }
                let value = map.next_value_seed(Seed {
                    depth: self.depth + 1,
                    remaining: self.remaining,
                })?;
                result.insert(key, value);
            }
            Ok(Value::Object(result))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
            if self.depth >= 16 {
                return Err(A::Error::custom("view-state nesting exceeds 16"));
            }
            let mut result = Vec::new();
            while let Some(value) = seq.next_element_seed(Seed {
                depth: self.depth + 1,
                remaining: self.remaining,
            })? {
                result.push(value);
            }
            Ok(Value::Array(result))
        }
        fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
            Ok(Value::String(value.into()))
        }
        fn visit_string<E: Error>(self, value: String) -> Result<Value, E> {
            Ok(Value::String(value))
        }
        fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
            Ok(Value::Bool(value))
        }
        fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
            Ok(Value::Number(value.into()))
        }
        fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
            Ok(Value::Number(value.into()))
        }
        fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
            Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| E::custom("non-finite saved number"))
        }
        fn visit_unit<E: Error>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, LayoutBox, Metrics, projection::DisplayGroup};
    use std::collections::BTreeMap;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "visualyze-state-test-{}-{}.json",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn state() -> PersistedView {
        let mut view = ViewState::default();
        view.enter("component:a");
        view.pins.insert("component:a".into(), Point::new(5, 5, 5));
        view.layer_preferences.insert("component:a".into(), 2);
        view.group_overrides.push(DisplayGroup {
            id: "visual:group".into(),
            parent: "scope:root".into(),
            members: vec!["component:a".into()],
            suggested: false,
        });
        PersistedView {
            version: STATE_VERSION,
            dismissed_proposals: vec![],
            model_id: "example:model".into(),
            revision: 1,
            view,
            accepted: LayoutSolution {
                model_id: "example:model".into(),
                revision: 1,
                version: LAYOUT_VERSION,
                projection_key: "test-view".into(),
                boxes: BTreeMap::from([(
                    "component:a".into(),
                    LayoutBox {
                        id: "component:a".into(),
                        semantic_ids: vec!["component:a".into()],
                        parent: None,
                        solid: true,
                        bounds: Bounds {
                            min: Point::ZERO,
                            max: Point::new(10, 10, 10),
                        },
                    },
                )]),
                routes: vec![],
                metrics: Metrics::default(),
                diagnostics: vec![],
            },
        }
    }
    #[test]
    fn dismissals_roundtrip_and_legacy_files_default_to_empty() {
        let path = Temp::new();
        let mut expected = state();
        expected.dismissed_proposals.push(DismissedProposal {
            encoding_version: DISMISSAL_ENCODING_VERSION,
            layout_version: LAYOUT_VERSION,
            model_id: expected.model_id.clone(),
            root: "component:a".into(),
            lens: visualyze_model::provider::SemanticLens::Logical,
            signature: [123, 456],
        });
        save(&path.0, &expected).unwrap();
        let loaded = load(&path.0).unwrap();
        assert_eq!(loaded.dismissed_proposals, expected.dismissed_proposals);
        assert_eq!(loaded.accepted, expected.accepted);
        let mut legacy = serde_json::to_value(&expected).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("dismissed_proposals");
        std::fs::write(&path.0, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(load(&path.0).unwrap().dismissed_proposals.is_empty());
    }
    #[test]
    fn pre_adr_0007_view_fields_load_and_the_planner_state_is_never_written() {
        // Files written before ADR 0007 carry a peer ceiling and group target
        // that nothing reads any more, and may lack them entirely.
        let path = Temp::new();
        let mut legacy = serde_json::to_value(state()).unwrap();
        let view = legacy["view"].as_object_mut().unwrap();
        view.insert("peer_ceiling".into(), 64.into());
        view.insert("group_target".into(), 32.into());
        view.insert("auto_expanded".into(), serde_json::json!(["component:a"]));
        std::fs::write(&path.0, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let loaded = load(&path.0).unwrap();
        assert_eq!(
            (loaded.view.peer_ceiling, loaded.view.group_target),
            (64, 32)
        );
        assert!(loaded.view.is_expanded("component:a"));
        assert_eq!(loaded.view.pins["component:a"], Point::new(5, 5, 5));
        // Odd legacy values are accepted too: they are ignored.
        let view = legacy["view"].as_object_mut().unwrap();
        view.insert("peer_ceiling".into(), 3.into());
        view.insert("group_target".into(), 9.into());
        std::fs::write(&path.0, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(load(&path.0).is_ok());
        let view = legacy["view"].as_object_mut().unwrap();
        view.remove("peer_ceiling");
        view.remove("group_target");
        std::fs::write(&path.0, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(load(&path.0).unwrap().view.peer_ceiling, 64);
        // Whatever the planner opened lives only on a working copy: a saved view
        // never names it, and a file that does is not a view this build wrote.
        let mut planned = loaded.clone();
        planned.view.plan_open("component:b");
        save(&path.0, &planned).unwrap();
        let text = std::fs::read_to_string(&path.0).unwrap();
        assert!(!text.contains("planned") && !text.contains("component:b"));
        assert!(!load(&path.0).unwrap().view.is_expanded("component:b"));
        let mut forged: serde_json::Value = serde_json::from_str(&text).unwrap();
        forged["view"]["planned"] = serde_json::json!(["component:b"]);
        std::fs::write(&path.0, serde_json::to_vec(&forged).unwrap()).unwrap();
        assert!(load(&path.0).is_err());
    }
    #[test]
    fn malformed_or_excessive_dismissals_preserve_the_existing_file() {
        let path = Temp::new();
        let original = state();
        save(&path.0, &original).unwrap();
        let before = std::fs::read(&path.0).unwrap();
        let valid = DismissedProposal {
            encoding_version: DISMISSAL_ENCODING_VERSION,
            layout_version: LAYOUT_VERSION,
            model_id: original.model_id.clone(),
            root: "component:a".into(),
            lens: visualyze_model::provider::SemanticLens::Logical,
            signature: [123, 456],
        };
        for mutation in 0..6 {
            let mut invalid = original.clone();
            let mut proposal = valid.clone();
            match mutation {
                0 => proposal.encoding_version += 1,
                1 => proposal.model_id = "another-model".into(),
                2 => proposal.root.clear(),
                3 => proposal.root = "x".repeat(4097),
                4 => proposal.layout_version += 1,
                _ => (),
            }
            invalid.dismissed_proposals = if mutation == 5 {
                vec![proposal; 33]
            } else {
                vec![proposal]
            };
            assert!(save(&path.0, &invalid).is_err());
            assert_eq!(std::fs::read(&path.0).unwrap(), before);
            let malformed = serde_json::to_vec(&invalid).unwrap();
            std::fs::write(&path.0, &malformed).unwrap();
            assert!(load(&path.0).is_err());
            assert_eq!(std::fs::read(&path.0).unwrap(), malformed);
            std::fs::write(&path.0, &before).unwrap();
        }
        let mut duplicate = original;
        duplicate.dismissed_proposals = vec![valid.clone(), valid];
        assert!(save(&path.0, &duplicate).is_err());
        assert_eq!(std::fs::read(&path.0).unwrap(), before);
    }
    #[test]
    fn roundtrip_preserves_layout_pins_groups_layers_and_breadcrumbs() {
        let path = Temp::new();
        let expected = state();
        save(&path.0, &expected).unwrap();
        let actual = load(&path.0).unwrap();
        assert_eq!(actual.accepted, expected.accepted);
        assert_eq!(actual.view.pins, expected.view.pins);
        assert_eq!(actual.view.group_overrides, expected.view.group_overrides);
        assert_eq!(
            actual.view.layer_preferences,
            expected.view.layer_preferences
        );
        assert_eq!(actual.view.breadcrumbs(), expected.view.breadcrumbs());
        assert_eq!(actual.view.focus, expected.view.focus);
        let mut changed = expected;
        changed.accepted.diagnostics.push("Updated".into());
        save(&path.0, &changed).unwrap();
        assert_eq!(load(&path.0).unwrap().accepted.diagnostics, ["Updated"]);
    }
    #[test]
    fn restart_inside_subsystem_then_back_restores_overview_pins() {
        let path = Temp::new();
        let mut saved = state();
        saved.view = ViewState::default();
        let overview_pin = Point::new(5, 5, 5);
        saved.view.pins.insert("component:a".into(), overview_pin);
        saved.view.save_frame_pins();
        saved.view.enter("subsystem:database");
        saved.view.restore_frame_pins();
        assert!(saved.view.pins.is_empty());
        let inner_pin = Point::new(20, 30, 40);
        saved.view.pins.insert("table:orders".into(), inner_pin);
        save(&path.0, &saved).unwrap();

        let mut restored = load(&path.0).unwrap().view;
        assert_eq!(restored.focus.as_deref(), Some("subsystem:database"));
        assert_eq!(restored.pins["table:orders"], inner_pin);
        restored.save_frame_pins();
        assert!(restored.back());
        restored.restore_frame_pins();
        assert_eq!(restored.pins["component:a"], overview_pin);
        assert!(!restored.pins.contains_key("table:orders"));
        restored.save_frame_pins();
        restored.enter("subsystem:database");
        restored.restore_frame_pins();
        assert_eq!(restored.pins["table:orders"], inner_pin);
    }

    #[test]
    fn frame_pin_limits_and_old_state_compatibility_are_explicit() {
        let path = Temp::new();
        let good = state();
        let mut old = serde_json::to_value(&good).unwrap();
        old["view"].as_object_mut().unwrap().remove("frame_pins");
        std::fs::write(&path.0, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(load(&path.0).unwrap().view.frame_pins.is_empty());
        let before = std::fs::read(&path.0).unwrap();
        let mut bad = good;
        for i in 0..65 {
            bad.view
                .frame_pins
                .insert(format!("f:scope:{i}"), BTreeMap::new());
        }
        assert!(save(&path.0, &bad).is_err());
        assert_eq!(std::fs::read(&path.0).unwrap(), before);
        bad.view.frame_pins.clear();
        bad.view.frame_pins.insert(
            "root".into(),
            BTreeMap::from([("node".into(), Point::new(10_000_001, 0, 0))]),
        );
        assert!(save(&path.0, &bad).is_err());
        assert_eq!(std::fs::read(&path.0).unwrap(), before);
    }
    #[test]
    fn invalid_or_oversized_state_never_truncates_the_accepted_file() {
        let path = Temp::new();
        let good = state();
        save(&path.0, &good).unwrap();
        let before = std::fs::read(&path.0).unwrap();
        let mut bad = good.clone();
        bad.revision = 2;
        assert!(save(&path.0, &bad).is_err());
        assert_eq!(std::fs::read(&path.0).unwrap(), before);
        bad = good.clone();
        bad.accepted.metrics.volume = f64::NAN;
        assert!(save(&path.0, &bad).is_err());
        assert_eq!(std::fs::read(&path.0).unwrap(), before);
        bad = good;
        bad.accepted.diagnostics.push("x".repeat(MAX_STATE_BYTES));
        assert!(matches!(
            save(&path.0, &bad),
            Err(PersistenceError::TooLarge)
        ));
        assert_eq!(std::fs::read(&path.0).unwrap(), before);
    }

    #[test]
    fn semantic_lenses_preserve_independent_pin_frames_across_restart() {
        use visualyze_model::provider::SemanticLens;
        let path = Temp::new();
        let mut saved = state();
        saved.view.frame_lens = Some(SemanticLens::Logical);
        let logical = saved.view.pins.clone();
        saved.view.save_frame_pins();
        saved.view.lens = Some(SemanticLens::Code);
        saved.view.restore_frame_pins();
        assert!(saved.view.pins.is_empty());
        saved
            .view
            .pins
            .insert("component:a".into(), Point::new(40, 5, 20));
        saved.view.save_frame_pins();
        save(&path.0, &saved).unwrap();
        let mut restored = load(&path.0).unwrap().view;
        restored.lens = Some(SemanticLens::Logical);
        restored.restore_frame_pins();
        assert_eq!(restored.pins, logical);
        restored.lens = Some(SemanticLens::Code);
        restored.restore_frame_pins();
        assert_eq!(restored.pins["component:a"], Point::new(40, 5, 20));
    }
    #[test]
    fn private_page_cursors_roundtrip_with_bounded_history() {
        let path = Temp::new();
        let mut saved = state();
        let cursor = serde_json::from_value(serde_json::json!({
            "model_id": saved.model_id, "revision": saved.revision,
            "query": "boundary:scope:app", "offset": 4096
        }))
        .unwrap();
        saved.view.child_page = 2;
        saved.view.relation_cursor = Some(cursor);
        saved.view.relation_history = vec![None];
        save(&path.0, &saved).unwrap();
        let restored = load(&path.0).unwrap();
        assert_eq!(restored.view.child_page, 2);
        assert_eq!(restored.view.relation_cursor, saved.view.relation_cursor);
        assert_eq!(restored.view.relation_history, [None]);
        let before = std::fs::read(&path.0).unwrap();
        saved.view.relation_history = vec![None; 65];
        assert!(save(&path.0, &saved).is_err());
        assert_eq!(std::fs::read(&path.0).unwrap(), before);
    }
    #[test]
    fn corrupt_oversized_unknown_and_duplicate_json_are_rejected() {
        let path = Temp::new();
        let encoded = serde_json::to_string(&state()).unwrap();
        for data in [
            "{".to_owned(),
            encoded.replacen("\"revision\":1", "\"revision\":1,\"revision\":2", 1),
            encoded.replacen("\"version\":1", "\"version\":1,\"unknown\":true", 1),
            " ".repeat(MAX_STATE_BYTES + 1),
        ] {
            std::fs::write(&path.0, &data).unwrap();
            assert!(load(&path.0).is_err());
            assert!(save(&path.0, &state()).is_err());
            assert_eq!(std::fs::read(&path.0).unwrap(), data.as_bytes());
        }
    }
    #[test]
    fn unsupported_versions_and_other_models_are_preserved() {
        let path = Temp::new();
        let mut future = state();
        future.version = 999;
        let bytes = serde_json::to_vec(&future).unwrap();
        std::fs::write(&path.0, &bytes).unwrap();
        assert!(matches!(
            load(&path.0),
            Err(PersistenceError::UnsupportedVersion(999))
        ));
        assert!(save(&path.0, &state()).is_err());
        assert_eq!(std::fs::read(&path.0).unwrap(), bytes);
        std::fs::remove_file(&path.0).unwrap();
        let mut other = state();
        other.model_id = "other:model".into();
        other.accepted.model_id = other.model_id.clone();
        save(&path.0, &other).unwrap();
        assert!(save(&path.0, &state()).is_err());
        assert_eq!(load(&path.0).unwrap().model_id, "other:model");
    }
    #[cfg(unix)]
    #[test]
    fn local_state_is_private_and_symlink_destinations_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let path = Temp::new();
        save(&path.0, &state()).unwrap();
        assert_eq!(
            std::fs::metadata(&path.0).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let link = Temp::new();
        symlink(&path.0, &link.0).unwrap();
        assert!(load(&link.0).is_err());
        assert!(save(&link.0, &state()).is_err());
        assert!(load(&path.0).is_ok());
    }
}
