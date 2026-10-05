use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use visualyze_model::provider::{Basis, BundleRecord, EntityKind, RelationKind};
use visualyze_rust_bundle::{Options, produce};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "visualyze-producer-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn options(output: PathBuf) -> Options {
    Options {
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nested"),
        output,
        revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        repository: "https://github.com/example/fixture".to_owned(),
        name: "Nested Rust fixture".to_owned(),
        model_id: None,
        model_revision: None,
        identity_scheme: Default::default(),
    }
}

fn records(path: &PathBuf) -> Vec<BundleRecord> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn ingests_real_nested_source_and_preserves_claim_meaning() {
    let temporary = Temporary::new();
    let options = options(temporary.0.join("model.jsonl"));
    let report = produce(&options).unwrap();
    assert_eq!(report.packages, 2);
    assert_eq!(report.rust_files, 4);
    assert_eq!(report.parsed_files, 4);
    assert_eq!(report.manifest_references, 2);
    assert_eq!(report.omitted_external_dependencies, 1);
    assert!(report.import_references >= 3);
    assert_eq!(report.unresolved_imports, 1);
    let records = records(&options.output);
    let entity = |label: &str| {
        records
            .iter()
            .find_map(|r| {
                if let BundleRecord::Entity(e) = r {
                    (e.summary.label == label).then_some(&e.summary)
                } else {
                    None
                }
            })
            .unwrap()
    };
    let application = entity("Application");
    let start = entity("Application::start");
    assert_eq!(application.kind, EntityKind::CodeType);
    assert_eq!(start.kind, EntityKind::CodeFunction);
    assert!(records.iter().any(|r| matches!(r, BundleRecord::Relation(r) if r.kind == RelationKind::OwnsCode && r.source == application.id && r.target == start.id)));
    assert_eq!(entity("inner::State").kind, EntityKind::CodeType);
    assert!(
        records
            .iter()
            .filter_map(|r| if let BundleRecord::Relation(r) = r {
                Some(r)
            } else {
                None
            })
            .all(|r| matches!(
                r.kind,
                RelationKind::Contains | RelationKind::OwnsCode | RelationKind::CodeReferences
            ))
    );
    assert!(records.iter().any(|r| matches!(r, BundleRecord::Relation(r) if r.label.as_ref().is_some_and(|s| s.starts_with("Syntactic import:")) && r.claim.basis == Basis::Inferred && r.claim.rationale.is_some())));
    assert!(records.iter().any(|r| matches!(r, BundleRecord::Evidence(e) if e.reference.as_ref().is_some_and(|s| s.contains("/blob/0123456789abcdef0123456789abcdef01234567/crates/app/src/lib.rs#L")))));
}

#[test]
fn output_is_deterministic_and_existing_files_are_preserved() {
    let temporary = Temporary::new();
    let first = options(temporary.0.join("a.jsonl"));
    let second = options(temporary.0.join("b.jsonl"));
    produce(&first).unwrap();
    produce(&second).unwrap();
    let original = fs::read(&first.output).unwrap();
    assert_eq!(original, fs::read(&second.output).unwrap());
    assert!(
        produce(&first)
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    assert_eq!(original, fs::read(&first.output).unwrap());
}

#[test]
fn default_identity_remains_source_specific_at_publication_revision_one() {
    let temporary = Temporary::new();
    let options = options(temporary.0.join("model.jsonl"));
    let report = produce(&options).unwrap();
    assert_eq!(report.model_revision, 1);
    assert_eq!(report.source_revision, options.revision);
    assert_eq!(
        report.model_id,
        visualyze_rust_bundle::stable_id(
            "model",
            &format!("{}:{}", options.repository, options.revision)
        )
    );
    assert!(
        matches!(&records(&options.output)[0], BundleRecord::Manifest(m)
        if m.model_id == report.model_id && m.revision == 1)
    );
}

#[test]
fn publication_identity_is_paired_valid_and_positive_before_reading_source() {
    let temporary = Temporary::new();
    for (id, revision, error) in [
        (Some("code:fixture"), None, "supplied together"),
        (None, Some(1), "supplied together"),
        (Some("code:fixture"), Some(0), "positive integer"),
        (Some(""), Some(1), "--model-id requires"),
        (Some("Code:fixture"), Some(1), "--model-id requires"),
        (Some("1fixture"), Some(1), "--model-id requires"),
        (Some("code/fixture"), Some(1), "--model-id requires"),
        (Some("code:fixturé"), Some(1), "--model-id requires"),
    ] {
        let mut options = options(temporary.0.join("model.jsonl"));
        options.root = temporary.0.join("absent-source");
        options.model_id = id.map(str::to_owned);
        options.model_revision = revision;
        assert!(produce(&options).unwrap_err().to_string().contains(error));
        assert!(!options.output.exists());
    }
    let mut options = options(temporary.0.join("model.jsonl"));
    options.model_id = Some("a".repeat(129));
    options.model_revision = Some(1);
    assert!(
        produce(&options)
            .unwrap_err()
            .to_string()
            .contains("--model-id requires")
    );
    assert!(!options.output.exists());
}

#[test]
fn increasing_publications_preserve_package_and_file_ids_with_new_source_evidence() {
    let temporary = Temporary::new();
    let mut first = options(temporary.0.join("first.jsonl"));
    first.model_id = Some("code:fixture.stable_v1-2".to_owned());
    first.model_revision = Some(7);
    let mut second = options(temporary.0.join("second.jsonl"));
    second.model_id = first.model_id.clone();
    second.model_revision = Some(8);
    // Distinct caller-supplied provenance tokens; Git verification is outside the producer.
    second.revision = "abcdef0123456789abcdef0123456789abcdef01".to_owned();
    let first_report = produce(&first).unwrap();
    let second_report = produce(&second).unwrap();
    for (options, report) in [(&first, &first_report), (&second, &second_report)] {
        assert_eq!(report.model_id, options.model_id.as_deref().unwrap());
        assert_eq!(report.model_revision, options.model_revision.unwrap());
        assert_eq!(report.source_revision, options.revision);
        let values = records(&options.output);
        assert!(matches!(&values[0], BundleRecord::Manifest(m)
            if m.model_id == report.model_id && m.revision == report.model_revision));
        assert!(
            values
                .iter()
                .filter_map(|record| match record {
                    BundleRecord::Evidence(e) => e.reference.as_ref(),
                    _ => None,
                })
                .all(|reference| reference.contains(&format!("/blob/{}/", options.revision)))
        );
    }
    let package_and_file_ids = |options: &Options| {
        records(&options.output)
            .into_iter()
            .filter_map(|record| match record {
                BundleRecord::Entity(e)
                    if matches!(
                        e.summary.kind,
                        EntityKind::CodePackage | EntityKind::CodeFile
                    ) =>
                {
                    Some((e.summary.label, e.summary.id))
                }
                _ => None,
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let mut legacy = options(temporary.0.join("legacy.jsonl"));
    legacy.revision = first.revision.clone();
    produce(&legacy).unwrap();
    assert_eq!(package_and_file_ids(&first), package_and_file_ids(&second));
    assert_eq!(package_and_file_ids(&first), package_and_file_ids(&legacy));
    let serialized_report = serde_json::to_value(second_report).unwrap();
    assert_eq!(serialized_report["model_revision"], 8);
    assert_eq!(serialized_report["source_revision"], second.revision);
}

#[test]
fn cli_accepts_paired_publication_flags_and_rejects_non_integer_revision() {
    let temporary = Temporary::new();
    let options = options(temporary.0.join("cli.jsonl"));
    let command = |revision: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_visualyze-rust-bundle"))
            .arg("--root")
            .arg(&options.root)
            .arg("--output")
            .arg(&options.output)
            .arg("--revision")
            .arg(&options.revision)
            .arg("--repository")
            .arg(&options.repository)
            .args(["--model-id", "code:fixture", "--model-revision", revision])
            .output()
            .unwrap()
    };
    for invalid in ["-1", "1.5", "18446744073709551616"] {
        let output = command(invalid);
        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("--model-revision")
        );
        assert!(!options.output.exists());
    }
    let output = command("2");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["model_id"], "code:fixture");
    assert_eq!(report["model_revision"], 2);
    assert_eq!(report["source_revision"], options.revision);
}

#[test]
fn malformed_rust_is_reported_without_fabricated_declarations() {
    let temporary = Temporary::new();
    let root = temporary.0.join("source");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='broken-fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("invalid.rs"), "fn {{ deliberately invalid Rust").unwrap();
    let mut options = options(temporary.0.join("model.jsonl"));
    options.root = root;
    let report = produce(&options).unwrap();
    assert_eq!(report.parsed_files, 0);
    assert_eq!(report.declarations, 0);
    assert_eq!(report.unparsed_files.len(), 1);
    assert_eq!(report.entities, 4); // scope, repository, package, supplied file
}

#[cfg(unix)]
#[test]
fn rejects_source_symlinks_without_reading_outside_root() {
    let temporary = Temporary::new();
    let root = temporary.0.join("source");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.join("outside.rs")).unwrap();
    let mut options = options(temporary.0.join("model.jsonl"));
    options.root = root;
    assert!(
        produce(&options)
            .unwrap_err()
            .to_string()
            .contains("symlink forbidden")
    );
    assert!(!options.output.exists());
}

#[test]
fn rejects_unpinned_provenance() {
    let temporary = Temporary::new();
    let mut options = options(temporary.0.join("model.jsonl"));
    options.revision = "main".to_owned();
    assert!(
        produce(&options)
            .unwrap_err()
            .to_string()
            .contains("full forty-character")
    );
}

#[test]
fn records_cargo_self_dependencies_as_explicit_omissions() {
    let temporary = Temporary::new();
    let root = temporary.0.join("source");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n[dev-dependencies]\nfixture={path='.'}\n",
    )
    .unwrap();
    fs::write(root.join("lib.rs"), "pub fn entry() {}\n").unwrap();
    let mut options = options(temporary.0.join("model.jsonl"));
    options.root = root;
    let report = produce(&options).unwrap();
    assert_eq!(report.omitted_self_dependencies, 1);
    assert_eq!(report.manifest_references, 0);
    assert!(
        records(&options.output)
            .iter()
            .all(|r| !matches!(r, BundleRecord::Relation(r) if r.source == r.target))
    );
}

#[cfg(unix)]
#[test]
fn reports_non_source_symlink_without_following_it() {
    let temporary = Temporary::new();
    let root = temporary.0.join("source");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("/does-not-exist", root.join("assets")).unwrap();
    let mut options = options(temporary.0.join("model.jsonl"));
    options.root = root;
    let report = produce(&options).unwrap();
    assert_eq!(report.skipped_symlinks, ["assets"]);
}

#[test]
fn rejects_oversized_source_before_creating_output() {
    let temporary = Temporary::new();
    let root = temporary.0.join("source");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::File::create(root.join("oversized.rs"))
        .unwrap()
        .set_len(8 * 1024 * 1024 + 1)
        .unwrap();
    let mut options = options(temporary.0.join("model.jsonl"));
    options.root = root;
    assert!(
        produce(&options)
            .unwrap_err()
            .to_string()
            .contains("exceeds 8 MiB")
    );
    assert!(!options.output.exists());
}
