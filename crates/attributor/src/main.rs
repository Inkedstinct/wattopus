use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tiny_http::{Header, Response, Server};

use attributor::{attribute, parse, weights, Span};

struct Metrics {
    energy: HashMap<String, f64>,
    power: HashMap<String, f64>,
    service_route: HashMap<(String, String), f64>,
    unattributed: HashMap<(String, String), f64>,
    coverage: HashMap<String, f64>,
    unresolved: usize,
}

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn pod_watts(prom_url: &str, query: &str) -> HashMap<(String, String), f64> {
    let resp = ureq::get(&format!("{prom_url}/api/v1/query"))
        .query("query", query)
        .timeout(Duration::from_secs(10))
        .call();
    let mut out = HashMap::new();
    if let Ok(r) = resp {
        if let Ok(body) = r.into_json::<Value>() {
            for m in body["data"]["result"].as_array().unwrap_or(&vec![]) {
                let ns = m["metric"]["namespace"].as_str().unwrap_or("").to_string();
                let pod = m["metric"]["pod"].as_str().unwrap_or("").to_string();
                if let Some(v) = m["value"][1].as_str().and_then(|s| s.parse::<f64>().ok()) {
                    out.insert((ns, pod), v);
                }
            }
        }
    }
    out
}

fn render(m: &Metrics, timestamp: u64) -> String {
    let mut out = String::new();
    out.push_str("# TYPE wattopus_route_energy_joules_total counter\n");
    for (route, j) in &m.energy {
        out.push_str(&format!(
            "wattopus_route_energy_joules_total{{route=\"{}\"}} {}\n",
            escape(route),
            j
        ));
    }
    out.push_str("# TYPE wattopus_route_power_watts gauge\n");
    for (route, w) in &m.power {
        out.push_str(&format!(
            "wattopus_route_power_watts{{route=\"{}\"}} {}\n",
            escape(route),
            w
        ));
    }
    out.push_str("# TYPE wattopus_service_route_power_watts gauge\n");
    for ((svc, route), w) in &m.service_route {
        out.push_str(&format!(
            "wattopus_service_route_power_watts{{service=\"{}\",route=\"{}\"}} {}\n",
            escape(svc),
            escape(route),
            w
        ));
    }
    out.push_str("# TYPE wattopus_unattributed_pod_watts gauge\n");
    for ((ns, pod), w) in &m.unattributed {
        out.push_str(&format!(
            "wattopus_unattributed_pod_watts{{namespace=\"{}\",pod=\"{}\"}} {}\n",
            escape(ns),
            escape(pod),
            w
        ));
    }
    out.push_str("# TYPE wattopus_service_trace_coverage gauge\n");
    for (svc, c) in &m.coverage {
        out.push_str(&format!(
            "wattopus_service_trace_coverage{{service=\"{}\"}} {}\n",
            escape(svc),
            c
        ));
    }
    out.push_str("# TYPE wattopus_unresolved_services gauge\n");
    out.push_str(&format!("wattopus_unresolved_services {}\n", m.unresolved));
    out.push_str("# TYPE wattopus_last_tick_timestamp_seconds gauge\n");
    out.push_str(&format!("wattopus_last_tick_timestamp_seconds {timestamp}\n"));
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let prom_url = env_str("PROM_URL", "http://prometheus:9090");
    let power_query =
        env_str("POWER_QUERY", "sum by (namespace, pod) (mockpower_pod_watts)");
    let route_attr = env_str("ROUTE_ATTR", "http.route");
    let interval = env_f64("INTERVAL", 15.0);

    let spans: Arc<Mutex<Vec<Span>>> = Arc::new(Mutex::new(Vec::new()));
    let metrics = Arc::new(Mutex::new(Metrics {
        energy: HashMap::new(),
        power: HashMap::new(),
        service_route: HashMap::new(),
        unattributed: HashMap::new(),
        coverage: HashMap::new(),
        unresolved: 0,
    }));

    
    {
        let spans = spans.clone();
        let route_attr = route_attr.clone();
        let server = Server::http("0.0.0.0:4318").expect("bind :4318");
        thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let path = req.url().split('?').next().unwrap_or("").to_string();
                if req.method() == &tiny_http::Method::Get {
                    let code = if path == "/healthz" { 200 } else { 404 };
                    let _ = req.respond(Response::from_string("").with_status_code(code));
                    continue;
                }
                if path != "/v1/traces" {
                    let _ = req.respond(Response::from_string("").with_status_code(404));
                    continue;
                }
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                match serde_json::from_str::<Value>(&body) {
                    Ok(v) => {
                        let batch = parse(&v, &route_attr);
                        spans.lock().unwrap().extend(batch);
                        let header =
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
                        let _ = req.respond(Response::from_string("{}").with_header(header));
                    }
                    Err(_) => {
                        let _ = req.respond(Response::from_string("").with_status_code(400));
                    }
                }
            }
        });
    }

    {
        let metrics = metrics.clone();
        let server = Server::http("0.0.0.0:9500").expect("bind :9500");
        thread::spawn(move || {
            for req in server.incoming_requests() {
                if req.url() == "/metrics" {
                    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                    let body = render(&metrics.lock().unwrap(), ts);
                    let _ = req.respond(Response::from_string(body));
                } else {
                    let _ = req.respond(Response::from_string("").with_status_code(404));
                }
            }
        });
    }

    log::info!("attributor: OTLP :4318, metrics :9500, prom {prom_url}");

    let mut last = Instant::now();
    loop {
        thread::sleep(Duration::from_secs_f64(interval));
        let elapsed = last.elapsed().as_secs_f64();
        last = Instant::now();

        let batch: Vec<Span> = std::mem::take(&mut *spans.lock().unwrap());
        let a = attribute(&weights(&batch), &pod_watts(&prom_url, &power_query));

        let mut m = metrics.lock().unwrap();
        m.power.clear();
        for (route, w) in &a.route_watts {
            *m.energy.entry(route.clone()).or_default() += w * elapsed;
            m.power.insert(route.clone(), *w);
        }
        m.service_route = a.service_route_watts;
        m.unattributed = a.unattributed;
        m.coverage = a
            .services
            .iter()
            .map(|(svc, (pods, busy))| (svc.clone(), busy / (elapsed * *pods as f64)))
            .collect();
        m.unresolved = a.unresolved;
    }
}
