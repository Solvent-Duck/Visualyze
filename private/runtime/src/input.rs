//! Explicit model input only: embedded specimen, a named JSON file, or stdin.
use std::{ffi::OsString, fs::File, io, path::PathBuf};

use crate::{LayoutEngine, ViewMode};
use visualyze_model::architecture::{Architecture, OBJECTS_FIXTURE};

pub const HELP: &str = "Visualyze — architecture simulator\n\nUsage: visualyze [--model <path|->] [--view auto|gallery|architecture] [--layout legacy|floorplan|composed|layered|constrained|treemap] [--validate [--json] [--walkthrough <path>]] [--serve]\n       visualyze mcp\n\n  --model <path>  Read an architecture JSON snapshot\n  --model -       Read a snapshot from standard input\n  --view <mode>   Automatic (default), standalone gallery, or connected architecture\n  --bundle <path> Read a versioned local semantic bundle instead of --model\n  --layout <mode> Experimental placement; legacy is the default\n  --validate      Validate and report without opening a window\n  --json          With --validate, report diagnostics and advice as JSON on stdout\n  --walkthrough <path>  With --validate, also check a walkthrough against the model\n  --serve         Accept models and walkthroughs from local agents (see visualyze mcp)\n  mcp             Run the MCP bridge for a coding agent over standard input/output\n  --help          Show this help\n\nWithout --model, load the embedded seven-object specimen.\nAuto assembles models that contain non-structural relationships.";

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
    pub json: bool,
    pub walkthrough: Option<PathBuf>,
    pub serve: bool,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Option<Options>, String> {
    let mut args = args.into_iter();
    let mut source = Source::Embedded;
    let mut has_source = false;
    let mut validate_only = false;
    let mut view = None;
    let mut layout = None;
    let mut bundle = None;
    let mut json = false;
    let mut walkthrough = None;
    let mut serve = false;
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
            Some("--json") if !json => json = true,
            Some("--serve") if !serve => serve = true,
            Some("--walkthrough") if walkthrough.is_none() => {
                let path = args
                    .next()
                    .ok_or("--walkthrough requires a JSON file path")?;
                if path.is_empty() || path.to_string_lossy().starts_with("--") || path == "-" {
                    return Err("--walkthrough requires a JSON file path".into());
                }
                walkthrough = Some(PathBuf::from(path));
            }
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
    if (json || walkthrough.is_some()) && !validate_only {
        return Err("--json and --walkthrough require --validate".into());
    }
    if walkthrough.is_some() && bundle.is_some() {
        return Err("--walkthrough checks against a --model snapshot".into());
    }
    if serve && (validate_only || bundle.is_some()) {
        return Err("--serve cannot be combined with --validate or --bundle".into());
    }
    let view = view.unwrap_or_default();
    // A supplied model gets the nested engine unless the specimen gallery was asked for;
    // the embedded specimen and bundles keep their own defaults.
    let default_layout = match (&source, view) {
        (Source::File(_) | Source::Stdin, view) if view != ViewMode::Gallery => {
            LayoutEngine::Composed
        }
        _ => LayoutEngine::default(),
    };
    Ok(Some(Options {
        source,
        validate_only,
        view,
        layout: layout.unwrap_or(default_layout),
        bundle,
        json,
        walkthrough,
        serve,
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
        let checked = options(&[
            "--validate",
            "--json",
            "--model",
            "m.json",
            "--walkthrough",
            "w.json",
        ])
        .unwrap()
        .unwrap();
        assert!(checked.json && checked.walkthrough.is_some());
        assert!(options(&["--serve"]).unwrap().unwrap().serve);
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
        assert_eq!(
            options(&["--model", "m.json"]).unwrap().unwrap().layout,
            LayoutEngine::Composed
        );
        assert_eq!(
            options(&["--model", "-"]).unwrap().unwrap().layout,
            LayoutEngine::Composed
        );
        assert_eq!(
            options(&["--model", "m.json", "--layout", "legacy"])
                .unwrap()
                .unwrap()
                .layout,
            LayoutEngine::Legacy
        );
        assert_eq!(
            options(&["--model", "m.json", "--view", "gallery"])
                .unwrap()
                .unwrap()
                .layout,
            LayoutEngine::Legacy
        );
        assert_eq!(options(&[]).unwrap().unwrap().layout, LayoutEngine::Legacy);
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
            vec!["--json"],
            vec!["--walkthrough", "w.json"],
            vec!["--validate", "--walkthrough"],
            vec!["--validate", "--bundle", "b", "--walkthrough", "w.json"],
            vec!["--serve", "--validate"],
            vec!["--serve", "--bundle", "b"],
            vec!["--serve", "--serve"],
        ] {
            assert!(options(&args).is_err(), "accepted {args:?}");
        }
    }
}
