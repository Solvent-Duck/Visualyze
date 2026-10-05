//! Public semantic models. The architecture module validates full draft snapshots;
//! the original Database type remains only for the historical Phase 0 fixture.

pub mod architecture;
pub mod provider;

use serde::Deserialize;

/// One hand-authored database for the first visual proof.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub id: String,
    pub label: String,
    pub kind: EntityKind,
    pub basis: EvidenceBasis,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Datastore,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceBasis {
    Declared,
}

impl Database {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let database: Self = serde_json::from_str(json)?;
        if database.id.trim().is_empty() || database.label.trim().is_empty() {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "database id and label must not be blank",
            ));
        }
        Ok(database)
    }
}

pub const DATABASE_FIXTURE: &str = include_str!("../../../examples/database.json");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_declared_database_with_stable_identity() {
        let database = Database::from_json(DATABASE_FIXTURE).unwrap();
        assert_eq!(database.id, "example-database");
        assert_eq!(database.label, "Database");
        assert_eq!(database.kind, EntityKind::Datastore);
        assert_eq!(database.basis, EvidenceBasis::Declared);
    }

    #[test]
    fn rejects_malformed_missing_and_unsupported_data() {
        for json in [
            "not json".to_owned(),
            "{}".to_owned(),
            DATABASE_FIXTURE.replace("datastore", "service"),
            DATABASE_FIXTURE.replace("declared", "supported"),
            DATABASE_FIXTURE.replace("example-database", "  "),
            DATABASE_FIXTURE.replace("Database", ""),
            DATABASE_FIXTURE.replace("\"kind\":", "\"mesh\": {}, \"kind\":"),
        ] {
            assert!(Database::from_json(&json).is_err(), "accepted: {json}");
        }
    }
}
