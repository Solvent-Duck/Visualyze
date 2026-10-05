//! Reproducible comparison. No GPU and no network services required.
use std::{collections::BTreeMap, sync::atomic::AtomicBool, time::Instant};
use visualyze_layout::{
    projection::{ViewState, project},
    *,
};
use visualyze_model::architecture::Architecture;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let paths = if args.is_empty() {
        vec![
            "public/examples/notes-crud.json".into(),
            "public/examples/sketch-crud.json".into(),
        ]
    } else {
        args
    };
    println!(
        "fixture,algorithm,quality_version,objects,routes,length,bends,volume,aspect,occlusion,shared_lane_length,crossing_points,congestion,cost,milliseconds,status"
    );
    for path in paths {
        let model = Architecture::from_reader(std::fs::File::open(&path)?)?;
        let projection = project(&model, &ViewState::default())?;
        for algorithm in [
            Algorithm::Layered,
            Algorithm::Floorplan,
            Algorithm::Constrained,
            Algorithm::Treemap,
        ] {
            let started = Instant::now();
            match solve(LayoutRequest {
                projection: &projection,
                algorithm,
                pins: &BTreeMap::new(),
                previous: None,
                quality: QualityProfile::default(),
                work_budget: 32,
                cancelled: &AtomicBool::new(false),
            }) {
                Ok(s) => {
                    let m = s.metrics;
                    println!(
                        "{path},{algorithm:?},{QUALITY_VERSION},{},{},{},{},{:.0},{:.3},{:.3},{},{},{:.3},{:.3},{:.1},valid",
                        s.boxes.len(),
                        s.routes.len(),
                        m.route_length,
                        m.bends,
                        m.volume,
                        m.aspect_ratio,
                        m.occlusion,
                        m.shared_lane_length,
                        m.crossing_points,
                        m.congestion,
                        m.structural_cost,
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                Err(e) => println!(
                    "{path},{algorithm:?},{QUALITY_VERSION},,,,,,,,,,,,{:.1},{e}",
                    started.elapsed().as_secs_f64() * 1000.0
                ),
            }
        }
    }
    Ok(())
}
