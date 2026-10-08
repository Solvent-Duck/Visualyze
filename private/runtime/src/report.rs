//! Machine-readable validation reports for agents. Advice is prose only: the
//! readability score and its thresholds stay private (ADR 0007).
use serde_json::{Value, json};
use visualyze_layout::advice::{AdviceLevel, ScopeAdvice};
use visualyze_model::{
    architecture::{Architecture, Diagnostic, EntityKind, RelationKind, ValidationError},
    walkthrough::Walkthrough,
};

/// Advice arrives noisiest first; an agent acts on the worst scopes first.
pub const MAX_REPORTED_ADVICE: usize = 25;

pub fn diagnostics(list: &[Diagnostic]) -> Value {
    Value::Array(
        list.iter()
            .map(|d| json!({"code": d.code, "path": d.path, "message": d.message}))
            .collect(),
    )
}

pub fn error(error: &ValidationError) -> Value {
    json!({"ok": false, "errors": diagnostics(&error.diagnostics)})
}

pub fn advice(advice: &[ScopeAdvice]) -> Value {
    Value::Array(
        advice
            .iter()
            .take(MAX_REPORTED_ADVICE)
            .map(|a| {
                json!({
                    "scope_id": a.scope_id,
                    "level": match a.level {
                        AdviceLevel::Consider => "consider",
                        AdviceLevel::Recommend => "recommend",
                        AdviceLevel::Strong => "strong",
                    },
                    "message": a.message,
                })
            })
            .collect(),
    )
}

/// A compact summary an agent can act on without re-reading its own model.
pub fn model(model: &Architecture) -> Value {
    let snapshot = model.snapshot();
    let top_level: Vec<Value> = snapshot
        .relations
        .iter()
        .filter(|r| r.kind == RelationKind::Contains && r.source == snapshot.system_id)
        .filter_map(|r| snapshot.entities.iter().find(|e| e.id == r.target))
        .map(|e| json!({"id": e.id, "label": e.label, "kind": kind(e.kind)}))
        .collect();
    json!({
        "ok": true,
        "model": {
            "model_id": snapshot.model_id,
            "revision": snapshot.revision,
            "system_id": snapshot.system_id,
            "source_revision": snapshot.snapshot.source_revision,
            "objects": snapshot.entities.len(),
            "relations": snapshot.relations.len(),
            "top_level": top_level,
        },
        "warnings": diagnostics(model.warnings()),
        "advice": advice(&visualyze_layout::advice::advise_snapshot(model)),
    })
}

pub fn walkthrough(walkthrough: &Walkthrough) -> Value {
    let script = walkthrough.script();
    json!({
        "ok": true,
        "walkthrough_id": script.id,
        "title": script.title,
        "steps": script.steps.iter().map(|s| json!({"id": s.id, "title": s.title})).collect::<Vec<_>>(),
        "warnings": diagnostics(walkthrough.warnings()),
    })
}

fn kind(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Scope => "scope",
        EntityKind::Client => "client",
        EntityKind::Service => "service",
        EntityKind::Datastore => "datastore",
        EntityKind::Interface => "interface",
        EntityKind::Concern => "concern",
        EntityKind::Deployment => "deployment",
    }
}
