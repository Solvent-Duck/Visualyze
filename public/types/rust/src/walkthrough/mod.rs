//! Validated walkthroughs: explanatory scene sequences over one revision of an architecture
//! model. A walkthrough references existing IDs only; it cannot add structure or connections.
mod teaching;
mod types;

pub use teaching::{ADVISED_MEMBERS, teaching_model};
pub use types::*;

use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

use crate::architecture::{
    Architecture, Diagnostic, EntityKind, MAX_BYTES, MAX_DIAGNOSTICS, RelationKind, Snapshot,
    ValidationError, input,
};

pub const SCHEMA_VERSION: &str = "walkthrough.0.1.0-draft.1";
pub const SCHEMA: &str = include_str!("../../../../schema/walkthrough.schema.json");
/// Focus sizes above this are advised against, never rejected.
pub const ADVISED_FOCUS: usize = 12;
/// Walkthroughs longer than this are advised against, never rejected.
pub const ADVISED_STEPS: usize = 12;
/// Walkthroughs at least this long are advised to open a subsystem at least once.
pub const OPENING_STEPS: usize = 5;

static VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str(SCHEMA).expect("embedded schema is JSON");
    jsonschema::validator_for(&schema).expect("embedded walkthrough schema is valid")
});

/// Only this validated wrapper can enter the renderer.
#[derive(Debug, Clone)]
pub struct Walkthrough {
    script: Script,
    resolved: Vec<ResolvedStep>,
    teaching: Vec<ResolvedTeachingView>,
    warnings: Vec<Diagnostic>,
}

impl Walkthrough {
    /// Validates a script against the model it references.
    pub fn from_json(bytes: &[u8], model: &Architecture) -> Result<Self, ValidationError> {
        if bytes.len() > MAX_BYTES {
            return Err(ValidationError::one(
                "payload_too_large",
                "",
                "walkthrough JSON exceeds 2 MiB",
            ));
        }
        let value = input::parse(bytes)?;
        let diagnostics: Vec<_> = VALIDATOR
            .iter_errors(&value)
            .take(MAX_DIAGNOSTICS)
            .map(|e| {
                Diagnostic::new(
                    "schema_violation",
                    e.instance_path().to_string(),
                    e.masked().to_string(),
                )
            })
            .collect();
        if !diagnostics.is_empty() {
            return Err(ValidationError { diagnostics });
        }
        let script: Script = serde_json::from_value(value)
            .map_err(|e| ValidationError::one("typed_decode", "", e.to_string()))?;
        let Checked {
            resolved,
            teaching,
            warnings,
        } = check(&script, model.snapshot())?;
        Ok(Self {
            script,
            resolved,
            teaching,
            warnings,
        })
    }

    pub fn script(&self) -> &Script {
        &self.script
    }
    /// Derived roots and hop directions, parallel to `script().steps`.
    pub fn resolved(&self) -> &[ResolvedStep] {
        &self.resolved
    }
    /// What each teaching view shows, parallel to `script().teaching_views`.
    pub fn teaching_views(&self) -> &[ResolvedTeachingView] {
        &self.teaching
    }
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }
}

/// Relation kinds a trace may animate. Structure and coverage are not travel.
pub fn is_flow(kind: RelationKind) -> bool {
    matches!(
        kind,
        RelationKind::Requests
            | RelationKind::Reads
            | RelationKind::Writes
            | RelationKind::DependsOn
            | RelationKind::Publishes
            | RelationKind::Subscribes
    )
}

/// Ownership lookups over a validated snapshot: `contains` and `exposes` parents.
pub struct Ownership<'a> {
    pub system: &'a str,
    entities: HashMap<&'a str, EntityKind>,
    relations: HashMap<&'a str, (RelationKind, &'a str, &'a str)>,
    parent: HashMap<&'a str, &'a str>,
}

impl<'a> Ownership<'a> {
    pub fn new(model: &'a Snapshot) -> Self {
        let entities = model
            .entities
            .iter()
            .map(|e| (e.id.as_str(), e.kind))
            .collect();
        let relations = model
            .relations
            .iter()
            .map(|r| {
                (
                    r.id.as_str(),
                    (r.kind, r.source.as_str(), r.target.as_str()),
                )
            })
            .collect();
        let parent = model
            .relations
            .iter()
            .filter(|r| matches!(r.kind, RelationKind::Contains | RelationKind::Exposes))
            .map(|r| (r.target.as_str(), r.source.as_str()))
            .collect();
        Self {
            system: &model.system_id,
            entities,
            relations,
            parent,
        }
    }

    pub fn entity(&self, id: &str) -> Option<EntityKind> {
        self.entities.get(id).copied()
    }

    pub fn relation(&self, id: &str) -> Option<(RelationKind, &'a str, &'a str)> {
        self.relations.get(id).copied()
    }

    /// The logical owner. Entities outside containment (external objects, deployments) are
    /// shown at the system level, so the system stands in as their owner.
    pub fn parent(&self, id: &str) -> Option<&'a str> {
        if id == self.system {
            return None;
        }
        Some(self.parent.get(id).copied().unwrap_or(self.system))
    }

    /// The entity itself, then each owner up to the system.
    pub fn lineage(&self, id: &'a str) -> Vec<&'a str> {
        let mut out = vec![id];
        let mut seen = HashSet::from([id]);
        while let Some(parent) = self.parent(out[out.len() - 1]) {
            if !seen.insert(parent) {
                break;
            }
            out.push(parent);
        }
        out
    }

    /// An interface stands for its owner when deciding where travel continues.
    pub fn place(&self, id: &'a str) -> &'a str {
        match self.entity(id) {
            Some(EntityKind::Interface) => self.parent.get(id).copied().unwrap_or(id),
            _ => id,
        }
    }

    /// The deepest entity that strictly owns every focus entity; the system if any focus is
    /// the system itself.
    pub fn common_owner(&self, focus: &[&'a str]) -> &'a str {
        let mut common: Option<Vec<&'a str>> = None;
        for id in focus {
            let owners = self.lineage(id).split_off(1);
            if owners.is_empty() {
                return self.system;
            }
            common = Some(match common {
                None => owners,
                Some(prev) => prev.into_iter().filter(|o| owners.contains(o)).collect(),
            });
        }
        common
            .and_then(|c| c.first().copied())
            .unwrap_or(self.system)
    }
}

struct Checked {
    resolved: Vec<ResolvedStep>,
    teaching: Vec<ResolvedTeachingView>,
    warnings: Vec<Diagnostic>,
}

fn check(script: &Script, model: &Snapshot) -> Result<Checked, ValidationError> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if script.model_id != model.model_id {
        push(
            &mut errors,
            "model_mismatch",
            "/model_id".into(),
            format!(
                "walkthrough is for model {} but {} is loaded",
                script.model_id, model.model_id
            ),
        );
    }
    if script.expected_revision != model.revision {
        push(
            &mut errors,
            "stale_revision",
            "/expected_revision".into(),
            format!(
                "walkthrough expects revision {} but revision {} is loaded; re-read the model and update the references",
                script.expected_revision, model.revision
            ),
        );
    }
    if !errors.is_empty() {
        return Err(ValidationError {
            diagnostics: errors,
        });
    }

    let own = Ownership::new(model);
    let mut view_ids = HashMap::new();
    let mut teaching = Vec::with_capacity(script.teaching_views.len());
    for (v, view) in script.teaching_views.iter().enumerate() {
        if view_ids.insert(view.id.as_str(), v).is_some() {
            push(
                &mut errors,
                "duplicate_teaching_view_id",
                format!("/teaching_views/{v}/id"),
                format!("teaching view ID {} is used twice", view.id),
            );
        }
        teaching.push(teaching::resolve(
            v,
            view,
            model,
            &own,
            &mut errors,
            &mut warnings,
        ));
    }
    let mut used_views = HashSet::new();
    let mut step_ids = HashSet::new();
    let mut resolved = Vec::with_capacity(script.steps.len());
    for (s, step) in script.steps.iter().enumerate() {
        let at = |rest: &str| format!("/steps/{s}{rest}");
        if !step_ids.insert(step.id.as_str()) {
            push(
                &mut errors,
                "duplicate_step_id",
                at("/id"),
                format!("step ID {} is used twice", step.id),
            );
        }
        if step.title.trim().is_empty() {
            push(
                &mut errors,
                "blank_text",
                at("/title"),
                "title is blank".into(),
            );
        }
        if is_numbered(&step.title) {
            push(
                &mut warnings,
                "numbered_title",
                at("/title"),
                "the viewer shows step progress and your chat headings number the steps; drop the number from the caption".into(),
            );
        }

        let view = match &step.view {
            None => None,
            Some(id) => match view_ids.get(id.as_str()) {
                Some(&v) => {
                    used_views.insert(v);
                    Some(v)
                }
                None => {
                    push(
                        &mut errors,
                        "missing_reference",
                        at("/view"),
                        format!("view {id} is not one of this walkthrough's teaching_views"),
                    );
                    None
                }
            },
        };
        // A step inside a teaching view can only use what that view shows.
        let shown = view.map(|v| &teaching[v]);
        let outside = |errors: &mut Vec<Diagnostic>, path: String, what: &str, id: &str| {
            if let Some(shown) = shown {
                push(
                    errors,
                    "outside_teaching_view",
                    path,
                    format!(
                        "{what} {id} is not in teaching view {}; add the object that holds it to the view's members, or play this step outside the view",
                        shown.id
                    ),
                );
            }
        };

        let mut focus = Vec::new();
        for (i, id) in step.focus.iter().enumerate() {
            match own.entity(id) {
                Some(_) if shown.is_some_and(|v| !v.entities.contains(id)) => {
                    outside(&mut errors, at(&format!("/focus/{i}")), "focus", id)
                }
                Some(_) => focus.push(id.as_str()),
                None => push(
                    &mut errors,
                    reference_code(&own, id),
                    at(&format!("/focus/{i}")),
                    format!("focus {id} is not an entity in this model"),
                ),
            }
        }
        if step.focus.len() > ADVISED_FOCUS {
            push(
                &mut warnings,
                "busy_step",
                at("/focus"),
                format!(
                    "{} focus objects is a lot to follow in one scene; consider splitting the step",
                    step.focus.len()
                ),
            );
        }

        let root = match &step.root {
            Some(_) if shown.is_some() => {
                push(
                    &mut errors,
                    "root_in_teaching_view",
                    at("/root"),
                    "a teaching view is shown whole; drop root from steps that play in it".into(),
                );
                None
            }
            _ if shown.is_some() => Some(own.system.to_owned()),
            Some(root) if own.entity(root).is_none() => {
                push(
                    &mut errors,
                    reference_code(&own, root),
                    at("/root"),
                    format!("root {root} is not an entity in this model"),
                );
                None
            }
            Some(root) => {
                for (i, id) in focus.iter().enumerate() {
                    if !own.lineage(id).contains(&root.as_str()) {
                        push(
                            &mut errors,
                            "focus_outside_root",
                            at(&format!("/focus/{i}")),
                            format!("{id} is not inside root {root}"),
                        );
                    }
                }
                Some(root.clone())
            }
            None => Some(own.common_owner(&focus).to_owned()),
        };

        let mut hops = Vec::new();
        let mut previous: Option<&str> = None;
        let mut requested = HashSet::new();
        for (i, hop) in step.trace.iter().enumerate() {
            let path = at(&format!("/trace/{i}"));
            let Some((kind, source, target)) = own.relation(&hop.relation) else {
                push(
                    &mut errors,
                    reference_code(&own, &hop.relation),
                    path,
                    format!("trace {} is not a relation in this model", hop.relation),
                );
                previous = None;
                continue;
            };
            if shown.is_some_and(|v| !v.relations.contains(&hop.relation)) {
                outside(&mut errors, path.clone(), "trace relation", &hop.relation);
            }
            if !is_flow(kind) {
                push(
                    &mut errors,
                    "invalid_trace_relation",
                    path,
                    format!(
                        "{} is {}; a trace only travels requests, reads, writes, depends_on, publishes, and subscribes",
                        hop.relation,
                        kind_name(kind)
                    ),
                );
                previous = None;
                continue;
            }
            if hop.returning && !matches!(kind, RelationKind::Requests | RelationKind::Reads) {
                push(
                    &mut errors,
                    "invalid_return",
                    format!("{path}/return"),
                    format!(
                        "{} is {}; only requests and reads have a response leg",
                        hop.relation,
                        kind_name(kind)
                    ),
                );
            } else if hop.returning && !requested.contains(hop.relation.as_str()) {
                // A response answers a request this trace already showed. Without one, a
                // return leg would only be a way to draw the relation backwards.
                push(
                    &mut errors,
                    "unpaired_return",
                    format!("{path}/return"),
                    format!(
                        "{} returns a response, but the trace has not travelled its request; trace the relation forward first, or model the direction the data actually moves",
                        hop.relation
                    ),
                );
            }
            if !hop.returning {
                requested.insert(hop.relation.as_str());
            }
            // A subscription points from subscriber to channel, but its payload travels the
            // other way, so the forward leg of a trace leaves from the channel.
            let (from, to) = if hop.returning != (kind == RelationKind::Subscribes) {
                (target, source)
            } else {
                (source, target)
            };
            if let Some(arrived) = previous
                && (own.place(arrived) != own.place(from)
                    || (arrived != from
                        && own.entity(arrived) == Some(EntityKind::Interface)
                        && own.entity(from) == Some(EntityKind::Interface)))
            {
                push(
                    &mut errors,
                    "trace_gap",
                    path.clone(),
                    format!(
                        "the trace reaches {arrived} but {} leaves from {from}; add the connecting relation (with evidence) to the model, or split the trace",
                        hop.relation
                    ),
                );
            }
            previous = Some(to);
            hops.push(ResolvedHop {
                relation: hop.relation.clone(),
                from: from.to_owned(),
                to: to.to_owned(),
                returning: hop.returning,
            });
        }
        if step.repeat_trace {
            match (hops.first(), hops.last()) {
                (Some(first), Some(last)) if own.place(&last.to) == own.place(&first.from) => {}
                (Some(_), Some(_)) => push(
                    &mut errors,
                    "open_loop",
                    at("/repeat_trace"),
                    "repeat_trace claims a loop, but the trace does not return to where it starts"
                        .into(),
                ),
                _ => push(
                    &mut errors,
                    "open_loop",
                    at("/repeat_trace"),
                    "repeat_trace needs a trace".into(),
                ),
            }
        }

        for (i, label) in step.labels.iter().enumerate() {
            if own.entity(&label.target).is_none() && own.relation(&label.target).is_none() {
                push(
                    &mut errors,
                    "missing_reference",
                    at(&format!("/labels/{i}/target")),
                    format!("label target {} is not in this model", label.target),
                );
            } else if shown.is_some_and(|v| {
                !v.entities.contains(&label.target) && !v.relations.contains(&label.target)
            }) {
                outside(
                    &mut errors,
                    at(&format!("/labels/{i}/target")),
                    "label target",
                    &label.target,
                );
            }
            if label.text.trim().is_empty() {
                push(
                    &mut errors,
                    "blank_text",
                    at(&format!("/labels/{i}/text")),
                    "label text is blank".into(),
                );
            }
        }

        resolved.push(ResolvedStep {
            root: root.unwrap_or_default(),
            hops,
            view,
        });
    }
    for (v, view) in script.teaching_views.iter().enumerate() {
        if !used_views.contains(&v) {
            push(
                &mut warnings,
                "unused_teaching_view",
                format!("/teaching_views/{v}"),
                format!("no step plays in teaching view {}", view.id),
            );
        }
    }
    // A tour that never leaves the whole-system view shows every scene with the rest of the
    // system dimmed around it. Advise opening the busiest real subsystem.
    if script.steps.len() >= OPENING_STEPS
        && resolved
            .iter()
            .all(|r| r.root == own.system && r.view.is_none())
    {
        let mut children: HashMap<&str, usize> = HashMap::new();
        for entity in &model.entities {
            if let Some(parent) = own.parent(&entity.id)
                && parent != own.system
                && own.entity(parent) == Some(EntityKind::Scope)
            {
                *children.entry(parent).or_default() += 1;
            }
        }
        if let Some((scope, count)) = children
            .into_iter()
            .filter(|(_, n)| *n >= 2)
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
        {
            push(
                &mut warnings,
                "never_opens",
                "/steps".into(),
                format!(
                    "every step shows the whole system; open a subsystem such as {scope} ({count} objects) by focusing only on objects inside it, or by setting root"
                ),
            );
        }
    }
    if script.steps.len() > ADVISED_STEPS {
        push(
            &mut warnings,
            "long_walkthrough",
            "/steps".into(),
            format!(
                "{} steps is long for one explanation; consider a shorter walkthrough and offering the rest as a follow-up",
                script.steps.len()
            ),
        );
    }

    if errors.is_empty() {
        Ok(Checked {
            resolved,
            teaching,
            warnings,
        })
    } else {
        Err(ValidationError {
            diagnostics: errors,
        })
    }
}

/// "1. Capture", "2) Ingest", "Step 3: Query", "4 · Recap": a caption that numbers itself.
fn is_numbered(title: &str) -> bool {
    let mut rest = title.trim_start();
    if rest
        .get(..5)
        .is_some_and(|w| w.eq_ignore_ascii_case("step "))
    {
        rest = rest[5..].trim_start();
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let after = rest[digits..].trim_start();
    after.starts_with(['.', ')', ':', '·', '-', '/'])
}

fn kind_name(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Contains => "contains",
        RelationKind::Exposes => "exposes",
        RelationKind::Requests => "requests",
        RelationKind::Reads => "reads",
        RelationKind::Writes => "writes",
        RelationKind::DependsOn => "depends_on",
        RelationKind::AppliesTo => "applies_to",
        RelationKind::ImplementedBy => "implemented_by",
        RelationKind::DeployedIn => "deployed_in",
        RelationKind::Publishes => "publishes",
        RelationKind::Subscribes => "subscribes",
    }
}

fn push(list: &mut Vec<Diagnostic>, code: &'static str, path: String, message: String) {
    if list.len() < MAX_DIAGNOSTICS {
        list.push(Diagnostic::new(code, path, message));
    }
}

fn reference_code(own: &Ownership, id: &str) -> &'static str {
    if own.entity(id).is_some() || own.relation(id).is_some() {
        "invalid_reference_kind"
    } else {
        "missing_reference"
    }
}
