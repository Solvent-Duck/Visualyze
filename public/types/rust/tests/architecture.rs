use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, Cursor, Read};
use visualyze_model::architecture::{Architecture, MAX_BYTES, MAX_DIAGNOSTICS, OBJECTS_FIXTURE};

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

#[test]
fn shared_contract_conformance() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../../conformance/architecture-cases.json")).unwrap();
    for case in cases {
        let fixture = match case.base.as_str() {
            "objects.json" => OBJECTS_FIXTURE,
            "architecture.json" => include_str!("../../../examples/architecture.json"),
            "architecture-messaging.json" => {
                include_str!("../../../examples/architecture-messaging.json")
            }
            "architecture-minimal.json" => {
                include_str!("../../../examples/architecture-minimal.json")
            }
            _ => panic!("unknown fixture"),
        };
        let mut value: Value = serde_json::from_str(fixture).unwrap();
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
                (Value::Array(array), "remove") => {
                    array.remove(key.parse::<usize>().unwrap());
                }
                _ => panic!("unsupported fixture operation"),
            }
        }
        let result = Architecture::from_json(value.to_string().as_bytes());
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
fn bounded_reader_stops_after_limit_and_preserves_io_errors() {
    let mut input = Cursor::new(vec![b' '; MAX_BYTES + 1000]);
    let error = Architecture::from_reader(&mut input).unwrap_err();
    assert_eq!(error.diagnostics[0].code, "payload_too_large");
    assert_eq!(input.position(), (MAX_BYTES + 1) as u64);
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("fixture read failed"))
        }
    }
    assert_eq!(
        Architecture::from_reader(Broken).unwrap_err().diagnostics[0].code,
        "input_io"
    );
    assert_eq!(
        Architecture::from_reader(Cursor::new(OBJECTS_FIXTURE))
            .unwrap()
            .snapshot()
            .entities
            .len(),
        7
    );
}

#[test]
fn rejects_malformed_duplicate_keys_trailing_data_and_excessive_depth() {
    for input in [
        b"not json".as_slice(),
        b"\xff",
        b"{} {}",
        b"{\"x\":1,\"x\":2}",
        b"{\"x\":1,\"\\u0078\":2}",
        b"{\"nested\":{\"x\":1,\"x\":2}}",
    ] {
        assert_eq!(
            Architecture::from_json(input).unwrap_err().diagnostics[0].code,
            "invalid_json"
        );
    }
    let deep = format!("{}0{}", "[".repeat(17), "]".repeat(17));
    let error = Architecture::from_json(deep.as_bytes()).unwrap_err();
    assert!(error.to_string().contains("nesting"));
    let boundary = format!("{}0{}", "[".repeat(16), "]".repeat(16));
    assert_eq!(
        Architecture::from_json(boundary.as_bytes())
            .unwrap_err()
            .diagnostics[0]
            .code,
        "schema_violation"
    );
}

#[test]
fn schema_integers_accept_integral_json_numbers() {
    let input = OBJECTS_FIXTURE.replace("\"revision\": 1,", "\"revision\": 1.0,");
    assert_eq!(
        Architecture::from_json(input.as_bytes())
            .unwrap()
            .snapshot()
            .revision,
        1
    );
}

#[test]
fn bounds_counts_and_number_of_diagnostics() {
    let mut value: Value = serde_json::from_str(OBJECTS_FIXTURE).unwrap();
    value["entities"] = json!(vec![value["entities"][0].clone(); 513]);
    assert_eq!(
        Architecture::from_json(value.to_string().as_bytes())
            .unwrap_err()
            .diagnostics[0]
            .code,
        "schema_violation"
    );
    value["entities"] = json!(vec![json!({}); 100]);
    let error = Architecture::from_json(value.to_string().as_bytes()).unwrap_err();
    assert_eq!(error.diagnostics.len(), MAX_DIAGNOSTICS);
    assert!(
        error
            .diagnostics
            .iter()
            .all(|d| d.path.starts_with("/entities/"))
    );
}

#[test]
fn retains_claims_and_warns_for_generic_forms_and_duplicate_claims() {
    let mut value: Value = serde_json::from_str(OBJECTS_FIXTURE).unwrap();
    value["entities"][1]["subtype"] = json!("custom");
    value["entities"][1]["roles"] = json!(["custom"]);
    for id in ["rel:read-one", "rel:read-two"] {
        value["relations"].as_array_mut().unwrap().push(json!({"id":id,"kind":"reads","source":"service:api","target":"store:users","basis":"declared"}));
    }
    let model = Architecture::from_json(value.to_string().as_bytes()).unwrap();
    let codes: Vec<_> = model.warnings().iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        ["generic_subtype", "descriptive_role", "duplicate_claim"]
    );
    assert_eq!(model.snapshot().relations.len(), 7);
}

#[test]
fn notes_crud_design_keeps_one_service_and_explicit_policy_coverage() {
    use visualyze_model::architecture::{Basis, EntityKind as Kind, RelationKind as Link};
    let model =
        Architecture::from_json(include_bytes!("../../../examples/notes-crud.json")).unwrap();
    assert!(model.warnings().is_empty());
    let snapshot = model.snapshot();
    assert_eq!(snapshot.entities.len(), 8);
    assert_eq!(snapshot.relations.len(), 16);
    assert_eq!(
        snapshot
            .entities
            .iter()
            .filter(|e| e.kind == Kind::Service)
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .entities
            .iter()
            .filter(|e| e.kind == Kind::Datastore)
            .count(),
        1
    );
    assert_eq!(
        snapshot
            .entities
            .iter()
            .filter(|e| e.kind == Kind::Concern)
            .count(),
        2
    );
    assert!(snapshot.entities.iter().all(|e| e.basis == Basis::Declared));
    assert!(
        snapshot
            .relations
            .iter()
            .all(|r| r.basis == Basis::Declared)
    );
    let coverage: Vec<_> = snapshot
        .relations
        .iter()
        .filter(|r| r.kind == Link::AppliesTo)
        .collect();
    assert_eq!(coverage.len(), 3);
    assert!(coverage.iter().all(|r| r.target != "store:notes"));
    assert!(
        snapshot
            .relations
            .iter()
            .filter(|r| r.kind == Link::ImplementedBy)
            .all(|r| r.target == "service:notes")
    );
}

const MESSAGING: &str = include_str!("../../../examples/architecture-messaging.json");

#[test]
fn messaging_vocabulary_is_recognized_without_warnings() {
    let model = Architecture::from_json(MESSAGING.as_bytes()).unwrap();
    assert!(model.warnings().is_empty(), "{:?}", model.warnings());
    let unknown = MESSAGING.replace("\"broker\"", "\"mailbox\"");
    let model = Architecture::from_json(unknown.as_bytes()).unwrap();
    assert!(model.warnings().iter().any(|w| w.code == "generic_subtype"));
}

#[test]
fn supported_claims_without_a_line_range_are_flagged() {
    let mut value: Value = serde_json::from_str(MESSAGING).unwrap();
    value["evidence"] = json!([
        {"id": "ev:file", "description": "whole file", "locator": {"kind": "source", "path": "compose.yaml"}},
        {"id": "ev:ref", "description": "opaque", "locator": {"kind": "reference", "value": "wiki"}},
        {"id": "ev:lines", "description": "ranged", "locator": {"kind": "source", "path": "compose.yaml", "start_line": 3, "end_line": 9}},
        {"id": "ev:declared", "description": "cited by a declared claim", "locator": {"kind": "reference", "value": "wiki"}}
    ]);
    for (i, evidence) in [(7, "ev:file"), (8, "ev:ref"), (9, "ev:lines")] {
        let relation = &mut value["relations"][i];
        relation["basis"] = json!("supported");
        relation["evidence_ids"] = json!([evidence]);
    }
    value["relations"][10]["evidence_ids"] = json!(["ev:declared"]);
    let model = Architecture::from_json(value.to_string().as_bytes()).unwrap();
    let flagged: Vec<_> = model
        .warnings()
        .iter()
        .filter(|w| w.code == "coarse_evidence")
        .map(|w| w.path.as_str())
        .collect();
    assert_eq!(flagged, ["/evidence/0", "/evidence/1"]);
}
