use std::collections::HashMap;

use attributor::{attribute, parse, weights};
use serde_json::json;

fn otlp() -> serde_json::Value {
    json!({
        "resourceSpans": [
            {
                "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "app-gateway"}}]},
                "scopeSpans": [{"spans": [{
                    "traceId": "t1", "spanId": "a", "name": "GET",
                    "attributes": [{"key": "http.route", "value": {"stringValue": "/checkout"}}],
                    "startTimeUnixNano": "0", "endTimeUnixNano": "400000000"
                }]}]
            },
            {
                "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "app-compute"}}]},
                "scopeSpans": [{"spans": [{
                    "traceId": "t1", "spanId": "b", "parentSpanId": "a", "name": "price",
                    "startTimeUnixNano": "0", "endTimeUnixNano": "100000000"
                }]}]
            }
        ]
    })
}

fn sample_watts() -> HashMap<(String, String), f64> {
    HashMap::from([
        (("wattopus".into(), "app-gateway-abc".into()), 4.0),
        (("wattopus".into(), "app-compute-def".into()), 2.0),
        (("kube-system".into(), "coredns-xyz".into()), 1.0),
    ])
}

#[test]
fn parse_reads_spans() {
    let spans = parse(&otlp(), "http.route");
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].route, "/checkout");
    assert!(spans[0].root);
    assert_eq!(spans[1].service, "app-compute");
    assert!(!spans[1].root);
    assert!((spans[1].busy - 0.1).abs() < 1e-9);
}

#[test]
fn root_route_propagates() {
    let w = weights(&parse(&otlp(), "http.route"));
    assert!((w[&("app-compute".into(), "/checkout".into())] - 0.1).abs() < 1e-9);
}

#[test]
fn attribution_conserves_power() {
    let watts = sample_watts();
    let a = attribute(&weights(&parse(&otlp(), "http.route")), &watts);
    assert_eq!(a.unresolved, 0);
    assert!((a.route_watts["/checkout"] - 6.0).abs() < 1e-9);
    assert!((a.route_watts["_unattributed"] - 1.0).abs() < 1e-9);
    let total: f64 = a.route_watts.values().sum();
    let expected: f64 = watts.values().sum();
    assert!((total - expected).abs() < 1e-9);
}

#[test]
fn unmatched_service_counts_unresolved() {
    let watts = sample_watts();
    let mut w = HashMap::new();
    w.insert(("ghost".to_string(), "/x".to_string()), 1.0);
    let a = attribute(&w, &watts);
    assert_eq!(a.unresolved, 1);
    assert!((a.route_watts["_unattributed"] - 7.0).abs() < 1e-9);
}

#[test]
fn unattributed_detail_sums_to_bucket() {
    let a = attribute(&weights(&parse(&otlp(), "http.route")), &sample_watts());
    let detail: f64 = a.unattributed.values().sum();
    assert!((detail - a.route_watts["_unattributed"]).abs() < 1e-9);
    // the one unclaimed pod is identified, claimed ones are not listed
    assert!(a.unattributed.contains_key(&("kube-system".into(), "coredns-xyz".into())));
    assert_eq!(a.unattributed.len(), 1);
}

#[test]
fn services_report_claimed_pods_and_busy_seconds() {
    let a = attribute(&weights(&parse(&otlp(), "http.route")), &sample_watts());
    let (pods, busy) = a.services["app-gateway"];
    assert_eq!(pods, 1);
    assert!((busy - 0.4).abs() < 1e-9);
    let (pods, busy) = a.services["app-compute"];
    assert_eq!(pods, 1);
    assert!((busy - 0.1).abs() < 1e-9);
}
