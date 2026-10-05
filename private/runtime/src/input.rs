//! Explicit model input only: embedded specimen, a named JSON file, or stdin.
use std::{ffi::OsString, fs::File, io, path::PathBuf};

use crate::{LayoutEngine, ViewMode};
use visualyze_model::architecture::{Architecture, OBJECTS_FIXTURE};

pub const HELP: &str = "Visualyze — architecture simulator\n\nUsage: visualyze [--model <path|->] [--view auto|gallery|architecture] [--layout legacy|floorplan|composed|layered|constrained|treemap] [--validate]\n\n  --model <path>  Read an architecture JSON snapshot\n  --model -       Read a snapshot from standard input\n  --view <mode>   Automatic (default), standalone gallery, or connected architecture\n  --bundle <path> Read a versioned local semantic bundle instead of --model\n  --layout <mode> Experimental placement; legacy is the default\n  --validate      Validate and report without opening a window\n  --help          Show this help\n\nWithout --model, load the embedded seven-object specimen.\nAuto assembles models that contain non-structural relationships.";

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    Embedded,
    File(PathBuf),
    Stdin,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub source: Source,
    pub validate_only: bool,
    pub view: ViewMode,
    pub layout: LayoutEngine,
    pub bundle: Option<PathBuf>,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Option<Options>, String> {
    let mut args = args.into_iter();
    let mut source = Source::Embedded;
    let mut has_source = false;
    let mut validate_only = false;
    let mut view = None;
    let mut layout = None;
    let mut bundle = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--bundle") if bundle.is_none() && !has_source => {
                let path = args.next().ok_or("--bundle requires a local bundle file")?;
                if path.is_empty() || path.to_string_lossy().starts_with("--") || path == "-" {
                    return Err("--bundle requires a local bundle file".into());
                }
                bundle = Some(PathBuf::from(path));
            }
            Some("--layout") if layout.is_none() => {
                layout =
                    Some(match args.next().as_deref().and_then(|s| s.to_str()) {
                        Some("legacy") => LayoutEngine::Legacy,
                        Some("floorplan") => LayoutEngine::Floorplan,
                        Some("composed") => LayoutEngine::Composed,
                        Some("layered") => LayoutEngine::Layered,
                        Some("constrained") => LayoutEngine::Constrained,
                        Some("treemap") => LayoutEngine::Treemap,
                        _ => return Err(
                            "--layout requires legacy, floorplan, composed, layered, constrained, or treemap"
                                .into(),
                        ),
                    });
            }
            Some("--help" | "-h") => return Ok(None),
            Some("--validate") if !validate_only => validate_only = true,
            Some("--view") if view.is_none() => {
                view = Some(match args.next().as_deref().and_then(|s| s.to_str()) {
                    Some("auto") => ViewMode::Auto,
                    Some("gallery") => ViewMode::Gallery,
                    Some("architecture") => ViewMode::Architecture,
                    _ => return Err("--view requires auto, gallery, or architecture".into()),
                });
            }
            Some("--model") if !has_source && bundle.is_none() => {
                let path = args
                    .next()
                    .ok_or("--model requires a JSON file path or -")?;
                if path.is_empty() || path.to_string_lossy().starts_with("--") {
                    return Err("--model requires a JSON file path or -".into());
                }
                source = if path == "-" {
                    Source::Stdin
                } else {
                    Source::File(path.into())
                };
                has_source = true;
            }
            _ => {
                return Err(format!(
                    "unknown or repeated argument: {}",
                    arg.to_string_lossy()
                ));
            }
        }
    }
    Ok(Some(Options {
        source,
        validate_only,
        view: view.unwrap_or_default(),
        layout: layout.unwrap_or_default(),
        bundle,
    }))
}

pub fn load(source: &Source) -> Result<Architecture, Box<dyn std::error::Error>> {
    Ok(match source {
        Source::Embedded => Architecture::from_json(OBJECTS_FIXTURE.as_bytes())?,
        Source::Stdin => Architecture::from_reader(io::stdin().lock())?,
        Source::File(path) => {
            if !std::fs::metadata(path)?.is_file() {
                return Err(
                    "--model must name a regular JSON file; use - for standard input".into(),
                );
            }
            let file = File::open(path)?;
            if !file.metadata()?.is_file() {
                return Err(
                    "--model must name a regular JSON file; use - for standard input".into(),
                );
            }
            Architecture::from_reader(file)?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<Option<Options>, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn selects_explicit_inputs_and_rejects_ambiguous_arguments() {
        assert_eq!(options(&[]).unwrap().unwrap().source, Source::Embedded);
        assert_eq!(
            options(&["--model", "-"]).unwrap().unwrap().source,
            Source::Stdin
        );
        assert!(
            options(&["--validate", "--model", "model.json"])
                .unwrap()
                .unwrap()
                .validate_only
        );
        assert!(options(&["--help"]).unwrap().is_none());
        assert_eq!(
            options(&["--view", "architecture"]).unwrap().unwrap().view,
            ViewMode::Architecture
        );
        assert_eq!(
            options(&["--view", "gallery"]).unwrap().unwrap().view,
            ViewMode::Gallery
        );
        assert_eq!(
            options(&["--layout", "floorplan"]).unwrap().unwrap().layout,
            LayoutEngine::Floorplan
        );
        assert!(
            options(&["--bundle", "example.jsonl"])
                .unwrap()
                .unwrap()
                .bundle
                .is_some()
        );
        for args in [
            vec!["--bundle"],
            vec!["--bundle", "-"],
            vec!["--bundle", "a", "--model", "b"],
            vec!["--model", "a", "--bundle", "b"],
            vec!["--layout", "invalid"],
            vec!["--layout", "floorplan", "--layout", "layered"],
            vec!["--view"],
            vec!["--view", "invalid"],
            vec!["--view", "auto", "--view", "gallery"],
            vec!["--model"],
            vec!["--model", "--validate"],
            vec!["model.json"],
            vec!["--validate", "--validate"],
            vec!["--model", "a", "--model", "b"],
        ] {
            assert!(options(&args).is_err(), "accepted {args:?}");
        }
    }
}
