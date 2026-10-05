//! Explicit offline producer for supplied Rust source; never invoked by the viewer.
//! Reads syntax and manifests without executing Cargo, macros, or source code.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use proc_macro2::Span;
use quote::ToTokens;
use serde::Serialize;
use syn::{Item, UseTree, spanned::Spanned};
use toml_edit::{DocumentMut, Item as TomlItem};
use visualyze_model::provider::{
    Basis, Boundary, BundleRecord, BundleWriter, Claim, Coverage, EntityDetail, EntityKind,
    EntityRecord, EntitySummary, EvidenceRecord, FORMAT_VERSION, Limits, Manifest, Profile,
    RelationKind, RelationRecord,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_FILES: usize = 100_000;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RECORDS: usize = 2_000_000;
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".visualyze",
    "visualyze-local",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityScheme {
    /// Original position-based keys; preserves published bundle bytes by default.
    #[default]
    LegacyV1,
    /// Lexical scope/name/conditional anchors; ambiguous duplicates are errors.
    /// A switch from legacy-v1 requires a new model identity, not an update.
    SyntaxV2,
}

impl std::str::FromStr for IdentityScheme {
    type Err = &'static str;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "legacy-v1" => Ok(Self::LegacyV1),
            "syntax-v2" => Ok(Self::SyntaxV2),
            _ => Err("--identity-scheme must be legacy-v1 or syntax-v2"),
        }
    }
}

pub struct Options {
    pub root: PathBuf,
    pub output: PathBuf,
    /// Forty hexadecimal characters, independently verified by the caller.
    pub revision: String,
    /// Source browser base, e.g. https://github.com/bevyengine/bevy.
    pub repository: String,
    pub name: String,
    /// Optional stable publication identity; requires `model_revision`.
    pub model_id: Option<String>,
    /// Positive publication counter, separate from source provenance.
    pub model_revision: Option<u64>,
    /// Keep constant across a publication sequence; the producer is stateless
    /// and cannot check the scheme used by an earlier bundle with this model ID.
    pub identity_scheme: IdentityScheme,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub producer: &'static str,
    pub identity_scheme: IdentityScheme,
    pub source_revision: String,
    pub repository: String,
    pub model_id: String,
    pub model_revision: u64,
    pub root_id: String,
    pub repository_id: String,
    pub packages: usize,
    pub scanned_source_files: usize,
    pub unowned_rust_files: Vec<String>,
    pub skipped_symlinks: Vec<String>,
    pub rust_files: usize,
    pub parsed_files: usize,
    pub unparsed_files: Vec<String>,
    pub source_bytes: u64,
    pub declarations: usize,
    pub manifest_references: usize,
    pub import_references: usize,
    pub unresolved_imports: usize,
    pub omitted_external_dependencies: usize,
    pub omitted_self_dependencies: usize,
    pub entities: usize,
    pub relations: usize,
    pub evidence: usize,
    pub bundle_bytes: u64,
}

struct Package {
    name: String,
    manifest: PathBuf,
    directory: PathBuf,
    id: String,
    document: DocumentMut,
    aliases: BTreeMap<String, String>,
}

struct Source {
    path: PathBuf,
    package: usize,
    id: String,
    /// Rust module path only for conventional src/ modules.
    module: Option<Vec<String>>,
}

struct Records {
    evidence: BTreeMap<String, EvidenceRecord>,
    entities: BTreeMap<String, EntityRecord>,
    relations: BTreeMap<String, RelationRecord>,
    keys: BTreeMap<String, String>,
    repository: String,
    revision: String,
}

impl Records {
    fn id(&mut self, prefix: &str, key: &str) -> Result<String> {
        let id = stable_id(prefix, key);
        if let Some(previous) = self.keys.insert(id.clone(), key.to_owned())
            && previous != key
        {
            return Err(format!("ID hash collision for {id}; refusing ambiguous bundle").into());
        }
        if self.keys.len() > MAX_RECORDS {
            return Err("producer record budget exceeded".into());
        }
        Ok(id)
    }

    fn evidence(&mut self, path: &Path, line: usize, description: &str) -> Result<String> {
        let relative = slash(path);
        let id = self.id("e", &format!("{relative}:{line}"))?;
        self.evidence.entry(id.clone()).or_insert_with(|| EvidenceRecord {
            id: id.clone(),
            description: format!("{description}; {relative}:{line}. Revision was supplied by the caller; syntax was read locally."),
            reference: Some(format!("{}/blob/{}/{}#L{}", self.repository, self.revision, url_path(&relative), line)),
        });
        Ok(id)
    }

    fn entity(
        &mut self,
        id: String,
        kind: EntityKind,
        label: String,
        detail: String,
        evidence: String,
    ) -> Result<()> {
        let record = EntityRecord {
            summary: EntitySummary {
                id: id.clone(),
                kind,
                label,
                boundary: Boundary::Internal,
                claim: supported(evidence),
            },
            detail: Some(EntityDetail {
                description: detail,
                subtype: Some("rust_syntax".to_owned()),
                roles: Vec::new(),
                technology: vec!["Rust".to_owned()],
            }),
        };
        if self.entities.insert(id, record).is_some() {
            return Err("duplicate semantic entity identity".into());
        }
        Ok(())
    }

    fn relation(
        &mut self,
        kind: RelationKind,
        source: &str,
        target: &str,
        evidence: String,
        label: &str,
        discriminator: &str,
    ) -> Result<String> {
        let id = self.id("r", &format!("{kind:?}:{source}:{target}:{discriminator}"))?;
        self.relations.entry(id.clone()).or_insert(RelationRecord {
            id: id.clone(),
            kind,
            source: source.to_owned(),
            target: target.to_owned(),
            claim: supported(evidence),
            label: Some(label.to_owned()),
            aspect: None,
        });
        Ok(id)
    }

    fn owns(&mut self, parent: &str, child: &str, evidence: String) -> Result<()> {
        self.relation(
            RelationKind::OwnsCode,
            parent,
            child,
            evidence,
            "Source ownership",
            "",
        )
        .map(|_| ())
    }
}

fn supported(evidence: String) -> Claim {
    Claim {
        basis: Basis::Supported,
        evidence_ids: vec![evidence],
        rationale: None,
    }
}

/// Stable syntax-key identity, not a cryptographic checksum. Collisions are fatal.
pub fn stable_id(prefix: &str, key: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in key.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("{prefix}:{hash:016x}")
}

pub fn produce(options: &Options) -> Result<Report> {
    validate_options(options)?;
    let root = options.root.canonicalize()?;
    if !fs::symlink_metadata(&options.root)?.is_dir() {
        return Err("source root must be a real directory, not a symlink".into());
    }
    let mut paths = Vec::new();
    let mut skipped_symlinks = Vec::new();
    discover(&root, Path::new(""), &mut paths, &mut skipped_symlinks, 0)?;
    paths.sort();
    let mut bytes = 0;
    let mut packages = Vec::new();
    let mut workspace = None;
    for path in paths
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "Cargo.toml"))
    {
        let text = read_source(&root, path, &mut bytes)?;
        let document = text
            .parse::<DocumentMut>()
            .map_err(|e| format!("{}: {e}", slash(path)))?;
        if path == Path::new("Cargo.toml") {
            workspace = Some(document.clone());
        }
        if let Some(name) = document
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(TomlItem::as_str)
        {
            packages.push(Package {
                name: name.to_owned(),
                manifest: path.clone(),
                directory: path.parent().unwrap_or(Path::new("")).to_owned(),
                id: stable_id("package", &slash(path)),
                document,
                aliases: BTreeMap::new(),
            });
        }
    }
    if packages.is_empty() {
        return Err("no Cargo package manifests were supplied".into());
    }
    let workspace = workspace.ok_or("source root must contain Cargo.toml for provenance")?;
    let model_id = options.model_id.clone().unwrap_or_else(|| {
        let legacy = format!("{}:{}", options.repository, options.revision);
        stable_id(
            "model",
            &match options.identity_scheme {
                IdentityScheme::LegacyV1 => legacy,
                IdentityScheme::SyntaxV2 => format!("{legacy}:syntax-v2"),
            },
        )
    });
    let model_revision = options.model_revision.unwrap_or(1);
    let root_id = stable_id("scope", &options.repository);
    let repository_id = stable_id("repository", &options.repository);
    let mut records = Records {
        evidence: BTreeMap::new(),
        entities: BTreeMap::new(),
        relations: BTreeMap::new(),
        keys: BTreeMap::new(),
        repository: options.repository.trim_end_matches('/').to_owned(),
        revision: options.revision.to_ascii_lowercase(),
    };
    let evidence = records.evidence(
        Path::new("Cargo.toml"),
        1,
        "Supplied Rust repository root manifest",
    )?;
    records.entity(
        root_id.clone(),
        EntityKind::Scope,
        bounded(&format!("{} code view", options.name), 512),
        "Supplied code inventory boundary; no runtime or deployment completeness is implied."
            .to_owned(),
        evidence.clone(),
    )?;
    records.entity(repository_id.clone(), EntityKind::CodeRepository, options.name.clone(), format!("Supplied Rust source at {} revision {}. This is a syntax inventory and declared dependency graph, not a runtime architecture or call graph.", options.repository, options.revision), evidence.clone())?;
    records.relation(
        RelationKind::Contains,
        &root_id,
        &repository_id,
        evidence,
        "Supplied repository",
        "",
    )?;
    for package in &packages {
        let evidence = records.evidence(&package.manifest, 1, "Cargo package declaration")?;
        records.entity(package.id.clone(), EntityKind::CodePackage, package.name.clone(), format!("Cargo package {} declared by {}. All conditional source and dependency declarations are included without evaluating features or targets.", package.name, slash(&package.manifest)), evidence.clone())?;
        records.owns(&repository_id, &package.id, evidence)?;
    }
    let mut report = Report {
        producer: "visualyze-rust-bundle/0.1",
        identity_scheme: options.identity_scheme,
        source_revision: options.revision.clone(),
        repository: options.repository.clone(),
        model_id: model_id.clone(),
        model_revision,
        root_id: root_id.clone(),
        repository_id,
        packages: packages.len(),
        scanned_source_files: paths.len(),
        unowned_rust_files: Vec::new(),
        skipped_symlinks,
        rust_files: 0,
        parsed_files: 0,
        unparsed_files: Vec::new(),
        source_bytes: 0,
        declarations: 0,
        manifest_references: 0,
        import_references: 0,
        unresolved_imports: 0,
        omitted_external_dependencies: 0,
        omitted_self_dependencies: 0,
        entities: 0,
        relations: 0,
        evidence: 0,
        bundle_bytes: 0,
    };
    manifest_dependencies(&mut packages, &workspace, &mut records, &mut report)?;
    let mut sources = Vec::new();
    for path in paths
        .iter()
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
    {
        let owner = packages
            .iter()
            .enumerate()
            .filter(|(_, p)| path.starts_with(&p.directory))
            .max_by_key(|(_, p)| p.directory.components().count())
            .map(|(i, _)| i);
        let Some(package) = owner else {
            report.unowned_rust_files.push(slash(path));
            continue;
        };
        let id = records.id("file", &slash(path))?;
        let evidence = records.evidence(path, 1, "Rust source file")?;
        records.entity(id.clone(), EntityKind::CodeFile, slash(path.strip_prefix(&packages[package].directory)?), format!("Source file {}. Inventory includes test, example, and conditional syntax. Macro expansions and build-generated source are not present.", slash(path)), evidence.clone())?;
        records.owns(&packages[package].id, &id, evidence)?;
        let module = module_path(path.strip_prefix(&packages[package].directory)?);
        sources.push(Source {
            path: path.clone(),
            package,
            id,
            module,
        });
    }
    let module_files = module_index(&sources);
    for source in &sources {
        let text = read_source(&root, &source.path, &mut bytes)?;
        let parsed = match syn::parse_file(&text) {
            Ok(file) => file,
            Err(error) => {
                // Compile-fail fixtures can contain deliberately malformed Rust.
                report.unparsed_files.push(format!(
                    "{}:{}: {}",
                    slash(&source.path),
                    error.span().start().line,
                    error
                ));
                continue;
            }
        };
        report.parsed_files += 1;
        let mut file = FileInventory {
            source,
            packages: &packages,
            module_files: &module_files,
            records: &mut records,
            report: &mut report,
            identity_scheme: options.identity_scheme,
            anchors: BTreeMap::new(),
        };
        file.items(
            &parsed.items,
            &[],
            &[],
            &conditional_context(&[], &parsed.attrs),
        )?;
    }
    report.rust_files = sources.len();
    report.source_bytes = bytes;
    let mut omissions = vec![
        "Partial code view: no macro expansion, generated source, compiler name resolution, call graph, dataflow, runtime/deployment architecture, or traffic inference.".to_owned(),
        "All files under the supplied root (except .git, target, node_modules and local-viewer data) belong to their nearest Cargo package. This includes tests, examples, tools and cfg-inactive declarations; workspace membership and compilation are not inferred.".to_owned(),
        "Only dependencies to supplied packages are represented. External packages are counted as omitted. Cargo version, target, feature and optional conditions are descriptive; no resolver is run.".to_owned(),
        "Use declarations resolve conservatively to declared dependency packages or conventional src module files, not imported symbols or calls. Reexports, custom #[path], include!, extern crates, renamed library targets, and ambiguous file modules are not compiler-resolved.".to_owned(),
        "Inline modules are flattened into qualified item names; block-local items, const/static items and fields are omitted. Methods are grouped under a same-file declared type when unambiguous, otherwise directly under the file.".to_owned(),
        format!("{} source files failed Rust syntax parsing; file records remain supplied, with failure paths in the producer report. Revision is caller-supplied provenance; verify the checkout independently before importing.", report.unparsed_files.len()),
        format!("{} non-source symlinks were skipped without following them. Explicit Rust/Cargo symlinks are rejected. Paths are reported by the producer.", report.skipped_symlinks.len()),
        format!("{} Cargo self-dependency declarations are counted but omitted because bundle v1 forbids self-relations.", report.omitted_self_dependencies),
    ];
    if options.identity_scheme == IdentityScheme::SyntaxV2 {
        omissions.push("Producer identity scheme syntax-v2: lexical declaration/import anchors exclude source positions. Switching schemes requires a new model identity; no saved-layout ID migration is supplied. Renames, file moves, self/trait syntax and conditional-context changes can change IDs; ambiguous identical anchors are rejected.".into());
    }
    let manifest = Manifest {
        format: FORMAT_VERSION.to_owned(),
        model_id,
        revision: model_revision,
        root_id,
        profiles: vec![Profile::Architecture, Profile::Code],
        coverage: Coverage::Partial,
        scope_description: format!(
            "{}: Rust source inventory at {}",
            options.name, options.revision
        ),
        omissions,
    };
    report.entities = records.entities.len();
    report.relations = records.relations.len();
    report.evidence = records.evidence.len();
    let mut writer = BundleWriter::create(&options.output, manifest, Limits::default())?;
    for record in records.evidence.into_values() {
        writer.push(&BundleRecord::Evidence(record))?;
    }
    for record in records.entities.into_values() {
        writer.push(&BundleRecord::Entity(record))?;
    }
    for record in records.relations.into_values() {
        writer.push(&BundleRecord::Relation(record))?;
    }
    let validated = writer.finish()?;
    if validated.stats().counts.entities != report.entities
        || validated.stats().counts.relations != report.relations
    {
        return Err("validated counts differ".into());
    }
    report.bundle_bytes = fs::metadata(&options.output)?.len();
    Ok(report)
}

fn validate_options(options: &Options) -> Result<()> {
    match (&options.model_id, options.model_revision) {
        (None, None) => {}
        (Some(id), Some(revision)) => {
            if id.is_empty()
                || id.len() > 128
                || !id.as_bytes()[0].is_ascii_lowercase()
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._:-".contains(&c))
            {
                return Err("--model-id requires 1–128 lowercase ASCII ID characters, starting with a letter; permitted punctuation: . _ : -".into());
            }
            if revision == 0 {
                return Err("--model-revision must be a positive integer".into());
            }
        }
        _ => return Err("--model-id and --model-revision must be supplied together".into()),
    }
    if options.revision.len() != 40 || !options.revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("--revision must be a full forty-character Git commit SHA".into());
    }
    if !options.repository.starts_with("https://")
        || options.repository.len() > 2048
        || options
            .repository
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        || options.repository.contains(['?', '#'])
    {
        return Err(
            "--repository must be an HTTPS source-browser URL without query or fragment".into(),
        );
    }
    if options.name.trim().is_empty() || options.name.len() > 512 || options.name.contains('\0') {
        return Err("--name must be nonempty and at most 512 bytes".into());
    }
    if options.output.exists() {
        return Err("output already exists; choose a new path".into());
    }
    Ok(())
}

fn discover(
    root: &Path,
    relative: &Path,
    result: &mut Vec<PathBuf>,
    skipped_symlinks: &mut Vec<String>,
    depth: usize,
) -> Result<()> {
    if depth > 64 {
        return Err("source directory nesting exceeds 64 levels".into());
    }
    let mut children = fs::read_dir(root.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let name = child.file_name();
        if SKIP_DIRS.iter().any(|skip| name == *skip) {
            continue;
        }
        let file_type = child.file_type()?;
        let path = relative.join(name);
        if file_type.is_symlink() {
            if path.file_name().is_some_and(|n| n == "Cargo.toml")
                || path.extension().is_some_and(|x| x == "rs")
            {
                return Err(
                    format!("symlink forbidden in supplied source: {}", slash(&path)).into(),
                );
            }
            skipped_symlinks.push(slash(&path));
            continue;
        }
        if file_type.is_dir() {
            discover(root, &path, result, skipped_symlinks, depth + 1)?;
        } else if file_type.is_file()
            && (path.file_name().is_some_and(|n| n == "Cargo.toml")
                || path.extension().is_some_and(|x| x == "rs"))
        {
            result.push(path);
            if result.len() > MAX_FILES {
                return Err("source file budget exceeded".into());
            }
        }
    }
    Ok(())
}

fn read_source(root: &Path, relative: &Path, total: &mut u64) -> Result<String> {
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || !path.canonicalize()?.starts_with(root) {
        return Err(format!("unsafe source path {}", slash(relative)).into());
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!("source file exceeds 8 MiB: {}", slash(relative)).into());
    }
    let file = File::open(&path)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    *total += bytes.len() as u64;
    if bytes.len() as u64 > MAX_FILE_BYTES || *total > MAX_SOURCE_BYTES {
        return Err("source byte budget exceeded".into());
    }
    let after = fs::symlink_metadata(&path)?;
    if !after.is_file()
        || metadata.len() != after.len()
        || metadata.modified()? != after.modified()?
    {
        return Err("source changed during ingestion; use an immutable checkout".into());
    }
    Ok(String::from_utf8(bytes)?)
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn url_path(path: &str) -> String {
    let mut out = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn module_path(path: &Path) -> Option<Vec<String>> {
    let path = path.strip_prefix("src").ok()?;
    let mut parts = path
        .components()
        .map(|c| c.as_os_str().to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let tail = parts.pop()?;
    if !["lib.rs", "main.rs", "mod.rs"].contains(&tail.as_str()) {
        parts.push(tail.strip_suffix(".rs")?.to_owned());
    }
    Some(parts)
}

fn module_index(sources: &[Source]) -> BTreeMap<(usize, Vec<String>), Option<String>> {
    let mut out = BTreeMap::new();
    for source in sources {
        if let Some(module) = &source.module {
            out.entry((source.package, module.clone()))
                .and_modify(|id| *id = None)
                .or_insert(Some(source.id.clone()));
        }
    }
    out
}

fn dependency_tables(document: &DocumentMut) -> Vec<(String, &TomlItem)> {
    let mut tables = Vec::new();
    for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(value) = document.get(kind) {
            tables.push((kind.to_owned(), value));
        }
    }
    if let Some(targets) = document.get("target").and_then(TomlItem::as_table_like) {
        for (target, values) in targets.iter() {
            for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(value) = values.get(kind) {
                    tables.push((format!("target.{target}.{kind}"), value));
                }
            }
        }
    }
    tables
}

fn lexical_path(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::CurDir => (),
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

fn manifest_dependencies(
    packages: &mut [Package],
    workspace: &DocumentMut,
    records: &mut Records,
    report: &mut Report,
) -> Result<()> {
    let directories: BTreeMap<_, _> = packages
        .iter()
        .map(|p| (p.directory.clone(), (p.id.clone(), p.name.clone())))
        .collect();
    for package in packages {
        let mut aliases: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (section, table) in dependency_tables(&package.document) {
            let Some(table) = table.as_table_like() else {
                continue;
            };
            for (alias, raw) in table.iter() {
                let inherited = raw.get("workspace").and_then(TomlItem::as_bool) == Some(true);
                let definition = if inherited {
                    workspace
                        .get("workspace")
                        .and_then(|w| w.get("dependencies"))
                        .and_then(|d| d.get(alias))
                        .unwrap_or(raw)
                } else {
                    raw
                };
                let path = definition.get("path").and_then(TomlItem::as_str);
                let target = path
                    .and_then(|path| {
                        lexical_path(&if inherited {
                            PathBuf::from(path)
                        } else {
                            package.directory.join(path)
                        })
                    })
                    .and_then(|path| directories.get(&path));
                let declared_package = definition
                    .get("package")
                    .and_then(TomlItem::as_str)
                    .unwrap_or(alias);
                let Some((target_id, name)) = target.filter(|(_, name)| name == declared_package)
                else {
                    report.omitted_external_dependencies += 1;
                    continue;
                };
                if target_id == &package.id {
                    report.omitted_self_dependencies += 1;
                    continue;
                }
                let alias = alias.replace('-', "_");
                aliases
                    .entry(alias)
                    .and_modify(|existing| {
                        if existing.as_ref() != Some(target_id) {
                            *existing = None;
                        }
                    })
                    .or_insert(Some(target_id.clone()));
                let evidence =
                    records.evidence(&package.manifest, 1, "Cargo dependency declaration")?;
                let label = format!(
                    "Declared {section}: {name}{}",
                    if raw.get("optional").and_then(TomlItem::as_bool) == Some(true) {
                        " (optional)"
                    } else {
                        ""
                    }
                );
                records.relation(
                    RelationKind::CodeReferences,
                    &package.id,
                    target_id,
                    evidence,
                    &label,
                    &section,
                )?;
                report.manifest_references += 1;
            }
        }
        package.aliases = aliases
            .into_iter()
            .filter_map(|(alias, id)| id.map(|id| (alias, id)))
            .collect();
    }
    Ok(())
}

struct FileInventory<'a> {
    source: &'a Source,
    packages: &'a [Package],
    module_files: &'a BTreeMap<(usize, Vec<String>), Option<String>>,
    records: &'a mut Records,
    report: &'a mut Report,
    identity_scheme: IdentityScheme,
    anchors: BTreeMap<String, usize>,
}

impl FileInventory<'_> {
    fn unique_anchor(&mut self, anchor: &str, line: usize) -> Result<()> {
        if let Some(previous) = self.anchors.insert(anchor.to_owned(), line) {
            return Err(format!(
                "ambiguous syntax-v2 anchor in {} at lines {previous} and {line}; cannot identify these declarations/imports uniquely without source positions; no bundle was published",
                slash(&self.source.path)
            ).into());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn declaration(
        &mut self,
        name: &str,
        subtype: &str,
        kind: EntityKind,
        span: Span,
        parent: &str,
        context: &[String],
        attributes: &[syn::Attribute],
    ) -> Result<String> {
        let line = span.start().line.max(1);
        let key = match self.identity_scheme {
            IdentityScheme::LegacyV1 => format!(
                "{}:{line}:{}:{subtype}:{name}",
                slash(&self.source.path),
                span.start().column
            ),
            IdentityScheme::SyntaxV2 => {
                let key = serde_json::to_string(&(
                    "syntax-v2",
                    slash(&self.source.path),
                    subtype,
                    name,
                    conditional_context(context, attributes),
                ))?;
                self.unique_anchor(&key, line)?;
                key
            }
        };
        let id = self.records.id("item", &key)?;
        let evidence = self
            .records
            .evidence(&self.source.path, line, "Rust syntax declaration")?;
        self.records.entity(id.clone(), kind, bounded(name, 512), format!("Rust {subtype} {name}, declared at {}:{line}. Lexical source declaration; no runtime behavior, feature activation or generated expansion is claimed.", slash(&self.source.path)), evidence.clone())?;
        self.records.owns(parent, &id, evidence)?;
        self.report.declarations += 1;
        Ok(id)
    }

    fn items(
        &mut self,
        items: &[Item],
        qualifiers: &[String],
        inline_module: &[String],
        context: &[String],
    ) -> Result<()> {
        let mut types: BTreeMap<String, Option<String>> = BTreeMap::new();
        let parent = self.source.id.clone();
        for item in items {
            let declared = match item {
                Item::Struct(x) => Some((&x.ident, "struct", &x.attrs)),
                Item::Enum(x) => Some((&x.ident, "enum", &x.attrs)),
                Item::Trait(x) => Some((&x.ident, "trait", &x.attrs)),
                Item::Union(x) => Some((&x.ident, "union", &x.attrs)),
                Item::Type(x) => Some((&x.ident, "type alias", &x.attrs)),
                _ => None,
            };
            if let Some((ident, subtype, attributes)) = declared {
                let name = qualified(qualifiers, &ident.to_string());
                let id = self.declaration(
                    &name,
                    subtype,
                    EntityKind::CodeType,
                    ident.span(),
                    &parent,
                    context,
                    attributes,
                )?;
                types
                    .entry(ident.to_string())
                    .and_modify(|existing| *existing = None)
                    .or_insert(Some(id));
            }
        }
        for item in items {
            match item {
                Item::Fn(function) => {
                    self.declaration(
                        &qualified(qualifiers, &function.sig.ident.to_string()),
                        "function",
                        EntityKind::CodeFunction,
                        function.sig.ident.span(),
                        &parent,
                        context,
                        &function.attrs,
                    )?;
                }
                Item::Mod(module) => {
                    if let Some((_, items)) = &module.content {
                        let mut names = qualifiers.to_vec();
                        names.push(module.ident.to_string());
                        let mut module_path = inline_module.to_vec();
                        module_path.push(module.ident.to_string());
                        self.items(
                            items,
                            &names,
                            &module_path,
                            &conditional_context(context, &module.attrs),
                        )?;
                    }
                }
                Item::Impl(implementation) => {
                    let mut impl_context = conditional_context(context, &implementation.attrs);
                    impl_context.push(serde_json::to_string(&(
                        "impl",
                        canonical_tokens(&implementation.self_ty),
                        implementation.trait_.as_ref().map(|(negative, path, _)| {
                            (negative.is_some(), canonical_tokens(path))
                        }),
                    ))?);
                    let name = match implementation.self_ty.as_ref() {
                        syn::Type::Path(path) => path
                            .path
                            .segments
                            .iter()
                            .map(|p| p.ident.to_string())
                            .collect::<Vec<_>>()
                            .join("::"),
                        _ => "<impl>".to_owned(),
                    };
                    let type_parent = if name.contains("::") {
                        None
                    } else {
                        types.get(&name).and_then(Option::as_ref)
                    };
                    let owner = type_parent.cloned().unwrap_or_else(|| parent.clone());
                    for method in &implementation.items {
                        if let syn::ImplItem::Fn(method) = method {
                            self.declaration(
                                &qualified(qualifiers, &format!("{name}::{}", method.sig.ident)),
                                "method",
                                EntityKind::CodeFunction,
                                method.sig.ident.span(),
                                &owner,
                                &impl_context,
                                &method.attrs,
                            )?;
                        }
                    }
                }
                Item::Trait(trait_item) => {
                    let trait_context = conditional_context(context, &trait_item.attrs);
                    let owner = types
                        .get(&trait_item.ident.to_string())
                        .and_then(Option::as_ref)
                        .cloned()
                        .unwrap_or_else(|| parent.clone());
                    for method in &trait_item.items {
                        if let syn::TraitItem::Fn(method) = method {
                            self.declaration(
                                &qualified(
                                    qualifiers,
                                    &format!("{}::{}", trait_item.ident, method.sig.ident),
                                ),
                                "trait method declaration",
                                EntityKind::CodeFunction,
                                method.sig.ident.span(),
                                &owner,
                                &trait_context,
                                &method.attrs,
                            )?;
                        }
                    }
                }
                Item::Use(import) => {
                    let line = import.span().start().line.max(1);
                    if self.identity_scheme == IdentityScheme::LegacyV1 {
                        let mut paths = BTreeSet::new();
                        use_paths(&import.tree, &[], &mut paths);
                        for path in paths {
                            self.import(&path, line, inline_module, None)?;
                        }
                    } else {
                        let mut entries = Vec::new();
                        use_entries(&import.tree, &[], &mut entries);
                        for (path, alias) in entries {
                            let anchor = serde_json::to_string(&(
                                "syntax-v2-import",
                                inline_module,
                                conditional_context(context, &import.attrs),
                                import.leading_colon.is_some(),
                                &path,
                                &alias,
                            ))?;
                            self.unique_anchor(&anchor, line)?;
                            self.import(&path, line, inline_module, Some(&anchor))?;
                        }
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }

    fn import(
        &mut self,
        path: &[String],
        line: usize,
        inline_module: &[String],
        anchor: Option<&str>,
    ) -> Result<()> {
        let package = &self.packages[self.source.package];
        let Some(first) = path.first() else {
            return Ok(());
        };
        let (target_package, mut remaining) = if first == "crate" {
            (self.source.package, path[1..].to_vec())
        } else if first == "self" || first == "super" {
            let Some(mut current) = self.source.module.clone() else {
                self.report.unresolved_imports += 1;
                return Ok(());
            };
            current.extend_from_slice(inline_module);
            let mut position = 0;
            while path.get(position).is_some_and(|part| part == "super") {
                if current.pop().is_none() {
                    self.report.unresolved_imports += 1;
                    return Ok(());
                }
                position += 1;
            }
            if path.get(position).is_some_and(|part| part == "self") {
                position += 1;
            }
            current.extend_from_slice(&path[position..]);
            (self.source.package, current)
        } else {
            let target = package
                .aliases
                .get(first)
                .and_then(|id| self.packages.iter().position(|p| &p.id == id));
            let Some(target) = target else {
                self.report.unresolved_imports += 1;
                return Ok(());
            };
            (target, path[1..].to_vec())
        };
        if remaining
            .last()
            .is_some_and(|last| last == "*" || last == "self")
        {
            remaining.pop();
        }
        let target = loop {
            if let Some(Some(id)) = self.module_files.get(&(target_package, remaining.clone())) {
                break id.clone();
            }
            if remaining.pop().is_none() {
                break self.packages[target_package].id.clone();
            }
        };
        if target != self.source.id {
            let evidence =
                self.records
                    .evidence(&self.source.path, line, "Syntactic Rust use declaration")?;
            let joined = path.join("::");
            let relation = self.records.relation(
                RelationKind::CodeReferences,
                &self.source.id,
                &target,
                evidence,
                &bounded(&format!("Syntactic import: {joined}"), 512),
                anchor.unwrap_or(&format!("{line}:{joined}")),
            )?;
            let claim = &mut self
                .records
                .relations
                .get_mut(&relation)
                .expect("just inserted")
                .claim;
            claim.basis = Basis::Inferred;
            claim.rationale = Some("Source use declaration mapped to a supplied manifest dependency and conventional source module path. No compiler resolution, reexport expansion, cfg evaluation or runtime call is claimed.".to_owned());
            self.report.import_references += 1;
        }
        Ok(())
    }
}

fn qualified(prefix: &[String], name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{}::{name}", prefix.join("::"))
    }
}

/// Canonical token structure ignores source spacing, including punctuation
/// jointness (e.g. `Vec<T>` versus `Vec < T >`). Literal spelling is retained:
/// this is lexical identity, not compiler/type equivalence.
fn canonical_tokens(value: &impl ToTokens) -> String {
    fn encode(tokens: proc_macro2::TokenStream, out: &mut String) {
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Group(group) => {
                    out.push_str(&format!("g{:?}[", group.delimiter()));
                    encode(group.stream(), out);
                    out.push(']');
                }
                proc_macro2::TokenTree::Ident(ident) => {
                    out.push_str(&format!("i{}:{ident};", ident.to_string().len()))
                }
                proc_macro2::TokenTree::Punct(punct) => {
                    out.push_str(&format!("p{};", punct.as_char()))
                }
                proc_macro2::TokenTree::Literal(literal) => {
                    out.push_str(&format!("l{}:{literal};", literal.to_string().len()))
                }
            }
        }
    }
    let mut result = String::new();
    encode(value.to_token_stream(), &mut result);
    result
}

fn conditional_context(parent: &[String], attributes: &[syn::Attribute]) -> Vec<String> {
    let mut result = parent.to_vec();
    let mut conditions: Vec<_> = attributes
        .iter()
        .filter(|attribute| {
            attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr")
        })
        .map(|attribute| canonical_tokens(&attribute.meta))
        .collect();
    conditions.sort();
    // Keep lexical nesting distinct without evaluating cfg expressions.
    if !conditions.is_empty() {
        result.push(serde_json::to_string(&conditions).expect("string list serializes"));
    }
    result
}

fn bounded(value: &str, bytes: usize) -> String {
    if value.len() <= bytes {
        return value.to_owned();
    }
    let mut end = bytes - 3;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

fn use_paths(tree: &UseTree, prefix: &[String], out: &mut BTreeSet<Vec<String>>) {
    match tree {
        UseTree::Path(path) => {
            let mut prefix = prefix.to_vec();
            prefix.push(path.ident.to_string());
            use_paths(&path.tree, &prefix, out);
        }
        UseTree::Name(name) => {
            let mut path = prefix.to_vec();
            path.push(name.ident.to_string());
            out.insert(path);
        }
        UseTree::Rename(rename) => {
            let mut path = prefix.to_vec();
            path.push(rename.ident.to_string());
            out.insert(path);
        }
        UseTree::Glob(_) => {
            let mut path = prefix.to_vec();
            path.push("*".to_owned());
            out.insert(path);
        }
        UseTree::Group(group) => {
            for item in &group.items {
                use_paths(item, prefix, out);
            }
        }
    }
}

fn use_entries(tree: &UseTree, prefix: &[String], out: &mut Vec<(Vec<String>, Option<String>)>) {
    match tree {
        UseTree::Path(path) => {
            let mut prefix = prefix.to_vec();
            prefix.push(path.ident.to_string());
            use_entries(&path.tree, &prefix, out);
        }
        UseTree::Name(name) => {
            let mut path = prefix.to_vec();
            path.push(name.ident.to_string());
            out.push((path, None));
        }
        UseTree::Rename(rename) => {
            let mut path = prefix.to_vec();
            path.push(rename.ident.to_string());
            out.push((path, Some(rename.rename.to_string())));
        }
        UseTree::Glob(_) => {
            let mut path = prefix.to_vec();
            path.push("*".into());
            out.push((path, None));
        }
        UseTree::Group(group) => {
            for item in &group.items {
                use_entries(item, prefix, out);
            }
        }
    }
}
