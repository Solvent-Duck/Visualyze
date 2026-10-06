fn main() -> bevy::app::AppExit {
    match run() {
        Ok(exit) => exit,
        Err(error) => {
            eprintln!("Unable to load architecture:\n{error}");
            bevy::app::AppExit::error()
        }
    }
}

fn run() -> Result<bevy::app::AppExit, Box<dyn std::error::Error>> {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "mcp") {
        return Ok(match visualyze_mcp::run(std::env::args_os().skip(2)) {
            Ok(()) => bevy::app::AppExit::Success,
            Err(error) => {
                eprintln!("visualyze mcp: {error}");
                bevy::app::AppExit::error()
            }
        });
    }
    let Some(options) = visualyze::input::parse(std::env::args_os().skip(1))? else {
        println!("{}", visualyze::input::HELP);
        return Ok(bevy::app::AppExit::Success);
    };
    if let Some(path) = options.bundle {
        if options.validate_only {
            use visualyze_model::provider::ModelProvider;
            let bundle = visualyze_model::provider::LocalBundle::open(
                path,
                visualyze_model::provider::Limits::default(),
            )?;
            println!(
                "Validated bundle {} revision {}: {:?}",
                bundle.manifest().model_id,
                bundle.manifest().revision,
                bundle.stats().counts
            );
            report_bundle_advice(bundle);
            return Ok(bevy::app::AppExit::Success);
        }
        return Ok(visualyze::app_with_bundle(path, options.layout).run());
    }
    if options.json {
        return Ok(validate_json(&options));
    }
    if options.serve {
        let initial = match &options.source {
            visualyze::input::Source::File(path) => {
                visualyze::input::load(&options.source)?;
                Some((std::fs::read(path)?, Some(path.clone())))
            }
            visualyze::input::Source::Stdin => {
                return Err(
                    "--serve reads models from agents; pass --model <path> or nothing".into(),
                );
            }
            visualyze::input::Source::Embedded => None,
        };
        return Ok(match visualyze::app_serving(initial, options.layout) {
            Ok(mut app) => app.run(),
            Err(error) => {
                eprintln!("Unable to serve agents: {error}");
                bevy::app::AppExit::error()
            }
        });
    }
    let model = visualyze::input::load(&options.source)?;
    for warning in model.warnings() {
        eprintln!("Warning: {warning}");
    }
    let snapshot = model.snapshot();
    println!(
        "Validated {} revision {}: {} objects, {} relations, {} warnings.",
        snapshot.model_id,
        snapshot.revision,
        snapshot.entities.len(),
        snapshot.relations.len(),
        model.warnings().len()
    );
    print_advice(&visualyze_layout::advice::advise_snapshot(&model));
    if let Some(path) = &options.walkthrough {
        let walkthrough =
            visualyze_model::walkthrough::Walkthrough::from_json(&std::fs::read(path)?, &model)?;
        for warning in walkthrough.warnings() {
            eprintln!("Warning: {warning}");
        }
        println!(
            "Validated walkthrough {}: {} steps.",
            walkthrough.script().id,
            walkthrough.script().steps.len()
        );
    }
    if options.validate_only {
        return Ok(bevy::app::AppExit::Success);
    }
    let mut app = match options.source {
        visualyze::input::Source::File(path) => {
            visualyze::app_with_snapshot_file(model, path, options.view, options.layout)
        }
        _ => visualyze::app_with_layout(model, options.view, options.layout),
    };
    Ok(app.run())
}

/// One JSON document on stdout, success or not, so an agent can repair its input.
fn validate_json(options: &visualyze::input::Options) -> bevy::app::AppExit {
    use visualyze::report;
    let report = (|| -> Result<serde_json::Value, serde_json::Value> {
        let model = visualyze::input::load(&options.source).map_err(|error| {
            match error.downcast_ref::<visualyze_model::architecture::ValidationError>() {
                Some(error) => report::error(error),
                None => serde_json::json!({"ok": false, "errors": [{"code": "input_io", "path": "", "message": error.to_string()}]}),
            }
        })?;
        let mut out = report::model(&model);
        if let Some(path) = &options.walkthrough {
            let bytes = std::fs::read(path).map_err(|error| {
                serde_json::json!({"ok": false, "errors": [{"code": "input_io", "path": "", "message": error.to_string()}]})
            })?;
            match visualyze_model::walkthrough::Walkthrough::from_json(&bytes, &model) {
                Ok(walkthrough) => out["walkthrough"] = report::walkthrough(&walkthrough),
                Err(error) => {
                    out["ok"] = false.into();
                    out["walkthrough"] = report::error(&error);
                }
            }
        }
        Ok(out)
    })();
    let (value, ok) = match report {
        Ok(value) => {
            let ok = value["ok"] == true;
            (value, ok)
        }
        Err(value) => (value, false),
    };
    println!("{value}");
    if ok {
        bevy::app::AppExit::Success
    } else {
        bevy::app::AppExit::error()
    }
}

/// Readability advice never rejects a model: print it, or say why it is
/// unavailable, and carry on.
fn report_bundle_advice(mut bundle: visualyze_model::provider::LocalBundle) {
    match visualyze_layout::advice::advise_bundle(&mut bundle) {
        Ok(advice) => print_advice(&advice),
        Err(error) => eprintln!("Advice unavailable: {error}"),
    }
}

/// Advice arrives noisiest first; a producer acts on the worst scopes before
/// the rest, so very large models do not print every advised scope.
const MAX_PRINTED_ADVICE: usize = 25;

fn print_advice(advice: &[visualyze_layout::advice::ScopeAdvice]) {
    for advice in advice.iter().take(MAX_PRINTED_ADVICE) {
        eprintln!("Advice: {}: {}", advice.scope_id, advice.message);
    }
    if advice.len() > MAX_PRINTED_ADVICE {
        eprintln!(
            "Advice: {} more scopes have readability advice; address the scopes above first.",
            advice.len() - MAX_PRINTED_ADVICE
        );
    }
}
