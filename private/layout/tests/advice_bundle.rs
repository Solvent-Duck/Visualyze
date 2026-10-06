use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use visualyze_layout::advice::{AdviceLevel, advise_bundle};
use visualyze_model::provider::*;

struct Fixture {
    bundle: LocalBundle,
    path: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// `root` holds scope `platform` (containing `services` services, the first
/// `links` consecutive ones depending on the next), datastore `db` (owning
/// `tables` tables, each referencing the next) and repository `repo` (owning
/// one package `pkg` with `files` files).
fn fixture(services: usize, links: usize, tables: usize, files: usize) -> Fixture {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "visualyze-advice-{}-{}.jsonl",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut writer = BundleWriter::create(
        &path,
        Manifest {
            format: FORMAT_VERSION.into(),
            model_id: "advice-fixture".into(),
            revision: 1,
            root_id: "root".into(),
            profiles: vec![Profile::Architecture, Profile::Data, Profile::Code],
            coverage: Coverage::Complete,
            scope_description: "Advice fixture".into(),
            omissions: vec![],
        },
        Limits::default(),
    )
    .unwrap();
    let entity = |id: &str, kind, label: &str| {
        BundleRecord::Entity(EntityRecord {
            summary: EntitySummary {
                id: id.into(),
                kind,
                label: label.into(),
                boundary: Boundary::Internal,
                claim: Claim::declared(),
            },
            detail: None,
        })
    };
    let relation = |id: String, kind, source: &str, target: String| {
        BundleRecord::Relation(RelationRecord {
            id,
            kind,
            source: source.into(),
            target,
            claim: Claim::declared(),
            label: None,
            aspect: None,
        })
    };
    let mut entities = vec![
        entity("root", EntityKind::Scope, "Root"),
        entity("platform", EntityKind::Scope, "Platform"),
        entity("db", EntityKind::Datastore, "Orders database"),
        entity("repo", EntityKind::CodeRepository, "Repository"),
        entity("pkg", EntityKind::CodePackage, "Package"),
    ];
    let mut relations = vec![
        relation("own:db".into(), RelationKind::Contains, "root", "db".into()),
        relation(
            "own:platform".into(),
            RelationKind::Contains,
            "root",
            "platform".into(),
        ),
        relation(
            "own:repo".into(),
            RelationKind::Contains,
            "root",
            "repo".into(),
        ),
        relation(
            "own:pkg".into(),
            RelationKind::OwnsCode,
            "repo",
            "pkg".into(),
        ),
    ];
    for i in 0..services {
        let id = format!("service:{i:03}");
        entities.push(entity(&id, EntityKind::Service, &id));
        relations.push(relation(
            format!("own:{id}"),
            RelationKind::Contains,
            "platform",
            id.clone(),
        ));
        if i < links {
            relations.push(relation(
                format!("dep:{i:03}"),
                RelationKind::DependsOn,
                &id,
                format!("service:{:03}", i + 1),
            ));
        }
    }
    for i in 0..tables {
        let id = format!("table:{i:03}");
        entities.push(entity(&id, EntityKind::Table, &id));
        relations.push(relation(
            format!("own:{id}"),
            RelationKind::OwnsTable,
            "db",
            id.clone(),
        ));
        if i + 1 < tables {
            relations.push(relation(
                format!("ref:{i:03}"),
                RelationKind::TableReferences,
                &id,
                format!("table:{:03}", i + 1),
            ));
        }
    }
    for i in 0..files {
        let id = format!("file:{i:03}");
        entities.push(entity(&id, EntityKind::CodeFile, &id));
        relations.push(relation(
            format!("own:{id}"),
            RelationKind::OwnsCode,
            "pkg",
            id.clone(),
        ));
    }
    // The writer requires canonical increasing ID order within each section.
    let id = |record: &BundleRecord| match record {
        BundleRecord::Entity(entity) => entity.summary.id.clone(),
        BundleRecord::Relation(relation) => relation.id.clone(),
        _ => unreachable!(),
    };
    entities.sort_by_key(id);
    relations.sort_by_key(id);
    for record in entities.iter().chain(&relations) {
        writer.push(record).unwrap();
    }
    Fixture {
        bundle: writer.finish().unwrap(),
        path,
    }
}

#[test]
fn a_wide_scope_is_advised_with_its_connectors() {
    let mut fixture = fixture(40, 30, 0, 0);
    let advice = advise_bundle(&mut fixture.bundle).unwrap();
    assert_eq!(advice.len(), 1, "{advice:#?}");
    let a = &advice[0];
    assert_eq!(a.scope_id, "platform");
    assert_eq!(a.level, AdviceLevel::Strong);
    assert_eq!(a.children, 40);
    assert!(!a.partial, "{a:#?}");
    assert_eq!(a.noise.boxes, 40.0);
    assert_eq!(a.noise.connectors, 30);
    assert!(a.message.contains("Platform"), "{}", a.message);
    assert!(a.message.contains("40 parts"), "{}", a.message);
}

#[test]
fn inventory_containers_are_never_advised() {
    // A wide datastore or code package has no truthful grouping to supply.
    let mut fixture = fixture(0, 0, 40, 300);
    assert!(advise_bundle(&mut fixture.bundle).unwrap().is_empty());
}

#[test]
fn small_bundles_get_no_advice() {
    let mut fixture = fixture(8, 4, 8, 5);
    assert!(advise_bundle(&mut fixture.bundle).unwrap().is_empty());
}

#[test]
fn a_scope_wider_than_one_page_is_advised_from_a_lower_bound() {
    let mut fixture = fixture(300, 0, 0, 0);
    let advice = advise_bundle(&mut fixture.bundle).unwrap();
    assert_eq!(advice.len(), 1, "{advice:#?}");
    let a = &advice[0];
    assert_eq!(a.scope_id, "platform");
    assert_eq!(a.children, 300);
    assert!(a.partial);
    assert_eq!(a.noise.boxes, 300.0, "the full child count, not the page");
    assert_eq!(
        a.noise.widest_layer().round(),
        300.0,
        "page layers are scaled to the full child count"
    );
    assert_eq!(a.level, AdviceLevel::Strong);
}
