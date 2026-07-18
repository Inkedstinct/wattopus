//! mock prediction loop. pulls the newest sample per deployment from the
//! twin,
use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use ingest::{DeployRef, LatestSample, Prediction};

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn twin(greycat: &str, func: &str, args: Value) -> Result<Value, String> {
    ureq::post(&format!("{greycat}/twin::{func}"))
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .send_json(args)
        .map_err(|e| format!("twin::{func}: {e}"))?
        .into_json()
        .map_err(|e| format!("twin::{func}: decode: {e}"))
}

fn twin_send(greycat: &str, func: &str, args: Value) -> Result<(), String> {
    ureq::post(&format!("{greycat}/twin::{func}"))
        .timeout(Duration::from_secs(10))
        .send_json(args)
        .map(|_| ())
        .map_err(|e| format!("twin::{func}: {e}"))
}

/// quiescent = the newest point stays within mean +/- sigma of the older
/// points. sigma gets a floor of 5% of the mean because mock metrics can be
/// perfectly flat 
fn classify(series: &[f64]) -> (f64, bool) {
    let newest = *series.last().unwrap_or(&0.0);
    if series.len() < 3 {
        return (newest, false); // too little history to call anything stable ? TODO : Check for threshold
    }
    let hist = &series[..series.len() - 1];
    let mean = hist.iter().sum::<f64>() / hist.len() as f64;
    let sigma =
        (hist.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / hist.len() as f64).sqrt();
    if (newest - mean).abs() <= sigma.max(mean.abs() * 0.05) {
        (series.iter().sum::<f64>() / series.len() as f64, true)
    } else {
        (newest, false)
    }
}

struct Window {
    last_seen: i64, // newest sample timestamp, to skip polls between feeder pushes
    cpu: Vec<f64>,
    joules: Vec<f64>,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let greycat = env_str("GREYCAT_URL", "http://greycat:8080");
    let interval = env_f64("INTERVAL", 30.0);
    let window = env_f64("WINDOW", 12.0) as usize;

    log::info!("predictor: greycat {greycat}, interval {interval}s, window {window}");

    let mut windows: HashMap<(String, String), Window> = HashMap::new();

    loop {
        thread::sleep(Duration::from_secs_f64(interval));

        let deploys: Vec<DeployRef> = match twin(&greycat, "deployments", json!([])) {
            Ok(v) => serde_json::from_value(v).unwrap_or_default(),
            Err(e) => {
                log::warn!("{e}");
                continue;
            }
        };

        for d in deploys {
            let sample = match twin(&greycat, "latest", json!([d.namespace, d.name])) {
                Ok(Value::Null) => continue, // known deployment, no metrics yet
                Ok(v) => match serde_json::from_value::<LatestSample>(v) {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("{}/{}: bad latest: {e}", d.namespace, d.name);
                        continue;
                    }
                },
                Err(e) => {
                    log::warn!("{e}");
                    continue;
                }
            };

            let w = windows.entry((d.namespace.clone(), d.name.clone())).or_insert(Window {
                last_seen: 0,
                cpu: Vec::new(),
                joules: Vec::new(),
            });
            if sample.timestamp <= w.last_seen {
                continue; // feeder hasn't pushed since our last poll
            }
            w.last_seen = sample.timestamp;
            w.cpu.push(sample.cpu_usage);
            w.joules.push(sample.joules);
            if w.cpu.len() > window {
                w.cpu.remove(0);
                w.joules.remove(0);
            }

            let (cpu_p, quiescent) = classify(&w.cpu);
            let (joules_p, _) = classify(&w.joules); // cpu drives scaling, its verdict wins

            let p = Prediction {
                namespace: d.namespace.clone(),
                name: d.name.clone(),
                timestamp: sample.timestamp,
                cpu_predicted: cpu_p,
                joules_predicted: joules_p,
                quiescent,
            };
            match twin_send(&greycat, "ingest_prediction", json!([p])) {
                Ok(()) => log::info!(
                    "{}/{}: cpu {:.3} joules {:.1} quiescent {}",
                    d.namespace, d.name, cpu_p, joules_p, quiescent
                ),
                Err(e) => log::warn!("{e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn flat_series_is_quiescent_and_predicts_mean() {
        let (p, q) = classify(&[0.2, 0.21, 0.19, 0.2]);
        assert!(q);
        assert!((p - 0.2).abs() < 0.01);
    }

    #[test]
    fn step_change_is_unstable_and_predicts_persistence() {
        let (p, q) = classify(&[0.2, 0.2, 0.2, 0.8]);
        assert!(!q);
        assert!((p - 0.8).abs() < 1e-12);
    }

    #[test]
    fn short_history_is_never_quiescent() {
        let (p, q) = classify(&[0.5, 0.5]);
        assert!(!q);
        assert!((p - 0.5).abs() < 1e-12);
    }
}
