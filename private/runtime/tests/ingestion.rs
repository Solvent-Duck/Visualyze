use std::{
    io::Write,
    process::{Command, Stdio},
};
use visualyze_model::architecture::OBJECTS_FIXTURE;

#[test]
fn validates_explicit_file_without_starting_gpu() {
    let result = Command::new(env!("CARGO_BIN_EXE_visualyze"))
        .args([
            "--validate",
            "--model",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../public/examples/objects.json"
            ),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("7 objects, 5 relations"));
    for path in [
        env!("CARGO_MANIFEST_DIR"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/nonexistent-model.json"),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_visualyze"))
            .args(["--validate", "--model", path])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Unable to load architecture"));
    }
}

#[test]
fn stdin_uses_same_validation_and_invalid_input_fails_before_window_creation() {
    for (input, success) in [(OBJECTS_FIXTURE, true), ("{}", false), ("not json", false)] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_visualyze"))
            .args(["--validate", "--model", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert_eq!(
            result.status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if !success {
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("Unable to load architecture")
            );
        }
    }
}

#[test]
fn notes_file_and_stdin_use_the_same_contract_with_view_options() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../public/examples/notes-crud.json"
    );
    let file = Command::new(env!("CARGO_BIN_EXE_visualyze"))
        .args(["--validate", "--view", "architecture", "--model", path])
        .output()
        .unwrap();
    assert!(
        file.status.success(),
        "{}",
        String::from_utf8_lossy(&file.stderr)
    );
    assert!(String::from_utf8_lossy(&file.stdout).contains("8 objects, 16 relations, 0 warnings"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_visualyze"))
        .args(["--validate", "--view", "gallery", "--model", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(include_bytes!("../../../public/examples/notes-crud.json"))
        .unwrap();
    let stdin = child.wait_with_output().unwrap();
    assert!(
        stdin.status.success(),
        "{}",
        String::from_utf8_lossy(&stdin.stderr)
    );
    assert_eq!(file.stdout, stdin.stdout);
}

/// A system holding one scope with `services` services, all in one layer.
fn wide_scope_json(services: usize) -> String {
    let mut entities = vec![
        r#"{"id":"system","kind":"scope","label":"System","boundary":"internal","subtype":"system","basis":"declared"}"#.to_owned(),
        r#"{"id":"wide","kind":"scope","label":"Wide scope","boundary":"internal","basis":"declared"}"#.to_owned(),
    ];
    let mut relations = vec![
        r#"{"id":"own-wide","kind":"contains","source":"system","target":"wide","basis":"declared"}"#
            .to_owned(),
    ];
    for i in 0..services {
        entities.push(format!(
            r#"{{"id":"svc{i}","kind":"service","label":"Service {i}","boundary":"internal","basis":"declared"}}"#
        ));
        relations.push(format!(
            r#"{{"id":"own-svc{i}","kind":"contains","source":"wide","target":"svc{i}","basis":"declared"}}"#
        ));
    }
    format!(
        r#"{{"schema_version":"0.1.0-draft.1","model_id":"wide","revision":1,"system_id":"system","snapshot":{{"id":"wide-001","source_revision":null,"coverage":"partial","scope_description":"wide","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
        entities.join(","),
        relations.join(",")
    )
}

fn validate_stdin(input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_visualyze"))
        .args(["--validate", "--model", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn wide_snapshot_scope_gets_advice_on_stderr_without_failing_or_changing_stdout() {
    let result = validate_stdin(&wide_scope_json(20));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "Validated wide revision 1: 22 objects, 21 relations, 0 warnings.\n"
    );
    let advice: Vec<_> = stderr
        .lines()
        .filter(|l| l.starts_with("Advice: "))
        .collect();
    assert_eq!(advice.len(), 1, "{stderr}");
    assert!(advice[0].starts_with("Advice: wide: Wide scope (wide) shows 20 parts"));
    assert!(advice[0].contains("Never invent groupings"));
}

#[test]
fn small_and_example_models_get_no_advice() {
    let small = validate_stdin(&wide_scope_json(6));
    assert!(small.status.success());
    assert!(!String::from_utf8_lossy(&small.stderr).contains("Advice"));
    let notes = Command::new(env!("CARGO_BIN_EXE_visualyze"))
        .args([
            "--validate",
            "--model",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../public/examples/notes-crud.json"
            ),
        ])
        .output()
        .unwrap();
    assert!(notes.status.success());
    assert!(!String::from_utf8_lossy(&notes.stderr).contains("Advice"));
}

#[test]
fn bundle_inventory_containers_get_no_advice() {
    let result = Command::new(env!("CARGO_BIN_EXE_visualyze"))
        .args([
            "--validate",
            "--bundle",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../layout/fixtures/distributed-ai.bundle.jsonl"
            ),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(
        String::from_utf8_lossy(&result.stdout).starts_with("Validated bundle synthetic:ai:"),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    // Its widest container is a datastore's tables: inventory, not architecture.
    assert!(!stderr.contains("Advice"), "{stderr}");
}
