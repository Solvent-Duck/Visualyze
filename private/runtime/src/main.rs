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
            return Ok(bevy::app::AppExit::Success);
        }
        return Ok(visualyze::app_with_bundle(path, options.layout).run());
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
