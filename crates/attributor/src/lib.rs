use std::collections::HashMap;

use serde_json::Value;

pub struct Span {
    pub trace: String,
    pub root: bool,
    pub service: String,
    pub route: String,
    pub busy: f64,
}

fn attr<'a>(attrs: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    attrs?.as_array()?.iter().find(|a| a["key"] == key).map(|a| &a["value"])
}

fn attr_str(attrs: Option<&Value>, key: &str) -> Option<String> {
    let v = attr(attrs, key)?;
    v["stringValue"]
        .as_str()
        .map(String::from)
        .or_else(|| v["intValue"].as_str().map(String::from))
        .or_else(|| v["intValue"].as_i64().map(|n| n.to_string()))
}

fn nano(v: &Value) -> u128 {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_u64().map(u128::from)).unwrap_or(0)
}

pub fn parse(body: &Value, route_attr: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for rs in body["resourceSpans"].as_array().unwrap_or(&vec![]) {
        let Some(service) = attr_str(Some(&rs["resource"]["attributes"]), "service.name") else {
            continue;
        };
        for ss in rs["scopeSpans"].as_array().unwrap_or(&vec![]) {
            for sp in ss["spans"].as_array().unwrap_or(&vec![]) {
                let start = nano(&sp["startTimeUnixNano"]);
                let end = nano(&sp["endTimeUnixNano"]);
                if end <= start {
                    continue;
                }
                let route = attr_str(Some(&sp["attributes"]), route_attr)
                    .unwrap_or_else(|| sp["name"].as_str().unwrap_or("").to_string());
                out.push(Span {
                    trace: sp["traceId"].as_str().unwrap_or("").to_string(),
                    root: sp["parentSpanId"].as_str().map_or(true, |s| s.is_empty()),
                    service: service.clone(),
                    route,
                    busy: (end - start) as f64 / 1e9,
                });
            }
        }
    }
    out
}
/// Horizontal atrtibution here
/// spans inherit their trace root's route
pub fn weights(spans: &[Span]) -> HashMap<(String, String), f64> {
    let roots: HashMap<&str, &str> =
        spans.iter().filter(|s| s.root).map(|s| (s.trace.as_str(), s.route.as_str())).collect();
    let mut w: HashMap<(String, String), f64> = HashMap::new();
    for s in spans {
        let route = roots.get(s.trace.as_str()).copied().unwrap_or(s.route.as_str());
        *w.entry((s.service.clone(), route.to_string())).or_default() += s.busy;
    }
    w
}

pub struct Attribution {
    pub route_watts: HashMap<String, f64>,
    /// unclaimed (namespace, pod) -> watts; sums to route_watts["_unattributed"]
    pub unattributed: HashMap<(String, String), f64>,
    /// traced service -> (claimed pods, busy seconds); busy/(interval*pods) = coverage
    pub services: HashMap<String, (usize, f64)>,
    pub unresolved: usize,
}

pub fn attribute(
    weights: &HashMap<(String, String), f64>,
    pod_watts: &HashMap<(String, String), f64>,
) -> Attribution {
    let mut per_service: HashMap<&str, HashMap<&str, f64>> = HashMap::new();
    for ((svc, route), wt) in weights {
        *per_service.entry(svc).or_default().entry(route).or_default() += wt;
    }

    let mut route_watts: HashMap<String, f64> = HashMap::new();
    let mut claimed: Vec<(String, String)> = Vec::new();
    let mut services: HashMap<String, (usize, f64)> = HashMap::new();
    let mut unresolved = 0;

    for (svc, routes) in &per_service {
        let pods: Vec<(&(String, String), &f64)> = pod_watts
            .iter()
            .filter(|(k, _)| k.1 == **svc || k.1.starts_with(&format!("{svc}-")))
            .collect();
        if pods.is_empty() {
            unresolved += 1;
            continue;
        }
        let svc_watts: f64 = pods.iter().map(|(_, w)| **w).sum();
        for (k, _) in &pods {
            claimed.push((*k).clone());
        }
        let total: f64 = routes.values().sum();
        for (route, wt) in routes {
            *route_watts.entry((*route).to_string()).or_default() += svc_watts * wt / total;
        }
        services.insert((*svc).to_string(), (pods.len(), total));
    }

    let unattributed: HashMap<(String, String), f64> = pod_watts
        .iter()
        .filter(|(k, _)| !claimed.contains(k))
        .map(|(k, w)| (k.clone(), *w))
        .collect();
    *route_watts.entry("_unattributed".into()).or_default() += unattributed.values().sum::<f64>();

    Attribution { route_watts, unattributed, services, unresolved }
}
