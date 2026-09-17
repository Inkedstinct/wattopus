//! pulls route history from the twin, regresses service watts against route
//! rates, pushes the coefficients back. same shape as the predictor loop.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use fitter::{fit, Sample};
use ingest::{HistoryRow, PowerModel, RouteCoef};

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn twin(greycat: &str, func: &str, args: Value) -> Result<Value, String> {
    ureq::post(&format!("{greycat}/twin::{func}"))
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(30))
        .send_json(args)
        .map_err(|e| format!("twin::{func}: {e}"))?
        .into_json()
        .map_err(|e| format!("twin::{func}: decode: {e}"))
}

/// service -> (route order, aligned samples)
type Designs = HashMap<String, (Vec<String>, Vec<Sample>)>;
/// service -> tick -> (service watts, route -> rps)
type Ticks = HashMap<String, HashMap<i64, (f64, HashMap<String, f64>)>>;

/// long rows -> one design matrix per service. routes are sorted so the
/// coefficient order is stable across ticks
fn design(rows: &[HistoryRow]) -> Designs {
    let routes: BTreeSet<String> = rows.iter().map(|r| r.route.clone()).collect();
    let routes: Vec<String> = routes.into_iter().collect();

    let mut per_service: Ticks = HashMap::new();
    for r in rows {
        let e = per_service
            .entry(r.service.clone())
            .or_default()
            .entry(r.timestamp)
            .or_insert((r.service_watts, HashMap::new()));
        e.0 = r.service_watts;
        e.1.insert(r.route.clone(), r.rps);
    }

    per_service
        .into_iter()
        .map(|(svc, ticks)| {
            let mut times: Vec<i64> = ticks.keys().copied().collect();
            times.sort_unstable();
            let samples = times
                .iter()
                .map(|t| {
                    let (watts, rates) = &ticks[t];
                    Sample {
                        watts: *watts,
                        rps: routes
                            .iter()
                            .map(|r| rates.get(r).copied().unwrap_or(0.0))
                            .collect(),
                    }
                })
                .collect();
            (svc, (routes.clone(), samples))
        })
        .collect()
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn render(models: &[PowerModel]) -> String {
    let mut out = String::new();
    out.push_str("# TYPE wattopus_fit_r2 gauge\n");
    for m in models {
        out.push_str(&format!(
            "wattopus_fit_r2{{service=\"{}\"}} {}\n",
            escape(&m.service),
            m.r2
        ));
    }
    out.push_str("# TYPE wattopus_fit_samples gauge\n");
    for m in models {
        out.push_str(&format!(
            "wattopus_fit_samples{{service=\"{}\"}} {}\n",
            escape(&m.service),
            m.samples
        ));
    }
    out.push_str("# TYPE wattopus_fit_intercept_watts gauge\n");
    for m in models {
        out.push_str(&format!(
            "wattopus_fit_intercept_watts{{service=\"{}\"}} {}\n",
            escape(&m.service),
            m.intercept
        ));
    }
    out.push_str("# TYPE wattopus_fit_watts_per_rps gauge\n");
    for m in models {
        for c in &m.coefs {
            out.push_str(&format!(
                "wattopus_fit_watts_per_rps{{service=\"{}\",route=\"{}\"}} {}\n",
                escape(&m.service),
                escape(&c.route),
                c.watts_per_rps
            ));
        }
    }
    out
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let greycat = env_str("GREYCAT_URL", "http://greycat:8080");
    let namespace = env_str("TWIN_NAMESPACE", "wattopus");
    let interval = env_f64("FIT_INTERVAL", 300.0);
    let window = env_f64("FIT_WINDOW", 7200.0);
    let min_samples = env_f64("FIT_MIN_SAMPLES", 60.0) as usize;
    let min_r2 = env_f64("FIT_MIN_R2", 0.5);
    let ridge = env_f64("FIT_RIDGE", 1e-9);

    let published: Arc<Mutex<Vec<PowerModel>>> = Arc::new(Mutex::new(Vec::new()));

    {
        let published = published.clone();
        let server = tiny_http::Server::http("0.0.0.0:9500").expect("bind :9500");
        thread::spawn(move || {
            for req in server.incoming_requests() {
                if req.url() == "/metrics" {
                    let body = render(&published.lock().unwrap());
                    let _ = req.respond(tiny_http::Response::from_string(body));
                } else {
                    let _ = req.respond(tiny_http::Response::from_string("").with_status_code(404));
                }
            }
        });
    }

    log::info!("fitter: twin {greycat}, ns {namespace}, every {interval}s over {window}s");

    loop {
        thread::sleep(Duration::from_secs_f64(interval));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let from = now - window as i64;

        let rows: Vec<HistoryRow> =
            match twin(&greycat, "route_history", json!([namespace, from, now])) {
                Ok(v) => serde_json::from_value(v).unwrap_or_default(),
                Err(e) => {
                    log::warn!("{e}");
                    continue;
                }
            };
        if rows.is_empty() {
            log::warn!("no history in the last {window}s");
            continue;
        }

        let mut fresh = Vec::new();
        for (service, (routes, samples)) in design(&rows) {
            if samples.len() < min_samples {
                log::info!(
                    "{service}: {} samples < {min_samples}, skipped",
                    samples.len()
                );
                continue;
            }
            let Some(f) = fit(&samples, routes.len(), ridge) else {
                log::warn!("{service}: degenerate design (collinear rates?), no model");
                continue;
            };
            if f.r2 < min_r2 {
                log::warn!("{service}: r2 {:.3} < {min_r2}, not published", f.r2);
                continue;
            }
            let model = PowerModel {
                namespace: namespace.clone(),
                service: service.clone(),
                fitted_at: now,
                intercept: f.intercept,
                coefs: routes
                    .iter()
                    .zip(f.coefs.iter())
                    .map(|(r, b)| RouteCoef {
                        route: r.clone(),
                        watts_per_rps: *b,
                    })
                    .collect(),
                r2: f.r2,
                samples: f.samples as i64,
            };
            log::info!(
                "{service}: intercept {:.3} W, r2 {:.3}, {} samples",
                f.intercept,
                f.r2,
                f.samples
            );
            if let Err(e) = twin(&greycat, "ingest_route_model", json!([model])) {
                log::warn!("{e}");
                continue;
            }
            fresh.push(model);
        }
        *published.lock().unwrap() = fresh;
    }
}
