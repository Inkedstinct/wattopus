use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tiny_http::{Response, Server};

const CPU_Q: &str = "sum by (namespace, pod) (rate(container_cpu_usage_seconds_total{container!=\"\",pod!=\"\"}[2m]))";

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn tick(prom_url: &str, watts_per_core: f64, idle: f64) -> HashMap<(String, String), f64> {
    let mut out: HashMap<(String, String), f64> = HashMap::new();
    let resp = ureq::get(&format!("{prom_url}/api/v1/query"))
        .query("query", CPU_Q)
        .timeout(Duration::from_secs(10))
        .call();
    if let Ok(r) = resp {
        if let Ok(body) = r.into_json::<Value>() {
            for m in body["data"]["result"].as_array().unwrap_or(&vec![]) {
                let ns = m["metric"]["namespace"].as_str().unwrap_or("").to_string();
                let pod = m["metric"]["pod"].as_str().unwrap_or("").to_string();
                let cores = m["value"][1].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0).max(0.0);
                out.insert((ns, pod), cores * watts_per_core + idle);
            }
        }
    }
    out
}

fn render(watts: &HashMap<(String, String), f64>) -> String {
    let mut out = String::from("# TYPE mockpower_pod_watts gauge\n");
    for ((ns, pod), w) in watts {
        out.push_str(&format!("mockpower_pod_watts{{namespace=\"{ns}\",pod=\"{pod}\"}} {w}\n"));
    }
    out
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let prom_url = env_str("PROM_URL", "http://prometheus:9090");
    let watts_per_core = env_f64("WATTS_PER_CORE", 10.0);
    let idle = env_f64("IDLE_WATTS", 0.5);
    let interval = env_f64("INTERVAL", 15.0);

    let watts: Arc<Mutex<HashMap<(String, String), f64>>> = Arc::new(Mutex::new(HashMap::new()));

    {
        let watts = watts.clone();
        let server = Server::http("0.0.0.0:9105").expect("bind :9105");
        thread::spawn(move || {
            for req in server.incoming_requests() {
                if req.url() == "/metrics" {
                    let body = render(&watts.lock().unwrap());
                    let _ = req.respond(Response::from_string(body));
                } else {
                    let _ = req.respond(Response::from_string("").with_status_code(404));
                }
            }
        });
    }

    log::info!("mockpower: metrics :9105, prom {prom_url}");
    loop {
        let fresh = tick(&prom_url, watts_per_core, idle);
        *watts.lock().unwrap() = fresh;
        thread::sleep(Duration::from_secs_f64(interval));
    }
}
