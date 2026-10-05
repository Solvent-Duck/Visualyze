use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    pub schema_version: String,
    pub model_id: String,
    #[serde(deserialize_with = "integer")]
    pub revision: u64,
    pub system_id: String,
    pub snapshot: SnapshotInfo,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub source_revision: Option<String>,
    pub coverage: Coverage,
    pub scope_description: String,
    pub omissions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Partial,
    Complete,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entity {
    pub id: String,
    pub kind: EntityKind,
    pub label: String,
    pub boundary: Boundary,
    pub subtype: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub technology: Vec<String>,
    pub protocol: Option<String>,
    pub basis: Basis,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Scope,
    Client,
    Service,
    Datastore,
    Interface,
    Concern,
    Deployment,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scope => "scope",
            Self::Client => "client",
            Self::Service => "service",
            Self::Datastore => "datastore",
            Self::Interface => "interface",
            Self::Concern => "concern",
            Self::Deployment => "deployment",
        }
    }

    pub(crate) fn is_component(self) -> bool {
        matches!(self, Self::Client | Self::Service | Self::Datastore)
    }

    pub(crate) fn is_caller(self) -> bool {
        matches!(self, Self::Client | Self::Service)
    }

    pub(crate) fn is_logical(self) -> bool {
        self.is_component() || matches!(self, Self::Scope | Self::Concern)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    Internal,
    External,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Supported,
    Inferred,
    Declared,
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Relation {
    pub id: String,
    pub kind: RelationKind,
    pub source: String,
    pub target: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub aspect: Option<String>,
    pub condition: Option<String>,
    pub dependency_phase: Option<DependencyPhase>,
    pub necessity: Option<Necessity>,
    pub basis: Basis,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyPhase {
    Runtime,
    Build,
    Startup,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Necessity {
    Required,
    Optional,
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub description: String,
    pub locator: Option<Locator>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Locator {
    Source {
        path: String,
        #[serde(default, deserialize_with = "optional_integer")]
        start_line: Option<u64>,
        #[serde(default, deserialize_with = "optional_integer")]
        end_line: Option<u64>,
    },
    Reference {
        value: String,
    },
}

// JSON Schema accepts 1.0 as an integer. Keep the Rust decoding consistent with it.
fn number(n: serde_json::Number) -> Result<u64, &'static str> {
    let value = n.as_f64().ok_or("expected a safe positive integer")?;
    if (1.0..=9_007_199_254_740_991.0).contains(&value) && value.fract() == 0.0 {
        Ok(value as u64)
    } else {
        Err("expected a safe positive integer")
    }
}

fn integer<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    number(serde_json::Number::deserialize(d)?).map_err(serde::de::Error::custom)
}

fn optional_integer<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::<serde_json::Number>::deserialize(d)?
        .map(number)
        .transpose()
        .map_err(serde::de::Error::custom)
}
