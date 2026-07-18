//! minimal OTLP/HTTP JSON trace emitter. one span per served request,
//! flushed fire-and-forget so a slow collector never blocks the handler.

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

#[derive(Clone)]
pub struct Tracer {
    service: String,
    tx: Option<Sender<serde_json::Value>>,
}

fn now_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn rand_id(n: usize) -> String {
    // enough entropy for a demo; seeded from clock + a counter
    use std::sync::atomic::{AtomicU64, Ordering};
    static C: AtomicU64 = AtomicU64::new(0);
    let seed = now_nanos() as u64 ^ (C.fetch_add(0x9e3779b9, Ordering::Relaxed));
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push((x & 0xff) as u8);
    }
    hex(&out)
}

pub struct Span {
    pub trace_id: String,
    pub span_id: String,
    parent: String,
    name: String,
    route: String,
    start: u128,
    tx: Option<Sender<serde_json::Value>>,
    service: String,
}

impl Tracer {
    /// reads OTEL_SERVICE_NAME and OTEL_EXPORTER_OTLP_ENDPOINT; if the latter
    /// is unset, tracing is a no-op (handy for local runs and tests).
    pub fn from_env() -> Self {
        let service = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "unknown".into());
        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        let tx = endpoint.map(|base| {
            let url = format!("{}/v1/traces", base.trim_end_matches('/'));
            let (tx, rx) = mpsc::channel::<serde_json::Value>();
            thread::spawn(move || {
                for span in rx {
                    let _ = ureq::post(&url)
                        .set("Content-Type", "application/json")
                        .send_json(span);
                }
            });
            tx
        });
        Tracer { service, tx }
    }

    /// root span: new trace id, propagate via headers returned to the caller.
    pub fn root(&self, name: &str, route: &str) -> Span {
        self.span(name, route, rand_id(16), String::new())
    }

    /// child span from an incoming traceparent header, if any.
    pub fn child(&self, name: &str, route: &str, traceparent: Option<&str>) -> Span {
        match parse_traceparent(traceparent) {
            Some((trace_id, parent)) => self.span(name, route, trace_id, parent),
            None => self.root(name, route),
        }
    }

    fn span(&self, name: &str, route: &str, trace_id: String, parent: String) -> Span {
        Span {
            trace_id,
            span_id: rand_id(8),
            parent,
            name: name.into(),
            route: route.into(),
            start: now_nanos(),
            tx: self.tx.clone(),
            service: self.service.clone(),
        }
    }
}

impl Span {
    /// W3C traceparent https://www.w3.org/TR/trace-context/#traceparent-header 
    pub fn traceparent(&self) -> String {
        format!("00-{}-{}-01", self.trace_id, self.span_id)
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(tx) = &self.tx else { return };
        let end = now_nanos();
        let mut span = json!({
            "traceId": self.trace_id,
            "spanId": self.span_id,
            "name": self.name,
            "startTimeUnixNano": self.start.to_string(),
            "endTimeUnixNano": end.to_string(),
            "attributes": [
                {"key": "http.route", "value": {"stringValue": self.route}}
            ],
        });
        if !self.parent.is_empty() {
            span["parentSpanId"] = json!(self.parent);
        }
        let payload = json!({
            "resourceSpans": [{
                "resource": {"attributes": [
                    {"key": "service.name", "value": {"stringValue": self.service}}
                ]},
                "scopeSpans": [{"spans": [span]}]
            }]
        });
        let _ = tx.send(payload);
    }
}

fn parse_traceparent(h: Option<&str>) -> Option<(String, String)> {
    let parts: Vec<&str> = h?.split('-').collect();
    if parts.len() == 4 && parts[1].len() == 32 && parts[2].len() == 16 {
        Some((parts[1].to_string(), parts[2].to_string()))
    } else {
        None
    }
}
