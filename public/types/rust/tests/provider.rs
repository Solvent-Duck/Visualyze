use std::{
    fs,
    io::Write,
    ops::ControlFlow,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use visualyze_model::provider::*;

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

#[test]
fn bundled_example_is_valid_and_has_explicit_table_ownership() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/data-messaging.bundle.jsonl");
    let mut provider = LocalBundle::open(path, Limits::default()).unwrap();
    assert_eq!(
        provider.stats().counts,
        Counts {
            entities: 16,
            relations: 23,
            evidence: 0
        }
    );
    let tables = provider
        .children(1, "datastore:orders", PageRequest::default())
        .unwrap();
    assert_eq!(tables.items.len(), 2);
    assert!(
        tables
            .items
            .iter()
            .all(|table| table.kind == EntityKind::Table)
    );
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "visualyze-bundle-test-{}-{}.jsonl",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        )))
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn manifest() -> Manifest {
    Manifest {
        format: FORMAT_VERSION.into(),
        model_id: "test:all-profiles".into(),
        revision: 7,
        root_id: "e:root".into(),
        profiles: vec![
            Profile::Architecture,
            Profile::Data,
            Profile::Code,
            Profile::Messaging,
            Profile::Instances,
        ],
        coverage: Coverage::Partial,
        scope_description: "Supplied synthetic profile fixture".into(),
        omissions: vec!["Not an extracted production architecture".into()],
    }
}

fn entity(id: &str, kind: EntityKind) -> BundleRecord {
    BundleRecord::Entity(EntityRecord {
        summary: EntitySummary {
            id: id.into(),
            kind,
            label: id.into(),
            boundary: Boundary::Internal,
            claim: Claim::declared(),
        },
        detail: Some(EntityDetail {
            description: format!("Description of {id}"),
            subtype: None,
            roles: vec![],
            technology: vec![],
        }),
    })
}

fn relation(id: &str, kind: RelationKind, source: &str, target: &str) -> BundleRecord {
    BundleRecord::Relation(RelationRecord {
        id: id.into(),
        kind,
        source: source.into(),
        target: target.into(),
        claim: Claim::declared(),
        label: None,
        aspect: None,
    })
}

fn records() -> Vec<BundleRecord> {
    use EntityKind as E;
    use RelationKind as R;
    let mut records = vec![BundleRecord::Evidence(EvidenceRecord {
        id: "proof:declared".into(),
        description: "Fixture declaration".into(),
        reference: Some("https://example.invalid/never-fetched".into()),
    })];
    for (id, kind) in [
        ("e:api", E::Interface),
        ("e:bus", E::MessageChannel),
        ("e:column", E::Column),
        ("e:db", E::Datastore),
        ("e:deployment", E::Deployment),
        ("e:file", E::CodeFile),
        ("e:function", E::CodeFunction),
        ("e:instance", E::DeploymentInstance),
        ("e:message", E::MessageType),
        ("e:package", E::CodePackage),
        ("e:repo", E::CodeRepository),
        ("e:root", E::Scope),
        ("e:service", E::Service),
        ("e:table-a", E::Table),
        ("e:table-b", E::Table),
        ("e:type", E::CodeType),
        ("e:web", E::Client),
    ] {
        records.push(entity(id, kind));
    }
    for (id, kind, source, target) in [
        ("r:api", R::Exposes, "e:service", "e:api"),
        ("r:bus", R::Contains, "e:root", "e:bus"),
        ("r:carries", R::Carries, "e:bus", "e:message"),
        (
            "r:code-reference",
            R::CodeReferences,
            "e:file",
            "e:function",
        ),
        ("r:column", R::OwnsColumn, "e:table-a", "e:column"),
        ("r:db", R::Contains, "e:root", "e:db"),
        ("r:file", R::OwnsCode, "e:package", "e:file"),
        ("r:function", R::OwnsCode, "e:type", "e:function"),
        ("r:instance", R::InstanceOf, "e:instance", "e:service"),
        ("r:location", R::DeployedIn, "e:instance", "e:deployment"),
        ("r:message", R::Contains, "e:root", "e:message"),
        ("r:package", R::OwnsCode, "e:repo", "e:package"),
        ("r:publishes", R::Publishes, "e:service", "e:bus"),
        ("r:reads", R::Reads, "e:service", "e:table-a"),
        ("r:repo", R::Contains, "e:root", "e:repo"),
        ("r:request", R::Requests, "e:web", "e:api"),
        ("r:service", R::Contains, "e:root", "e:service"),
        ("r:subscribes", R::Subscribes, "e:web", "e:bus"),
        ("r:table-a", R::OwnsTable, "e:db", "e:table-a"),
        ("r:table-b", R::OwnsTable, "e:db", "e:table-b"),
        (
            "r:table-reference",
            R::TableReferences,
            "e:table-a",
            "e:table-b",
        ),
        ("r:type", R::OwnsCode, "e:file", "e:type"),
        ("r:web", R::Contains, "e:root", "e:web"),
        ("r:writes", R::Writes, "e:service", "e:table-a"),
    ] {
        records.push(relation(id, kind, source, target));
    }
    records
}

fn build(path: &Temp, limits: Limits) -> LocalBundle {
    let mut writer = BundleWriter::create(&path.0, manifest(), limits).unwrap();
    for record in records() {
        writer.push(&record).unwrap();
    }
    writer.finish().unwrap()
}

fn build_nested_deployment(path: &Temp) -> LocalBundle {
    let mut input = records();
    input.extend([
        entity("e:deployment-child", EntityKind::Deployment),
        entity("e:deployment-other", EntityKind::Deployment),
        entity("e:instance-child", EntityKind::DeploymentInstance),
        relation(
            "r:child-component",
            RelationKind::InstanceOf,
            "e:instance-child",
            "e:web",
        ),
        relation(
            "r:child-home",
            RelationKind::DeployedIn,
            "e:instance-child",
            "e:deployment-child",
        ),
        relation(
            "r:component-home",
            RelationKind::DeployedIn,
            "e:service",
            "e:deployment-child",
        ),
        relation(
            "r:deployment-nesting",
            RelationKind::DeployedIn,
            "e:deployment-child",
            "e:deployment",
        ),
    ]);
    input.sort_by_key(|record| match record {
        BundleRecord::Evidence(e) => (0, e.id.clone()),
        BundleRecord::Entity(e) => (1, e.summary.id.clone()),
        BundleRecord::Relation(r) => (2, r.id.clone()),
        _ => unreachable!(),
    });
    let mut writer = BundleWriter::create(&path.0, manifest(), Limits::default()).unwrap();
    for record in input {
        writer.push(&record).unwrap();
    }
    writer.finish().unwrap()
}

#[test]
fn semantic_lenses_filter_before_child_paging_and_bind_cursors() {
    let path = Temp::new();
    let mut provider = build(&path, Limits::default());
    use SemanticLens as L;
    assert_eq!(provider.child_count(7, "e:root").unwrap(), 6);
    assert_eq!(provider.child_count_in(7, L::Logical, "e:root").unwrap(), 5);
    assert_eq!(provider.child_count_in(7, L::Code, "e:root").unwrap(), 1);
    // The first unfiltered child is the message bus, but a one-record code page
    // still returns the repository, with an offset in the filtered child index.
    let code = provider.children_at_in(7, L::Code, "e:root", 0, 1).unwrap();
    assert_eq!(code.items[0].id, "e:repo");
    assert!(code.next.is_none());
    let logical = provider
        .children_in(
            7,
            L::Logical,
            "e:root",
            PageRequest {
                limit: 2,
                after: None,
            },
        )
        .unwrap();
    assert_eq!(
        logical
            .items
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["e:bus", "e:db"]
    );
    let cursor = logical.next.unwrap();
    assert!(matches!(
        provider.children_in(
            7,
            L::Code,
            "e:root",
            PageRequest {
                limit: 2,
                after: Some(cursor.clone())
            }
        ),
        Err(BundleError::InvalidCursor)
    ));
    assert!(matches!(
        provider.children(
            7,
            "e:root",
            PageRequest {
                limit: 2,
                after: Some(cursor.clone())
            }
        ),
        Err(BundleError::InvalidCursor)
    ));
    let next = provider
        .children_in(
            7,
            L::Logical,
            "e:root",
            PageRequest {
                limit: 2,
                after: Some(cursor),
            },
        )
        .unwrap();
    assert_eq!(
        next.items.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["e:message", "e:service"]
    );
    assert_eq!(
        provider
            .owner_id_in(7, L::Code, "e:function")
            .unwrap()
            .as_deref(),
        Some("e:type")
    );
    assert!(provider.in_lens(7, L::Code, "e:function").unwrap());
    assert!(!provider.in_lens(7, L::Logical, "e:function").unwrap());
    assert!(!provider.in_lens(7, L::Code, "e:absent").unwrap());
    assert!(
        provider
            .children_in(7, L::Code, "e:db", PageRequest::default())
            .is_err()
    );
    assert!(provider.owner_id_in(7, L::Logical, "e:repo").is_err());
    assert!(matches!(
        provider.child_count_in(6, L::Code, "e:root"),
        Err(BundleError::StaleRevision { .. })
    ));
    assert!(matches!(
        provider.children_at_in(7, L::Code, "e:root", 2, 1),
        Err(BundleError::InvalidCursor)
    ));
    assert_eq!(provider.stats().cached_records, 0);
}

#[test]
fn deployment_lens_uses_deployed_in_forest_and_virtual_root_without_inventing_claims() {
    let path = Temp::new();
    let mut provider = build_nested_deployment(&path);
    let lens = SemanticLens::Deployment;
    assert_eq!(provider.child_count_in(7, lens, "e:root").unwrap(), 2);
    let roots = provider
        .children_in(7, lens, "e:root", PageRequest::default())
        .unwrap();
    assert_eq!(
        roots
            .items
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["e:deployment", "e:deployment-other"]
    );
    let children = provider
        .children_in(7, lens, "e:deployment", PageRequest::default())
        .unwrap();
    assert_eq!(
        children
            .items
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["e:deployment-child", "e:instance"]
    );
    assert_eq!(
        provider
            .owner_id_in(7, lens, "e:deployment")
            .unwrap()
            .as_deref(),
        Some("e:root")
    );
    assert_eq!(
        provider
            .owner_id_in(7, lens, "e:instance-child")
            .unwrap()
            .as_deref(),
        Some("e:deployment-child")
    );
    assert_eq!(provider.owner_id(7, "e:instance-child").unwrap(), None);
    assert_eq!(provider.child_count(7, "e:deployment").unwrap(), 0);
    assert!(!provider.in_lens(7, lens, "e:service").unwrap());

    let mut request = RegionRequest {
        lens: Some(lens),
        focus: "e:root".into(),
        visible: [
            "e:root",
            "e:deployment",
            "e:deployment-child",
            "e:instance",
            "e:instance-child",
        ]
        .map(str::to_owned)
        .to_vec(),
        scope_roots: vec![],
        page: PageRequest {
            limit: 1,
            after: None,
        },
        scan_budget: 2,
        reference_budget: 8,
        existing_references: vec![],
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut found = std::collections::BTreeMap::new();
    loop {
        let page = provider
            .region_relations(7, request.clone(), &cancel)
            .unwrap();
        assert!(page.scanned <= 2);
        for item in page.items {
            assert_eq!(
                provider.relation(7, &item.relation.id).unwrap().unwrap(),
                item.relation
            );
            assert!(found.insert(item.relation.id.clone(), item).is_none());
        }
        let Some(cursor) = page.next else {
            break;
        };
        request.page.after = Some(cursor);
    }
    assert_eq!(found.len(), 6);
    for id in ["r:location", "r:child-home", "r:deployment-nesting"] {
        assert!(found[id].structural);
        assert!(found[id].source_inside && found[id].target_inside);
    }
    let identity = &found["r:child-component"];
    assert_eq!(identity.relation.kind, RelationKind::InstanceOf);
    assert_eq!(identity.source_display, "e:instance-child");
    assert_eq!(identity.target_display, "e:web");
    assert!(identity.source_inside && !identity.target_inside && !identity.structural);
    let home = &found["r:component-home"];
    assert!(!home.source_inside && home.target_inside && !home.structural);
    assert_eq!(home.relation.source, "e:service");
    assert!(!found.values().any(|item| item.relation.kind.is_ownership()));
    assert_eq!(provider.stats().cached_records, 0);
}

#[test]
fn lens_region_predicates_scopes_and_cursors_preserve_original_semantics() {
    let path = Temp::new();
    let mut provider = build_nested_deployment(&path);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let base = RegionRequest {
        lens: Some(SemanticLens::Logical),
        focus: "e:root".into(),
        visible: vec!["e:root".into()],
        scope_roots: vec![],
        page: PageRequest::default(),
        scan_budget: 8192,
        reference_budget: 8,
        existing_references: vec![],
    };
    let logical = provider.region_relations(7, base.clone(), &cancel).unwrap();
    assert!(logical.items.is_empty());
    assert!(
        logical
            .internal_relation_ids
            .contains(&"r:publishes".into())
    );
    assert!(
        logical
            .internal_relation_ids
            .contains(&"r:table-reference".into())
    );
    assert!(
        !logical
            .internal_relation_ids
            .contains(&"r:code-reference".into())
    );
    assert!(!logical.internal_relation_ids.contains(&"r:instance".into()));
    let mut code = base.clone();
    code.lens = Some(SemanticLens::Code);
    let code_page = provider.region_relations(7, code, &cancel).unwrap();
    assert_eq!(code_page.internal_relation_ids, ["r:code-reference"]);
    let mut wrong = base.clone();
    wrong.visible.push("e:repo".into());
    assert!(provider.region_relations(7, wrong, &cancel).is_err());
    let mut first = base.clone();
    first.scan_budget = 1;
    let cursor = provider
        .region_relations(7, first.clone(), &cancel)
        .unwrap()
        .next
        .unwrap();
    first.page.after = Some(cursor);
    first.lens = Some(SemanticLens::Code);
    assert!(matches!(
        provider.region_relations(7, first, &cancel),
        Err(BundleError::InvalidCursor)
    ));

    let mut nested = base;
    nested.lens = Some(SemanticLens::Deployment);
    nested.focus = "e:deployment".into();
    nested.visible = vec!["e:deployment".into(), "e:deployment-child".into()];
    nested.scope_roots = vec!["e:deployment-child".into()];
    let page = provider
        .region_relations(7, nested.clone(), &cancel)
        .unwrap();
    let child_identity = page
        .items
        .iter()
        .find(|item| item.relation.id == "r:child-component")
        .unwrap();
    assert_eq!(child_identity.source_display, "e:deployment-child");
    assert_eq!(child_identity.relation.source, "e:instance-child");
    assert!(
        !page
            .items
            .iter()
            .any(|item| item.relation.id == "r:instance")
    );
    nested.scope_roots.push("e:instance-child".into());
    assert!(
        provider.region_relations(7, nested, &cancel).is_err(),
        "overlapping deployment subtrees are rejected"
    );
}

fn raw(path: &Temp, manifest: Manifest, records: &[BundleRecord], complete: bool) {
    let mut file = fs::File::create(&path.0).unwrap();
    let mut write = |record: &BundleRecord| {
        serde_json::to_writer(&mut file, record).unwrap();
        file.write_all(b"\n").unwrap();
    };
    write(&BundleRecord::Manifest(manifest));
    let mut counts = Counts::default();
    for record in records {
        match record {
            BundleRecord::Entity(_) => counts.entities += 1,
            BundleRecord::Relation(_) => counts.relations += 1,
            BundleRecord::Evidence(_) => counts.evidence += 1,
            _ => {}
        }
        write(record);
    }
    if complete {
        write(&BundleRecord::Complete(counts));
    }
}

#[test]
fn profiles_preserve_owned_tables_code_async_direction_and_instance_meaning() {
    let path = Temp::new();
    let mut provider = build(&path, Limits::default());
    let tables = provider
        .children(7, "e:db", PageRequest::default())
        .unwrap();
    assert_eq!(
        tables
            .items
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["e:table-a", "e:table-b"]
    );
    assert!(tables.items.iter().all(|e| e.kind == EntityKind::Table));
    assert_eq!(
        provider
            .children(7, "e:type", PageRequest::default())
            .unwrap()
            .items[0]
            .kind,
        EntityKind::CodeFunction
    );
    let incident = provider
        .incident(7, "e:bus", PageRequest::default())
        .unwrap();
    let subscription = incident
        .items
        .iter()
        .find(|r| r.kind == RelationKind::Subscribes)
        .unwrap();
    assert_eq!(
        (&*subscription.source, &*subscription.target),
        ("e:web", "e:bus")
    );
    assert_eq!(
        provider.summary(7, "e:instance").unwrap().unwrap().kind,
        EntityKind::DeploymentInstance
    );
    assert_eq!(
        provider
            .evidence(7, "proof:declared")
            .unwrap()
            .unwrap()
            .reference
            .as_deref(),
        Some("https://example.invalid/never-fetched")
    );
    assert!(provider.summary(7, "e:missing").unwrap().is_none());
    assert!(matches!(
        provider.children(7, "e:missing", PageRequest::default()),
        Err(BundleError::NotFound(_))
    ));
}

#[test]
fn indexed_reads_preserve_complete_records_in_nonsequential_order() {
    let path = Temp::new();
    let mut provider = build(
        &path,
        Limits {
            cache_records: 0,
            ..Limits::default()
        },
    );
    let supplied = records();
    // Alternate both ends of the file, then reverse that order. Entity detail,
    // relation claims and evidence must not depend on the last file position.
    let order: Vec<_> = (0..supplied.len().div_ceil(2))
        .flat_map(|i| [i, supplied.len() - 1 - i])
        .collect();
    for index in order.iter().chain(order.iter().rev()) {
        match &supplied[*index] {
            BundleRecord::Entity(entity) => {
                assert_eq!(
                    provider.summary(7, &entity.summary.id).unwrap().as_ref(),
                    Some(&entity.summary)
                );
                assert_eq!(
                    provider
                        .detail(7, &entity.summary.id)
                        .unwrap()
                        .detail
                        .as_deref(),
                    entity.detail.as_ref()
                );
            }
            BundleRecord::Relation(relation) => assert_eq!(
                provider.relation(7, &relation.id).unwrap().as_ref(),
                Some(relation)
            ),
            BundleRecord::Evidence(evidence) => assert_eq!(
                provider.evidence(7, &evidence.id).unwrap().as_ref(),
                Some(evidence)
            ),
            _ => unreachable!("fixture contains only data records"),
        }
    }
    assert_eq!(provider.stats().cached_records, 0);
}

#[test]
fn queries_reject_stale_revisions_and_cross_query_or_model_cursors() {
    let path = Temp::new();
    let mut provider = build(&path, Limits::default());
    assert!(matches!(
        provider.summary(6, "e:root"),
        Err(BundleError::StaleRevision {
            expected: 6,
            actual: 7
        })
    ));
    let first = provider
        .children(
            7,
            "e:db",
            PageRequest {
                limit: 1,
                after: None,
            },
        )
        .unwrap();
    assert_eq!(first.items[0].id, "e:table-a");
    let cursor = first.next.unwrap();
    assert!(matches!(
        provider.incident(
            7,
            "e:db",
            PageRequest {
                limit: 1,
                after: Some(cursor.clone())
            }
        ),
        Err(BundleError::InvalidCursor)
    ));
    let second = provider
        .children(
            7,
            "e:db",
            PageRequest {
                limit: 1,
                after: Some(cursor.clone()),
            },
        )
        .unwrap();
    assert_eq!(second.items[0].id, "e:table-b");
    assert!(second.next.is_none());
    let other = Temp::new();
    let mut other_manifest = manifest();
    other_manifest.model_id = "test:other".into();
    raw(&other, other_manifest, &records(), true);
    let mut other = LocalBundle::open(&other.0, Limits::default()).unwrap();
    assert!(matches!(
        other.children(
            7,
            "e:db",
            PageRequest {
                limit: 1,
                after: Some(cursor)
            }
        ),
        Err(BundleError::InvalidCursor)
    ));
    assert!(matches!(
        provider.children(
            7,
            "e:root",
            PageRequest {
                limit: 257,
                after: None
            }
        ),
        Err(BundleError::Limit(_))
    ));
}

#[test]
fn detail_is_demand_loaded_with_bounded_lru_and_distinguishes_unsupplied() {
    let path = Temp::new();
    let mut records = records();
    if let BundleRecord::Entity(root) = records
        .iter_mut()
        .find(|r| matches!(r, BundleRecord::Entity(e) if e.summary.id == "e:root"))
        .unwrap()
    {
        root.detail = None;
    }
    raw(&path, manifest(), &records, true);
    let mut provider = LocalBundle::open(
        &path.0,
        Limits {
            cache_records: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        provider.detail_state(7, "e:root").unwrap(),
        DetailState::NotSupplied
    );
    assert!(provider.detail(7, "e:root").unwrap().detail.is_none());
    assert_eq!(
        provider.detail_state(7, "e:db").unwrap(),
        DetailState::Unloaded
    );
    provider.summary(7, "e:db").unwrap();
    assert_eq!(provider.stats().cached_records, 0);
    let retained = provider.detail(7, "e:db").unwrap().detail.unwrap();
    assert_eq!(
        provider.detail_state(7, "e:db").unwrap(),
        DetailState::Loaded
    );
    provider.detail(7, "e:table-a").unwrap();
    assert_eq!(
        provider.detail_state(7, "e:db").unwrap(),
        DetailState::Unloaded
    );
    assert_eq!(retained.description, "Description of e:db");
    assert_eq!(provider.stats().cached_records, 1);
    provider.clear_detail_cache();
    assert_eq!(provider.stats().cached_bytes, 0);
    let mut no_cache = LocalBundle::open(
        &path.0,
        Limits {
            cache_bytes: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        no_cache.detail(7, "e:db").unwrap().state,
        DetailState::Loaded
    );
    assert_eq!(
        no_cache.detail_state(7, "e:db").unwrap(),
        DetailState::Unloaded
    );
}

#[test]
fn supported_evidence_and_coverage_survive_queries() {
    let path = Temp::new();
    let mut records = records();
    if let BundleRecord::Relation(relation) = records
        .iter_mut()
        .find(|r| matches!(r, BundleRecord::Relation(e) if e.id == "r:reads"))
        .unwrap()
    {
        relation.claim = Claim {
            basis: Basis::Supported,
            evidence_ids: vec!["proof:declared".into()],
            rationale: None,
        };
    }
    raw(&path, manifest(), &records, true);
    let mut provider = LocalBundle::open(&path.0, Limits::default()).unwrap();
    let reads = provider
        .incident(7, "e:table-a", PageRequest::default())
        .unwrap()
        .items
        .into_iter()
        .find(|r| r.id == "r:reads")
        .unwrap();
    assert_eq!(reads.claim.evidence_ids, ["proof:declared"]);
    assert_eq!(reads.claim.basis, Basis::Supported);
    assert_eq!(provider.manifest().coverage, Coverage::Partial);
}

#[test]
fn validation_rejects_wrong_profiles_ownership_cycles_endpoints_and_missing_evidence() {
    for case in 0..8 {
        let path = Temp::new();
        let mut model = manifest();
        let mut input = records();
        match case {
            0 => model.profiles.retain(|p| *p != Profile::Data),
            1 => {
                if let BundleRecord::Relation(r) = input
                    .iter_mut()
                    .find(|r| matches!(r, BundleRecord::Relation(r) if r.id == "r:table-a"))
                    .unwrap()
                {
                    r.source = "e:service".into();
                }
            }
            2 => input.push(relation(
                "r:zz-cycle",
                RelationKind::Contains,
                "e:root",
                "e:root",
            )),
            3 => input.push(relation(
                "r:zz-owner",
                RelationKind::OwnsTable,
                "e:db",
                "e:table-a",
            )),
            4 => {
                if let BundleRecord::Entity(e) = &mut input[1] {
                    e.summary.claim = Claim {
                        basis: Basis::Supported,
                        evidence_ids: vec!["proof:missing".into()],
                        rationale: None,
                    };
                }
            }
            5 => input.retain(|r| !matches!(r, BundleRecord::Relation(r) if r.id == "r:instance")),
            6 => {
                if let BundleRecord::Relation(r) = input
                    .iter_mut()
                    .find(|r| matches!(r, BundleRecord::Relation(r) if r.id == "r:api"))
                    .unwrap()
                {
                    r.source = "e:db".into();
                }
            }
            _ => {
                input.push(relation(
                    "r:zz-code-cycle",
                    RelationKind::OwnsCode,
                    "e:package",
                    "e:package",
                ));
            }
        }
        raw(&path, model, &input, true);
        assert!(
            LocalBundle::open(&path.0, Limits::default()).is_err(),
            "accepted invalid case {case}"
        );
    }
}

#[test]
fn rejects_missing_completion_out_of_order_unknown_fields_duplicates_and_oversized_lines() {
    let path = Temp::new();
    raw(&path, manifest(), &records(), false);
    assert!(LocalBundle::open(&path.0, Limits::default()).is_err());
    let mut input = records();
    input.swap(2, 3);
    raw(&path, manifest(), &input, true);
    assert!(LocalBundle::open(&path.0, Limits::default()).is_err());
    raw(&path, manifest(), &records(), true);
    let original = fs::read_to_string(&path.0).unwrap();
    for changed in [
        original.replacen(
            "\"record\":\"manifest\"",
            "\"record\":\"manifest\",\"record\":\"manifest\"",
            1,
        ),
        original.replacen(
            "\"record\":\"manifest\"",
            "\"record\":\"manifest\",\"geometry\":{}",
            1,
        ),
        original.replacen("\"revision\":7", "\"revision\":7,\"revision\":8", 1),
        original.replacen(
            "\"label\":\"e:api\"",
            "\"label\":\"e:api\",\"label\":\"hidden\"",
            1,
        ),
        original.replacen(
            "\"revision\":7",
            "\"revision\":7,\"coordinates\":[0,0,0]",
            1,
        ),
        original.replacen("\"revision\":7", "\"revision\":0", 1),
        original.replacen("visualyze.bundle.v1", "visualyze.bundle.v2", 1),
        format!("{original}{}\n", " ".repeat(65537)),
        original.trim_end().to_owned(),
    ] {
        fs::write(&path.0, changed).unwrap();
        assert!(LocalBundle::open(&path.0, Limits::default()).is_err());
    }
    fs::write(&path.0, format!("{}0{}\n", "[".repeat(17), "]".repeat(17))).unwrap();
    assert!(matches!(
        LocalBundle::open(&path.0, Limits::default()),
        Err(BundleError::Limit("JSON nesting depth"))
    ));
}

#[test]
fn indexing_cancellation_limits_and_source_mutation_fail_without_partial_provider() {
    let path = Temp::new();
    let mut provider = build(&path, Limits::default());
    assert!(matches!(
        LocalBundle::open_with_progress(&path.0, Limits::default(), |_| ControlFlow::Break(())),
        Err(BundleError::Cancelled)
    ));
    assert!(matches!(
        LocalBundle::open(
            &path.0,
            Limits {
                max_index_bytes: 512,
                ..Limits::default()
            }
        ),
        Err(BundleError::Limit("index bytes"))
    ));
    assert!(matches!(
        LocalBundle::open(
            &path.0,
            Limits {
                max_entities: 2,
                ..Limits::default()
            }
        ),
        Err(BundleError::Limit("entity count"))
    ));
    fs::OpenOptions::new()
        .append(true)
        .open(&path.0)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert!(matches!(
        provider.summary(7, "e:root"),
        Err(BundleError::SourceChanged)
    ));
}

#[test]
fn completed_index_requires_its_full_reported_memory_budget() {
    let path = Temp::new();
    let provider = build_nested_deployment(&path);
    let expected = provider.stats();
    let bytes = expected.estimated_index_bytes;
    drop(provider);
    assert!(bytes > 0);
    assert!(matches!(
        LocalBundle::open(
            &path.0,
            Limits {
                max_index_bytes: bytes - 1,
                ..Limits::default()
            }
        ),
        Err(BundleError::Limit("index bytes"))
    ));
    let exact = LocalBundle::open(
        &path.0,
        Limits {
            max_index_bytes: bytes,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(exact.stats().estimated_index_bytes, bytes);
    assert_eq!(exact.stats().counts, expected.counts);
}

#[test]
fn writer_is_streaming_canonical_and_never_overwrites_existing_files() {
    let path = Temp::new();
    let mut writer = BundleWriter::create(&path.0, manifest(), Limits::default()).unwrap();
    assert!(BundleWriter::create(&path.0, manifest(), Limits::default()).is_err());
    writer.push(&entity("e:root", EntityKind::Scope)).unwrap();
    assert!(writer.push(&entity("e:root", EntityKind::Scope)).is_err());
    assert!(writer.push(&entity("e:aaa", EntityKind::Scope)).is_err());
    let provider = writer.finish().unwrap();
    assert_eq!(provider.stats().counts.entities, 1);
    assert_eq!(provider.stats().cached_records, 0);
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_and_nonregular_paths() {
    let path = Temp::new();
    let link = Temp::new();
    build(&path, Limits::default());
    std::os::unix::fs::symlink(&path.0, &link.0).unwrap();
    assert!(LocalBundle::open(&link.0, Limits::default()).is_err());
    assert!(LocalBundle::open(std::env::temp_dir(), Limits::default()).is_err());
}

#[test]
fn byte_budgets_bound_pages_and_cache_even_when_record_count_is_small() {
    let path = Temp::new();
    let mut input = records();
    for record in &mut input {
        if let BundleRecord::Entity(e) = record {
            e.detail.as_mut().unwrap().description = "x".repeat(400);
        }
    }
    raw(&path, manifest(), &input, true);
    let limits = Limits {
        max_record_bytes: 1024,
        max_page_bytes: 1024,
        cache_bytes: 5000,
        ..Limits::default()
    };
    let mut provider = LocalBundle::open(&path.0, limits).unwrap();
    let page = provider
        .children(7, "e:db", PageRequest::default())
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next.is_some());
    provider.detail(7, "e:table-a").unwrap();
    provider.detail(7, "e:table-b").unwrap();
    assert_eq!(provider.stats().cached_records, 1);
    assert!(provider.stats().cached_bytes <= 5000);
}

#[test]
fn nontrivial_ownership_cycles_fail_but_runtime_cycles_and_parallel_claims_survive() {
    let path = Temp::new();
    let cyclic = vec![
        entity("e:a", EntityKind::Scope),
        entity("e:b", EntityKind::Scope),
        entity("e:root", EntityKind::Scope),
        relation("r:a", RelationKind::Contains, "e:a", "e:b"),
        relation("r:b", RelationKind::Contains, "e:b", "e:a"),
    ];
    raw(&path, manifest(), &cyclic, true);
    assert!(
        matches!(LocalBundle::open(&path.0, Limits::default()), Err(BundleError::Invalid(message)) if message.contains("cycle"))
    );
    let mut input = records();
    input.push(relation(
        "r:zz-return-call",
        RelationKind::Requests,
        "e:service",
        "e:web",
    ));
    raw(&path, manifest(), &input, true);
    let mut provider = LocalBundle::open(&path.0, Limits::default()).unwrap();
    let claims = provider
        .incident(7, "e:table-a", PageRequest::default())
        .unwrap()
        .items;
    assert!(claims.iter().any(|r| r.kind == RelationKind::Reads));
    assert!(claims.iter().any(|r| r.kind == RelationKind::Writes));
}

#[test]
fn cancellation_is_observed_during_indexing_after_successful_initial_progress() {
    let path = Temp::new();
    let mut model = manifest();
    model.root_id = "e:0000".into();
    let mut writer = BundleWriter::create(&path.0, model, Limits::default()).unwrap();
    for i in 0..1100 {
        writer
            .push(&entity(
                &format!("e:{i:04}"),
                if i == 0 {
                    EntityKind::Scope
                } else {
                    EntityKind::Service
                },
            ))
            .unwrap();
    }
    for i in 1..1100 {
        writer
            .push(&relation(
                &format!("r:{i:04}"),
                RelationKind::Contains,
                "e:0000",
                &format!("e:{i:04}"),
            ))
            .unwrap();
    }
    writer.finish().unwrap();
    let mut updates = 0;
    let result = LocalBundle::open_with_progress(&path.0, Limits::default(), |progress| {
        updates += 1;
        if progress.records_indexed >= 1024 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    assert!(matches!(result, Err(BundleError::Cancelled)));
    assert_eq!(updates, 2);
}

#[test]
fn indexed_region_scan_preserves_hidden_boundary_claims_and_internal_counts() {
    let temp = Temp::new();
    let mut writer = BundleWriter::create(&temp.0, manifest(), Limits::default()).unwrap();
    for record in records() {
        writer.push(&record).unwrap();
    }
    let mut provider = writer.finish().unwrap();
    let mut request = RegionRequest {
        lens: None,
        focus: "e:root".into(),
        visible: ["e:root", "e:db", "e:repo", "e:service", "e:web", "e:bus"]
            .map(str::to_owned)
            .to_vec(),
        page: PageRequest {
            limit: 1,
            after: None,
        },
        scope_roots: vec![],
        scan_budget: 3,
        reference_budget: 128,
        existing_references: vec![],
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut claims = std::collections::BTreeMap::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut internal_ids = std::collections::BTreeSet::new();
    loop {
        let page = provider
            .region_relations(7, request.clone(), &cancel)
            .unwrap();
        assert!(page.items.len() <= 1 && page.scanned <= 3);
        for item in page.items {
            assert!(claims.insert(item.relation.id.clone(), item).is_none());
        }
        for (owner, count) in page.internal_counts {
            *counts.entry(owner).or_default() += count;
        }
        internal_ids.extend(page.internal_relation_ids);
        match page.next {
            Some(cursor) => request.page.after = Some(cursor),
            None => break,
        }
    }
    let reads = &claims["r:reads"];
    assert_eq!(reads.relation.target, "e:table-a");
    assert_eq!(reads.target_display, "e:db");
    assert_eq!(reads.relation.kind, RelationKind::Reads);
    assert_eq!(claims["r:writes"].relation.kind, RelationKind::Writes);
    assert_eq!(claims["r:request"].target_display, "e:service");
    assert_eq!(counts["e:db"], 1);
    assert_eq!(counts["e:repo"], 1);
    assert_eq!(
        provider
            .relation(7, "r:code-reference")
            .unwrap()
            .unwrap()
            .source,
        "e:file"
    );
    assert!(internal_ids.contains("r:table-reference"));
    assert_eq!(
        provider.stats().cached_records,
        0,
        "boundary scans must not load entity detail"
    );

    let stale = provider.region_relations(6, request.clone(), &cancel);
    assert!(matches!(stale, Err(BundleError::StaleRevision { .. })));
    request.focus = "e:db".into();
    request.visible = vec!["e:db".into()];
    assert!(matches!(
        provider.region_relations(7, request, &cancel),
        Err(BundleError::InvalidCursor)
    ));
    assert!(matches!(
        provider.region_relations(
            7,
            RegionRequest {
                lens: None,
                focus: "e:root".into(),
                visible: vec!["e:root".into()],
                page: PageRequest::default(),
                scope_roots: vec![],
                scan_budget: 1,
                reference_budget: 128,
                existing_references: vec![],
            },
            &std::sync::atomic::AtomicBool::new(true)
        ),
        Err(BundleError::Cancelled)
    ));
}

#[test]
fn local_region_work_is_independent_of_unrelated_relation_prefixes() {
    let path = Temp::new();
    let mut input = vec![
        entity("e:root", EntityKind::Scope),
        entity("e:local", EntityKind::Scope),
        entity("e:other", EntityKind::Scope),
        entity("e:a", EntityKind::Service),
        entity("e:b", EntityKind::Service),
        entity("e:c", EntityKind::Service),
        entity("e:d", EntityKind::Service),
    ];
    for (parent, child) in [
        ("root", "local"),
        ("root", "other"),
        ("local", "a"),
        ("local", "b"),
        ("other", "c"),
        ("other", "d"),
    ] {
        input.push(relation(
            &format!("r:owns-{child}"),
            RelationKind::Contains,
            &format!("e:{parent}"),
            &format!("e:{child}"),
        ));
    }
    for i in 0..10_000 {
        input.push(relation(
            &format!("r:irrelevant-{i:05}"),
            RelationKind::Requests,
            "e:c",
            "e:d",
        ));
    }
    for (id, source, target) in [
        ("r:z-in", "e:c", "e:a"),
        ("r:z-local", "e:a", "e:b"),
        ("r:z-local-parallel", "e:a", "e:b"),
        ("r:z-out", "e:b", "e:d"),
    ] {
        input.push(relation(id, RelationKind::Requests, source, target));
    }
    input.sort_by_key(|record| match record {
        BundleRecord::Entity(e) => (0, e.summary.id.clone()),
        BundleRecord::Relation(r) => (1, r.id.clone()),
        _ => unreachable!(),
    });
    let mut writer = BundleWriter::create(&path.0, manifest(), Limits::default()).unwrap();
    for record in input {
        writer.push(&record).unwrap();
    }
    let mut provider = writer.finish().unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    for lens in [None, Some(SemanticLens::Logical)] {
        for group in [false, true] {
            let mut request = RegionRequest {
                lens,
                focus: "e:local".into(),
                visible: ["e:local", "e:a", "e:b"].map(str::to_owned).to_vec(),
                scope_roots: if group {
                    vec!["e:a".into(), "e:b".into()]
                } else {
                    vec![]
                },
                page: PageRequest {
                    limit: 1,
                    after: None,
                },
                scan_budget: 1,
                reference_budget: 8,
                existing_references: vec![],
            };
            let mut seen = std::collections::BTreeMap::new();
            let mut scanned = 0;
            loop {
                let page = provider
                    .region_relations(7, request.clone(), &cancel)
                    .unwrap();
                scanned += page.scanned;
                assert!(
                    scanned <= 11,
                    "unrelated edges must never consume local scan work"
                );
                for item in page.items {
                    assert!(
                        seen.insert(item.relation.id.clone(), item).is_none(),
                        "duplicate original relation"
                    );
                }
                if page.next.is_none() {
                    break;
                }
                assert_ne!(page.next, request.page.after);
                request.page.after = page.next;
            }
            assert_eq!(seen.len(), 6); // two visible ownership and four runtime claims
            assert_eq!(seen["r:z-in"].source_display, "e:other");
            assert_eq!(seen["r:z-out"].target_display, "e:other");
            assert_eq!(seen["r:z-local"].relation.source, "e:a");
            assert_eq!(seen["r:z-local-parallel"].relation.target, "e:b");
        }
    }
    assert_eq!(provider.stats().cached_records, 0);
}

#[test]
fn persisted_global_region_cursor_keeps_its_original_traversal() {
    let path = Temp::new();
    let mut provider = build(&path, Limits::default());
    // Preserve the old relation-index traversal across serialization and reopening,
    // including a nonzero cursor saved after five visits to the global index.
    let all = records();
    let visible: Vec<_> = all
        .iter()
        .filter_map(|r| match r {
            BundleRecord::Entity(e) if e.summary.id == "e:root" => Some(e.summary.id.clone()),
            _ => None,
        })
        .collect();
    let root_index = all
        .iter()
        .filter_map(|r| match r {
            BundleRecord::Entity(e) => Some(&e.summary.id),
            _ => None,
        })
        .position(|id| id == "e:root")
        .unwrap();
    let legacy_query = format!("region:None:e:root:[{root_index}]:[{root_index}]");
    let cursor: PageCursor = serde_json::from_value(serde_json::json!({
        "model_id": "test:all-profiles", "revision": 7,
        "query": legacy_query, "offset": 0,
    }))
    .unwrap();
    let mut request = RegionRequest {
        lens: None,
        focus: "e:root".into(),
        visible,
        scope_roots: vec![],
        page: PageRequest {
            limit: 1,
            after: Some(cursor),
        },
        scan_budget: 1,
        reference_budget: 8,
        existing_references: vec![],
    };
    let mut scanned = 0;
    let mut internal = 0;
    let mut seen = std::collections::BTreeSet::new();
    let mut prefix_claims = None;
    let mut suffix_scanned = 0;
    loop {
        let page = provider
            .region_relations(
                7,
                request.clone(),
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(page.scanned, 1);
        if prefix_claims.is_some() {
            suffix_scanned += page.scanned;
        }
        scanned += page.scanned;
        internal += page.internal_counts.values().sum::<usize>();
        assert_eq!(
            page.internal_counts.values().sum::<usize>(),
            page.internal_relation_ids.len(),
            "one scan visit must retain every internal claim ID"
        );
        for id in page
            .items
            .into_iter()
            .map(|item| item.relation.id)
            .chain(page.internal_relation_ids)
        {
            assert!(
                seen.insert(id),
                "saved traversal repeated an original claim"
            );
        }
        let Some(next) = page.next else {
            break;
        };
        assert_eq!(serde_json::to_value(&next).unwrap()["query"], legacy_query);
        if scanned == 5 {
            assert_eq!(serde_json::to_value(&next).unwrap()["offset"], 5);
            assert_eq!(
                seen,
                ["r:carries", "r:code-reference"]
                    .map(str::to_owned)
                    .into_iter()
                    .collect()
            );
            prefix_claims = Some(seen.clone());
            let saved = serde_json::to_vec(&next).unwrap();
            drop(provider);
            provider = LocalBundle::open(&path.0, Limits::default()).unwrap();
            request.page.after = Some(serde_json::from_slice(&saved).unwrap());
        } else {
            request.page.after = Some(next);
        }
    }
    assert_eq!(scanned, provider.stats().counts.relations);
    assert_eq!(suffix_scanned, provider.stats().counts.relations - 5);
    let prefix_claims = prefix_claims.expect("nonzero cursor was saved and resumed");
    assert_eq!(seen.difference(&prefix_claims).count(), 7);
    assert_eq!(
        seen,
        [
            "r:carries",
            "r:code-reference",
            "r:instance",
            "r:publishes",
            "r:reads",
            "r:request",
            "r:subscribes",
            "r:table-reference",
            "r:writes",
        ]
        .map(str::to_owned)
        .into_iter()
        .collect()
    );
    assert!(internal > 0);
}

#[test]
fn indexed_search_and_direct_child_windows_remain_revision_bound() {
    let temp = Temp::new();
    let mut writer = BundleWriter::create(&temp.0, manifest(), Limits::default()).unwrap();
    for record in records() {
        writer.push(&record).unwrap();
    }
    let mut provider = writer.finish().unwrap();
    let count = provider.child_count(7, "e:root").unwrap();
    let mut ids = std::collections::BTreeSet::new();
    for offset in 0..count {
        let page = provider.children_at(7, "e:root", offset, 1).unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(ids.insert(page.items[0].id.clone()));
    }
    assert_eq!(ids.len(), count);
    assert!(matches!(
        provider.children_at(7, "e:root", count + 1, 1),
        Err(BundleError::InvalidCursor)
    ));
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let first = provider
        .search(
            7,
            "table",
            PageRequest {
                limit: 1,
                after: None,
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(first.items[0].id, "e:table-a");
    assert!(matches!(
        provider.search(
            7,
            "service",
            PageRequest {
                limit: 1,
                after: first.next
            },
            &cancel
        ),
        Err(BundleError::InvalidCursor)
    ));
    assert!(matches!(
        provider.search(
            7,
            "table",
            PageRequest::default(),
            &std::sync::atomic::AtomicBool::new(true)
        ),
        Err(BundleError::Cancelled)
    ));
    assert_eq!(provider.stats().cached_records, 0);
}

#[test]
fn cancellation_during_final_validation_and_lens_build_never_publishes_a_partial_index() {
    const WIDTH: usize = 4096;
    let path = Temp::new();
    let mut model = manifest();
    model.root_id = "e:0000".into();
    let mut writer = BundleWriter::create(&path.0, model, Limits::default()).unwrap();
    for i in 0..WIDTH {
        writer
            .push(&entity(&format!("d:{i:04}"), EntityKind::Deployment))
            .unwrap();
    }
    for i in 0..WIDTH {
        writer
            .push(&entity(&format!("e:{i:04}"), EntityKind::Scope))
            .unwrap();
    }
    // A wide reverse-ordered sibling set exercises cancellable child sorting.
    for i in 1..WIDTH {
        writer
            .push(&relation(
                &format!("r:contains:{i:04}"),
                RelationKind::Contains,
                "e:0000",
                &format!("e:{:04}", WIDTH - i),
            ))
            .unwrap();
    }
    // A deep independent deployment forest exercises both forest walks and
    // lens interval construction, without recursive traversal or detail loads.
    for i in 1..WIDTH {
        writer
            .push(&relation(
                &format!("r:deployment:{i:04}"),
                RelationKind::DeployedIn,
                &format!("d:{i:04}"),
                &format!("d:{:04}", i - 1),
            ))
            .unwrap();
    }
    drop(writer.finish().unwrap());
    let original_bytes = fs::metadata(&path.0).unwrap().len();
    let mut completed_byte_callbacks = 0;
    let baseline = LocalBundle::open_with_progress(&path.0, Limits::default(), |progress| {
        if progress.bytes_read == progress.total_bytes {
            completed_byte_callbacks += 1;
        }
        ControlFlow::Continue(())
    })
    .unwrap();
    let expected = baseline.stats();
    assert!(
        completed_byte_callbacks > 30,
        "finalization must report cancellable checkpoints within its passes"
    );
    drop(baseline);
    // These deterministic callback positions include early and late finalization.
    // The first full-byte callback precedes finish; its final two checkpoints are
    // finish completion and the outer completion report. Neither is selected.
    for cancel_at in [
        5,
        completed_byte_callbacks / 3,
        completed_byte_callbacks * 2 / 3,
        completed_byte_callbacks - 2,
    ] {
        let mut reached = 0;
        let result = LocalBundle::open_with_progress(&path.0, Limits::default(), |progress| {
            if progress.bytes_read == progress.total_bytes {
                reached += 1;
            }
            if reached == cancel_at {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert!(matches!(result, Err(BundleError::Cancelled)));
        assert_eq!(reached, cancel_at);
        assert_eq!(fs::metadata(&path.0).unwrap().len(), original_bytes);
    }
    let mut retried = LocalBundle::open(&path.0, Limits::default()).unwrap();
    assert_eq!(retried.stats().counts, expected.counts);
    assert_eq!(
        retried.stats().estimated_index_bytes,
        expected.estimated_index_bytes
    );
    assert_eq!(retried.stats().cached_records, 0);
    let children = retried.children_at(7, "e:0000", 0, 128).unwrap();
    assert_eq!(children.items.first().unwrap().id, "e:0001");
    assert_eq!(children.items.last().unwrap().id, "e:0128");
    assert_eq!(
        retried
            .child_count_in(7, SemanticLens::Deployment, "e:0000")
            .unwrap(),
        1
    );
    let nested = retried
        .children_at_in(7, SemanticLens::Deployment, "d:2048", 0, 2)
        .unwrap();
    assert_eq!(nested.items[0].id, "d:2049");
    let boundary = retried
        .region_relations(
            7,
            RegionRequest {
                focus: "d:2048".into(),
                lens: Some(SemanticLens::Deployment),
                visible: vec!["d:2048".into(), "d:2049".into()],
                scope_roots: vec![],
                page: PageRequest::default(),
                scan_budget: 8192,
                reference_budget: 8,
                existing_references: vec![],
            },
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(boundary.items.len(), 1);
    assert_eq!(boundary.items[0].relation.source, "d:2049");
    assert_eq!(boundary.items[0].relation.target, "d:2048");
}
