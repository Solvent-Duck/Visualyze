use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use visualyze_model::provider::{BundleRecord, EntityKind, RelationKind};
use visualyze_rust_bundle::{IdentityScheme, Options, produce, stable_id};

static COUNTER: AtomicUsize = AtomicUsize::new(0);
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
const NEXT_REVISION: &str = "abcdef0123456789abcdef0123456789abcdef01";

struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "visualyze-syntax-identity-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        fs::create_dir_all(directory.join("source/src")).unwrap();
        fs::write(
            directory.join("source/Cargo.toml"),
            "[package]\nname='identity-fixture'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(
            directory.join("source/src/helper.rs"),
            "pub struct A;\npub struct B;\n",
        )
        .unwrap();
        let fixture = Self(directory);
        fixture.edit(source);
        fixture
    }

    fn edit(&self, source: &str) {
        fs::write(self.0.join("source/src/lib.rs"), source).unwrap();
    }

    fn options(&self, output: &str) -> Options {
        Options {
            root: self.0.join("source"),
            output: self.0.join(output),
            revision: REVISION.to_owned(),
            repository: "https://github.com/example/identity-fixture".to_owned(),
            name: "Identity fixture".to_owned(),
            model_id: None,
            model_revision: None,
            identity_scheme: IdentityScheme::SyntaxV2,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn records(path: &Path) -> Vec<BundleRecord> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn entity_ids(
    records: &[BundleRecord],
    kinds: &[EntityKind],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut result: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for record in records {
        if let BundleRecord::Entity(entity) = record
            && kinds.contains(&entity.summary.kind)
        {
            assert!(
                result
                    .entry(entity.summary.label.clone())
                    .or_default()
                    .insert(entity.summary.id.clone())
            );
        }
    }
    result
}

fn relation_ids(records: &[BundleRecord], kind: RelationKind) -> BTreeSet<String> {
    records
        .iter()
        .filter_map(|record| match record {
            BundleRecord::Relation(relation) if relation.kind == kind => Some(relation.id.clone()),
            _ => None,
        })
        .collect()
}

fn declaration_reference(records: &[BundleRecord], label: &str) -> String {
    let evidence = records
        .iter()
        .find_map(|record| match record {
            BundleRecord::Entity(entity) if entity.summary.label == label => {
                Some(&entity.summary.claim.evidence_ids[0])
            }
            _ => None,
        })
        .unwrap();
    records
        .iter()
        .find_map(|record| match record {
            BundleRecord::Evidence(record) if &record.id == evidence => record.reference.clone(),
            _ => None,
        })
        .unwrap()
}

#[test]
fn legacy_default_preserves_pre_change_fixture_bytes() {
    let fixture = Fixture::new("");
    let options = Options {
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nested"),
        output: fixture.0.join("legacy.jsonl"),
        revision: REVISION.to_owned(),
        repository: "https://github.com/example/fixture".to_owned(),
        name: "Nested Rust fixture".to_owned(),
        model_id: None,
        model_revision: None,
        identity_scheme: IdentityScheme::default(),
    };
    let report = produce(&options).unwrap();
    let bundle = fs::read_to_string(options.output).unwrap();
    // Captured from the original producer before adding syntax-v2, not regenerated
    // from an expected implementation path. This guards the published byte format.
    assert_eq!(bundle.len(), 23_005);
    assert_eq!(stable_id("golden", &bundle), "golden:09a4f6f4d9f1ff2a");
    assert_eq!(report.identity_scheme, IdentityScheme::LegacyV1);
    assert_eq!(
        serde_json::to_value(report).unwrap()["identity_scheme"],
        "legacy-v1"
    );
}

#[test]
fn source_edits_preserve_items_ownership_and_import_claims_but_update_evidence() {
    let fixture = Fixture::new(
        "mod helper;\nuse crate::helper::{A, B as Renamed};\npub struct Container;\nimpl Container { pub fn run(&self) -> u32 { 1 } }\nfn entry() {}\nmod child { pub fn nested() {} }\n",
    );
    let mut first = fixture.options("first.jsonl");
    first.model_id = Some("code:fixture-syntax-v2".into());
    first.model_revision = Some(1);
    let first_report = produce(&first).unwrap();
    let before = records(&first.output);
    let edited = r#"// A leading comment changes all old line numbers.

fn unrelated() {}
mod child {
    /// Documentation is presentation, not identity.
    pub fn nested() { let changed_body = 99; }
}
fn entry ( ) { let changed_body = true; }
impl Container {
    pub fn run (&self) -> u32 { 42 }
}
/// New documentation and an ordinary attribute preserve the anchor.
#[allow(dead_code)]
pub struct Container { added_field: usize }
use crate :: helper :: { B as Renamed, A };
mod helper;
"#;
    fixture.edit(edited);
    let mut second = fixture.options("second.jsonl");
    second.model_id = first.model_id.clone();
    second.model_revision = Some(2);
    second.revision = NEXT_REVISION.into();
    let second_report = produce(&second).unwrap();
    let after = records(&second.output);
    let kinds = [EntityKind::CodeType, EntityKind::CodeFunction];
    let before_ids = entity_ids(&before, &kinds);
    let mut after_ids = entity_ids(&after, &kinds);
    assert_eq!(after_ids.remove("unrelated").unwrap().len(), 1);
    assert_eq!(before_ids, after_ids);
    assert_eq!(
        relation_ids(&before, RelationKind::CodeReferences),
        relation_ids(&after, RelationKind::CodeReferences)
    );
    assert_eq!(first_report.import_references, 2);
    assert_eq!(second_report.import_references, 2);
    assert!(
        relation_ids(&before, RelationKind::OwnsCode)
            .is_subset(&relation_ids(&after, RelationKind::OwnsCode))
    );
    assert_eq!(first_report.model_id, second_report.model_id);
    assert_eq!(second_report.model_revision, 2);
    assert_eq!(second_report.source_revision, NEXT_REVISION);
    assert_eq!(
        declaration_reference(&before, "Container"),
        format!("{}/blob/{REVISION}/src/lib.rs#L3", first.repository)
    );
    let new_line = edited
        .lines()
        .position(|line| line.starts_with("pub struct Container"))
        .unwrap()
        + 1;
    assert_eq!(
        declaration_reference(&after, "Container"),
        format!(
            "{}/blob/{NEXT_REVISION}/src/lib.rs#L{new_line}",
            second.repository
        )
    );
    assert!(
        after
            .iter()
            .filter_map(|record| match record {
                BundleRecord::Evidence(record) => record.reference.as_ref(),
                _ => None,
            })
            .all(|reference| reference.contains(&format!("/blob/{NEXT_REVISION}/")))
    );
}

#[test]
fn syntax_scheme_is_an_explicit_model_boundary_with_compatible_package_and_file_ids() {
    let fixture = Fixture::new("mod helper;\nuse crate::helper::A;\npub fn entry() {}\n");
    let syntax = fixture.options("syntax.jsonl");
    let mut legacy = fixture.options("legacy.jsonl");
    legacy.identity_scheme = IdentityScheme::LegacyV1;
    let syntax_report = produce(&syntax).unwrap();
    let legacy_report = produce(&legacy).unwrap();
    assert_ne!(syntax_report.model_id, legacy_report.model_id);
    let syntax_records = records(&syntax.output);
    let legacy_records = records(&legacy.output);
    assert_eq!(
        entity_ids(
            &syntax_records,
            &[EntityKind::CodePackage, EntityKind::CodeFile]
        ),
        entity_ids(
            &legacy_records,
            &[EntityKind::CodePackage, EntityKind::CodeFile]
        )
    );
    assert_ne!(
        entity_ids(&syntax_records, &[EntityKind::CodeFunction]),
        entity_ids(&legacy_records, &[EntityKind::CodeFunction])
    );
    assert_ne!(
        relation_ids(&syntax_records, RelationKind::CodeReferences),
        relation_ids(&legacy_records, RelationKind::CodeReferences)
    );
    assert_eq!(
        serde_json::to_value(syntax_report).unwrap()["identity_scheme"],
        "syntax-v2"
    );
    assert!(
        matches!(&syntax_records[0], BundleRecord::Manifest(m) if m.omissions.iter().any(|message| message.contains("syntax-v2") && message.contains("new model identity")))
    );
}

#[test]
fn impl_trait_self_type_and_conditional_contexts_disambiguate_without_order_or_position() {
    let blocks = [
        "mod helper;",
        "struct Holder<T>(T);",
        "trait Alpha { fn act(&self); }",
        "trait Beta { fn act(&self); }",
        "impl<T> Holder<T> { fn method(&self) {} }",
        "impl Holder<u8> { fn method(&self) {} }",
        "impl Holder<u16> { fn method(&self) {} }",
        "impl Alpha for Holder<u8> { fn act(&self) {} }",
        "impl Beta for Holder<u8> { fn act(&self) {} }",
        "#[cfg(feature = \"a\")] fn choice() {}",
        "#[cfg(not(feature = \"a\"))] fn choice() {}",
        "#[cfg(feature = \"a\")] mod branch { fn nested() {} use crate::helper::A; }",
        "#[cfg(not(feature = \"a\"))] mod branch { fn nested() {} use crate::helper::A; }",
        "#[cfg(feature = \"a\")] impl Holder<u32> { fn conditional(&self) {} }",
        "#[cfg(not(feature = \"a\"))] impl Holder<u32> { fn conditional(&self) {} }",
        "impl Holder<u64> { #[cfg(feature = \"a\")] fn conditional(&self) {} #[cfg(not(feature = \"a\"))] fn conditional(&self) {} }",
        "#[cfg(feature = \"a\")] trait Conditional { fn member(&self); }",
        "#[cfg(not(feature = \"a\"))] trait Conditional { fn member(&self); }",
        "#[cfg(feature = \"a\")] #[cfg_attr(feature = \"b\", inline)] fn attributes() {}",
        "use crate::helper::{A as First, A as Second};",
    ];
    let fixture = Fixture::new(&blocks.join("\n"));
    let first = fixture.options("first.jsonl");
    let report = produce(&first).unwrap();
    assert!(report.unparsed_files.is_empty());
    let before = records(&first.output);
    let ids = entity_ids(&before, &[EntityKind::CodeType, EntityKind::CodeFunction]);
    for (label, count) in [
        ("Holder::method", 3),
        ("Holder::act", 2),
        ("choice", 2),
        ("branch::nested", 2),
        ("Holder::conditional", 4),
        ("Conditional", 2),
        ("Conditional::member", 2),
    ] {
        assert_eq!(ids[label].len(), count, "{label}");
    }
    assert_eq!(report.import_references, 4); // Two cfg branches plus two explicit aliases.
    assert_eq!(relation_ids(&before, RelationKind::CodeReferences).len(), 4);
    let edited = blocks
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n\n// moved block\n\n")
        .replace("Holder<", "Holder < ")
        .replace("feature =", "feature=")
        .replace("fn method(&self) {}", "fn method(&self) { let body = 10; }")
        .replace(
            "#[cfg(feature= \"a\")] #[cfg_attr(feature= \"b\", inline)]",
            "#[cfg_attr(feature= \"b\", inline)] #[cfg(feature= \"a\")]",
        );
    fixture.edit(&edited);
    let second = fixture.options("second.jsonl");
    let report = produce(&second).unwrap();
    assert!(report.unparsed_files.is_empty());
    let after = records(&second.output);
    assert_eq!(
        ids,
        entity_ids(&after, &[EntityKind::CodeType, EntityKind::CodeFunction])
    );
    assert_eq!(
        relation_ids(&before, RelationKind::OwnsCode),
        relation_ids(&after, RelationKind::OwnsCode)
    );
    assert_eq!(
        relation_ids(&before, RelationKind::CodeReferences),
        relation_ids(&after, RelationKind::CodeReferences)
    );
}

#[test]
fn unresolved_identical_anchors_fail_before_publication_instead_of_merging() {
    for source in [
        "fn duplicate() {}\nfn duplicate() { let different_body = 2; }",
        "struct Holder; impl Holder { fn same() {} } impl Holder { fn same() {} }",
        "mod helper; use crate::helper::A; use crate::helper::A;",
        "mod helper; use crate::helper::{A, A};",
        "mod helper; use crate::helper::{A as Alias, A as Alias};",
        "use unknown_external::A; use unknown_external::A;",
    ] {
        let fixture = Fixture::new(source);
        let options = fixture.options("ambiguous.jsonl");
        let error = produce(&options).unwrap_err().to_string();
        assert!(
            error.contains("ambiguous syntax-v2 anchor in src/lib.rs at lines"),
            "{error}"
        );
        assert!(error.contains("no bundle was published"), "{error}");
        assert!(!options.output.exists());
    }
}

#[test]
fn cli_reports_explicit_scheme_and_rejects_unknown_scheme_before_writing() {
    let fixture = Fixture::new("pub fn entry() {}\n");
    let options = fixture.options("cli.jsonl");
    let run = |scheme: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_visualyze-rust-bundle"))
            .arg("--root")
            .arg(&options.root)
            .arg("--output")
            .arg(&options.output)
            .args([
                "--revision",
                REVISION,
                "--repository",
                &options.repository,
                "--identity-scheme",
                scheme,
            ])
            .output()
            .unwrap()
    };
    let invalid = run("syntax-v3");
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("legacy-v1 or syntax-v2")
    );
    assert!(!options.output.exists());
    let valid = run("syntax-v2");
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&valid.stdout).unwrap();
    assert_eq!(report["identity_scheme"], "syntax-v2");
}
