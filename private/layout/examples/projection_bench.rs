//! Profile repeated bounded projections independently of rendering and solving.
//! Usage: projection_bench BUNDLE FOCUS ITERATIONS NEW_REPORT
use std::{fs::OpenOptions, sync::atomic::AtomicBool, time::Instant};
use visualyze_layout::{
    projection::LayoutPolicy,
    provider_projection::{ProviderViewRequest, project_provider},
};
use visualyze_model::provider::{Limits, LocalBundle, ModelProvider, SemanticLens};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let bundle = args.next().ok_or("expected bundle")?;
    let focus = args.next().ok_or("expected focus")?;
    let iterations: usize = args.next().ok_or("expected iterations")?.parse()?;
    let report = args.next().ok_or("expected new report path")?;
    if args.next().is_some() || !(1..=1000).contains(&iterations) {
        return Err("usage: projection_bench BUNDLE FOCUS ITERATIONS(1..1000) NEW_REPORT".into());
    }
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report)?;
    let started = Instant::now();
    let mut provider = LocalBundle::open(bundle, Limits::default())?;
    let indexing_ms = started.elapsed().as_secs_f64() * 1000.0;
    let cancel = AtomicBool::new(false);
    let mut request = ProviderViewRequest {
        focus: Some(focus),
        lens: Some(SemanticLens::Code),
        policy: LayoutPolicy::Code,
        ..Default::default()
    };
    let mut cases = Vec::new();
    // Keep requests fixed within each case. Retention can legitimately change
    // the relationship sample; equality is required within, not across, cases.
    for name in ["fresh_groups", "retained_groups"] {
        let mut times = Vec::new();
        let mut reference = None;
        let mut groups = vec![];
        for _ in 0..iterations {
            let started = Instant::now();
            let result = project_provider(&mut provider, &request, &cancel)?;
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            // Outside the measured interval: ensure every semantic field,
            // including original memberships/evidence, remains unchanged.
            let identity = format!("{result:?}");
            if let Some(reference) = &reference {
                assert_eq!(reference, &identity);
            } else {
                reference = Some(identity);
                groups = result.graph.suggested_groups.clone();
            }
        }
        cases.push(serde_json::json!({"case":name,"projection_ms":times}));
        request.retained_groups = groups;
    }
    serde_json::to_writer_pretty(
        output,
        &serde_json::json!({
            "scope":"Headless Code-lens provider projection only; no desktop projection memo, layout, rendering, saving or GPU measurement. Indexing is separate. Bundle OS cache may be warm. Repeated results are compared in full outside the measured interval.",
            "model_id":provider.manifest().model_id,"revision":provider.manifest().revision,
            "source_counts":provider.stats().counts,"indexing_ms":indexing_ms,
            "focus":request.focus,"iterations":iterations,"cases":cases
        }),
    )?;
    Ok(())
}
