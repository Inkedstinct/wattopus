use std::env;
use std::sync::Mutex;

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
        .and_then(|r| Ok(r.into_json::<Value>()?))
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

