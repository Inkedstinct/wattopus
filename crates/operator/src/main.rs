use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use ingest::ScaleSimulation;
use k8s::Client;
use otlp::Tracer;

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

fn respond(req: tiny_http::Request, status: u16, body: Value) {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let _ = req.respond(Response::from_string(body.to_string()).with_status_code(status).with_header(header));
}

fn handle_simulate(
    cli: &Client,
    greycat: &str,
    cpu_limit: f64,
    body: &Value,
) -> (u16, Value) {
    let ns = body["namespace"].as_str().unwrap_or("");
    let deploy = body["deployment"].as_str().unwrap_or("");
    let replicas = body["replicas"].as_i64().unwrap_or(0);

    // twin::simulate_scale stages the change and returns a ScaleSimulation
    let sim = match twin(greycat, "simulate_scale", json!([ns, deploy, replicas])) {
        Ok(v) => v,
        Err(e) => return (502, json!({"error": e})),
    };
    if sim.is_null() {
        return (404, json!({"error": format!("{ns}/{deploy} unknown to the twin (no metrics yet?)")}));
    }
    let result: ScaleSimulation = match serde_json::from_value(sim) {
        Ok(s) => s,
        Err(e) => return (502, json!({"error": format!("twin::simulate_scale: unexpected shape: {e}")})),
    };

    if result.cpu_per_pod_predicted > cpu_limit {
        let _ = twin(greycat, "rollback_scale", json!([ns, deploy]));
        return (
            409, // TODO : Check if its well supported 
            json!({
                "applied": false, "simulation": result,
                "reason": format!(
                    "requirement not met: predicted {:.3} CPU/pod exceeds limit {}; twin restored",
                    result.cpu_per_pod_predicted, cpu_limit
                )
            }),
        );
    }

    if let Err(e) = cli.patch_scale(ns, deploy, replicas) {
        let _ = twin(greycat, "rollback_scale", json!([ns, deploy]));
        return (502, json!({"applied": false, "simulation": result, "reason": format!("kubectl scale failed: {e}; twin restored")}));
    }
    let _ = twin(greycat, "commit_scale", json!([ns, deploy]));
    (200, json!({"applied": true, "simulation": result}))
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let greycat = env_str("GREYCAT_URL", "http://greycat:8080");
    let cpu_limit = env_f64("CPU_LIMIT_PER_POD", 0.8);
    let tracer = Tracer::from_env();
    let cli = Client::in_cluster().expect("kubernetes in-cluster client");

    let server = Server::http("0.0.0.0:8080").expect("bind :8080");
    log::info!("operator: :8080, greycat {greycat}, cpu_limit {cpu_limit}");

    for mut req in server.incoming_requests() {
        let path = req.url().split('?').next().unwrap_or("").to_string();
        let method = req.method().clone();

        if path == "/healthz" {
            let _ = req.respond(Response::from_string("ok"));
            continue;
        }
        if method != tiny_http::Method::Post || path != "/simulate" {
            respond(req, 404, json!({"error": "not found"}));
            continue;
        }

        let _span = tracer.root("simulate", "/simulate");
        let mut raw = String::new();
        let _ = req.as_reader().read_to_string(&mut raw);
        let body: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        let (status, out) = handle_simulate(&cli, &greycat, cpu_limit, &body);
        respond(req, status, out);
    }
}
