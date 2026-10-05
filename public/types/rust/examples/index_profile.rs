//! Profile an explicitly supplied bundle without rendering or loading detail.
//! Usage: index_profile BUNDLE NEW_REPORT_JSON
use std::{error::Error, fs::OpenOptions, ops::ControlFlow, path::PathBuf, time::Instant};

use visualyze_model::provider::{Limits, LocalBundle, ModelProvider};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(args.next().ok_or("expected bundle path")?);
    let destination = PathBuf::from(args.next().ok_or("expected new report path")?);
    if args.next().is_some() {
        return Err("usage: index_profile BUNDLE NEW_REPORT_JSON".into());
    }
    // Refuse to overwrite measurements or supplied data. Reserve the output
    // before indexing so a successful import cannot silently lose its report.
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut first_complete_bytes = None;
    let mut complete_byte_callbacks = 0;
    let mut callbacks = 0;
    let started = Instant::now();
    let provider = LocalBundle::open_with_progress(&source, Limits::default(), |progress| {
        callbacks += 1;
        if progress.total_bytes > 0 && progress.bytes_read == progress.total_bytes {
            first_complete_bytes.get_or_insert_with(|| started.elapsed());
            complete_byte_callbacks += 1;
        }
        ControlFlow::Continue(())
    })?;
    let total = started.elapsed();
    let read = first_complete_bytes.ok_or("provider did not report its full-byte boundary")?;
    let stats = provider.stats();
    assert_eq!(stats.cached_records, 0, "profiling must not load detail");
    let manifest = provider.manifest();
    let report = serde_json::json!({
        "source": source,
        "process_id": std::process::id(),
        "model_id": manifest.model_id,
        "revision": manifest.revision,
        "source_counts": stats.counts,
        "bundle_bytes": std::fs::metadata(&source)?.len(),
        "total_index_ms": total.as_secs_f64() * 1000.0,
        "read_decode_record_validation_ms": read.as_secs_f64() * 1000.0,
        "after_full_bytes_finalize_ms": (total - read).as_secs_f64() * 1000.0,
        "progress_callbacks": callbacks,
        "full_byte_callbacks": complete_byte_callbacks,
        "estimated_index_bytes": stats.estimated_index_bytes,
        "cached_details": stats.cached_records,
        "notes": "Fresh provider/process. First phase includes opening, record reads, JSON decode, record validation and incremental metadata indexing. Second phase starts at the first full-byte progress callback and includes final ownership/lens/endpoint validation/indexes, file fingerprint check and provider construction. Total is measured before querying manifest/stats or writing this report. OS cache state is not controlled. No scene, source-code dereference, GPU work or entity detail loading. Timings are observations, not work budgets or correctness gates; peak RSS must be joined from an external process measurement."
    });
    serde_json::to_writer_pretty(output, &report)?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
