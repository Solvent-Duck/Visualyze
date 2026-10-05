//! Declared distributed AI-platform fixture; no claim about any actual company.
//! Usage: generate_ai_fixture NEW_BUNDLE [REGIONS WORKERS TABLES]
use std::{collections::BTreeMap, error::Error};
use visualyze_model::provider::*;

#[derive(Default)]
struct Fixture {
    entities: BTreeMap<String, EntityRecord>,
    relations: BTreeMap<String, RelationRecord>,
}
impl Fixture {
    fn object(&mut self, id: &str, kind: EntityKind, label: &str, parent: Option<&str>) {
        assert!(self.entities.insert(id.into(), EntityRecord {
            summary: EntitySummary { id: id.into(), kind, label: label.into(),
                boundary: Boundary::Internal, claim: Claim::declared() },
            detail: Some(EntityDetail {
                description: "Declared synthetic architecture for placement evaluation; not extracted from a real AI provider.".into(),
                subtype: None, roles: vec![], technology: vec![],
            }),
        }).is_none(), "duplicate synthetic identity");
        if let Some(parent) = parent {
            let relation = match kind {
                EntityKind::Table => RelationKind::OwnsTable,
                EntityKind::Column => RelationKind::OwnsColumn,
                _ => RelationKind::Contains,
            };
            self.edge("ownership", relation, parent, id);
        }
    }
    fn edge(&mut self, name: &str, kind: RelationKind, source: &str, target: &str) {
        let id = format!("rel:{name}:{source}:{target}");
        assert!(
            self.relations
                .insert(
                    id.clone(),
                    RelationRecord {
                        id,
                        kind,
                        source: source.into(),
                        target: target.into(),
                        claim: Claim::declared(),
                        label: None,
                        aspect: (kind == RelationKind::AppliesTo).then(|| "authentication".into()),
                    }
                )
                .is_none(),
            "duplicate synthetic relationship"
        );
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .ok_or("usage: generate_ai_fixture NEW_BUNDLE [REGIONS WORKERS TABLES]")?;
    let regions = args
        .next()
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(3);
    let workers = args
        .next()
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(12);
    let tables = args
        .next()
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(80);
    if args.next().is_some()
        || !(2..=12).contains(&regions)
        || !(2..=256).contains(&workers)
        || !(2..=10_000).contains(&tables)
    {
        return Err("expected 2..12 regions, 2..256 workers, 2..10000 tables".into());
    }
    let mut f = Fixture::default();
    f.object(
        "scope:ai-platform",
        EntityKind::Scope,
        "Declared AI platform",
        None,
    );
    for (key, label) in [
        ("edge", "Entry and identity"),
        ("inference", "Inference"),
        ("retrieval", "Retrieval"),
        ("learning", "Learning and evaluation"),
        ("events", "Asynchronous exchanges"),
        ("data", "Shared data"),
        ("operations", "Operations"),
    ] {
        f.object(
            &format!("scope:{key}"),
            EntityKind::Scope,
            label,
            Some("scope:ai-platform"),
        );
    }
    f.object(
        "client:web",
        EntityKind::Client,
        "Conversation client",
        Some("scope:edge"),
    );
    for (id, label, parent) in [
        ("gateway", "Request gateway", "edge"),
        ("identity", "Shared identity", "edge"),
        ("router", "Inference router", "inference"),
        ("scheduler", "Batch scheduler", "inference"),
        ("retriever", "Context retrieval", "retrieval"),
        ("embedder", "Embedding service", "retrieval"),
        ("trainer", "Training coordinator", "learning"),
        ("evaluator", "Evaluation", "learning"),
        ("feedback", "Feedback ingestion", "learning"),
        ("telemetry", "Telemetry collector", "operations"),
        ("billing", "Usage accounting", "operations"),
        ("diagnostic", "Isolated diagnostic", "operations"),
    ] {
        f.object(
            &format!("service:{id}"),
            EntityKind::Service,
            label,
            Some(&format!("scope:{parent}")),
        );
    }
    for i in 0..workers {
        f.object(
            &format!("service:worker-{i:03}"),
            EntityKind::Service,
            &format!("Inference worker {i:03}"),
            Some("scope:inference"),
        );
    }
    f.object(
        "concern:auth",
        EntityKind::Concern,
        "Explicit auth coverage",
        Some("scope:edge"),
    );
    f.edge(
        "implementation",
        RelationKind::ImplementedBy,
        "concern:auth",
        "service:identity",
    );
    for target in ["service:gateway", "service:router", "service:retriever"] {
        f.edge("coverage", RelationKind::AppliesTo, "concern:auth", target);
    }
    for (id, label) in [
        ("conversations", "Conversation storage"),
        ("vectors", "Vector index"),
        ("models", "Model registry"),
        ("datasets", "Training datasets"),
        ("usage", "Usage records"),
    ] {
        f.object(
            &format!("datastore:{id}"),
            EntityKind::Datastore,
            label,
            Some("scope:data"),
        );
    }
    for i in 0..tables {
        let table = format!("table:conversation-{i:05}");
        f.object(
            &table,
            EntityKind::Table,
            &format!("Conversation table {i:05}"),
            Some("datastore:conversations"),
        );
        for column in ["id", "parent"] {
            f.object(
                &format!("column:conversation-{i:05}-{column}"),
                EntityKind::Column,
                column,
                Some(&table),
            );
        }
        f.edge(
            "cross-table",
            RelationKind::TableReferences,
            &table,
            &format!("table:conversation-{:05}", (i + 1) % tables),
        );
    }
    for (id, label) in [
        ("jobs", "Inference jobs"),
        ("completed", "Completion events"),
        ("feedback", "Feedback events"),
    ] {
        let channel = format!("channel:{id}");
        let message = format!("message:{id}");
        f.object(
            &channel,
            EntityKind::MessageChannel,
            label,
            Some("scope:events"),
        );
        f.object(
            &message,
            EntityKind::MessageType,
            &format!("{label} payload"),
            Some("scope:events"),
        );
        f.edge("carries", RelationKind::Carries, &channel, &message);
    }
    for (source, target) in [
        ("client:web", "service:gateway"),
        ("service:gateway", "service:router"),
        ("service:gateway", "service:identity"),
        ("service:router", "service:retriever"),
        ("service:retriever", "service:embedder"),
        ("service:router", "service:scheduler"),
    ] {
        f.edge("request", RelationKind::Requests, source, target);
    }
    for (source, target) in [
        ("gateway", "conversations"),
        ("retriever", "vectors"),
        ("trainer", "datasets"),
        ("evaluator", "models"),
    ] {
        f.edge(
            "read",
            RelationKind::Reads,
            &format!("service:{source}"),
            &format!("datastore:{target}"),
        );
    }
    f.edge(
        "write",
        RelationKind::Writes,
        "service:gateway",
        "table:conversation-00000",
    );
    f.edge(
        "write",
        RelationKind::Writes,
        "service:feedback",
        "datastore:datasets",
    );
    f.edge(
        "write",
        RelationKind::Writes,
        "service:billing",
        "datastore:usage",
    );
    f.edge(
        "write",
        RelationKind::Writes,
        "service:trainer",
        "datastore:models",
    );
    for (source, channel) in [
        ("gateway", "jobs"),
        ("scheduler", "jobs"),
        ("feedback", "feedback"),
        ("evaluator", "feedback"),
    ] {
        f.edge(
            "publish",
            RelationKind::Publishes,
            &format!("service:{source}"),
            &format!("channel:{channel}"),
        );
    }
    for (source, channel) in [
        ("evaluator", "completed"),
        ("billing", "completed"),
        ("telemetry", "completed"),
        ("trainer", "feedback"),
    ] {
        f.edge(
            "subscribe",
            RelationKind::Subscribes,
            &format!("service:{source}"),
            &format!("channel:{channel}"),
        );
    }
    for i in 0..workers {
        let worker = format!("service:worker-{i:03}");
        f.edge(
            "subscribe",
            RelationKind::Subscribes,
            &worker,
            "channel:jobs",
        );
        f.edge(
            "publish",
            RelationKind::Publishes,
            &worker,
            "channel:completed",
        );
        f.edge("read", RelationKind::Reads, &worker, "datastore:models");
        f.edge("coverage", RelationKind::AppliesTo, "concern:auth", &worker);
        // Explicit directed ring is a declared dependency, not a fabricated request.
        f.edge(
            "feedback-cycle",
            RelationKind::DependsOn,
            &worker,
            &format!("service:worker-{:03}", (i + 1) % workers),
        );
    }
    // Deployment has its own forest. Its nodes intentionally have no invented
    // logical owner. Instances resolve to one supplied logical component each.
    f.object(
        "deployment:global",
        EntityKind::Deployment,
        "Declared global deployment",
        None,
    );
    for region in 0..regions {
        let region_id = format!("deployment:region-{region:02}");
        f.object(
            &region_id,
            EntityKind::Deployment,
            &format!("Region {region:02}"),
            None,
        );
        f.edge(
            "location",
            RelationKind::DeployedIn,
            &region_id,
            "deployment:global",
        );
        for pool in ["cpu", "accelerators"] {
            let pool_id = format!("deployment:region-{region:02}-{pool}");
            f.object(
                &pool_id,
                EntityKind::Deployment,
                &format!("Region {region:02} {pool}"),
                None,
            );
            f.edge("location", RelationKind::DeployedIn, &pool_id, &region_id);
        }
        let components = ["gateway", "router", "scheduler", "retriever", "identity"]
            .into_iter()
            .map(str::to_owned)
            .chain((0..workers).map(|i| format!("worker-{i:03}")));
        for component in components {
            let instance = format!("instance:region-{region:02}-{component}");
            f.object(
                &instance,
                EntityKind::DeploymentInstance,
                &format!("{component} in region {region:02}"),
                None,
            );
            f.edge(
                "instance",
                RelationKind::InstanceOf,
                &instance,
                &format!("service:{component}"),
            );
            let pool = if component.starts_with("worker-") {
                "accelerators"
            } else {
                "cpu"
            };
            f.edge(
                "location",
                RelationKind::DeployedIn,
                &instance,
                &format!("deployment:region-{region:02}-{pool}"),
            );
        }
    }
    let mut writer = BundleWriter::create(output, Manifest {
        format: FORMAT_VERSION.into(), model_id: format!("synthetic:ai:{regions}:{workers}:{tables}"),
        revision: 1, root_id: "scope:ai-platform".into(),
        profiles: vec![Profile::Architecture, Profile::Data, Profile::Messaging, Profile::Instances],
        coverage: Coverage::Partial,
        scope_description: format!("Declared AI-platform evaluation fixture with {regions} regions, {workers} logical workers, shared identity/data, asynchronous fan-out, a dependency cycle, and {tables} tables."),
        omissions: vec!["Entirely synthetic; no claim to model OpenAI, ChatGPT, or any real company's internal architecture.".into(),
            "No capacity, latency, scale, failover, security-completeness, deployment traffic or operational correctness claims. Relationships across instances are not inferred from logical dependencies.".into(),
            "Code and fields beyond the two declared columns per table are not supplied.".into()],
    }, Limits::default())?;
    for entity in f.entities.into_values() {
        writer.push(&BundleRecord::Entity(entity))?;
    }
    for relation in f.relations.into_values() {
        writer.push(&BundleRecord::Relation(relation))?;
    }
    let provider = writer.finish()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&provider.stats().counts)?
    );
    Ok(())
}
