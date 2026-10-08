use serde::Deserialize;
use serde_json::Value;
use visualyze_model::architecture::Architecture;
use visualyze_model::walkthrough::{CameraIntent, Context, Reveal, Walkthrough};

const MODEL: &str = include_str!("../../../examples/notes-crud.json");
const WALKTHROUGH: &str = include_str!("../../../examples/notes-crud.walkthrough.json");

#[derive(Deserialize)]
struct Case {
    name: String,
    base: String,
    expected_error: Option<String>,
    changes: Vec<Change>,
}

#[derive(Deserialize)]
struct Change {
    op: String,
    path: String,
    value: Option<Value>,
}

fn model() -> Architecture {
    Architecture::from_json(MODEL.as_bytes()).unwrap()
}

#[test]
fn shared_walkthrough_conformance() {
    let model = model();
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../../conformance/walkthrough-cases.json")).unwrap();
    for case in cases {
        assert_eq!(case.base, "notes-crud.walkthrough.json", "unknown fixture");
        let mut value: Value = serde_json::from_str(WALKTHROUGH).unwrap();
        for change in case.changes {
            let (parent, key) = change.path.rsplit_once('/').unwrap();
            let target = value.pointer_mut(parent).unwrap();
            match (target, change.op.as_str()) {
                (Value::Object(object), "add") => {
                    object.insert(key.to_owned(), change.value.unwrap_or(Value::Null));
                }
                (Value::Object(object), "remove") => {
                    assert!(object.remove(key).is_some());
                }
                (Value::Array(array), "add") if key == "-" => array.push(change.value.unwrap()),
                (Value::Array(array), "add") => {
                    array[key.parse::<usize>().unwrap()] = change.value.unwrap();
                }
                (Value::Array(array), "remove") => {
                    array.remove(key.parse::<usize>().unwrap());
                }
                _ => panic!("unsupported fixture operation"),
            }
        }
        let result = Walkthrough::from_json(value.to_string().as_bytes(), &model);
        match case.expected_error {
            None => {
                result.unwrap_or_else(|e| panic!("{}: {e}", case.name));
            }
            Some(code) => {
                let error = result.expect_err(&case.name);
                assert!(
                    error.diagnostics.iter().any(|e| e.code == code),
                    "{}: expected {code}, got {error}",
                    case.name
                );
            }
        }
    }
}

#[test]
fn applies_defaults_and_resolves_roots_and_travel() {
    let walkthrough = Walkthrough::from_json(WALKTHROUGH.as_bytes(), &model()).unwrap();
    let script = walkthrough.script();
    assert_eq!(script.steps.len(), walkthrough.resolved().len());
    let entry = &script.steps[1];
    assert_eq!(entry.context(), Context::Hide);
    assert_eq!(entry.reveal, Reveal::Step);
    assert_eq!(entry.camera, CameraIntent::FollowTrace);
    assert!(walkthrough.warnings().is_empty());

    // The system itself has no owner, so an overview of it is rooted at the system.
    assert_eq!(walkthrough.resolved()[0].root, "scope:notes");
    // The interface's home is its owning service, which sits in the system scope.
    assert_eq!(walkthrough.resolved()[1].root, "scope:notes");

    let hops = &walkthrough.resolved()[2].hops;
    let travel: Vec<_> = hops
        .iter()
        .map(|h| (h.from.as_str(), h.to.as_str()))
        .collect();
    assert_eq!(
        travel,
        [
            ("client:web", "interface:sessions"),
            ("service:notes", "store:notes"),
            ("store:notes", "service:notes"),
            ("interface:sessions", "client:web"),
        ]
    );
    assert!(hops[2].returning && !hops[1].returning);
}

#[test]
fn advises_on_long_walkthroughs_without_rejecting() {
    let mut value: Value = serde_json::from_str(WALKTHROUGH).unwrap();
    // This model is too small for a busy step; a long script exercises the same advice path.
    let step = value["steps"][3].clone();
    let steps = value["steps"].as_array_mut().unwrap();
    for i in 0..10 {
        let mut s = step.clone();
        s["id"] = Value::from(format!("more-{i}"));
        steps.push(s);
    }
    let walkthrough = Walkthrough::from_json(value.to_string().as_bytes(), &model()).unwrap();
    let codes: Vec<_> = walkthrough.warnings().iter().map(|w| w.code).collect();
    assert_eq!(codes, ["long_walkthrough"]);
}

#[test]
fn rejects_oversized_and_duplicate_key_input() {
    let model = model();
    let big = vec![b' '; 2 * 1024 * 1024 + 1];
    let error = Walkthrough::from_json(&big, &model).unwrap_err();
    assert_eq!(error.diagnostics[0].code, "payload_too_large");
    let duplicate = WALKTHROUGH.replacen("\"title\"", "\"title\": \"a\", \"title\"", 1);
    let error = Walkthrough::from_json(duplicate.as_bytes(), &model).unwrap_err();
    assert_eq!(error.diagnostics[0].code, "invalid_json");
}

const MESSAGING_MODEL: &str = include_str!("../../../examples/architecture-messaging.json");
const MESSAGING_WALKTHROUGH: &str =
    include_str!("../../../examples/architecture-messaging.walkthrough.json");

fn messaging_model(extra_topic: bool) -> Architecture {
    let mut value: Value = serde_json::from_str(MESSAGING_MODEL).unwrap();
    if extra_topic {
        value["entities"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": "interface:refunds-topic", "kind": "interface", "label": "Refunds topic",
                "boundary": "internal", "subtype": "topic", "basis": "declared"
            }));
        value["relations"].as_array_mut().unwrap().extend([
            serde_json::json!({"id": "rel:broker-exposes-refunds", "kind": "exposes",
                "source": "service:broker", "target": "interface:refunds-topic", "basis": "declared"}),
            serde_json::json!({"id": "rel:accounting-subscribes-refunds", "kind": "subscribes",
                "source": "service:accounting", "target": "interface:refunds-topic", "basis": "declared"}),
        ]);
    }
    Architecture::from_json(value.to_string().as_bytes()).unwrap()
}

#[test]
fn a_subscription_travels_from_the_channel_to_the_subscriber() {
    let walkthrough =
        Walkthrough::from_json(MESSAGING_WALKTHROUGH.as_bytes(), &messaging_model(false)).unwrap();
    let travel: Vec<_> = walkthrough.resolved()[1]
        .hops
        .iter()
        .map(|h| (h.from.as_str(), h.to.as_str()))
        .collect();
    assert_eq!(
        travel,
        [
            ("service:checkout", "interface:orders-topic"),
            ("interface:orders-topic", "service:accounting"),
            ("service:accounting", "store:ledger"),
        ]
    );
    assert!(walkthrough.warnings().is_empty());
}

#[test]
fn messaging_traces_reject_response_legs_and_topic_swaps() {
    let model = messaging_model(true);
    let mut value: Value = serde_json::from_str(MESSAGING_WALKTHROUGH).unwrap();
    value["steps"][1]["trace"][0]["return"] = Value::from(true);
    let error = Walkthrough::from_json(value.to_string().as_bytes(), &model).unwrap_err();
    assert!(error.diagnostics.iter().any(|d| d.code == "invalid_return"));

    // Publishing to one topic and being delivered from another is not a path.
    let mut value: Value = serde_json::from_str(MESSAGING_WALKTHROUGH).unwrap();
    value["steps"][1]["trace"][1]["relation"] = Value::from("rel:accounting-subscribes-refunds");
    let error = Walkthrough::from_json(value.to_string().as_bytes(), &model).unwrap_err();
    assert!(error.diagnostics.iter().any(|d| d.code == "trace_gap"));

    // The same swap through the broker as a whole is not a distinct-channel claim.
    let mut value: Value = serde_json::from_str(MESSAGING_WALKTHROUGH).unwrap();
    value["steps"][1]["trace"][1]["relation"] = Value::from("rel:notifier-subscribes-broker");
    value["steps"][1]["trace"]
        .as_array_mut()
        .unwrap()
        .truncate(2);
    assert!(Walkthrough::from_json(value.to_string().as_bytes(), &model).is_ok());
}
