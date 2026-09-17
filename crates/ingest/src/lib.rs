//! the data contract between the Rust components and the GreyCat twin.
//! these structs mirror the `In*` types in greycat/server/twin.gcl and the
//! `simulate_scale` return.
//!
//! evolution rule: additive only. new fields nullable on the GCL side
//! version the function (twin::ingest_v2) on a hard break.

use serde::{Deserialize, Serialize};

/// the curret seven metrics carried for every entity, matching GCL `Res`/`InRes`.
/// TODO : Check maintainability
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Res {
    pub cpu_usage: f64,
    pub cpu_available: f64,
    pub ram_usage: f64,
    pub ram_available: f64,
    pub disk_usage: f64,
    pub disk_available: f64,
    pub joules: f64,
}

impl Res {
    pub fn addition(self, o: Res) -> Res {
        Res {
            cpu_usage: self.cpu_usage + o.cpu_usage,
            cpu_available: self.cpu_available + o.cpu_available,
            ram_usage: self.ram_usage + o.ram_usage,
            ram_available: self.ram_available + o.ram_available,
            disk_usage: self.disk_usage + o.disk_usage,
            disk_available: self.disk_available + o.disk_available,
            joules: self.joules + o.joules,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub metrics: Res,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Namespace {
    pub name: String,
    pub metrics: Res,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Deployment {
    pub namespace: String,
    pub name: String,
    pub replicas: i64,
    pub metrics: Res,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Service {
    pub namespace: String,
    pub name: String,
    pub deployment: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pod {
    pub namespace: String,
    pub name: String,
    pub knode: String,
    pub deployment: String,
    pub metrics: Res,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Container {
    pub namespace: String,
    pub pod: String,
    pub name: String,
    pub metrics: Res,
}

/// the payload POSTed (wrapped in a one-element array) to twin::ingest
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub timestamp: u64,
    pub nodes: Vec<Node>,
    pub namespaces: Vec<Namespace>,
    pub deployments: Vec<Deployment>,
    pub services: Vec<Service>,
    pub pods: Vec<Pod>,
    pub containers: Vec<Container>,
}

/// what twin::simulate_scale returns
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ScaleSimulation {
    pub current_replicas: f64,
    pub target_replicas: f64,
    pub cpu_per_pod_now: f64,
    pub cpu_per_pod_predicted: f64,
    pub joules_per_interval_now: f64,
    pub joules_per_interval_predicted: f64,
}

/// twin::deployments item: what the twin currently knows to predict for
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeployRef {
    pub namespace: String,
    pub name: String,
}

/// twin::latest return: the newest ingested sample of one deployment
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LatestSample {
    pub timestamp: i64,
    pub cpu_usage: f64,
    pub joules: f64,
}

/// the payload POSTed (wrapped in a one-element array) to
/// twin::ingest_prediction
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    pub namespace: String,
    pub name: String,
    pub timestamp: i64,
    pub cpu_predicted: f64,
    pub joules_predicted: f64,
    pub quiescent: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RouteObservation {
    pub name: String,
    pub rps: f64,
    pub watts_billed: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceObservation {
    pub name: String,
    pub watts: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RouteSnapshot {
    pub timestamp: u64,
    pub namespace: String,
    pub routes: Vec<RouteObservation>,
    pub services: Vec<ServiceObservation>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RouteCoef {
    pub route: String,
    pub watts_per_rps: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PowerModel {
    pub namespace: String,
    pub service: String,
    pub fitted_at: i64,
    pub intercept: f64,
    pub coefs: Vec<RouteCoef>,
    pub r2: f64,
    pub samples: i64,
}

/// long format: one row per (timestamp, service, route)
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryRow {
    pub timestamp: i64,
    pub namespace: String,
    pub service: String,
    pub service_watts: f64,
    pub route: String,
    pub rps: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServicePrediction {
    pub service: String,
    pub deployment: String,
    pub watts_now: f64,
    pub watts_predicted: f64,
}

/// what twin::simulate_load returns
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadSimulation {
    pub namespace: String,
    pub route: String,
    pub rps: f64,
    pub total_watts: f64,
    pub services: Vec<ServicePrediction>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the golden "fixture" from "ingest.sample.json"
    #[test]
    fn snapshot_matches_golden_fixture() {
        let raw = include_str!("../../../schema/ingest.sample.json");
        let snap: Snapshot =
            serde_json::from_str(raw).expect("fixture must deserialize into Snapshot");

        // re-serialize and compare as values to see in the other direction of changes
        let reserialized = serde_json::to_value(&snap).unwrap();
        let original: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            reserialized, original,
            "struct and fixture disagree on shape"
        );
    }

    /// workaround because greycat wraps typed objects with a _type key and serde must ignore it
    #[test]
    fn simulation_tolerates_greycat_type_envelope() {
        let raw = r#"{"_type":"twin.ScaleSimulation","current_replicas":2.0,
            "target_replicas":1.0,"cpu_per_pod_now":0.1,"cpu_per_pod_predicted":0.2,
            "joules_per_interval_now":30.0,"joules_per_interval_predicted":15.0}"#;
        let sim: ScaleSimulation = serde_json::from_str(raw).unwrap();
        assert!((sim.cpu_per_pod_predicted - 0.2).abs() < 1e-12);
    }

    /// twin::latest returns this _type too
    #[test]
    fn latest_tolerates_greycat_type_envelope() {
        let raw = r#"{"_type":"twin.LatestSample","timestamp":1730800000,
            "cpu_usage":0.2,"joules":30.0}"#;
        let s: LatestSample = serde_json::from_str(raw).unwrap();
        assert_eq!(s.timestamp, 1730800000);
    }

    #[test]
    fn prediction_roundtrips() {
        let p = Prediction {
            namespace: "wattopus".into(),
            name: "app-store".into(),
            timestamp: 1730800030,
            cpu_predicted: 0.2,
            joules_predicted: 28.0,
            quiescent: true,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Prediction>(&json).unwrap(), p);
    }

    #[test]
    fn simulation_roundtrips() {
        let sim = ScaleSimulation {
            current_replicas: 2.0,
            target_replicas: 1.0,
            cpu_per_pod_now: 0.3,
            cpu_per_pod_predicted: 0.6,
            joules_per_interval_now: 120.0,
            joules_per_interval_predicted: 60.0,
        };
        let json = serde_json::to_string(&sim).unwrap();
        assert_eq!(serde_json::from_str::<ScaleSimulation>(&json).unwrap(), sim);
    }

    /// the "golden" fixture
    #[test]
    fn route_snapshot_matches_golden_fixture() {
        let raw = include_str!("../../../schema/routes.sample.json");
        let snap: RouteSnapshot =
            serde_json::from_str(raw).expect("fixture must deserialize into RouteSnapshot");
        let reserialized = serde_json::to_value(&snap).unwrap();
        let original: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            reserialized, original,
            "struct and fixture disagree on shape"
        );
    }

    #[test]
    fn power_model_tolerates_greycat_type_envelope() {
        let raw = r#"{"_type":"twin.PowerModel","namespace":"wattopus","service":"app-compute",
            "fitted_at":1730800030,"intercept":0.4,
            "coefs":[{"_type":"twin.RouteCoef","route":"/checkout","watts_per_rps":0.12}],
            "r2":0.93,"samples":120}"#;
        let m: PowerModel = serde_json::from_str(raw).unwrap();
        assert_eq!(m.coefs.len(), 1);
        assert!((m.coefs[0].watts_per_rps - 0.12).abs() < 1e-12);
    }

    #[test]
    fn load_simulation_roundtrips() {
        let sim = LoadSimulation {
            namespace: "wattopus".into(),
            route: "/checkout".into(),
            rps: 5.0,
            total_watts: 3.5,
            services: vec![ServicePrediction {
                service: "app-compute".into(),
                deployment: "app-compute".into(),
                watts_now: 1.0,
                watts_predicted: 2.0,
            }],
        };
        let json = serde_json::to_string(&sim).unwrap();
        assert_eq!(serde_json::from_str::<LoadSimulation>(&json).unwrap(), sim);
    }
}
