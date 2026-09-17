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
    attrs?
        .as_array()?
        .iter()
        .find(|a| a["key"] == key)
        .map(|a| &a["value"])
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
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_u64().map(u128::from))
        .unwrap_or(0)
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
    let roots: HashMap<&str, &str> = spans
        .iter()
        .filter(|s| s.root)
        .map(|s| (s.trace.as_str(), s.route.as_str()))
        .collect();
    let mut w: HashMap<(String, String), f64> = HashMap::new();
    for s in spans {
        let route = roots
            .get(s.trace.as_str())
            .copied()
            .unwrap_or(s.route.as_str());
        *w.entry((s.service.clone(), route.to_string())).or_default() += s.busy;
    }
    w
}

/// root spans per second, per route. the exogenous input the model regresses
/// against - children inherit their root's route and must not be counted twice
pub fn route_rates(spans: &[Span], elapsed: f64) -> HashMap<String, f64> {
    let mut counts: HashMap<String, f64> = HashMap::new();
    for s in spans.iter().filter(|s| s.root) {
        *counts.entry(s.route.clone()).or_default() += 1.0;
    }
    for v in counts.values_mut() {
        *v = if elapsed > 0.0 { *v / elapsed } else { 0.0 };
    }
    counts
}

/// where the fitted intercept goes. idle = Aumann-Shapley (fixed cost stays
/// unallocated), equal = Shapley with v(empty) shared out, proportional = a
/// proportional cost-sharing rule
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptPolicy {
    Idle,
    Equal,
    Proportional,
}

impl InterceptPolicy {
    pub fn parse(s: &str) -> InterceptPolicy {
        match s {
            "equal" => InterceptPolicy::Equal,
            "proportional" => InterceptPolicy::Proportional,
            _ => InterceptPolicy::Idle,
        }
    }
}

pub struct Model {
    pub intercept: f64,
    pub coefs: HashMap<String, f64>,
}

pub struct FitContext<'a> {
    pub models: &'a HashMap<String, Model>,
    pub rps: &'a HashMap<String, f64>,
    pub policy: InterceptPolicy,
}

impl FitContext<'_> {
    fn shares(&self, svc: &str) -> Option<(Vec<(String, f64)>, f64)> {
        let m = self.models.get(svc)?;
        let mut out = Vec::new();
        for (route, beta) in &m.coefs {
            let raw = beta.max(0.0) * self.rps.get(route).copied().unwrap_or(0.0);
            if raw > 0.0 {
                out.push((route.clone(), raw));
            }
        }
        let idle = m.intercept.max(0.0);
        if out.iter().map(|(_, v)| v).sum::<f64>() + idle <= 0.0 {
            return None;
        }
        Some((out, idle))
    }
}

pub struct Attribution {
    pub route_watts: HashMap<String, f64>,
    pub service_route_watts: HashMap<(String, String), f64>,
    pub unattributed: HashMap<(String, String), f64>,
    pub services: HashMap<String, (usize, f64)>,
    pub service_watts: HashMap<String, f64>,
    pub idle_watts: HashMap<String, f64>,
    pub unresolved: usize,
}

pub fn attribute(
    weights: &HashMap<(String, String), f64>,
    pod_watts: &HashMap<(String, String), f64>,
    categories: &HashMap<(String, String), String>,
) -> Attribution {
    attribute_with(weights, pod_watts, categories, None)
}

pub fn attribute_with(
    weights: &HashMap<(String, String), f64>,
    pod_watts: &HashMap<(String, String), f64>,
    categories: &HashMap<(String, String), String>,
    fit: Option<&FitContext>,
) -> Attribution {
    // Fold map into nested service -> route -> busy
    let mut per_service: HashMap<&str, HashMap<&str, f64>> = HashMap::new();
    for ((svc, route), wt) in weights {
        *per_service
            .entry(svc)
            .or_default()
            .entry(route)
            .or_default() += wt;
    }

    let mut route_watts: HashMap<String, f64> = HashMap::new();
    let mut service_route_watts: HashMap<(String, String), f64> = HashMap::new();
    let mut claimed: Vec<(String, String)> = Vec::new();
    let mut services: HashMap<String, (usize, f64)> = HashMap::new();
    let mut service_watts: HashMap<String, f64> = HashMap::new();
    let mut idle_watts: HashMap<String, f64> = HashMap::new();
    let mut unresolved = 0;

    // Matching pods on name regex. Skips service if no pods
    // TODO: Have k8s metadata
    // service (svc) Watts (W) = sum of matching pods
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

        // Fallback to busy-time if either:
        // 1. fit === None (we asked for busy)
        // 2. There is not model for svc (Fitter rejected it)
        // 3. Somehow, model returns wrong value type
        let (split, idle): (Vec<(String, f64)>, f64) = match fit.and_then(|f| f.shares(svc)) {
            Some((raw, raw_idle)) => {
                let sum: f64 = raw.iter().map(|(_, v)| v).sum::<f64>() + raw_idle;
                let scale = svc_watts / sum;
                let scaled: Vec<(String, f64)> =
                    raw.into_iter().map(|(r, v)| (r, v * scale)).collect();
                let idle_w = raw_idle * scale;
                let routed: f64 = scaled.iter().map(|(_, v)| v).sum();
                // Spliting the Idle depending on the InterceptPolicy

                match fit.map(|f| f.policy) {
                    Some(InterceptPolicy::Equal) if !scaled.is_empty() => {
                        let n = scaled.len() as f64;
                        let s = scaled
                            .into_iter()
                            .map(|(r, v)| (r, v + idle_w / n))
                            .collect();
                        (s, 0.0)
                    }
                    Some(InterceptPolicy::Proportional) if routed > 0.0 => {
                        let s = scaled
                            .into_iter()
                            .map(|(r, v)| (r, v + idle_w * v / routed))
                            .collect();
                        (s, 0.0)
                    }
                    _ => (scaled, idle_w),
                }
            }
            None => (
                routes
                    .iter()
                    .map(|(route, wt)| ((*route).to_string(), svc_watts * wt / total))
                    .collect(),
                0.0,
            ),
        };

        if idle > 0.0 {
            *route_watts.entry("_idle".into()).or_default() += idle;
            idle_watts.insert((*svc).to_string(), idle);
        }
        for (route, w) in split {
            *route_watts.entry(route.clone()).or_default() += w;
            *service_route_watts
                .entry(((*svc).to_string(), route))
                .or_default() += w;
        }
        services.insert((*svc).to_string(), (pods.len(), total));
        service_watts.insert((*svc).to_string(), svc_watts);
    }

    // Going through ignored svc to account them in "unattributed"
    let mut unattributed: HashMap<(String, String), f64> = HashMap::new();
    for (k, w) in pod_watts {
        if claimed.contains(k) {
            continue;
        }
        match categories.get(k) {
            Some(cat) => *route_watts.entry(format!("_{cat}")).or_default() += *w,
            None => {
                unattributed.insert(k.clone(), *w);
            }
        }
    }
    *route_watts.entry("_unattributed".into()).or_default() += unattributed.values().sum::<f64>();

    Attribution {
        route_watts,
        service_route_watts,
        unattributed,
        services,
        service_watts,
        idle_watts,
        unresolved,
    }
}

pub fn category_watts(
    pod_watts: &HashMap<(String, String), f64>,
    categories: &HashMap<(String, String), String>,
) -> HashMap<(String, String), (String, f64)> {
    pod_watts
        .iter()
        .map(|(k, w)| {
            let bucket = match categories.get(k) {
                Some(c) => format!("_{c}"),
                None => "_unlabeled".into(),
            };
            (k.clone(), (bucket, *w))
        })
        .collect()
}
