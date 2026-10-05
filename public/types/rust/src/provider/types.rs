use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: &str = "visualyze.bundle.v1";

/// A semantic view, independent of the geometric placement policy. Deployment
/// follows supplied `deployed_in` relations; its root scope is a display envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticLens {
    Logical,
    Code,
    Deployment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Profile {
    #[serde(rename = "architecture.v1")]
    Architecture,
    #[serde(rename = "data.v1")]
    Data,
    #[serde(rename = "code.v1")]
    Code,
    #[serde(rename = "messaging.v1")]
    Messaging,
    #[serde(rename = "instances.v1")]
    Instances,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub model_id: String,
    pub revision: u64,
    pub root_id: String,
    pub profiles: Vec<Profile>,
    pub coverage: Coverage,
    pub scope_description: String,
    pub omissions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    Internal,
    External,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Supported,
    Inferred,
    Declared,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub basis: Basis,
    pub evidence_ids: Vec<String>,
    pub rationale: Option<String>,
}

impl Claim {
    pub fn declared() -> Self {
        Self {
            basis: Basis::Declared,
            evidence_ids: Vec::new(),
            rationale: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Scope,
    Client,
    Service,
    Datastore,
    Interface,
    Concern,
    Deployment,
    Table,
    Column,
    CodeRepository,
    CodePackage,
    CodeFile,
    CodeType,
    CodeFunction,
    MessageChannel,
    MessageType,
    DeploymentInstance,
}

impl EntityKind {
    pub fn profile(self) -> Profile {
        match self {
            Self::Table | Self::Column => Profile::Data,
            Self::CodeRepository
            | Self::CodePackage
            | Self::CodeFile
            | Self::CodeType
            | Self::CodeFunction => Profile::Code,
            Self::MessageChannel | Self::MessageType => Profile::Messaging,
            Self::DeploymentInstance => Profile::Instances,
            _ => Profile::Architecture,
        }
    }

    pub(super) fn component(self) -> bool {
        matches!(self, Self::Client | Self::Service | Self::Datastore)
    }

    pub(super) fn caller(self) -> bool {
        matches!(self, Self::Client | Self::Service)
    }

    pub(super) fn code(self) -> bool {
        self.profile() == Profile::Code
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitySummary {
    pub id: String,
    pub kind: EntityKind,
    pub label: String,
    pub boundary: Boundary,
    pub claim: Claim,
}

/// Descriptive data only. No file or URL in this profile is dereferenced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityDetail {
    pub description: String,
    pub subtype: Option<String>,
    pub roles: Vec<String>,
    pub technology: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRecord {
    pub summary: EntitySummary,
    /// None means the producer supplied no additional detail.
    pub detail: Option<EntityDetail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Contains,
    Exposes,
    Requests,
    Reads,
    Writes,
    DependsOn,
    AppliesTo,
    ImplementedBy,
    DeployedIn,
    OwnsTable,
    OwnsColumn,
    OwnsCode,
    TableReferences,
    CodeReferences,
    Publishes,
    Subscribes,
    Carries,
    InstanceOf,
}

impl RelationKind {
    pub fn profile(self) -> Profile {
        match self {
            Self::OwnsTable | Self::OwnsColumn | Self::TableReferences => Profile::Data,
            Self::OwnsCode | Self::CodeReferences => Profile::Code,
            Self::Publishes | Self::Subscribes | Self::Carries => Profile::Messaging,
            Self::InstanceOf => Profile::Instances,
            _ => Profile::Architecture,
        }
    }

    pub fn is_ownership(self) -> bool {
        matches!(
            self,
            Self::Contains | Self::Exposes | Self::OwnsTable | Self::OwnsColumn | Self::OwnsCode
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationRecord {
    pub id: String,
    pub kind: RelationKind,
    pub source: String,
    pub target: String,
    pub claim: Claim,
    pub label: Option<String>,
    pub aspect: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    pub id: String,
    pub description: String,
    /// Opaque display text, never fetched or interpreted as an executable path.
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub entities: usize,
    pub relations: usize,
    pub evidence: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "record",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum BundleRecord {
    Manifest(Manifest),
    Evidence(EvidenceRecord),
    Entity(EntityRecord),
    Relation(RelationRecord),
    Complete(Counts),
}

impl BundleRecord {
    pub(super) fn identity(&self) -> Option<(&str, u8)> {
        match self {
            Self::Evidence(e) => Some((&e.id, 1)),
            Self::Entity(e) => Some((&e.summary.id, 2)),
            Self::Relation(e) => Some((&e.id, 3)),
            _ => None,
        }
    }
}
