//! Teaching views (ADR 0010): validation against real systems, and the temporary
//! flattened snapshot the viewer lays out.
use std::collections::BTreeSet;

use serde_json::{Value, json};
use visualyze_model::architecture::{Architecture, RelationKind};
use visualyze_model::walkthrough::{SCHEMA_VERSION, TeachingView, Walkthrough, teaching_model};

const OTEL: &str = include_str!("../../../examples/otel-demo.json");
const SYSTEMS: [&str; 3] = [
    OTEL,
    include_str!("../../../examples/posthog.json"),
    include_str!("../../../examples/sentry.json"),
];

/// The otel-demo AI assistant path: members drawn from three different owners.
const ASSISTANT: [&str; 5] = [
    "service:frontend-proxy",
    "service:chatbot",
    "service:agent",
    "service:mcp",
    "service:llm",
];

fn otel() -> Architecture {
    Architecture::from_json(OTEL.as_bytes()).unwrap()
}

fn script(model: &Architecture, members: &[&str], step: Value) -> Value {
    let snapshot = model.snapshot();
    json!({
        "schema_version": SCHEMA_VERSION,
        "id": "walk:assistant",
        "model_id": snapshot.model_id,
        "expected_revision": snapshot.revision,
        "title": "The assistant loop",
        "teaching_views": [{"id": "tv:assistant", "title": "AI assistant", "members": members}],
        "steps": [step],
    })
}

fn validate(model: &Architecture, script: &Value) -> Result<Walkthrough, Vec<&'static str>> {
    Walkthrough::from_json(script.to_string().as_bytes(), model)
        .map_err(|e| e.diagnostics.iter().map(|d| d.code).collect())
}

fn assistant_step() -> Value {
    json!({
        "id": "loop",
        "title": "The agent calls its tools",
        "view": "tv:assistant",
        "focus": ["service:agent", "service:mcp"],
        "trace": [
            {"relation": "rel:requests-chatbot--agent"},
            {"relation": "rel:requests-agent--mcp"},
            {"relation": "rel:requests-agent--mcp", "return": true},
        ],
    })
}

#[test]
fn a_cross_scope_view_validates_and_reports_what_it_leaves_out() {
    let model = otel();
    let walkthrough = validate(&model, &script(&model, &ASSISTANT, assistant_step())).unwrap();
    assert_eq!(walkthrough.resolved()[0].view, Some(0));
    assert_eq!(walkthrough.resolved()[0].root, model.snapshot().system_id);
    let view = &walkthrough.teaching_views()[0];
    for member in ASSISTANT {
        assert!(view.entities.contains(member), "{member}");
    }
    for relation in [
        "rel:requests-frontend-proxy--chatbot",
        "rel:requests-chatbot--agent",
        "rel:requests-agent--mcp",
        "rel:requests-agent--llm",
    ] {
        assert!(view.relations.contains(relation), "{relation}");
    }
    // The agent's calls to the frontend, flagd, and the collector leave the view.
    assert!(
        view.outside_connections >= 3,
        "{}",
        view.outside_connections
    );
    assert_eq!(view.model_entities, model.snapshot().entities.len());
}

#[test]
fn steps_in_a_view_stay_inside_it() {
    let model = otel();
    let cases = [
        (
            json!({"id": "s", "title": "t", "view": "tv:assistant", "focus": ["service:flagd"]}),
            "outside_teaching_view",
        ),
        (
            json!({"id": "s", "title": "t", "view": "tv:assistant", "focus": ["service:agent"],
                   "trace": [{"relation": "rel:requests-agent--flagd"}]}),
            "outside_teaching_view",
        ),
        (
            json!({"id": "s", "title": "t", "view": "tv:assistant", "focus": ["service:agent"],
                   "labels": [{"target": "service:frontend", "text": "x"}]}),
            "outside_teaching_view",
        ),
        (
            json!({"id": "s", "title": "t", "view": "tv:assistant", "root": "scope:ai-assistant",
                   "focus": ["service:agent"]}),
            "root_in_teaching_view",
        ),
        (
            json!({"id": "s", "title": "t", "view": "tv:missing", "focus": ["service:agent"]}),
            "missing_reference",
        ),
    ];
    for (step, code) in cases {
        let errors = validate(&model, &script(&model, &ASSISTANT, step.clone())).unwrap_err();
        assert!(errors.contains(&code), "{step}: {errors:?}");
    }
}

#[test]
fn members_must_be_flat_and_name_real_objects() {
    let model = otel();
    let step = json!({"id": "s", "title": "t", "view": "tv:assistant", "focus": ["service:agent"]});
    for (members, code) in [
        (
            vec!["scope:ai-assistant", "service:agent"],
            "nested_teaching_member",
        ),
        (vec!["service:agent", "service:nope"], "missing_reference"),
        (
            vec!["scope:otel-demo", "service:agent"],
            "teaching_member_system",
        ),
    ] {
        let errors = validate(&model, &script(&model, &members, step.clone())).unwrap_err();
        assert!(errors.contains(&code), "{members:?}: {errors:?}");
    }
}

#[test]
fn unused_and_busy_views_are_advice_not_errors() {
    let model = otel();
    let entities: Vec<&str> = model
        .snapshot()
        .entities
        .iter()
        .filter(|e| e.kind.as_str() == "service")
        .map(|e| e.id.as_str())
        .collect();
    assert!(entities.len() > visualyze_model::walkthrough::ADVISED_MEMBERS);
    let step = json!({"id": "s", "title": "t", "focus": ["service:agent"]});
    let walkthrough = validate(&model, &script(&model, &entities, step)).unwrap();
    let codes: Vec<_> = walkthrough.warnings().iter().map(|w| w.code).collect();
    assert!(codes.contains(&"busy_teaching_view"), "{codes:?}");
    assert!(codes.contains(&"unused_teaching_view"), "{codes:?}");
}

#[test]
fn the_flattened_snapshot_keeps_identity_and_cites_only_the_model() {
    let model = otel();
    let full = model.snapshot();
    let view = TeachingView {
        id: "tv:assistant".into(),
        title: "AI assistant".into(),
        members: ASSISTANT.iter().map(|s| s.to_string()).collect(),
        layout: Default::default(),
    };
    let flat = teaching_model(&model, &view).unwrap();
    let snapshot = flat.snapshot();
    assert_eq!(snapshot.model_id, full.model_id);
    assert_eq!(snapshot.revision, full.revision);
    assert_eq!(snapshot.system_id, full.system_id);

    let original: BTreeSet<_> = full.relations.iter().map(|r| r.id.as_str()).collect();
    for relation in &snapshot.relations {
        if original.contains(relation.id.as_str()) {
            assert_ne!(relation.kind, RelationKind::Contains, "{}", relation.id);
            continue;
        }
        // The only added records place members directly in the system, and say so.
        assert_eq!(relation.kind, RelationKind::Contains, "{}", relation.id);
        assert_eq!(relation.source, full.system_id);
        assert!(ASSISTANT.contains(&relation.target.as_str()));
        assert!(
            relation
                .rationale
                .as_deref()
                .unwrap()
                .contains("Teaching view")
        );
    }
    for entity in &snapshot.entities {
        assert!(
            entity.id == full.system_id
                || ASSISTANT.contains(&entity.id.as_str())
                || entity.kind.as_str() == "interface",
            "{}",
            entity.id
        );
    }
}

/// Every real system: each scope's children, lifted out as a teaching view, produce a
/// valid flattened model.
#[test]
fn every_real_scope_flattens_into_a_valid_model() {
    for json in SYSTEMS {
        let model = Architecture::from_json(json.as_bytes()).unwrap();
        let full = model.snapshot();
        let mut checked = 0;
        for scope in full.entities.iter().filter(|e| e.kind.as_str() == "scope") {
            let members: Vec<String> = full
                .relations
                .iter()
                .filter(|r| r.kind == RelationKind::Contains && r.source == scope.id)
                .map(|r| r.target.clone())
                .collect();
            if members.len() < 2 || scope.id == full.system_id {
                continue;
            }
            let view = TeachingView {
                id: "tv:scope".into(),
                title: "Scope".into(),
                members,
                layout: Default::default(),
            };
            teaching_model(&model, &view)
                .unwrap_or_else(|e| panic!("{} {}: {e}", full.model_id, scope.id));
            checked += 1;
        }
        assert!(
            checked > 0,
            "{} has no nested scope to check",
            full.model_id
        );
    }
}
