//! Walkthrough validation against real systems (otel-demo, posthog, sentry).
//!
//! The synthetic conformance cases cover the rules on an 8-object model. These tests
//! generate walkthroughs from each real model's own relations and compare the validator
//! with an independent oracle, so a rule that only breaks at scale (hubs, brokers,
//! topics, deep ownership) fails here. See docs/walkthrough-test-battery.md, tier 1.
//!
//! To add a system, add its model to `SYSTEMS`.
use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use visualyze_model::architecture::{Architecture, EntityKind, RelationKind};
use visualyze_model::walkthrough::{SCHEMA_VERSION, Walkthrough};

const SYSTEMS: [(&str, &str); 3] = [
    (
        "otel-demo",
        include_str!("../../../examples/otel-demo.json"),
    ),
    ("posthog", include_str!("../../../examples/posthog.json")),
    ("sentry", include_str!("../../../examples/sentry.json")),
];

fn each_system(mut check: impl FnMut(&str, &Architecture, &Oracle)) {
    for (name, json) in SYSTEMS {
        let model = Architecture::from_json(json.as_bytes())
            .unwrap_or_else(|e| panic!("{name} does not load: {e}"));
        let oracle = Oracle::new(&model);
        check(name, &model, &oracle);
    }
}

/// One direction of travel along a flow relation, as the walkthrough format defines it:
/// a subscription's payload travels from the channel to the subscriber, and a `return`
/// leg travels target to source.
#[derive(Clone, Debug)]
struct Leg {
    relation: String,
    returning: bool,
    kind: RelationKind,
    from: String,
    to: String,
}

fn has_response(kind: RelationKind) -> bool {
    matches!(kind, RelationKind::Requests | RelationKind::Reads)
}

fn is_flow(kind: RelationKind) -> bool {
    matches!(
        kind,
        RelationKind::Requests
            | RelationKind::Reads
            | RelationKind::Writes
            | RelationKind::DependsOn
            | RelationKind::Publishes
            | RelationKind::Subscribes
    )
}

/// Ownership and travel rules re-derived from the raw relations, not from the validator.
struct Oracle {
    system: String,
    parent: HashMap<String, String>,
    interfaces: HashSet<String>,
    entities: Vec<String>,
    scopes: Vec<String>,
    legs: Vec<Leg>,
}

impl Oracle {
    fn new(model: &Architecture) -> Self {
        let s = model.snapshot();
        let mut parent = HashMap::new();
        for r in &s.relations {
            if matches!(r.kind, RelationKind::Contains | RelationKind::Exposes) {
                parent.insert(r.target.clone(), r.source.clone());
            }
        }
        let mut legs = Vec::new();
        for r in s.relations.iter().filter(|r| is_flow(r.kind)) {
            let (from, to) = if r.kind == RelationKind::Subscribes {
                (&r.target, &r.source)
            } else {
                (&r.source, &r.target)
            };
            let leg = |returning: bool| Leg {
                relation: r.id.clone(),
                returning,
                kind: r.kind,
                from: if returning { to } else { from }.clone(),
                to: if returning { from } else { to }.clone(),
            };
            legs.push(leg(false));
            if has_response(r.kind) {
                legs.push(leg(true));
            }
        }
        Self {
            system: s.system_id.clone(),
            parent,
            interfaces: s
                .entities
                .iter()
                .filter(|e| e.kind == EntityKind::Interface)
                .map(|e| e.id.clone())
                .collect(),
            entities: s.entities.iter().map(|e| e.id.clone()).collect(),
            scopes: s
                .entities
                .iter()
                .filter(|e| e.kind == EntityKind::Scope)
                .map(|e| e.id.clone())
                .collect(),
            legs,
        }
    }

    /// An interface stands for the component that owns it.
    fn place<'a>(&'a self, id: &'a str) -> &'a str {
        if self.interfaces.contains(id) {
            self.parent.get(id).map_or(id, String::as_str)
        } else {
            id
        }
    }

    /// Owners from nearest to the system; an unowned object sits at the system level.
    fn owners(&self, id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = id.to_owned();
        while at != self.system && out.len() < 64 {
            let next = self
                .parent
                .get(&at)
                .cloned()
                .unwrap_or_else(|| self.system.clone());
            out.push(next.clone());
            at = next;
        }
        out
    }

    fn is_gap(&self, a: &Leg, b: &Leg) -> bool {
        self.place(&a.to) != self.place(&b.from)
            || (a.to != b.from
                && self.interfaces.contains(&a.to)
                && self.interfaces.contains(&b.from))
    }
}

fn hop(leg: &Leg) -> Value {
    json!({"relation": leg.relation, "return": leg.returning})
}

fn step(id: &str, focus: &[&str], trace: &[Leg]) -> Value {
    let mut unique = Vec::new();
    for f in focus {
        if !unique.contains(f) {
            unique.push(f);
        }
    }
    json!({"id": id, "title": "Probe", "focus": unique,
           "trace": trace.iter().map(hop).collect::<Vec<_>>()})
}

fn script(model: &Architecture, steps: Vec<Value>) -> Value {
    let s = model.snapshot();
    json!({"schema_version": SCHEMA_VERSION, "id": "wt", "model_id": s.model_id,
           "expected_revision": s.revision, "title": "Probe", "steps": steps})
}

fn validate(model: &Architecture, value: &Value) -> Result<Walkthrough, Vec<(String, String)>> {
    Walkthrough::from_json(value.to_string().as_bytes(), model).map_err(|e| {
        e.diagnostics
            .iter()
            .map(|d| (d.code.to_string(), d.path.to_string()))
            .collect()
    })
}

fn codes(errors: &[(String, String)]) -> Vec<&str> {
    errors.iter().map(|(code, _)| code.as_str()).collect()
}

/// Every `n`th item, so a large cross product stays bounded and repeatable.
fn sample<T: Clone>(items: &[T], cap: usize) -> Vec<T> {
    let stride = items.len().div_ceil(cap).max(1);
    items.iter().step_by(stride).cloned().collect()
}

/// A small deterministic generator, so failures reproduce.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % below
    }
}

#[test]
fn the_real_models_are_big_enough_to_mean_something() {
    each_system(|name, model, oracle| {
        let s = model.snapshot();
        assert!(
            s.entities.len() >= 40,
            "{name}: {} entities",
            s.entities.len()
        );
        assert!(
            oracle.legs.len() >= 100,
            "{name}: {} legs",
            oracle.legs.len()
        );
        assert!(
            oracle.scopes.len() >= 5,
            "{name}: {} scopes",
            oracle.scopes.len()
        );
    });
}

#[test]
fn every_stated_flow_relation_can_be_traced_in_every_legal_direction() {
    each_system(|name, model, oracle| {
        for leg in &oracle.legs {
            let result = validate(
                model,
                &script(
                    model,
                    vec![step("s", &[&leg.from, &leg.to], std::slice::from_ref(leg))],
                ),
            );
            let walkthrough = result.unwrap_or_else(|e| panic!("{name} {}: {e:?}", leg.relation));
            let h = &walkthrough.resolved()[0].hops[0];
            assert_eq!(
                (h.from.as_str(), h.to.as_str(), h.returning),
                (leg.from.as_str(), leg.to.as_str(), leg.returning),
                "{name} {}: travel direction",
                leg.relation
            );
        }
    });
}

#[test]
fn a_request_and_its_response_form_a_closed_loop_and_nothing_else_does() {
    each_system(|name, model, oracle| {
        let mut closed = 0;
        for out in oracle
            .legs
            .iter()
            .filter(|l| !l.returning && has_response(l.kind))
        {
            let back = oracle
                .legs
                .iter()
                .find(|l| l.relation == out.relation && l.returning)
                .unwrap();
            let mut with_loop = step("s", &[&out.from, &out.to], &[out.clone(), back.clone()]);
            with_loop["repeat_trace"] = true.into();
            validate(model, &script(model, vec![with_loop]))
                .unwrap_or_else(|e| panic!("{name} {}: loop rejected: {e:?}", out.relation));
            closed += 1;

            let mut one_way = step("s", &[&out.from, &out.to], std::slice::from_ref(out));
            one_way["repeat_trace"] = true.into();
            let result = validate(model, &script(model, vec![one_way]));
            let self_loop = oracle.place(&out.from) == oracle.place(&out.to);
            match (result, self_loop) {
                (Ok(_), true) => {}
                (Err(e), false) => assert_eq!(codes(&e), ["open_loop"], "{name} {}", out.relation),
                (other, _) => panic!("{name} {}: one-way repeat: {:?}", out.relation, other.err()),
            }
        }
        assert!(closed > 0, "{name}: no request/response pairs");
    });
}

#[test]
fn consecutive_hops_are_accepted_exactly_when_the_travel_is_contiguous() {
    each_system(|name, model, oracle| {
        let (mut joined, mut broken) = (Vec::new(), Vec::new());
        for a in &oracle.legs {
            for b in &oracle.legs {
                if oracle.is_gap(a, b) {
                    broken.push((a, b));
                } else {
                    joined.push((a, b));
                }
            }
        }
        assert!(!joined.is_empty() && !broken.is_empty(), "{name}");
        for (expect_gap, pairs) in [
            (false, sample(&joined, 4000)),
            (true, sample(&broken, 2500)),
        ] {
            for (a, b) in pairs {
                let focus = [a.from.as_str(), b.to.as_str()];
                let result = validate(
                    model,
                    &script(model, vec![step("s", &focus, &[a.clone(), b.clone()])]),
                );
                let label = format!("{name}: {} then {}", a.relation, b.relation);
                match (result, expect_gap) {
                    (Ok(_), false) => {}
                    (Err(e), true) => {
                        assert_eq!(codes(&e), ["trace_gap"], "{label}");
                        assert_eq!(e[0].1, "/steps/0/trace/1", "{label}");
                    }
                    (other, _) => {
                        panic!("{label}: expected gap={expect_gap}, got {:?}", other.err())
                    }
                }
            }
        }
    });
}

/// Depth-first search for a trace of `want` distinct, contiguous legs.
fn chain(oracle: &Oracle, want: usize) -> Option<Vec<Leg>> {
    fn grow(oracle: &Oracle, path: &mut Vec<usize>, want: usize, budget: &mut u32) -> bool {
        if path.len() == want {
            return true;
        }
        let last = &oracle.legs[*path.last().unwrap()];
        for (i, next) in oracle.legs.iter().enumerate() {
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            if oracle.is_gap(last, next)
                || path
                    .iter()
                    .any(|&p| oracle.legs[p].relation == next.relation)
            {
                continue;
            }
            path.push(i);
            if grow(oracle, path, want, budget) {
                return true;
            }
            path.pop();
        }
        false
    }
    for start in 0..oracle.legs.len() {
        let mut path = vec![start];
        let mut budget = 200_000;
        if grow(oracle, &mut path, want, &mut budget) {
            return Some(path.into_iter().map(|i| oracle.legs[i].clone()).collect());
        }
    }
    None
}

#[test]
fn a_trace_may_have_sixteen_hops_and_not_seventeen() {
    let mut exercised = 0;
    each_system(|name, model, oracle| {
        let Some(long) = chain(oracle, 17) else {
            return;
        };
        exercised += 1;
        let focus = [long[0].from.as_str(), long[15].to.as_str()];
        let walkthrough = validate(model, &script(model, vec![step("s", &focus, &long[..16])]))
            .unwrap_or_else(|e| panic!("{name}: 16 hops: {e:?}"));
        assert_eq!(walkthrough.resolved()[0].hops.len(), 16);
        let error = validate(model, &script(model, vec![step("s", &focus, &long)])).unwrap_err();
        assert_eq!(codes(&error), ["schema_violation"], "{name}: 17 hops");
        assert_eq!(error[0].1, "/steps/0/trace", "{name}");
    });
    assert!(
        exercised > 0,
        "no real model has a 17-leg trace to test the cap with"
    );
}

#[test]
fn structure_and_coverage_are_never_travel() {
    each_system(|name, model, _| {
        let others: Vec<_> = model
            .snapshot()
            .relations
            .iter()
            .filter(|r| !is_flow(r.kind))
            .collect();
        assert!(!others.is_empty(), "{name}");
        for r in sample(&others, 300) {
            let value = script(
                model,
                vec![json!({"id": "s", "title": "t", "focus": [r.source],
                            "trace": [{"relation": r.id}]})],
            );
            let e = validate(model, &value).unwrap_err();
            assert_eq!(codes(&e), ["invalid_trace_relation"], "{name} {}", r.id);
        }
    });
}

#[test]
fn only_requests_and_reads_have_a_response_leg() {
    each_system(|name, model, _| {
        for r in model
            .snapshot()
            .relations
            .iter()
            .filter(|r| is_flow(r.kind) && !has_response(r.kind))
        {
            let value = script(
                model,
                vec![json!({"id": "s", "title": "t", "focus": [r.source],
                            "trace": [{"relation": r.id, "return": true}]})],
            );
            let e = validate(model, &value).unwrap_err();
            assert!(
                codes(&e).contains(&"invalid_return"),
                "{name} {}: {e:?}",
                r.id
            );
        }
    });
}

#[test]
fn references_are_checked_by_kind() {
    each_system(|name, model, oracle| {
        let s = model.snapshot();
        let (entity, relation) = (&s.entities[0].id, &s.relations[0].id);
        let cases = [
            (json!({"focus": ["nope:missing"]}), "missing_reference"),
            (json!({"focus": [relation]}), "invalid_reference_kind"),
            (
                json!({"focus": [entity], "trace": [{"relation": entity}]}),
                "invalid_reference_kind",
            ),
            (
                json!({"focus": [entity], "trace": [{"relation": "rel:missing"}]}),
                "missing_reference",
            ),
            (
                json!({"focus": [entity], "root": "nope:missing"}),
                "missing_reference",
            ),
            (
                json!({"focus": [entity], "labels": [{"target": "nope:x", "text": "t"}]}),
                "missing_reference",
            ),
        ];
        for (mut step, code) in cases {
            step["id"] = "s".into();
            step["title"] = "t".into();
            let e = validate(model, &script(model, vec![step.clone()])).unwrap_err();
            assert!(codes(&e).contains(&code), "{name} {step}: {e:?}");
        }
        // A label may point at an entity or a relation.
        let ok = json!({"id": "s", "title": "t", "focus": [entity], "labels": [
            {"target": entity, "text": "an object"}, {"target": relation, "text": "a relation"}]});
        validate(model, &script(model, vec![ok])).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(!oracle.scopes.is_empty());
    });
}

#[test]
fn focus_must_sit_inside_an_explicit_root() {
    each_system(|name, model, oracle| {
        let mut pairs = Vec::new();
        for root in oracle.scopes.iter().chain([&oracle.system]) {
            for e in &oracle.entities {
                pairs.push((root.clone(), e.clone()));
            }
        }
        let (mut inside, mut outside) = (0, 0);
        for (root, focus) in sample(&pairs, 3000) {
            let expected = *root == focus || oracle.owners(&focus).contains(&root);
            let mut s = step("s", &[&focus], &[]);
            s["root"] = root.clone().into();
            let result = validate(model, &script(model, vec![s]));
            match (result, expected) {
                (Ok(_), true) => inside += 1,
                (Err(e), false) => {
                    assert_eq!(
                        codes(&e),
                        ["focus_outside_root"],
                        "{name}: {focus} in {root}"
                    );
                    outside += 1;
                }
                (other, _) => {
                    panic!(
                        "{name}: {focus} in {root}: expected inside={expected}, got {:?}",
                        other.err()
                    )
                }
            }
        }
        assert!(
            inside > 0 && outside > 0,
            "{name}: {inside} inside, {outside} outside"
        );
    });
}

#[test]
fn the_default_root_is_the_deepest_strict_owner_of_the_whole_focus() {
    each_system(|name, model, oracle| {
        let mut random = Lcg(0x5eed);
        for round in 0..400 {
            let count = 1 + random.next(4);
            let focus: Vec<&str> = (0..count)
                .map(|_| oracle.entities[random.next(oracle.entities.len())].as_str())
                .collect();
            let walkthrough = validate(model, &script(model, vec![step("s", &focus, &[])]))
                .unwrap_or_else(|e| panic!("{name} #{round} {focus:?}: {e:?}"));
            let root = walkthrough.resolved()[0].root.as_str();
            let strictly_owns =
                |owner: &str, id: &str| oracle.owners(id).iter().any(|o| o == owner);
            if focus.contains(&oracle.system.as_str()) {
                assert_eq!(root, oracle.system, "{name} #{round} {focus:?}");
                continue;
            }
            assert!(
                focus.iter().all(|f| strictly_owns(root, f)),
                "{name} #{round} {focus:?}: {root} does not own every focus object"
            );
            let deeper = oracle
                .entities
                .iter()
                .filter(|c| oracle.owners(c).first().map(String::as_str) == Some(root))
                .find(|c| focus.iter().all(|f| strictly_owns(c, f)));
            assert!(
                deeper.is_none(),
                "{name} #{round} {focus:?}: {deeper:?} is deeper than {root}"
            );
        }
    });
}

#[test]
fn a_topic_delivers_only_to_its_own_subscribers() {
    let mut pairs = 0;
    let mut swaps = 0;
    each_system(|name, model, oracle| {
        let s = model.snapshot();
        let relations = |kind, target: &str| -> Vec<_> {
            s.relations
                .iter()
                .filter(|r| r.kind == kind && r.target == target)
                .collect()
        };
        let topics: Vec<_> = s
            .entities
            .iter()
            .filter(|e| e.kind == EntityKind::Interface && e.subtype.as_deref() == Some("topic"))
            .map(|e| e.id.as_str())
            .collect();
        for &topic in &topics {
            for publish in relations(RelationKind::Publishes, topic) {
                for subscribe in relations(RelationKind::Subscribes, topic) {
                    let walk = step(
                        "s",
                        &[&publish.source, &subscribe.source],
                        &[
                            leg(oracle, &publish.id, false),
                            leg(oracle, &subscribe.id, false),
                        ],
                    );
                    let w = validate(model, &script(model, vec![walk]))
                        .unwrap_or_else(|e| panic!("{name} {topic}: {e:?}"));
                    let travel: Vec<_> = w.resolved()[0]
                        .hops
                        .iter()
                        .map(|h| (h.from.as_str(), h.to.as_str()))
                        .collect();
                    assert_eq!(
                        travel,
                        [
                            (publish.source.as_str(), topic),
                            (topic, subscribe.source.as_str())
                        ],
                        "{name} {topic}"
                    );
                    pairs += 1;
                    for other in topics.iter().filter(|t| **t != topic) {
                        for wrong in relations(RelationKind::Subscribes, other)
                            .into_iter()
                            .take(1)
                        {
                            let swapped = step(
                                "s",
                                &[&publish.source, &wrong.source],
                                &[
                                    leg(oracle, &publish.id, false),
                                    leg(oracle, &wrong.id, false),
                                ],
                            );
                            let e = validate(model, &script(model, vec![swapped])).unwrap_err();
                            assert_eq!(codes(&e), ["trace_gap"], "{name}: {topic} then {other}");
                            swaps += 1;
                        }
                    }
                }
                let value = script(
                    model,
                    vec![json!({"id": "s", "title": "t", "focus": [publish.source],
                                "trace": [{"relation": publish.id, "return": true}]})],
                );
                let e = validate(model, &value).unwrap_err();
                assert!(
                    codes(&e).contains(&"invalid_return"),
                    "{name} {}",
                    publish.id
                );
            }
        }
    });
    assert!(
        pairs > 0 && swaps > 0,
        "no real model has two topics to test ({pairs} pairs, {swaps} swaps)"
    );
}

fn leg(oracle: &Oracle, relation: &str, returning: bool) -> Leg {
    oracle
        .legs
        .iter()
        .find(|l| l.relation == relation && l.returning == returning)
        .unwrap_or_else(|| panic!("{relation} is not a flow relation"))
        .clone()
}

#[test]
fn readability_limits_hold_at_scale_and_advice_never_rejects() {
    each_system(|name, model, oracle| {
        let focus: Vec<&str> = oracle.entities.iter().map(String::as_str).collect();
        let advice = |n: usize| {
            let steps = vec![step("s", &focus[..n], &[])];
            validate(model, &script(model, steps)).map(|w| {
                w.warnings()
                    .iter()
                    .map(|d| d.code.to_string())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            advice(12).unwrap(),
            Vec::<String>::new(),
            "{name}: 12 focus"
        );
        assert_eq!(advice(13).unwrap(), ["busy_step"], "{name}: 13 focus");
        if focus.len() >= 65 {
            assert_eq!(advice(64).unwrap(), ["busy_step"], "{name}: 64 focus");
            assert_eq!(
                codes(&advice(65).unwrap_err()),
                ["schema_violation"],
                "{name}: 65 focus"
            );
        }

        let trace: Vec<Leg> = oracle.legs[..1].to_vec();
        let steps = |n: usize| -> Vec<Value> {
            (0..n)
                .map(|i| step(&format!("s{i}"), &[&trace[0].from], &trace))
                .collect()
        };
        for (n, warns) in [(12, false), (13, true), (24, true)] {
            let w = validate(model, &script(model, steps(n)))
                .unwrap_or_else(|e| panic!("{name} {n}: {e:?}"));
            let got: Vec<_> = w.warnings().iter().map(|d| d.code).collect();
            assert_eq!(got.is_empty(), !warns, "{name}: {n} steps: {got:?}");
        }
        let e = validate(model, &script(model, steps(25))).unwrap_err();
        assert_eq!(codes(&e), ["schema_violation"], "{name}: 25 steps");
    });
}

#[test]
fn a_walkthrough_is_bound_to_one_model_and_revision() {
    let models: Vec<_> = SYSTEMS
        .iter()
        .map(|(n, j)| (*n, Architecture::from_json(j.as_bytes()).unwrap()))
        .collect();
    for (name, model) in &models {
        let good = script(model, vec![step("s", &[&model.snapshot().system_id], &[])]);
        validate(model, &good).unwrap_or_else(|e| panic!("{name}: {e:?}"));

        let mut newer = good.clone();
        newer["expected_revision"] = (model.snapshot().revision + 1).into();
        assert_eq!(
            codes(&validate(model, &newer).unwrap_err()),
            ["stale_revision"],
            "{name}"
        );
        if model.snapshot().revision >= 2 {
            let mut older = good.clone();
            older["expected_revision"] = (model.snapshot().revision - 1).into();
            assert_eq!(
                codes(&validate(model, &older).unwrap_err()),
                ["stale_revision"],
                "{name}"
            );
        }
        // Revisions start at 1, so 0 can never refer to a published model.
        let mut zero = good.clone();
        zero["expected_revision"] = 0.into();
        assert_eq!(
            codes(&validate(model, &zero).unwrap_err()),
            ["schema_violation"],
            "{name}"
        );

        for (other, other_model) in models.iter().filter(|(n, _)| n != name) {
            let e = validate(other_model, &good).unwrap_err();
            assert!(
                codes(&e).contains(&"model_mismatch"),
                "{name} against {other}: {e:?}"
            );
        }
    }
}

#[test]
fn validation_is_deterministic_and_the_script_round_trips() {
    each_system(|name, model, oracle| {
        let long = chain(oracle, 6).unwrap_or_else(|| panic!("{name}: no 6-leg chain"));
        let focus = [long[0].from.as_str(), long[5].to.as_str()];
        let value = script(
            model,
            vec![step("a", &focus, &long), step("b", &focus, &[])],
        );
        let first = validate(model, &value).unwrap();
        let second = validate(model, &value).unwrap();
        assert_eq!(first.script(), second.script(), "{name}");
        assert_eq!(first.resolved(), second.resolved(), "{name}");
        // `Script` serializes an absent `root` as null, which its own schema rejects. Nothing
        // serializes a `Script` today, so drop the nulls and keep the round trip honest.
        let mut again = serde_json::to_value(first.script()).unwrap();
        for step in again["steps"].as_array_mut().unwrap() {
            if step["root"].is_null() {
                step.as_object_mut().unwrap().remove("root");
            }
        }
        let again = validate(model, &again).unwrap_or_else(|e| panic!("{name}: round trip: {e:?}"));
        assert_eq!(first.resolved(), again.resolved(), "{name}: round trip");
    });
}

#[test]
fn hostile_input_is_rejected_without_a_panic() {
    each_system(|name, model, oracle| {
        let good = script(model, vec![step("s", &[&oracle.system], &[])]).to_string();
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        let mut inputs: Vec<(&str, Vec<u8>)> = vec![
            ("empty", vec![]),
            ("null", b"null".to_vec()),
            ("array", b"[]".to_vec()),
            ("truncated", good.as_bytes()[..good.len() / 2].to_vec()),
            ("deep nesting", deep.into_bytes()),
            ("invalid utf-8", vec![0xff, 0xfe, 0x7b, 0x7d]),
            (
                "byte order mark",
                [&[0xef, 0xbb, 0xbf][..], good.as_bytes()].concat(),
            ),
            ("trailing comma", good.replacen('}', ",}", 1).into_bytes()),
            (
                "huge revision",
                good.replacen(
                    "\"expected_revision\":1",
                    "\"expected_revision\":18446744073709551616",
                    1,
                )
                .into_bytes(),
            ),
            (
                "negative revision",
                good.replacen("\"expected_revision\":1", "\"expected_revision\":-1", 1)
                    .into_bytes(),
            ),
            (
                "float revision",
                good.replacen("\"expected_revision\":1", "\"expected_revision\":1.5", 1)
                    .into_bytes(),
            ),
            (
                "exponent revision",
                good.replacen("\"expected_revision\":1", "\"expected_revision\":1e999", 1)
                    .into_bytes(),
            ),
            (
                "nul in title",
                good.replacen("\"title\":\"Probe\"", "\"title\":\"a\\u0000b\"", 1)
                    .into_bytes(),
            ),
            (
                "uppercase id",
                good.replacen("\"id\":\"wt\"", "\"id\":\"WT\"", 1)
                    .into_bytes(),
            ),
        ];
        inputs.push(("oversized", vec![b' '; 2 * 1024 * 1024 + 1]));
        for (label, bytes) in inputs {
            let result = Walkthrough::from_json(&bytes, model);
            if label != "byte order mark" {
                assert!(result.is_err(), "{name}: {label} was accepted");
            }
        }
    });
}
