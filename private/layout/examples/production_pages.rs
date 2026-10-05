//! Exhaust every child/relationship page in sampled supplied Rust regions.
//! Usage: production_pages BUNDLE NEW_REPORT [additional semantic focus IDs...]
//! The reference oracle reads supplied records independently of provider indexes.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader},
    sync::atomic::AtomicBool,
    time::Instant,
};
use visualyze_layout::{
    Algorithm, LayoutRequest, QualityProfile,
    projection::{LayoutPolicy, ViewState},
    provider_projection::{Continuation, ProviderViewRequest, project_provider},
    solve, validate,
};
use visualyze_model::provider::{
    BundleRecord, EntityKind, EntitySummary, Limits, LocalBundle, ModelProvider, PageRequest,
    RelationKind, RelationRecord, SemanticLens,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Default)]
struct Oracle {
    parents: BTreeMap<String, String>,
    references: BTreeMap<String, RelationRecord>,
}

impl Oracle {
    fn read(path: &str) -> Result<Self> {
        let mut oracle = Self::default();
        for line in BufReader::new(File::open(path)?).lines() {
            if let BundleRecord::Relation(relation) = serde_json::from_str(&line?)? {
                oracle.insert(relation)?;
            }
        }
        Ok(oracle)
    }

    fn insert(&mut self, relation: RelationRecord) -> Result<()> {
        match relation.kind {
            RelationKind::Contains | RelationKind::OwnsCode => {
                if self
                    .parents
                    .insert(relation.target, relation.source)
                    .is_some()
                {
                    return Err("oracle requires a single supplied ownership forest".into());
                }
            }
            RelationKind::CodeReferences => {
                if self
                    .references
                    .insert(relation.id.clone(), relation)
                    .is_some()
                {
                    return Err("duplicate supplied reference ID".into());
                }
            }
            _ => {
                return Err("this audit supports the explicit Rust producer's code profile".into());
            }
        }
        Ok(())
    }

    fn inside(&self, focus: &str, id: &str) -> bool {
        let mut current = Some(id);
        for _ in 0..=self.parents.len() {
            let Some(id) = current else { return false };
            if id == focus {
                return true;
            }
            current = self.parents.get(id).map(String::as_str);
        }
        panic!("ownership cycle in independently read source");
    }

    fn relevant(&self, focus: &str) -> BTreeSet<String> {
        self.references
            .values()
            .filter(|relation| {
                self.inside(focus, &relation.source) || self.inside(focus, &relation.target)
            })
            .map(|relation| relation.id.clone())
            .collect()
    }
}

fn children(provider: &mut LocalBundle, id: &str) -> Result<Vec<EntitySummary>> {
    let revision = provider.manifest().revision;
    let mut result = vec![];
    let mut after = None;
    loop {
        let page = provider.children(revision, id, PageRequest { limit: 128, after })?;
        result.extend(page.items);
        after = page.next;
        if after.is_none() {
            return Ok(result);
        }
    }
}

fn audit(provider: &mut LocalBundle, oracle: &Oracle, focus: &str) -> Result<serde_json::Value> {
    let expected = oracle.relevant(focus);
    let revision = provider.manifest().revision;
    let label = provider
        .summary(revision, focus)?
        .ok_or("missing focus")?
        .label;
    let mut view = ViewState::default();
    let mut pages = vec![];
    let mut child_page = 0;
    let mut routed_across_pages = BTreeSet::new();
    loop {
        let mut after = None;
        let mut cursors = BTreeSet::new();
        let mut loaded = BTreeSet::new();
        let mut internal_count = 0;
        let mut relation_page = 0;
        let (total_children, page_size) = loop {
            let start = Instant::now();
            let p = project_provider(
                provider,
                &ProviderViewRequest {
                    focus: Some(focus.into()),
                    lens: Some(SemanticLens::Code),
                    expected_revision: Some(revision),
                    child_page,
                    boundary_after: after,
                    retained_groups: view.retained_suggestions().to_vec(),
                    policy: LayoutPolicy::Code,
                    ..Default::default()
                },
                &AtomicBool::new(false),
            )?;
            let projection_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert!(
                p.unrepresented_relation_ids.is_empty(),
                "{focus}: loaded claim omitted"
            );
            // Coarsening also counts loaded claims inside a display group.
            // Their records enter `loaded` below, so count that intersection
            // exactly once alongside the provider's unloaded internal totals.
            let loaded_internal: BTreeSet<_> = p
                .graph
                .internal_relation_ids
                .iter()
                .filter(|id| p.relations.contains_key(*id))
                .collect();
            internal_count += p
                .internal_relation_counts
                .values()
                .sum::<usize>()
                .checked_sub(loaded_internal.len())
                .expect("internal counts must include loaded internal memberships");
            for relation in p
                .relations
                .values()
                .filter(|r| r.kind == RelationKind::CodeReferences)
            {
                assert!(expected.contains(&relation.id), "irrelevant supplied claim");
                assert_eq!(
                    oracle.references.get(&relation.id),
                    Some(relation),
                    "claim altered"
                );
                assert!(
                    loaded.insert(relation.id.clone()),
                    "relationship pages overlap"
                );
            }
            for id in &p.graph.internal_relation_ids {
                assert!(
                    expected.contains(id),
                    "internal membership is not a relevant supplied claim"
                );
            }
            let start = Instant::now();
            let pins = BTreeMap::new();
            let solution = solve(LayoutRequest {
                projection: &p.graph,
                algorithm: Algorithm::Floorplan,
                pins: &pins,
                previous: None,
                quality: QualityProfile::default(),
                work_budget: 32,
                cancelled: &AtomicBool::new(false),
            })?;
            let solve_ms = start.elapsed().as_secs_f64() * 1000.0;
            validate(&p.graph, &solution, &pins, 2)?;
            assert!(solution.boxes.len() <= 250);
            for route in &solution.routes {
                for id in &route.semantic_ids {
                    assert!(
                        p.relations.contains_key(id),
                        "route has no loaded original record"
                    );
                    assert!(
                        expected.contains(id),
                        "route is not a relevant supplied reference"
                    );
                    routed_across_pages.insert(id.clone());
                }
            }
            for object in solution.boxes.values() {
                for id in &object.semantic_ids {
                    assert!(
                        provider.summary(revision, id)?.is_some(),
                        "object has no supplied record"
                    );
                }
            }
            view.remember_groups(&p.graph);
            pages.push(serde_json::json!({
                "child_page":child_page,"relationship_page":relation_page,
                "objects":solution.boxes.len(),"routes":solution.routes.len(),
                "original_reference_records_loaded":p.relations.values().filter(|r|r.kind==RelationKind::CodeReferences).count(),
                "internal_references_counted":p.internal_relation_counts.values().sum::<usize>(),
                "projection_ms":projection_ms,"solve_ms":solve_ms,
                "route_length":solution.metrics.route_length,"bends":solution.metrics.bends,
                "congestion":solution.metrics.congestion,"volume":solution.metrics.volume
            }));
            let next: Vec<_> = p
                .continuations
                .iter()
                .filter_map(|c| match c {
                    Continuation::Boundary { after } => Some(after.clone()),
                    _ => None,
                })
                .collect();
            assert!(next.len() <= 1, "ambiguous relationship continuation");
            let Some(cursor) = next.into_iter().next() else {
                break (p.total_children, p.child_page_size);
            };
            assert!(
                cursors.insert(serde_json::to_string(&cursor)?),
                "relationship cursor repeated"
            );
            after = Some(cursor);
            relation_page += 1;
            assert!(
                relation_page <= oracle.references.len() + 1,
                "relationship pagination made no progress"
            );
        };
        assert_eq!(
            loaded.len() + internal_count,
            expected.len(),
            "{focus} child page {child_page}: missing or duplicate supplied references"
        );
        child_page += 1;
        if child_page * page_size >= total_children {
            break;
        }
        assert!(page_size > 0, "child pagination made no progress");
    }
    eprintln!(
        "{label}: {} validated pages, {child_page} child pages",
        pages.len()
    );
    Ok(
        serde_json::json!({"focus":focus,"label":label,"status":"valid",
        "child_pages":child_page,"pages":pages,"supplied_references_relevant":expected.len(),
        "distinct_original_references_routed":routed_across_pages.len(),
        "all_reference_pages_accounted_for":true}),
    )
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: production_pages BUNDLE NEW_REPORT [FOCUS_IDS...]")?;
    let output = args.next().ok_or("missing report path")?;
    let mut provider = LocalBundle::open(&path, Limits::default())?;
    let oracle = Oracle::read(&path)?;
    let manifest = provider.manifest().clone();
    let mut focuses = BTreeSet::from([manifest.root_id.clone()]);
    focuses.extend(args);
    let mut packages = children(&mut provider, &manifest.root_id)?;
    for repository in packages
        .clone()
        .iter()
        .filter(|e| e.kind == EntityKind::CodeRepository)
    {
        focuses.insert(repository.id.clone());
        packages.extend(children(&mut provider, &repository.id)?);
    }
    for package in packages
        .iter()
        .filter(|e| e.kind == EntityKind::CodePackage)
    {
        focuses.insert(package.id.clone());
        if let Some(file) = children(&mut provider, &package.id)?
            .iter()
            .find(|f| f.kind == EntityKind::CodeFile)
        {
            focuses.insert(file.id.clone());
        }
    }
    let start = Instant::now();
    let mut reports = vec![];
    for focus in focuses {
        reports.push(match audit(&mut provider, &oracle, &focus) {
            Ok(report) => report,
            Err(error) => {
                serde_json::json!({"focus":focus,"status":"failed","error":error.to_string()})
            }
        });
    }
    let failed = reports.iter().filter(|r| r["status"] != "valid").count();
    let report = serde_json::json!({"model_id":manifest.model_id,"revision":manifest.revision,
        "quality_version":visualyze_layout::QUALITY_VERSION,"layout_version":visualyze_layout::LAYOUT_VERSION,
        "source_counts":provider.stats().counts,"elapsed_ms":start.elapsed().as_secs_f64()*1000.0,
        "regions":reports,"failed_regions":failed,
        "scope":"All child and relationship pages of root/repository, each package and one file per package, plus explicit focus IDs. Independent raw ownership/reference oracle counts every supplied code reference per focus per child page. Internal IDs may remain a bounded sample; counts are exact. Not an exhaustive traversal of every declaration or expanded combination."});
    serde_json::to_writer_pretty(
        File::options().write(true).create_new(true).open(&output)?,
        &report,
    )?;
    if failed > 0 {
        return Err(format!("{failed} sampled regions failed").into());
    }
    println!("Report: {output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use visualyze_model::provider::Claim;

    fn relation(id: &str, kind: RelationKind, source: &str, target: &str) -> RelationRecord {
        RelationRecord {
            id: id.into(),
            kind,
            source: source.into(),
            target: target.into(),
            claim: Claim::declared(),
            label: None,
            aspect: None,
        }
    }

    #[test]
    fn raw_forest_oracle_counts_incoming_outgoing_internal_and_parallel_references() {
        let mut oracle = Oracle::default();
        for (id, source, target) in [
            ("own-a", "root", "a"),
            ("own-b", "root", "b"),
            ("own-f", "a", "file"),
            ("own-fn", "file", "function"),
        ] {
            oracle
                .insert(relation(id, RelationKind::OwnsCode, source, target))
                .unwrap();
        }
        for (id, source, target) in [
            ("outgoing", "function", "b"),
            ("parallel", "function", "b"),
            ("incoming", "b", "file"),
            ("internal", "file", "function"),
            ("unrelated", "b", "root"),
        ] {
            oracle
                .insert(relation(id, RelationKind::CodeReferences, source, target))
                .unwrap();
        }
        assert_eq!(oracle.relevant("root").len(), 5);
        assert_eq!(
            oracle.relevant("a"),
            BTreeSet::from([
                "outgoing".into(),
                "parallel".into(),
                "incoming".into(),
                "internal".into(),
            ])
        );
        assert_eq!(
            oracle.relevant("function"),
            BTreeSet::from(["outgoing".into(), "parallel".into(), "internal".into(),])
        );
        assert!(oracle.relevant("missing").is_empty());
        assert!(
            oracle
                .insert(relation(
                    "outgoing",
                    RelationKind::CodeReferences,
                    "function",
                    "b"
                ))
                .is_err()
        );
        assert!(
            oracle
                .insert(relation("new", RelationKind::Requests, "a", "b"))
                .is_err()
        );
    }
}
