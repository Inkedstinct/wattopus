use std::env;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tiny_http::{Header, Method, Request, Response, Server};

use otlp::{Span, Tracer};

// Random operation to generate some activity
fn burn(n: u64) -> String {
    let mut h = Sha256::digest(b"wattopus").to_vec();
    for _ in 0..n {
        h = Sha256::digest(&h).to_vec();
    }
    h.iter().map(|b| format!("{b:02x}")).collect()
}

// Returns the value of a env variable as i64
fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// seeded
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CD01D)
    }
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + unit * (hi - lo)
    }
}

fn run_load() {
    let target = env::var("TARGET").unwrap_or_else(|_| "http://app-gateway:8000".into());
    let routes: Vec<String> = env::var("ROUTES")
        .unwrap_or_else(|_| "/checkout,/catalog,/report".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let seed = env_u64("SEED", 42);
    let phase = env_u64("PHASE", 300) as f64;
    let rate_min = env_f64("RATE_MIN", 0.2);
    let rate_max = env_f64("RATE_MAX", 3.0);

    let start = Instant::now();
    let mut rates = vec![0.0f64; routes.len()];
    let mut due = vec![0.0f64; routes.len()];
    let mut cur_phase = u64::MAX;

    log::info!("load: target={target} seed={seed} phase={phase}s");
    loop {
        let now = start.elapsed().as_secs_f64();

        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let p = epoch / phase as u64;
        if p != cur_phase {
            cur_phase = p;
            let mut rng = Rng::new(seed ^ p.wrapping_mul(0x9E3779B97F4A7C15));
            for (i, r) in routes.iter().enumerate() {
                rates[i] = rng.uniform(rate_min, rate_max);
                due[i] = now;
                log::info!("phase {p} route={r} target_rps={:.3}", rates[i]);
            }
        }
        for i in 0..routes.len() {
            if now >= due[i] {
                let _ = ureq::get(&format!("{target}{}", routes[i]))
                    .timeout(Duration::from_secs(10))
                    .call();
                due[i] = now + 1.0 / rates[i];
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
}

// Uses W3C traceparent https://www.w3.org/TR/trace-context/#traceparent-header
// Check in OTLP crate
fn traceparent(req: &Request) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv("traceparent")) // equiv is relaxed matching
        .map(|h| h.value.as_str().to_string())
}

fn get_json(url: &str, span: &Span) -> Value {
    ureq::get(url)
        .set("traceparent", &span.traceparent())
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .unwrap_or(Value::Null)
}

fn respond(req: Request, status: u16, body: Value) {
    let data = body.to_string();
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let _ = req.respond(
        Response::from_string(data)
            .with_status_code(status)
            .with_header(header),
    );
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let role = env::var("ROLE").expect("ROLE is required");
    if role == "load" {
        run_load(); // never returns; the load role serves nothing
        return;
    }
    let compute = env::var("COMPUTE_URL").unwrap_or_else(|_| "http://app-compute:8000".into());
    let store = env::var("STORE_URL").unwrap_or_else(|_| "http://app-store:8000".into());
    let work = env_u64("WORK", 50_000);
    let tracer = Tracer::from_env();

    let items: Mutex<Vec<Value>> = Mutex::new(Vec::new());

    let port = env::var("PORT").unwrap_or_else(|_| "8000".into());
    let server = Server::http(format!("0.0.0.0:{port}")).expect("bind port");
    log::info!("app role={role} listening on :{port}");

    for req in server.incoming_requests() {
        let path = req.url().split('?').next().unwrap_or("").to_string();
        let method = req.method().clone();
        let tp = traceparent(&req);

        if path == "/healthz" {
            let _ = req.respond(Response::from_string("ok"));
            continue;
        }

        match (role.as_str(), method, path.as_str()) {
            ("gateway", Method::Get, "/checkout") => {
                let span = tracer.child("checkout", "/checkout", tp.as_deref());
                let price = get_json(&format!("{compute}/price"), &span)["price"].clone();
                let saved = get_json(
                    &format!("{store}/save?price={}", price.as_f64().unwrap_or(0.0)),
                    &span,
                );
                respond(req, 200, json!({"order": saved["id"], "price": price}));
            }
            ("gateway", Method::Get, "/catalog") => {
                let span = tracer.child("catalog", "/catalog", tp.as_deref());
                let list = get_json(&format!("{store}/list"), &span);
                respond(req, 200, list);
            }
            ("gateway", Method::Get, "/report") => {
                let span = tracer.child("report", "/report", tp.as_deref());
                let stats = get_json(&format!("{compute}/stats"), &span);
                respond(req, 200, stats);
            }

            ("compute", Method::Get, "/price") => {
                let _span = tracer.child("price", "/price", tp.as_deref());
                let digest = burn(work);
                let price = u32::from_str_radix(&digest[..4], 16).unwrap_or(0) % 100 + 1;
                respond(req, 200, json!({"price": price}));
            }
            ("compute", Method::Get, "/stats") => {
                let span = tracer.child("stats", "/stats", tp.as_deref());
                let list = get_json(&format!("{store}/list"), &span);
                let orders = list["items"].as_array().cloned().unwrap_or_default();
                let total: f64 = orders.iter().filter_map(|o| o["price"].as_f64()).sum();
                burn(work * 2);
                respond(req, 200, json!({"count": orders.len(), "total": total}));
            }

            ("store", Method::Get, "/save") => {
                let _span = tracer.child("save", "/save", tp.as_deref());
                let price: f64 = req
                    .url()
                    .split("price=")
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let mut guard = items.lock().unwrap();
                let item = json!({"id": guard.len() + 1, "price": price});
                guard.push(item.clone());
                respond(req, 200, item);
            }
            ("store", Method::Get, "/list") => {
                let _span = tracer.child("list", "/list", tp.as_deref());
                let guard = items.lock().unwrap();
                respond(req, 200, json!({"items": *guard}));
            }

            _ => respond(req, 404, json!({"error": "not found"})),
        }
    }
}
