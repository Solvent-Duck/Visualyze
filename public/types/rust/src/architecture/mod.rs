//! Validated draft architecture snapshots, independent of rendering and file paths.
mod graph;
pub(crate) mod input;
mod types;

pub use types::*;

use std::{fmt, io::Read, sync::LazyLock};

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_DEPTH: usize = 16;
pub const MAX_DIAGNOSTICS: usize = 32;
pub const SCHEMA: &str = include_str!("../../../../schema/architecture.schema.json");
pub const OBJECTS_FIXTURE: &str = include_str!("../../../../examples/objects.json");

static VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str(SCHEMA).expect("embedded schema is JSON");
    jsonschema::validator_for(&schema).expect("embedded architecture schema is valid")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn new(
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {}: {}",
            self.code,
            if self.path.is_empty() {
                "/"
            } else {
                &self.path
            },
            self.message
        )
    }
}

#[derive(Debug)]
pub struct ValidationError {
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationError {
    pub(crate) fn one(
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            diagnostics: vec![Diagnostic::new(code, path, message)],
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, diagnostic) in self.diagnostics.iter().enumerate() {
            if i != 0 {
                writeln!(f)?;
            }
            write!(f, "{diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

/// Only this validated, immutable wrapper can enter the renderer.
#[derive(Debug)]
pub struct Architecture {
    snapshot: Snapshot,
    warnings: Vec<Diagnostic>,
}

impl Architecture {
    pub fn from_reader(reader: impl Read) -> Result<Self, ValidationError> {
        let mut bytes = Vec::new();
        reader
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| ValidationError::one("input_io", "", e.to_string()))?;
        Self::from_json(&bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, ValidationError> {
        if bytes.len() > MAX_BYTES {
            return Err(ValidationError::one(
                "payload_too_large",
                "",
                "architecture JSON exceeds 2 MiB",
            ));
        }
        let value = input::parse(bytes)?;
        let diagnostics: Vec<_> = VALIDATOR
            .iter_errors(&value)
            .take(MAX_DIAGNOSTICS)
            .map(|e| {
                Diagnostic::new(
                    "schema_violation",
                    e.instance_path().to_string(),
                    e.masked().to_string(),
                )
            })
            .collect();
        if !diagnostics.is_empty() {
            return Err(ValidationError { diagnostics });
        }
        let snapshot = serde_json::from_value(value)
            .map_err(|e| ValidationError::one("typed_decode", "", e.to_string()))?;
        let warnings = graph::validate(&snapshot)?;
        Ok(Self { snapshot, warnings })
    }

    /// For snapshots derived from an already validated model, such as teaching views. The
    /// graph rules still run, so a derivation cannot produce an invalid model.
    pub(crate) fn from_snapshot(snapshot: Snapshot) -> Result<Self, ValidationError> {
        let warnings = graph::validate(&snapshot)?;
        Ok(Self { snapshot, warnings })
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }
}
