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
