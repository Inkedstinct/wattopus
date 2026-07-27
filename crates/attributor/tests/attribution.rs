use std::collections::HashMap;

use attributor::{attribute, category_watts, parse, weights};
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

fn no_labels() -> HashMap<(String, String), String> {
    HashMap::new()
}

fn labels() -> HashMap<(String, String), String> {
    HashMap::from([
        (
            ("wattopus".into(), "prometheus-0".into()),
            "observability".into(),
        ),
        (
            ("wattopus".into(), "app-gateway-abc".into()),
            "application".into(),
        ),
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
    let a = attribute(
        &weights(&parse(&otlp(), "http.route")),
        &watts,
        &no_labels(),
    );
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
    let a = attribute(&w, &watts, &no_labels());
    assert_eq!(a.unresolved, 1);
    assert!((a.route_watts["_unattributed"] - 7.0).abs() < 1e-9);
}

#[test]
fn unattributed_detail_sums_to_bucket() {
    let a = attribute(
        &weights(&parse(&otlp(), "http.route")),
        &sample_watts(),
        &no_labels(),
    );
    let detail: f64 = a.unattributed.values().sum();
    assert!((detail - a.route_watts["_unattributed"]).abs() < 1e-9);
    // the one unclaimed pod is identified, claimed ones are not listed
    assert!(a
        .unattributed
        .contains_key(&("kube-system".into(), "coredns-xyz".into())));
    assert_eq!(a.unattributed.len(), 1);
}

#[test]
fn service_route_split_sums_to_routes() {
    let a = attribute(
        &weights(&parse(&otlp(), "http.route")),
        &sample_watts(),
        &no_labels(),
    );
    assert!(
        (a.service_route_watts[&("app-gateway".into(), "/checkout".into())] - 4.0).abs() < 1e-9
    );
    assert!(
        (a.service_route_watts[&("app-compute".into(), "/checkout".into())] - 2.0).abs() < 1e-9
    );
    let split: f64 = a.service_route_watts.values().sum();
    let routes: f64 = a.route_watts.values().sum::<f64>() - a.route_watts["_unattributed"];
    assert!((split - routes).abs() < 1e-9);
}

#[test]
fn services_report_claimed_pods_and_busy_seconds() {
    let a = attribute(
        &weights(&parse(&otlp(), "http.route")),
        &sample_watts(),
        &no_labels(),
    );
    let (pods, busy) = a.services["app-gateway"];
    assert_eq!(pods, 1);
    assert!((busy - 0.4).abs() < 1e-9);
    let (pods, busy) = a.services["app-compute"];
    assert_eq!(pods, 1);
    assert!((busy - 0.1).abs() < 1e-9);
}

#[test]
fn labeled_unclaimed_pod_bills_to_its_bucket() {
    let mut watts = sample_watts();
    watts.insert(("wattopus".into(), "prometheus-0".into()), 3.0);
    let a = attribute(&weights(&parse(&otlp(), "http.route")), &watts, &labels());
    assert!((a.route_watts["_observability"] - 3.0).abs() < 1e-9);
    // coredns has no label: still residual
    assert!((a.route_watts["_unattributed"] - 1.0).abs() < 1e-9);
    assert_eq!(a.unattributed.len(), 1);
    assert!(a
        .unattributed
        .contains_key(&("kube-system".into(), "coredns-xyz".into())));
    // conservation across real routes + _observability + _unattributed
    let total: f64 = a.route_watts.values().sum();
    assert!((total - watts.values().sum::<f64>()).abs() < 1e-9);
}

#[test]
fn traced_pod_label_is_ignored_on_route_axis() {
    let a = attribute(
        &weights(&parse(&otlp(), "http.route")),
        &sample_watts(),
        &labels(),
    );
    // gateway is claimed by traces: still billed to /checkout, no _application line
    assert!((a.route_watts["/checkout"] - 6.0).abs() < 1e-9);
    assert!(!a.route_watts.contains_key("_application"));
}

#[test]
fn category_axis_conserves_and_buckets() {
    let watts = sample_watts();
    let cw = category_watts(&watts, &labels());
    let total: f64 = cw.values().map(|(_, w)| w).sum();
    assert!((total - watts.values().sum::<f64>()).abs() < 1e-9);
    assert_eq!(
        cw[&("wattopus".into(), "app-gateway-abc".into())].0,
        "_application"
    );
    assert_eq!(
        cw[&("kube-system".into(), "coredns-xyz".into())].0,
        "_unlabeled"
    );
}

#[test]
fn traced_pod_appears_on_both_axes() {
    let watts = sample_watts();
    let a = attribute(&weights(&parse(&otlp(), "http.route")), &watts, &labels());
    let cw = category_watts(&watts, &labels());
    assert!((a.route_watts["/checkout"] - 6.0).abs() < 1e-9);
    let (bucket, w) = &cw[&("wattopus".into(), "app-gateway-abc".into())];
    assert_eq!(bucket, "_application");
    assert!((w - 4.0).abs() < 1e-9);
}
