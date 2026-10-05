use std::path::PathBuf;
use visualyze_rust_bundle::{Options, produce};

fn main() {
    if let Err(error) = run() {
        eprintln!("rust-bundle: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut values = std::collections::BTreeMap::new();
    while let Some(key) = args.next() {
        if key == "--help" {
            println!(
                "visualyze-rust-bundle --root SOURCE --output NEW_BUNDLE --revision FULL_GIT_SHA --repository HTTPS_SOURCE_URL [--name LABEL] [--model-id STABLE_ID --model-revision POSITIVE_INTEGER] [--identity-scheme legacy-v1|syntax-v2]\nReads Rust source and manifests without running Cargo, build scripts, or source code. Output must not already exist. Source revision provenance is supplied by the caller; use an immutable, independently verified checkout. Optional model identity and numeric revision must be supplied together; callers manage increasing publication revisions. Identity defaults to legacy-v1. Opt-in syntax-v2 preserves lexical declaration/import anchors through ordinary source edits; switching schemes requires a new model identity. No saved-ID migration is provided."
            );
            return Ok(());
        }
        if ![
            "--root",
            "--output",
            "--revision",
            "--repository",
            "--name",
            "--model-id",
            "--model-revision",
            "--identity-scheme",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("unknown argument {key}").into());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {key}"))?;
        if values.insert(key.clone(), value).is_some() {
            return Err(format!("repeated argument {key}").into());
        }
    }
    let mut required = |key: &str| values.remove(key).ok_or_else(|| format!("required: {key}"));
    let options = Options {
        root: PathBuf::from(required("--root")?),
        output: PathBuf::from(required("--output")?),
        revision: required("--revision")?,
        repository: required("--repository")?,
        name: values
            .remove("--name")
            .unwrap_or_else(|| "Rust source".to_owned()),
        model_id: values.remove("--model-id"),
        model_revision: values
            .remove("--model-revision")
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|_| "--model-revision must be a positive integer fitting u64")?,
        identity_scheme: values
            .remove("--identity-scheme")
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or_default(),
    };
    println!("{}", serde_json::to_string_pretty(&produce(&options)?)?);
    Ok(())
}
