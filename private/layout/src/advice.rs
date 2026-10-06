//! Producer advice (ADR 0007 sections 2 and 3): which supplied scopes show too
//! much at once, graded, with prose that asks the producer whether a real
//! subsystem groups some of the parts. Advice never rejects a model.
//!
//! This is the single advice entry point. The command-line loader and the future
//! MCP bridge must both call [`advise_snapshot`] / [`advise_bundle`] (the bridge
//! reaches them through the private runtime, for example `visualyze
//! --validate`), so a model is never treated differently on the two paths.
//!
//! The score behind the advice is [`crate::noise`]. Its formula and knees stay
//! private: messages carry counts of the producer's own model and plain-language
//! guidance, never the knees or the threshold.
use crate::{
    LayoutError, Projection,
    noise::{FALLBACK_NOISE, NOISE_THRESHOLD, VisualNoise, interior_noise, noise_from_counts},
    projection::ViewState,
    provider_projection::{ProviderViewRequest, project_provider},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
use visualyze_model::{
    architecture::{Architecture, RelationKind},
    provider::{ModelProvider, PageRequest, SemanticLens},
};

/// Interior noise above this, and up to `FALLBACK_NOISE`, is `Recommend`.
const RECOMMEND_NOISE: f64 = 30.0;

/// Scopes with fewer direct children than this are not evaluated.
///
/// With 12 or fewer children no box or layer term can be positive, so the only
/// way such a scope could be noisy is through connectors alone (many dozens of
/// drawn lines among at most 12 boxes). That is not a grouping problem worth a
/// producer's attention, and skipping it keeps validation cheap on very large
/// bundles, where every candidate costs one bounded projection.
pub const MIN_CHILDREN_FOR_ADVICE: usize = 13;

/// The largest child page a bundle projection will load.
const BUNDLE_PAGE: usize = 128;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum AdviceLevel {
    /// Worth a look: if a real subsystem exists, state it.
    Consider,
    /// Probably hides structure the producer knows about.
    Recommend,
    /// Far too wide to read as one level.
    Strong,
}

impl AdviceLevel {
    /// `None` when the interior is within budget.
    pub fn from_noise(total: f64) -> Option<Self> {
        if total <= NOISE_THRESHOLD {
            None
        } else if total <= RECOMMEND_NOISE {
            Some(Self::Consider)
        } else if total <= FALLBACK_NOISE {
            Some(Self::Recommend)
        } else {
            Some(Self::Strong)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeAdvice {
    pub scope_id: String,
    pub level: AdviceLevel,
    /// Supplied direct children of the scope.
    pub children: usize,
    /// The figures are a lower bound: only a first page of a very wide scope was
    /// projected, or the scope's relationships were only partly scanned.
    pub partial: bool,
    pub noise: VisualNoise,
    pub message: String,
}

fn advice_for(
    scope_id: &str,
    name: &str,
    children: usize,
    partial: bool,
    noise: VisualNoise,
) -> Option<ScopeAdvice> {
    let level = AdviceLevel::from_noise(noise.total)?;
    let widest = noise.widest_layer();
    let shown = format!(
        "{name} ({scope_id}) shows {children} parts at once, and its widest layer holds {}.",
        widest.round() as usize
    );
    let guidance = match level {
        AdviceLevel::Consider => {
            "Consider whether a real subsystem groups some of them; if none does, leave this level wide."
        }
        AdviceLevel::Recommend => {
            "Recommend checking whether real subsystems group them: if the system has components that belong together, supply them as scopes with contains relations."
        }
        AdviceLevel::Strong => {
            "Strongly recommend looking for real subsystems among them: this level is too wide to read at once, so supply the grouping the system actually has, as scopes with contains relations."
        }
    };
    let closing = "Never invent groupings just to make this level smaller; a grouping you deduced rather than found stated must have basis `inferred` and a rationale.";
    let sampled = if partial {
        " Only part of this scope could be measured, so it may be busier than reported."
    } else {
        ""
    };
    Some(ScopeAdvice {
        scope_id: scope_id.to_owned(),
        level,
        children,
        partial,
        message: format!("{shown} {guidance} {closing}{sampled}"),
        noise,
    })
}

fn sorted(advice: impl IntoIterator<Item = ScopeAdvice>) -> Vec<ScopeAdvice> {
    let mut advice: Vec<_> = advice.into_iter().collect();
    advice.sort_by(|a, b| {
        b.noise
            .total
            .total_cmp(&a.noise.total)
            .then_with(|| a.scope_id.cmp(&b.scope_id))
    });
    advice
}

/// A budget large enough that it never changes what the producer supplied.
/// Fallback grouping is switched off by projecting with `project_supplied`.
fn unbudgeted_view(scope: &str) -> ViewState {
    let mut view = ViewState::with_visible_budget(usize::MAX);
    view.focus = Some(scope.to_owned());
    view
}

/// Advice for every scope of a validated snapshot that shows at least
/// `MIN_CHILDREN_FOR_ADVICE` direct children: any entity with that many
/// `contains`/`exposes` children, and the system root, which also shows the
/// uncontained external objects the projection parents to it. Each is projected
/// as the focus with its children collapsed and the automatic fallback grouping
/// disabled, so advice sees the structure as supplied. Connectors are projected
/// edges, one per relation among the children.
pub fn advise_snapshot(model: &Architecture) -> Vec<ScopeAdvice> {
    let snapshot = model.snapshot();
    let mut children: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for relation in &snapshot.relations {
        if matches!(
            relation.kind,
            RelationKind::Contains | RelationKind::Exposes
        ) {
            children
                .entry(relation.source.as_str())
                .or_default()
                .insert(relation.target.as_str());
        }
    }
    let mut candidates: BTreeSet<&str> = children
        .iter()
        .filter(|(_, kids)| kids.len() >= MIN_CHILDREN_FOR_ADVICE)
        .map(|(scope, _)| *scope)
        .collect();
    candidates.insert(snapshot.system_id.as_str());
    let labels: BTreeMap<&str, &str> = snapshot
        .entities
        .iter()
        .map(|entity| (entity.id.as_str(), entity.label.as_str()))
        .collect();
    let mut advice = Vec::new();
    for scope in candidates {
        // A scope that cannot be projected (for example an unusual focus) simply
        // gets no advice: advice must never reject.
        let Ok(projection) = crate::projection::project_supplied(model, &unbudgeted_view(scope))
        else {
            continue;
        };
        let noise = interior_noise(&projection);
        let shown = children_shown(&projection);
        if shown < MIN_CHILDREN_FOR_ADVICE {
            continue;
        }
        let name = labels.get(scope).copied().unwrap_or(scope);
        advice.extend(advice_for(scope, name, shown, false, noise));
    }
    sorted(advice)
}

/// Advice for every supplied scope of a bundle, in each semantic lens it
/// appears in; a scope in more than one lens keeps its noisiest reading.
///
/// Walks each lens's ownership forest from the root through the provider's
/// indexed child counts and projects only scopes with at least
/// `MIN_CHILDREN_FOR_ADVICE` children, as one bounded first page of 128, unpaged
/// by noise and with automatic grouping off. A scope with more children than one page is advised
/// from that page: its box count is the full child count, its layer counts are
/// the page's, scaled up to the full child count, and its connectors are those
/// found among the page. Such results are marked `partial`, as are scopes whose
/// relationships were not fully scanned (a lower bound on connectors).
pub fn advise_bundle(provider: &mut impl ModelProvider) -> Result<Vec<ScopeAdvice>, LayoutError> {
    let cancelled = AtomicBool::new(false);
    let revision = provider.manifest().revision;
    let root = provider.manifest().root_id.clone();
    let mut best: BTreeMap<String, ScopeAdvice> = BTreeMap::new();
    for lens in [
        SemanticLens::Logical,
        SemanticLens::Code,
        SemanticLens::Deployment,
    ] {
        if !provider
            .in_lens(revision, lens, &root)
            .map_err(|error| LayoutError::Invalid(error.to_string()))?
        {
            continue;
        }
        let mut stack = vec![root.clone()];
        while let Some(id) = stack.pop() {
            let count = provider
                .child_count_in(revision, lens, &id)
                .map_err(|error| LayoutError::Invalid(error.to_string()))?;
            if count == 0 {
                continue;
            }
            if count >= MIN_CHILDREN_FOR_ADVICE
                && let Some(advice) = bundle_scope_advice(provider, lens, &id, &cancelled)?
                && best
                    .get(&advice.scope_id)
                    .is_none_or(|kept| kept.noise.total < advice.noise.total)
            {
                best.insert(advice.scope_id.clone(), advice);
            }
            let mut after = None;
            loop {
                let page = provider
                    .children_in(
                        revision,
                        lens,
                        &id,
                        PageRequest {
                            limit: BUNDLE_PAGE,
                            after: after.clone(),
                        },
                    )
                    .map_err(|error| LayoutError::Invalid(error.to_string()))?;
                if page.items.is_empty() {
                    break;
                }
                for child in page.items {
                    stack.push(child.id);
                }
                match page.next {
                    Some(next) if Some(&next) != after.as_ref() => after = Some(next),
                    _ => break,
                }
            }
        }
    }
    Ok(sorted(best.into_values()))
}

fn bundle_scope_advice(
    provider: &mut impl ModelProvider,
    lens: SemanticLens,
    scope: &str,
    cancelled: &AtomicBool,
) -> Result<Option<ScopeAdvice>, LayoutError> {
    let request = ProviderViewRequest {
        focus: Some(scope.to_owned()),
        lens: Some(lens),
        max_children: BUNDLE_PAGE,
        automatic_groups: false,
        page_by_noise: false,
        max_references: 128,
        load_focus_detail: false,
        ..ProviderViewRequest::default()
    };
    let view = project_provider(provider, &request, cancelled)?;
    let noise = interior_noise(&view.graph);
    let shown = children_shown(&view.graph);
    let (noise, truncated) = if view.total_children > shown && shown > 0 {
        let factor = view.total_children as f64 / shown as f64;
        let layers: BTreeMap<i32, f64> = noise
            .layers
            .iter()
            .map(|(band, n)| (*band, n * factor))
            .collect();
        (
            noise_from_counts(view.total_children as f64, &layers, noise.connectors),
            true,
        )
    } else {
        (noise, false)
    };
    let name = view
        .entities
        .get(scope)
        .map_or(scope, |entity| entity.label.as_str());
    Ok(advice_for(
        scope,
        name,
        view.total_children,
        truncated || !view.complete,
        noise,
    ))
}

/// Direct children in the projection: neither references nor the root, and
/// not interface pads, which belong to their owner.
fn children_shown(projection: &Projection) -> usize {
    projection
        .nodes
        .iter()
        .filter(|node| node.id != projection.root && !projection.references.contains_key(&node.id))
        .filter(|node| node.kind != crate::NodeKind::Interface || node.parent.is_none())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A system holding one scope, `sub`, and a sibling service `other`. `sub`
    /// holds `services` services and `datastores` datastores; `inner` are
    /// requests among its services and `outer` are requests from its services to
    /// `other`, which lies outside `sub`.
    fn architecture(
        services: usize,
        datastores: usize,
        inner: &[(usize, usize)],
        outer: usize,
    ) -> Architecture {
        let entity = |id: &str, kind: &str, label: &str, extra: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","label":"{label}","boundary":"internal","basis":"declared"{extra}}}"#
            )
        };
        let relation = |id: &str, kind: &str, source: &str, target: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","source":"{source}","target":"{target}","basis":"declared"}}"#
            )
        };
        let mut entities = vec![
            entity("system", "scope", "System", r#","subtype":"system""#),
            entity("sub", "scope", "Big subsystem", ""),
            entity("other", "service", "Other", ""),
        ];
        let mut rels = vec![
            relation("own-sub", "contains", "system", "sub"),
            relation("own-other", "contains", "system", "other"),
        ];
        for i in 0..services {
            entities.push(entity(
                &format!("svc{i}"),
                "service",
                &format!("Service {i}"),
                "",
            ));
            rels.push(relation(
                &format!("own-svc{i}"),
                "contains",
                "sub",
                &format!("svc{i}"),
            ));
        }
        for i in 0..datastores {
            entities.push(entity(
                &format!("db{i}"),
                "datastore",
                &format!("Store {i}"),
                "",
            ));
            rels.push(relation(
                &format!("own-db{i}"),
                "contains",
                "sub",
                &format!("db{i}"),
            ));
        }
        for (n, (a, b)) in inner.iter().enumerate() {
            rels.push(relation(
                &format!("call{n}"),
                "requests",
                &format!("svc{a}"),
                &format!("svc{b}"),
            ));
        }
        // Each service requests each outside service at most once, because
        // parallel relations of one kind between the same endpoints aggregate
        // into a single connector (ADR 0007 section 4).
        let width = services.max(1);
        for k in 0..outer.div_ceil(width) {
            entities.push(entity(
                &format!("ext{k}"),
                "service",
                &format!("Ext {k}"),
                "",
            ));
            rels.push(relation(
                &format!("own-ext{k}"),
                "contains",
                "system",
                &format!("ext{k}"),
            ));
        }
        for n in 0..outer {
            rels.push(relation(
                &format!("out{n}"),
                "requests",
                &format!("svc{}", n % width),
                &format!("ext{}", n / width),
            ));
        }
        let json = format!(
            r#"{{"schema_version":"0.1.0-draft.1","model_id":"test","revision":1,"system_id":"system","snapshot":{{"id":"test-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
            entities.join(","),
            rels.join(",")
        );
        Architecture::from_json(json.as_bytes()).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn levels_are_graded_by_interior_noise() {
        assert_eq!(AdviceLevel::from_noise(NOISE_THRESHOLD), None);
        assert_eq!(AdviceLevel::from_noise(0.0), None);
        assert_eq!(
            AdviceLevel::from_noise(NOISE_THRESHOLD + 0.1),
            Some(AdviceLevel::Consider)
        );
        assert_eq!(
            AdviceLevel::from_noise(RECOMMEND_NOISE),
            Some(AdviceLevel::Consider)
        );
        assert_eq!(
            AdviceLevel::from_noise(RECOMMEND_NOISE + 0.1),
            Some(AdviceLevel::Recommend)
        );
        assert_eq!(
            AdviceLevel::from_noise(FALLBACK_NOISE),
            Some(AdviceLevel::Recommend)
        );
        assert_eq!(
            AdviceLevel::from_noise(FALLBACK_NOISE + 0.1),
            Some(AdviceLevel::Strong)
        );
    }

    #[test]
    fn a_wide_single_layer_scope_gets_strong_advice() {
        let advice = advise_snapshot(&architecture(35, 0, &[], 0));
        assert_eq!(advice.len(), 1, "{advice:#?}");
        let a = &advice[0];
        assert_eq!(a.scope_id, "sub");
        assert_eq!(a.level, AdviceLevel::Strong);
        assert_eq!(a.children, 35);
        assert!(!a.partial);
        assert_eq!(a.noise.boxes, 35.0);
        assert_eq!(a.noise.widest_layer(), 35.0);
    }

    #[test]
    fn levels_follow_the_width_of_the_widest_layer() {
        for (services, level) in [
            (17, AdviceLevel::Consider),
            (19, AdviceLevel::Recommend),
            (25, AdviceLevel::Strong),
        ] {
            let advice = advise_snapshot(&architecture(services, 0, &[], 0));
            assert_eq!(advice.len(), 1, "{services}: {advice:#?}");
            assert_eq!(advice[0].level, level, "{services}: {:?}", advice[0].noise);
        }
    }

    #[test]
    fn small_and_moderate_scopes_get_no_advice() {
        assert!(advise_snapshot(&architecture(6, 2, &[], 0)).is_empty());
        assert!(advise_snapshot(&architecture(14, 0, &[], 0)).is_empty());
        // Two layers, each within its knee, and a modest connector count.
        assert!(advise_snapshot(&architecture(12, 12, &[(0, 1), (1, 2)], 3)).is_empty());
        // The system root, with only its two children, is never advised.
        let advice = advise_snapshot(&architecture(30, 0, &[], 0));
        assert!(advice.iter().all(|a| a.scope_id != "system"), "{advice:#?}");
    }

    #[test]
    fn references_and_edges_touching_them_are_not_interior_noise() {
        // Sixty requests from `sub` to a service outside it make the whole view
        // noisy, but none of that belongs to the interior.
        let model = architecture(14, 0, &[], 60);
        let projection =
            crate::projection::project_supplied(&model, &unbudgeted_view("sub")).unwrap();
        assert_eq!(projection.references.len(), 5);
        assert_eq!(projection.edges.len(), 60);
        assert!(crate::noise::visual_noise(&projection).over_threshold());
        let inside = interior_noise(&projection);
        assert_eq!(inside.boxes, 14.0);
        assert_eq!(inside.connectors, 0);
        assert!(advise_snapshot(&model).is_empty());
    }

    #[test]
    fn connectors_among_children_count() {
        let relations: Vec<_> = (0..14)
            .flat_map(|a| (0..14).filter(move |b| a != *b).map(move |b| (a, b)))
            .take(80)
            .collect();
        let advice = advise_snapshot(&architecture(14, 0, &relations, 5));
        assert_eq!(advice.len(), 1, "{advice:#?}");
        assert_eq!(
            advice[0].noise.connectors, 80,
            "the 5 outside links are excluded"
        );
        assert_eq!(advice[0].level, AdviceLevel::Strong);
    }

    #[test]
    fn messages_are_prose_without_formula_or_knees() {
        for (services, firmness) in [(17, "Consider"), (19, "Recommend"), (35, "Strongly")] {
            let advice = advise_snapshot(&architecture(services, 0, &[], 0));
            let message = &advice[0].message;
            assert!(message.contains("Big subsystem"), "{message}");
            assert!(message.contains(&format!("{services} parts")), "{message}");
            assert!(message.contains(firmness), "{message}");
            assert!(message.contains("Never invent groupings"), "{message}");
            assert!(message.contains("`inferred`"), "{message}");
            for banned in ["knee", "threshold", "noise", "27", "36", "100"] {
                assert!(!message.contains(banned), "{banned} in {message}");
            }
        }
    }

    #[test]
    fn advice_is_sorted_noisiest_first_then_by_id() {
        let a = |id: &str, total: f64| {
            let mut noise = noise_from_counts(0.0, &BTreeMap::new(), 0);
            noise.total = total;
            ScopeAdvice {
                scope_id: id.into(),
                level: AdviceLevel::Consider,
                children: 0,
                partial: false,
                noise,
                message: String::new(),
            }
        };
        let order: Vec<_> = sorted([a("b", 20.0), a("c", 50.0), a("a", 20.0)])
            .into_iter()
            .map(|advice| advice.scope_id)
            .collect();
        assert_eq!(order, ["c", "a", "b"]);
    }
}
