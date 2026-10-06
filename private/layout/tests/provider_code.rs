use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use visualyze_layout::provider_projection::{
    BundleProjection, ProviderViewRequest, project_provider,
};
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

fn code_fixture() -> Fixture {
    code_fixture_with_fanout(false)
}

fn code_fixture_with_fanout(fanout: bool) -> Fixture {
    code_fixture_sized(fanout, 130)
}

fn code_fixture_sized(fanout: bool, file_count: usize) -> Fixture {
    code_fixture_with_external_peers(fanout, file_count, 0)
}

fn code_fixture_with_external_peers(
    fanout: bool,
    file_count: usize,
    external_count: usize,
) -> Fixture {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "visualyze-code-projection-{}-{}.jsonl",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut writer = BundleWriter::create(
        &path,
        Manifest {
            format: FORMAT_VERSION.into(),
            model_id: "bounded-code".into(),
            revision: 3,
            root_id: "root".into(),
            profiles: vec![Profile::Architecture, Profile::Code],
            coverage: Coverage::Complete,
            scope_description: "Declared ownership and reference fixture".into(),
            omissions: vec![],
        },
        Limits::default(),
    )
    .unwrap();
    writer
        .push(&BundleRecord::Evidence(EvidenceRecord {
            id: "evidence:code".into(),
            description: "Declared fixture membership".into(),
            reference: Some("fixture://code".into()),
        }))
        .unwrap();
    let mut entities = BTreeMap::new();
    let mut relations = BTreeMap::new();
    let mut add_entity = |id: String, kind| {
        entities.insert(
            id.clone(),
            BundleRecord::Entity(EntityRecord {
                summary: EntitySummary {
                    id: id.clone(),
                    kind,
                    label: id,
                    boundary: Boundary::Internal,
                    claim: Claim::declared(),
                },
                detail: Some(EntityDetail {
                    description: "Demand-loaded code detail".into(),
                    subtype: None,
                    roles: vec![],
                    technology: vec![],
                }),
            }),
        );
    };
    let mut add_relation = |id: String, kind, source: String, target: String| {
        relations.insert(
            id.clone(),
            BundleRecord::Relation(RelationRecord {
                id,
                kind,
                source,
                target,
                claim: Claim {
                    basis: Basis::Supported,
                    evidence_ids: vec!["evidence:code".into()],
                    rationale: None,
                },
                label: Some("Supplied relation".into()),
                aspect: None,
            }),
        );
    };
    for (id, kind) in [
        ("root", EntityKind::Scope),
        ("repo", EntityKind::CodeRepository),
        ("package:a", EntityKind::CodePackage),
        ("package:b", EntityKind::CodePackage),
    ] {
        add_entity(id.into(), kind);
    }
    add_relation(
        "owns:repo".into(),
        RelationKind::Contains,
        "root".into(),
        "repo".into(),
    );
    for package in ["a", "b"] {
        add_relation(
            format!("owns:package:{package}"),
            RelationKind::OwnsCode,
            "repo".into(),
            format!("package:{package}"),
        );
        for i in 0..if package == "a" { file_count } else { 1 } {
            let file = format!("file:{package}:{i:03}");
            add_entity(file.clone(), EntityKind::CodeFile);
            add_relation(
                format!("owns:{file}"),
                RelationKind::OwnsCode,
                format!("package:{package}"),
                file.clone(),
            );
            for j in 0..4 {
                let function = format!("function:{package}:{i:03}:{j}");
                add_entity(function.clone(), EntityKind::CodeFunction);
                add_relation(
                    format!("owns:{function}"),
                    RelationKind::OwnsCode,
                    file.clone(),
                    function,
                );
            }
        }
    }
    for (id, source, target) in [
        ("call:a", "function:a:000:0", "function:b:000:0"),
        ("call:b", "function:a:001:0", "function:b:000:1"),
        ("call:c", "function:b:000:2", "function:a:129:0"),
        ("call:internal", "function:a:000:0", "function:a:000:1"),
    ] {
        add_relation(
            id.into(),
            RelationKind::CodeReferences,
            source.into(),
            target.into(),
        );
    }
    if fanout {
        for i in 1..130 {
            add_relation(
                format!("fan:{i:03}"),
                RelationKind::CodeReferences,
                "function:a:000:0".into(),
                format!("function:a:{i:03}:0"),
            );
        }
    }
    for i in 0..external_count {
        let package = format!("package:external:{i:03}");
        let file = format!("file:external:{i:03}");
        let function = format!("function:external:{i:03}");
        add_entity(package.clone(), EntityKind::CodePackage);
        add_entity(file.clone(), EntityKind::CodeFile);
        add_entity(function.clone(), EntityKind::CodeFunction);
        for (level, source, target) in [
            ("package", "repo".to_owned(), package.clone()),
            ("file", package, file.clone()),
            ("function", file, function.clone()),
        ] {
            add_relation(
                format!("owns:external:{i:03}:{level}"),
                RelationKind::OwnsCode,
                source,
                target,
            );
        }
        // Interleave genuine internal attraction with disjoint outside peers.
        // A smaller reference budget must not resample the provisional groups.
        for (suffix, target) in [
            ("inside", format!("function:a:{:03}:0", (i * 17 + 7) % 128)),
            ("outside", function),
        ] {
            add_relation(
                format!("external:{i:03}:{suffix}"),
                RelationKind::CodeReferences,
                format!("function:a:{:03}:0", i % 128),
                target,
            );
        }
    }
    for entity in entities.into_values() {
        writer.push(&entity).unwrap();
    }
    for relation in relations.into_values() {
        writer.push(&relation).unwrap();
    }
    Fixture {
        bundle: writer.finish().unwrap(),
        path,
    }
}

/// A logical level that paging cannot make uniformly quiet. `hub` holds 128
/// children in alternating layers (a datastore and a service per pair), so its
/// first window of 128 measures as an even mix, then `tail` scopes in a single
/// layer. Pages are cut from the even mix (38 children), so later pages made of
/// tail services alone exceed the fallback noise and get suggested groups, while
/// the earlier pages do not. Every tail service exposes one interface and calls
/// the next tail service. With `outside_claims`, `outside` services beside `hub`
/// are each called by one tail service, giving the hub many disjoint references;
/// `inside_claims` adds pairs of tail services that attract each other.
fn hub_fixture(tail: usize, outside: usize, outside_claims: bool) -> Fixture {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "visualyze-hub-projection-{}-{}.jsonl",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut writer = BundleWriter::create(
        &path,
        Manifest {
            format: FORMAT_VERSION.into(),
            model_id: "hub".into(),
            revision: 3,
            root_id: "root".into(),
            profiles: vec![Profile::Architecture],
            coverage: Coverage::Complete,
            scope_description: "Mixed-layer level fixture".into(),
            omissions: vec![],
        },
        Limits::default(),
    )
    .unwrap();
    let mut entities = BTreeMap::new();
    let mut relations = BTreeMap::new();
    let mut add_entity = |id: String, kind| {
        entities.insert(
            id.clone(),
            BundleRecord::Entity(EntityRecord {
                summary: EntitySummary {
                    id: id.clone(),
                    kind,
                    label: id,
                    boundary: Boundary::Internal,
                    claim: Claim::declared(),
                },
                detail: None,
            }),
        );
    };
    let mut add_relation = |kind, source: &str, target: &str| {
        let id = format!("r:{}:{source}:{target}", format!("{kind:?}").to_lowercase());
        relations.insert(
            id.clone(),
            BundleRecord::Relation(RelationRecord {
                id,
                kind,
                source: source.into(),
                target: target.into(),
                claim: Claim::declared(),
                label: None,
                aspect: None,
            }),
        );
    };
    add_entity("root".into(), EntityKind::Scope);
    add_entity("hub".into(), EntityKind::Scope);
    add_relation(RelationKind::Contains, "root", "hub");
    for i in 0..64 {
        let (store, service) = (format!("m:{i:03}:d"), format!("m:{i:03}:s"));
        add_entity(store.clone(), EntityKind::Datastore);
        add_entity(service.clone(), EntityKind::Service);
        add_relation(RelationKind::Contains, "hub", &store);
        add_relation(RelationKind::Contains, "hub", &service);
        add_relation(RelationKind::Reads, &service, &store);
    }
    for j in 0..tail {
        let scope = format!("t:{j:03}");
        let service = format!("t:{j:03}:s");
        add_entity(scope.clone(), EntityKind::Scope);
        add_entity(service.clone(), EntityKind::Service);
        add_relation(RelationKind::Contains, "hub", &scope);
        add_relation(RelationKind::Contains, &scope, &service);
        if j + 1 < tail {
            add_relation(
                RelationKind::Requests,
                &service,
                &format!("t:{:03}:s", j + 1),
            );
        }
    }
    for k in 0..outside {
        let id = format!("outside:{k:03}");
        add_entity(id.clone(), EntityKind::Service);
        add_relation(RelationKind::Contains, "root", &id);
        if outside_claims {
            add_relation(
                RelationKind::Requests,
                &format!("t:{:03}:s", k % tail.max(1)),
                &id,
            );
        }
    }
    for entity in entities.into_values() {
        writer.push(&entity).unwrap();
    }
    for relation in relations.into_values() {
        writer.push(&relation).unwrap();
    }
    Fixture {
        bundle: writer.finish().unwrap(),
        path,
    }
}

/// The noise-driven page size of `hub` (an even mix of two layers), and the
/// first page that lies wholly in the single-layer tail.
const HUB_PAGE: usize = 38;
const HUB_TAIL_PAGE: usize = 4;

fn hub_page(fixture: &mut Fixture, request: ProviderViewRequest) -> BundleProjection {
    project_provider(
        &mut fixture.bundle,
        &ProviderViewRequest {
            focus: Some("hub".into()),
            ..request
        },
        &AtomicBool::new(false),
    )
    .unwrap()
}

#[test]
fn collapsed_subtrees_aggregate_direction_and_keep_inspectable_original_membership() {
    let mut fixture = code_fixture();
    assert!(fixture.bundle.stats().counts.entities > 512);
    let request = ProviderViewRequest {
        focus: Some("repo".into()),
        ..Default::default()
    };
    let projection =
        project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
    assert!(projection.complete);
    assert_eq!(projection.graph.nodes.len(), 3);
    assert_eq!(projection.graph.edges.len(), 2);
    let forward = projection
        .graph
        .edges
        .iter()
        .find(|e| e.source == "package:a")
        .unwrap();
    assert_eq!(forward.target, "package:b");
    assert_eq!(forward.semantic_ids, ["call:a", "call:b"]);
    let reverse = projection
        .graph
        .edges
        .iter()
        .find(|e| e.source == "package:b")
        .unwrap();
    assert_eq!(reverse.semantic_ids, ["call:c"]);
    assert_eq!(projection.relations["call:a"].source, "function:a:000:0");
    assert_eq!(
        projection.relations["call:a"].claim.evidence_ids,
        ["evidence:code"]
    );
    assert_eq!(
        projection.relations["call:a"].kind,
        RelationKind::CodeReferences
    );
    assert_eq!(projection.internal_relation_counts["package:a"], 1);
    assert!(
        projection
            .graph
            .internal_relation_ids
            .contains(&"call:internal".into())
    );
    assert_eq!(
        fixture.bundle.stats().cached_records,
        1,
        "only the focus detail may be loaded"
    );
}

#[test]
fn code_child_pages_cover_every_file_without_expanding_neighbor_regions() {
    let mut fixture = code_fixture();
    let mut ids = BTreeSet::new();
    let mut keys = BTreeSet::new();
    // 130 files share one layer, so noise (ADR 0007) pages them 24 at a time.
    for child_page in 0..6 {
        let request = ProviderViewRequest {
            focus: Some("package:a".into()),
            child_page,
            automatic_groups: false,
            ..Default::default()
        };
        let projection =
            project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
        assert_eq!(projection.total_children, 130);
        assert_eq!(projection.child_page_size, 24);
        assert!(projection.graph.nodes.len() <= 34);
        assert!(keys.insert(projection.graph.key));
        for entity in projection
            .entities
            .values()
            .filter(|e| e.kind == EntityKind::CodeFile)
        {
            assert!(ids.insert(entity.id.clone()));
        }
        assert!(!projection.entities.contains_key("function:a:000:0"));
        assert!(!projection.entities.contains_key("file:b:000"));
        assert!(
            projection
                .graph
                .references
                .values()
                .any(|id| id == "package:b")
        );
    }
    assert_eq!(ids.len(), 130);
    assert!(ids.contains("file:a:129"));
    assert!(
        project_provider(
            &mut fixture.bundle,
            &ProviderViewRequest {
                focus: Some("package:a".into()),
                child_page: 6,
                automatic_groups: false,
                ..Default::default()
            },
            &AtomicBool::new(false)
        )
        .is_err()
    );
}

#[test]
fn entering_a_file_reveals_internal_claims_and_external_assemblies_truthfully() {
    let mut fixture = code_fixture();
    let request = ProviderViewRequest {
        focus: Some("file:a:000".into()),
        ..Default::default()
    };
    let projection =
        project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
    assert!(projection.complete);
    let internal = &projection.relations["call:internal"];
    assert_eq!(internal.source, "function:a:000:0");
    assert_eq!(internal.target, "function:a:000:1");
    assert!(
        projection
            .graph
            .references
            .values()
            .any(|id| id == "package:b")
    );
    assert!(!projection.entities.contains_key("function:b:000:0"));
    assert!(
        projection
            .graph
            .edges
            .iter()
            .any(|e| e.id == "call:internal"
                && e.source == internal.source
                && e.target == internal.target)
    );
}

#[test]
fn bounded_relation_pages_reach_all_claims_without_false_whole_view_completeness() {
    use visualyze_layout::provider_projection::Continuation;
    let mut fixture = code_fixture();
    let mut request = ProviderViewRequest {
        focus: Some("repo".into()),
        max_relation_records: 1,
        ..Default::default()
    };
    let mut records = BTreeSet::new();
    let mut pages = 0;
    loop {
        let projection =
            project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
        pages += 1;
        assert!(!projection.complete);
        for id in projection.relations.keys() {
            assert!(
                records.insert(id.clone()),
                "claim repeated between disjoint pages"
            );
        }
        let next = projection
            .continuations
            .iter()
            .find_map(|continuation| match continuation {
                Continuation::Boundary { after } => Some(after.clone()),
                _ => None,
            });
        match next {
            Some(cursor) => request.boundary_after = Some(cursor),
            None => break,
        }
        assert!(pages < 20);
    }
    for id in ["call:a", "call:b", "call:c"] {
        assert!(records.contains(id));
    }
    assert!(pages > 1);
}

#[test]
fn leaf_symbols_remain_visible_and_mixed_expansion_keeps_neighbor_collapsed() {
    let mut fixture = code_fixture();
    let leaf = project_provider(
        &mut fixture.bundle,
        &ProviderViewRequest {
            focus: Some("function:a:000:0".into()),
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        leaf.graph
            .nodes
            .iter()
            .find(|n| n.id == "function:a:000:0")
            .unwrap()
            .solid
    );
    assert!(
        leaf.graph
            .nodes
            .iter()
            .any(|n| n.id.starts_with("view:reference:"))
    );
    let mixed = project_provider(
        &mut fixture.bundle,
        &ProviderViewRequest {
            focus: Some("repo".into()),
            expanded: BTreeSet::from(["package:a".into(), "file:a:000".into()]),
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(mixed.entities.contains_key("function:a:000:0"));
    assert!(!mixed.entities.contains_key("file:b:000"));
    assert!(
        mixed
            .graph
            .nodes
            .iter()
            .find(|n| n.id == "package:b")
            .unwrap()
            .solid
    );
    assert!(mixed.graph.nodes.len() <= 250);
    assert_eq!(
        mixed.graph.semantic_to_display["function:b:000:0"],
        "package:b"
    );
}

#[test]
fn reference_budget_pages_resume_before_overflow_and_reach_every_original_claim() {
    use visualyze_layout::provider_projection::Continuation;
    let mut fixture = code_fixture_with_fanout(true);
    let mut request = ProviderViewRequest {
        focus: Some("function:a:000:0".into()),
        max_references: 64,
        ..Default::default()
    };
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    loop {
        let projection =
            project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
        pages += 1;
        assert!(
            projection.unrepresented_relation_ids.is_empty(),
            "loaded claims must all have displayed endpoints"
        );
        // A focus without children leaves one layer's worth of slots (24) for
        // references, whatever `max_references` allows.
        assert!(projection.graph.references.len() <= 24);
        assert!(projection.graph.nodes.len() <= 250);
        let represented: BTreeSet<_> = projection
            .graph
            .edges
            .iter()
            .flat_map(|edge| edge.semantic_ids.iter())
            .collect();
        for relation in projection
            .relations
            .values()
            .filter(|r| !r.kind.is_ownership())
        {
            assert!(represented.contains(&relation.id));
            assert!(
                seen.insert(relation.id.clone()),
                "a resumed page repeated a claim"
            );
        }
        let next = projection
            .continuations
            .iter()
            .find_map(|continuation| match continuation {
                Continuation::Boundary { after } => Some(after.clone()),
                _ => None,
            });
        match next {
            Some(cursor) => {
                assert_ne!(request.boundary_after, Some(cursor.clone()));
                request.boundary_after = Some(cursor);
            }
            None => break,
        }
        assert!(pages < 10, "bounded pages must advance");
    }
    assert_eq!(pages, 6);
    assert_eq!(seen.len(), 131);
    assert!(seen.contains("fan:129"));
    assert!(seen.contains("call:a"));
    assert!(seen.contains("call:internal"));
}

#[test]
fn immediate_peer_budget_preserves_groups_and_pages_every_disjoint_external_claim() {
    use visualyze_layout::provider_projection::Continuation;
    // Same hub and chain, without outside claims: nothing needs a second
    // reference-budget pass, so these are the groups the abstraction should have.
    let mut quiet = hub_fixture(200, 0, false);
    let request = ProviderViewRequest {
        child_page: HUB_TAIL_PAGE,
        ..Default::default()
    };
    let baseline = hub_page(&mut quiet, request.clone());
    assert!(!baseline.graph.suggested_groups.is_empty());
    // Ninety-six disjoint outside peers want far more reference slots than a
    // page of groups leaves in one layer's budget.
    let mut fixture = hub_fixture(200, 96, true);
    let cancelled = AtomicBool::new(false);
    let first = hub_page(&mut fixture, request.clone());
    assert_eq!(
        first.graph.suggested_groups, baseline.graph.suggested_groups,
        "the smaller reference sample must not resample the groups"
    );
    let slots = visualyze_layout::noise::layer_capacity(visualyze_layout::noise::FALLBACK_NOISE)
        - first.graph.suggested_groups.len();
    assert_eq!(first.graph.references.len(), slots);
    assert!(!first.complete);
    assert!(first.region_diagnostics.contains_key("view:peer-budget"));
    let first_key = first.graph.key;
    let groups = first.graph.suggested_groups;
    let mut request = ProviderViewRequest {
        focus: Some("hub".into()),
        retained_groups: groups.clone(),
        ..request
    };
    // Visible on this page: tail scopes 024..=061, so their chain claims and
    // outside claims 024..=061 are the claims the page must eventually reach,
    // plus the chain claims that enter the page from 023 and leave to 062.
    let expected: BTreeSet<String> = (23..=61)
        .map(|j| format!("r:requests:t:{j:03}:s:t:{:03}:s", j + 1))
        .chain((24..=61).map(|k| format!("r:requests:t:{k:03}:s:outside:{k:03}")))
        .collect();
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = project_provider(&mut fixture.bundle, &request, &cancelled).unwrap();
        pages += 1;
        assert_eq!(page.graph.suggested_groups, groups);
        assert!(page.unrepresented_relation_ids.is_empty());
        assert!(page.graph.nodes.len() <= 250);
        assert!(
            page.graph
                .nodes
                .iter()
                .filter(|node| node.parent.as_ref() == Some(&page.graph.root))
                .count()
                <= 24 + 2
        );
        let represented: BTreeSet<_> = page
            .graph
            .edges
            .iter()
            .flat_map(|edge| edge.semantic_ids.iter())
            .chain(page.graph.internal_relation_ids.iter())
            .collect();
        for relation in page
            .relations
            .values()
            .filter(|relation| !relation.kind.is_ownership())
        {
            assert!(represented.contains(&relation.id));
        }
        for id in represented.into_iter().filter(|id| expected.contains(*id)) {
            assert!(seen.insert(id.clone()), "resumed page repeated {id}");
        }
        let next = page
            .continuations
            .iter()
            .find_map(|continuation| match continuation {
                Continuation::Boundary { after } => Some(after.clone()),
                _ => None,
            });
        let Some(cursor) = next else { break };
        assert_ne!(request.boundary_after, Some(cursor.clone()));
        request.boundary_after = Some(cursor);
        assert!(pages < 20, "every deferred claim must remain reachable");
    }
    assert!(pages >= 3);
    assert_eq!(seen, expected);
    request.boundary_after = None;
    let returned = project_provider(&mut fixture.bundle, &request, &cancelled).unwrap();
    assert_eq!(returned.graph.key, first_key);
    assert_eq!(returned.graph.suggested_groups, groups);
}

#[test]
fn explicit_expansion_is_never_refused_on_noise_and_respects_the_page_and_budget() {
    let mut fixture = code_fixture_with_external_peers(false, 130, 96);
    let group = visualyze_layout::projection::DisplayGroup {
        id: "view:local-group:expanded".into(),
        parent: "package:a".into(),
        members: (0..128).map(|i| format!("file:a:{i:03}")).collect(),
        suggested: false,
    };
    let members_under = |projection: &visualyze_layout::provider_projection::BundleProjection| {
        projection
            .graph
            .nodes
            .iter()
            .filter(|node| node.parent.as_ref() == Some(&group.id))
            .count()
    };
    let request = ProviderViewRequest {
        focus: Some("package:a".into()),
        groups: vec![group.clone()],
        expanded: BTreeSet::from([group.id.clone()]),
        ..Default::default()
    };
    // Paged by noise: the expanded group shows the members on this page and
    // says the rest are on other pages.
    let paged = project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
    assert_eq!(members_under(&paged), 24);
    assert!(paged.region_diagnostics[&group.id].contains("other child pages"));
    assert!(paged.graph.nodes.len() <= 250);
    // Without noise paging, explicit expansion shows all 128 members and is
    // limited only by the visible-object budget.
    let whole = project_provider(
        &mut fixture.bundle,
        &ProviderViewRequest {
            page_by_noise: false,
            ..request
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(members_under(&whole), 128);
    assert!(whole.graph.nodes.len() <= 250);
    assert!(whole.unrepresented_relation_ids.is_empty());
}

#[test]
fn explicit_semantic_expansion_opens_only_its_fresh_or_retained_containing_group() {
    let mut fixture = hub_fixture(200, 96, true);
    let base = ProviderViewRequest {
        child_page: HUB_TAIL_PAGE,
        ..Default::default()
    };
    let collapsed = hub_page(&mut fixture, base.clone());
    for retained_groups in [Vec::new(), collapsed.graph.suggested_groups.clone()] {
        let request = ProviderViewRequest {
            expanded: BTreeSet::from(["t:030".into()]),
            retained_groups,
            ..base.clone()
        };
        let expanded = hub_page(&mut fixture, request);
        let group = expanded
            .graph
            .suggested_groups
            .iter()
            .find(|group| group.members.iter().any(|member| member == "t:030"))
            .unwrap();
        // Expansion does not change which groups the page has.
        assert_eq!(
            expanded.graph.suggested_groups,
            collapsed.graph.suggested_groups
        );
        assert!(
            expanded.region_diagnostics.contains_key("view:peer-budget"),
            "the regression must exercise the second reference-budget pass"
        );
        assert!(expanded.region_diagnostics[&group.id].contains("explicit member expansion"));
        assert!(expanded.unrepresented_relation_ids.is_empty());
        assert!(expanded.graph.nodes.len() <= 250);
        let nodes: BTreeMap<_, _> = expanded
            .graph
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        assert!(!nodes[group.id.as_str()].solid);
        assert!(!nodes["t:030"].solid);
        assert_eq!(nodes["t:030"].parent.as_ref(), Some(&group.id));
        assert_eq!(nodes["t:030:s"].parent.as_deref(), Some("t:030"));
        assert_eq!(expanded.graph.semantic_to_display["t:030:s"], "t:030:s");
        for sibling in group.members.iter().filter(|id| id.as_str() != "t:030") {
            assert!(nodes[sibling.as_str()].solid);
            assert!(
                !expanded
                    .graph
                    .nodes
                    .iter()
                    .any(|node| node.parent.as_ref() == Some(sibling))
            );
        }
        for other in expanded
            .graph
            .suggested_groups
            .iter()
            .filter(|other| other.id != group.id)
        {
            assert!(nodes[other.id.as_str()].solid);
        }
    }
}

#[test]
fn manual_group_reports_and_respects_explicit_member_expansion() {
    let mut fixture = code_fixture();
    let group = local_group("package:a", &["file:a:000", "file:a:001"]);
    let projection = project_provider(
        &mut fixture.bundle,
        &ProviderViewRequest {
            focus: Some("package:a".into()),
            expanded: BTreeSet::from(["file:a:000".into()]),
            groups: vec![group.clone()],
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        projection
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "function:a:000:0")
    );
    assert!(
        !projection
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "function:a:001:0")
    );
    assert!(projection.region_diagnostics[&group.id].contains("explicit member expansion"));
    assert_eq!(
        projection
            .graph
            .suggested_groups
            .iter()
            .find(|g| g.id == group.id),
        Some(&group)
    );
}

fn local_group(parent: &str, members: &[&str]) -> visualyze_layout::projection::DisplayGroup {
    visualyze_layout::projection::DisplayGroup {
        id: "view:local-group:test".into(),
        parent: parent.into(),
        members: members.iter().map(|id| (*id).into()).collect(),
        suggested: false,
    }
}

#[test]
fn local_groups_collapse_expand_and_enter_without_fabricating_ownership() {
    use visualyze_layout::{Algorithm, LayoutRequest, QualityProfile, solve, validate};
    let mut fixture = code_fixture();
    let group = local_group("repo", &["package:a", "package:b"]);
    let mut request = ProviderViewRequest {
        focus: Some("repo".into()),
        groups: vec![group.clone()],
        ..Default::default()
    };
    let collapsed =
        project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
    assert_eq!(collapsed.graph.nodes.len(), 2);
    assert_eq!(collapsed.graph.internal_relation_ids.len(), 4);
    assert_eq!(collapsed.internal_relation_counts[&group.id], 4);
    assert!(collapsed.graph.edges.is_empty());
    assert_eq!(collapsed.graph.semantic_to_display["package:a"], group.id);
    assert_eq!(
        collapsed.graph.semantic_to_display["function:a:000:0"],
        group.id
    );
    assert_eq!(collapsed.relations["owns:package:a"].source, "repo");
    assert!(!collapsed.graph.suggested_groups[0].suggested);
    request.expanded.insert(group.id.clone());
    let expanded =
        project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        expanded
            .graph
            .nodes
            .iter()
            .find(|n| n.id == "package:a")
            .unwrap()
            .parent
            .as_deref(),
        Some(group.id.as_str())
    );
    assert!(
        !expanded
            .graph
            .nodes
            .iter()
            .find(|n| n.id == group.id)
            .unwrap()
            .solid
    );
    assert_eq!(expanded.graph.edges.len(), 2);
    assert_ne!(collapsed.graph.key, expanded.graph.key);
    request.focus = Some(group.id.clone());
    let focused = project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
    assert_eq!(focused.graph.root, group.id);
    assert_eq!(focused.total_children, 2);
    assert!(!focused.graph.nodes.iter().any(|n| n.id == "repo"));
    assert_eq!(
        focused
            .graph
            .nodes
            .iter()
            .find(|n| n.id == group.id)
            .unwrap()
            .parent,
        None
    );
    assert_eq!(
        fixture.bundle.owner_id(3, "package:a").unwrap().as_deref(),
        Some("repo")
    );
    for graph in [&collapsed.graph, &expanded.graph, &focused.graph] {
        let pins = BTreeMap::new();
        let solution = solve(LayoutRequest {
            projection: graph,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 8,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        validate(graph, &solution, &pins, 2).unwrap();
    }
}

#[test]
fn focused_group_pages_members_and_routes_to_true_external_siblings() {
    let mut fixture = code_fixture_with_fanout(true);
    let group = local_group("package:a", &["file:a:000", "file:a:001"]);
    let request = ProviderViewRequest {
        focus: Some(group.id.clone()),
        groups: vec![group.clone()],
        max_children: 1,
        ..Default::default()
    };
    let first = project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
    assert_eq!(first.total_children, 2);
    assert!(!first.complete);
    assert!(first.entities.contains_key("file:a:000"));
    assert!(!first.entities.contains_key("file:a:001"));
    let endpoint = &first.graph.semantic_to_display["function:a:002:0"];
    assert_eq!(first.graph.references[endpoint], "file:a:002");
    assert!(
        first
            .graph
            .edges
            .iter()
            .any(|edge| edge.semantic_ids.contains(&"fan:002".into()))
    );
    let second = project_provider(
        &mut fixture.bundle,
        &ProviderViewRequest {
            child_page: 1,
            ..request
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(second.entities.contains_key("file:a:001"));
    assert!(!second.entities.contains_key("file:a:000"));
    assert_ne!(first.graph.key, second.graph.key);
    assert!(second.graph.nodes.len() <= 250);
    assert!(
        second.unrepresented_relation_ids.is_empty(),
        "unrepresented={:?}; refs={:?}",
        second.unrepresented_relation_ids,
        second.graph.references
    );
}

#[test]
fn local_groups_reject_cross_owner_duplicate_and_semantic_identity_collisions() {
    let mut fixture = code_fixture();
    let mut group = local_group("package:a", &["file:a:000", "file:b:000"]);
    for invalid in [
        group.clone(),
        {
            group.members = vec!["file:a:000".into(), "file:a:000".into()];
            group.clone()
        },
        {
            group.id = "package:a".into();
            group.members = vec!["file:a:000".into(), "file:a:001".into()];
            group.clone()
        },
    ] {
        assert!(
            project_provider(
                &mut fixture.bundle,
                &ProviderViewRequest {
                    groups: vec![invalid],
                    ..Default::default()
                },
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
}

#[test]
fn suggested_groups_run_only_when_paging_cannot_quiet_a_page_and_are_bounded_repeatable_enterable_and_dismissible()
 {
    let mut fixture = hub_fixture(200, 0, false);
    // Paging by noise is enough for the evenly mixed pages: no invented groups.
    for child_page in 0..HUB_TAIL_PAGE - 1 {
        let page = hub_page(
            &mut fixture,
            ProviderViewRequest {
                child_page,
                ..Default::default()
            },
        );
        assert_eq!(page.child_page_size, HUB_PAGE);
        assert!(page.graph.suggested_groups.is_empty(), "page {child_page}");
        assert_eq!(page.graph.nodes.len(), HUB_PAGE + 1);
    }
    // A page of tail scopes alone is one noisy layer; only then does fallback
    // grouping run, with a count one readable layer can hold.
    let request = ProviderViewRequest {
        child_page: HUB_TAIL_PAGE,
        ..Default::default()
    };
    let first = hub_page(&mut fixture, request.clone());
    assert_eq!(first.child_page_size, HUB_PAGE);
    let fallback =
        visualyze_layout::noise::layer_capacity(visualyze_layout::noise::NOISE_THRESHOLD);
    assert!(first.graph.suggested_groups.len() <= fallback);
    assert!(first.graph.suggested_groups.len() >= 2);
    assert_eq!(
        first
            .graph
            .suggested_groups
            .iter()
            .map(|group| group.members.len())
            .sum::<usize>(),
        HUB_PAGE
    );
    assert!(
        first
            .graph
            .suggested_groups
            .iter()
            .all(|group| group.suggested && group.members.len() <= HUB_PAGE.div_ceil(fallback))
    );
    let group = first.graph.suggested_groups[0].clone();
    let repeated = hub_page(
        &mut fixture,
        ProviderViewRequest {
            retained_groups: first.graph.suggested_groups.clone(),
            ..request.clone()
        },
    );
    assert_eq!(first.graph.key, repeated.graph.key);
    let focused = project_provider(
        &mut fixture.bundle,
        &ProviderViewRequest {
            focus: Some(group.id.clone()),
            retained_groups: first.graph.suggested_groups.clone(),
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(focused.graph.root, group.id);
    assert_eq!(focused.total_children, group.members.len());
    assert!(
        focused
            .entities
            .keys()
            .filter(|id| id.starts_with("t:") && !id.ends_with(":s"))
            .all(|id| group.members.contains(id))
    );
    assert!(focused.graph.nodes.len() <= 250);
    let expanded = hub_page(
        &mut fixture,
        ProviderViewRequest {
            expanded: BTreeSet::from([group.id.clone()]),
            retained_groups: first.graph.suggested_groups.clone(),
            ..request.clone()
        },
    );
    for id in &group.members {
        assert_eq!(
            expanded
                .graph
                .nodes
                .iter()
                .find(|node| &node.id == id)
                .unwrap()
                .parent
                .as_deref(),
            Some(group.id.as_str())
        );
        assert_eq!(
            fixture.bundle.owner_id(3, id).unwrap().as_deref(),
            Some("hub")
        );
    }
    let dismissed = hub_page(
        &mut fixture,
        ProviderViewRequest {
            retained_groups: first.graph.suggested_groups.clone(),
            dismissed_groups: BTreeSet::from([group.id.clone()]),
            ..request
        },
    );
    assert!(
        !dismissed
            .graph
            .suggested_groups
            .iter()
            .any(|candidate| candidate.id == group.id)
    );
    assert!(
        group
            .members
            .iter()
            .all(|id| dismissed.graph.nodes.iter().any(|node| &node.id == id))
    );
}

#[test]
fn manual_groups_can_span_child_pages_without_creating_unreachable_members() {
    let mut fixture = code_fixture();
    let group = local_group("package:a", &["file:a:000", "file:a:129"]);
    // Pages hold 24 files: page 0 has file 000, page 5 (files 120-129) has file 129.
    for child_page in [0, 5] {
        let projection = project_provider(
            &mut fixture.bundle,
            &ProviderViewRequest {
                focus: Some("package:a".into()),
                groups: vec![group.clone()],
                child_page,
                automatic_groups: false,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            projection
                .graph
                .nodes
                .iter()
                .any(|node| node.id == group.id)
        );
        assert_eq!(projection.graph.semantic_to_display["file:a:129"], group.id);
        assert_eq!(
            projection.graph.semantic_to_display["function:a:129:0"],
            group.id
        );
        assert!(projection.unrepresented_relation_ids.is_empty());
    }
    let focused = project_provider(
        &mut fixture.bundle,
        &ProviderViewRequest {
            focus: Some(group.id.clone()),
            groups: vec![group.clone()],
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(focused.total_children, 2);
    assert!(focused.entities.contains_key("file:a:000"));
    assert!(focused.entities.contains_key("file:a:129"));
}

#[test]
fn child_windows_do_not_skip_records_when_storage_pages_hit_the_byte_limit() {
    let path = std::env::temp_dir().join(format!(
        "visualyze-large-child-window-{}.jsonl",
        std::process::id()
    ));
    let limits = Limits {
        max_page_bytes: 64 * 1024,
        ..Default::default()
    };
    let mut writer = BundleWriter::create(
        &path,
        Manifest {
            format: FORMAT_VERSION.into(),
            model_id: "byte-window".into(),
            revision: 1,
            root_id: "e:root".into(),
            profiles: vec![Profile::Architecture],
            coverage: Coverage::Complete,
            scope_description: "Declared byte window test".into(),
            omissions: vec![],
        },
        limits,
    )
    .unwrap();
    for id in ["e:000", "e:001", "e:002", "e:003", "e:root"] {
        writer
            .push(&BundleRecord::Entity(EntityRecord {
                summary: EntitySummary {
                    id: id.into(),
                    kind: if id == "e:root" {
                        EntityKind::Scope
                    } else {
                        EntityKind::Service
                    },
                    label: id.into(),
                    boundary: Boundary::Internal,
                    claim: Claim::declared(),
                },
                detail: Some(EntityDetail {
                    description: "x".repeat(32768),
                    subtype: None,
                    roles: vec![],
                    technology: vec![],
                }),
            }))
            .unwrap();
    }
    for i in 0..4 {
        writer
            .push(&BundleRecord::Relation(RelationRecord {
                id: format!("r:{i}"),
                kind: RelationKind::Contains,
                source: "e:root".into(),
                target: format!("e:{i:03}"),
                claim: Claim::declared(),
                label: None,
                aspect: None,
            }))
            .unwrap();
    }
    let mut fixture = Fixture {
        bundle: writer.finish().unwrap(),
        path,
    };
    let mut seen = BTreeSet::new();
    for child_page in 0..2 {
        let projection = project_provider(
            &mut fixture.bundle,
            &ProviderViewRequest {
                max_children: 3,
                child_page,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        for id in projection
            .entities
            .keys()
            .filter(|id| id.as_str() != "e:root")
        {
            assert!(seen.insert(id.clone()));
        }
        assert_eq!(
            projection.entities.len(),
            if child_page == 0 { 4 } else { 2 }
        );
    }
    assert_eq!(seen.len(), 4);
}

#[test]
fn semantic_lenses_filter_before_paging_and_keep_other_view_overrides_dormant() {
    let mut bundle = LocalBundle::open(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/examples/data-messaging.bundle.jsonl"),
        Limits::default(),
    )
    .unwrap();
    let request = ProviderViewRequest {
        lens: Some(SemanticLens::Code),
        max_children: 1,
        groups: vec![local_group(
            "scope:orders",
            &["client:web", "service:orders"],
        )],
        ..Default::default()
    };
    let p = project_provider(&mut bundle, &request, &AtomicBool::new(false)).unwrap();
    assert_eq!(p.total_children, 1);
    assert!(p.entities.contains_key("code:repository"));
    assert!(!p.entities.contains_key("client:web"));
    assert!(p.graph.suggested_groups.is_empty());
    assert_eq!(p.entities.len(), 2);
    let mut logical = request.clone();
    logical.lens = Some(SemanticLens::Logical);
    logical.focus = Some("code:package".into());
    assert!(project_provider(&mut bundle, &logical, &AtomicBool::new(false)).is_err());
}

#[test]
fn held_out_ai_logical_view_excludes_deployment_references_and_retains_typed_claims() {
    let mut bundle = LocalBundle::open(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/distributed-ai.bundle.jsonl"),
        Limits::default(),
    )
    .unwrap();
    let p = project_provider(
        &mut bundle,
        &ProviderViewRequest {
            lens: Some(SemanticLens::Logical),
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(p.entities.values().all(|entity| !matches!(
        entity.kind,
        EntityKind::Deployment | EntityKind::DeploymentInstance
    )));
    assert!(
        p.graph
            .references
            .values()
            .all(|id| !id.starts_with("deployment:") && !id.starts_with("instance:"))
    );
    assert!(
        p.relations
            .values()
            .any(|relation| relation.kind == RelationKind::Publishes)
    );
    assert!(p.relations.values().all(|relation| !matches!(
        relation.kind,
        RelationKind::InstanceOf | RelationKind::DeployedIn
    )));
    assert!(p.unrepresented_relation_ids.is_empty());
    assert!(p.graph.nodes.len() < 20);
}

#[test]
fn held_out_ai_deployment_view_navigates_independent_forest_and_preserves_instance_claims() {
    let mut bundle = LocalBundle::open(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/distributed-ai.bundle.jsonl"),
        Limits::default(),
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    let request = ProviderViewRequest {
        lens: Some(SemanticLens::Deployment),
        ..Default::default()
    };
    let overview = project_provider(&mut bundle, &request, &cancel).unwrap();
    assert_eq!(overview.total_children, 1);
    assert_eq!(
        overview
            .graph
            .nodes
            .iter()
            .find(|node| node.id == "deployment:global")
            .unwrap()
            .parent
            .as_deref(),
        Some("scope:ai-platform")
    );
    assert!(
        overview
            .relations
            .values()
            .all(|relation| relation.source != "scope:ai-platform")
    );
    let region = project_provider(
        &mut bundle,
        &ProviderViewRequest {
            focus: Some("deployment:region-00-accelerators".into()),
            ..request
        },
        &cancel,
    )
    .unwrap();
    assert_eq!(region.total_children, 12);
    assert!(
        region
            .graph
            .nodes
            .iter()
            .filter(|node| node.id.starts_with("instance:"))
            .all(|node| node.parent.as_deref() == Some("deployment:region-00-accelerators"))
    );
    let original = region
        .relations
        .values()
        .find(|relation| {
            relation.kind == RelationKind::InstanceOf
                && relation.source == "instance:region-00-worker-000"
        })
        .unwrap();
    assert_eq!(original.target, "service:worker-000");
    assert_eq!(
        bundle.relation(1, &original.id).unwrap().as_ref(),
        Some(original)
    );
    assert!(
        region
            .graph
            .edges
            .iter()
            .any(|edge| edge.semantic_ids.contains(&original.id))
    );
    assert!(
        region
            .graph
            .references
            .values()
            .any(|id| id == "service:worker-000")
    );
    assert!(region.unrepresented_relation_ids.is_empty());
}

#[test]
fn retained_groups_from_prior_pages_do_not_prevent_bounded_coarsening_of_later_pages() {
    // 300 tail scopes: tail pages 4 through 10 are each a full page.
    let mut fixture = hub_fixture(300, 0, false);
    let mut retained = Vec::new();
    let mut seen = BTreeSet::new();
    for child_page in HUB_TAIL_PAGE..=10 {
        let p = hub_page(
            &mut fixture,
            ProviderViewRequest {
                child_page,
                retained_groups: retained.clone(),
                ..Default::default()
            },
        );
        assert_eq!(p.child_page_size, HUB_PAGE);
        assert!(p.graph.nodes.len() <= 40);
        assert!(!p.graph.suggested_groups.is_empty());
        for group in &p.graph.suggested_groups {
            for id in &group.members {
                assert!(seen.insert(id.clone()));
            }
        }
        retained.extend(p.graph.suggested_groups);
        if retained.len() > 64 {
            retained.drain(..retained.len() - 64);
        }
    }
    assert_eq!(seen.len(), 7 * HUB_PAGE);
}

#[test]
#[ignore = "requires the explicitly generated Bevy production bundle"]
fn production_navigation_preserves_group_membership_after_other_regions_fill_retention() {
    use visualyze_layout::projection::ViewState;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../visualyze-local/production/bevy-0.18.1.bundle.jsonl");
    let mut provider = LocalBundle::open(path, Limits::default()).unwrap();
    let mut view = ViewState::default();
    let project = |provider: &mut LocalBundle, view: &mut ViewState| {
        let p = project_provider(
            provider,
            &ProviderViewRequest {
                lens: Some(SemanticLens::Code),
                focus: view.focus.clone(),
                child_page: view.child_page,
                boundary_after: view.relation_cursor.clone(),
                retained_groups: view.retained_suggestions().to_vec(),
                groups: view.group_overrides.clone(),
                expanded: view.expanded_ids(),
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        view.remember_groups(&p.graph);
        p
    };
    project(&mut provider, &mut view);
    view.enter("repository:05317a01f80959f2");
    project(&mut provider, &mut view);
    view.enter("package:d1c2531f3a0a9b57");
    let original = project(&mut provider, &mut view);
    view.child_page = 1;
    project(&mut provider, &mut view);
    view.child_page = 0;
    assert_eq!(
        original.graph.key,
        project(&mut provider, &mut view).graph.key
    );
    view.enter("file:b6ae9420da8c77aa");
    let file = project(&mut provider, &mut view);
    view.relation_cursor = file.continuations.iter().find_map(|next| match next {
        visualyze_layout::provider_projection::Continuation::Boundary { after } => {
            Some(after.clone())
        }
        _ => None,
    });
    assert!(view.relation_cursor.is_some());
    project(&mut provider, &mut view);
    view.relation_cursor = None;
    project(&mut provider, &mut view);
    view.enter("item:86c82b85a551c18a");
    project(&mut provider, &mut view);
    view.child_page = 1;
    project(&mut provider, &mut view);
    view.child_page = 0;
    project(&mut provider, &mut view);
    view.enter("item:a6ee753440d7d417");
    project(&mut provider, &mut view);
    for _ in 0..2 {
        assert!(view.back());
        project(&mut provider, &mut view);
    }
    assert!(view.back());
    let returned = project(&mut provider, &mut view);
    assert_eq!(
        original.graph.suggested_groups, returned.graph.suggested_groups,
        "retention pressure changed established display membership"
    );
    assert_eq!(
        original.graph.key, returned.graph.key,
        "return must reuse the same validated geometry cache identity"
    );
}

#[test]
fn a_focused_virtual_group_pages_by_noise_like_any_other_level() {
    let mut fixture = code_fixture();
    let members: Vec<_> = (0..60).map(|i| format!("file:a:{i:03}")).collect();
    let group = visualyze_layout::projection::DisplayGroup {
        id: "view:local-group:wide".into(),
        parent: "package:a".into(),
        members: members.clone(),
        suggested: false,
    };
    let mut seen = BTreeSet::new();
    for child_page in 0..3 {
        let page = project_provider(
            &mut fixture.bundle,
            &ProviderViewRequest {
                focus: Some(group.id.clone()),
                groups: vec![group.clone()],
                child_page,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(page.total_children, 60);
        assert_eq!(page.child_page_size, 24);
        assert!(page.graph.suggested_groups.iter().all(|g| !g.suggested));
        seen.extend(
            page.entities
                .keys()
                .filter(|id| id.starts_with("file:a:"))
                .cloned(),
        );
    }
    assert_eq!(seen, members.into_iter().collect());
}
