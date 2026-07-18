use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use ingest::{Container, Deployment, Namespace, Node, Pod, Res, Service, Snapshot};
use k8s::Client;

const CPU_Q: &str = "sum by (namespace, pod, container) (rate(container_cpu_usage_seconds_total{container!=\"\"}[2m]))";
const RAM_Q: &str = "sum by (namespace, pod, container) (container_memory_working_set_bytes{container!=\"\"})";
const DISK_Q: &str = "sum by (namespace, pod, container) (container_fs_usage_bytes{container!=\"\"})";

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn prom_by_container(prom_url: &str, query: &str) -> HashMap<(String, String, String), f64> {
    let mut out = HashMap::new();
    let resp = ureq::get(&format!("{prom_url}/api/v1/query"))
        .query("query", query)
        .timeout(Duration::from_secs(10))
        .call();
    if let Ok(r) = resp {
        if let Ok(body) = r.into_json::<Value>() {
            for m in body["data"]["result"].as_array().unwrap_or(&vec![]) {
                let k = (
                    m["metric"]["namespace"].as_str().unwrap_or("").to_string(),
                    m["metric"]["pod"].as_str().unwrap_or("").to_string(),
                    m["metric"]["container"].as_str().unwrap_or("").to_string(),
                );
                if let Some(v) = m["value"][1].as_str().and_then(|s| s.parse::<f64>().ok()) {
                    out.insert(k, v);
                }
            }
        }
    }
    out
}

fn pod_watts(prom_url: &str, query: &str) -> HashMap<(String, String), f64> {
    let mut out = HashMap::new();
    let resp = ureq::get(&format!("{prom_url}/api/v1/query"))
        .query("query", query)
        .timeout(Duration::from_secs(10))
        .call();
    if let Ok(r) = resp {
        if let Ok(body) = r.into_json::<Value>() {
            for m in body["data"]["result"].as_array().unwrap_or(&vec![]) {
                let k = (
                    m["metric"]["namespace"].as_str().unwrap_or("").to_string(),
                    m["metric"]["pod"].as_str().unwrap_or("").to_string(),
                );
                if let Some(v) = m["value"][1].as_str().and_then(|s| s.parse::<f64>().ok()) {
                    out.insert(k, v);
                }
            }
        }
    }
    out
}

fn quantity(s: &str) -> f64 {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix("Ki") {
        (n, 1024.0)
    } else if let Some(n) = s.strip_suffix("Mi") {
        (n, 1024f64.powi(2))
    } else if let Some(n) = s.strip_suffix("Gi") {
        (n, 1024f64.powi(3))
    } else if let Some(n) = s.strip_suffix("Ti") {
        (n, 1024f64.powi(4))
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 0.001)
    } else {
        (s, 1.0)
    };
    num.parse::<f64>().unwrap_or(0.0) * mult
}

fn deployment_of(pod: &Value) -> String {
    for o in pod["metadata"]["ownerReferences"].as_array().unwrap_or(&vec![]) {
        if o["kind"] == "ReplicaSet" {
            let rs = o["name"].as_str().unwrap_or("");
            if let Some(idx) = rs.rfind('-') {
                return rs[..idx].to_string();
            }
        }
    }
    String::new()
}

fn snapshot(cli: &Client, prom_url: &str, power_query: &str, elapsed: f64) -> Result<Snapshot, String> {
    let cpu = prom_by_container(prom_url, CPU_Q);
    let ram = prom_by_container(prom_url, RAM_Q);
    let disk = prom_by_container(prom_url, DISK_Q);
    let watts = pod_watts(prom_url, power_query);

    let pod_list = cli.list("/api/v1", "pods")?;
    let node_list = cli.list("/api/v1", "nodes")?;
    let svc_list = cli.list("/api/v1", "services")?;
    let deploy_list = cli.list("/apis/apps/v1", "deployments")?;

    let mut containers = Vec::new();
    let mut pods = Vec::new();
    let mut pod_res: HashMap<(String, String), Res> = HashMap::new();
    let mut pod_node: HashMap<(String, String), String> = HashMap::new();
    let mut deploy_res: HashMap<(String, String), Res> = HashMap::new();
    let mut ns_res: HashMap<String, Res> = HashMap::new();

    for p in &pod_list {
        let ns = p["metadata"]["namespace"].as_str().unwrap_or("").to_string();
        let name = p["metadata"]["name"].as_str().unwrap_or("").to_string();
        let node = p["spec"]["nodeName"].as_str().unwrap_or("").to_string();
        let deployment = deployment_of(p);
        let mut total = Res {
            joules: watts.get(&(ns.clone(), name.clone())).copied().unwrap_or(0.0) * elapsed,
            ..Default::default()
        };
        for c in p["spec"]["containers"].as_array().unwrap_or(&vec![]) {
            let cname = c["name"].as_str().unwrap_or("").to_string();
            let key = (ns.clone(), name.clone(), cname.clone());
            let m = Res {
                cpu_usage: cpu.get(&key).copied().unwrap_or(0.0),
                ram_usage: ram.get(&key).copied().unwrap_or(0.0),
                disk_usage: disk.get(&key).copied().unwrap_or(0.0),
                ..Default::default()
            };
            containers.push(Container {
                namespace: ns.clone(),
                pod: name.clone(),
                name: cname,
                metrics: m,
            });
            total = total.add(m);
        }
        pod_res.insert((ns.clone(), name.clone()), total);
        pod_node.insert((ns.clone(), name.clone()), node.clone());
        pods.push(Pod {
            namespace: ns.clone(),
            name: name.clone(),
            knode: node,
            deployment: deployment.clone(),
            metrics: total,
        });
        if !deployment.is_empty() {
            let e = deploy_res.entry((ns.clone(), deployment)).or_default();
            *e = e.add(total);
        }
        let e = ns_res.entry(ns).or_default();
        *e = e.add(total);
    }

    let mut nodes = Vec::new();
    for n in &node_list {
        let nname = n["metadata"]["name"].as_str().unwrap_or("").to_string();
        let mut usage = Res::default();
        for ((ns, pod), m) in &pod_res {
            if pod_node.get(&(ns.clone(), pod.clone())) == Some(&nname) {
                usage = usage.add(*m);
            }
        }
        let alloc = &n["status"]["allocatable"];
        usage.cpu_available = quantity(alloc["cpu"].as_str().unwrap_or("0")) - usage.cpu_usage;
        usage.ram_available = quantity(alloc["memory"].as_str().unwrap_or("0")) - usage.ram_usage;
        usage.disk_available =
            quantity(alloc["ephemeral-storage"].as_str().unwrap_or("0")) - usage.disk_usage;
        nodes.push(Node { name: nname, metrics: usage });
    }

    let deployments: Vec<Deployment> = deploy_list
        .iter()
        .map(|d| {
            let ns = d["metadata"]["namespace"].as_str().unwrap_or("").to_string();
            let name = d["metadata"]["name"].as_str().unwrap_or("").to_string();
            let metrics = deploy_res.get(&(ns.clone(), name.clone())).copied().unwrap_or_default();
            Deployment {
                namespace: ns,
                name,
                replicas: d["spec"]["replicas"].as_i64().unwrap_or(0),
                metrics,
            }
        })
        .collect();

    let services: Vec<Service> = svc_list
        .iter()
        .map(|s| Service {
            namespace: s["metadata"]["namespace"].as_str().unwrap_or("").to_string(),
            name: s["metadata"]["name"].as_str().unwrap_or("").to_string(),
            deployment: s["spec"]["selector"]["app"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    let namespaces: Vec<Namespace> =
        ns_res.into_iter().map(|(name, metrics)| Namespace { name, metrics }).collect();

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    Ok(Snapshot { timestamp, nodes, namespaces, deployments, services, pods, containers })
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let prom_url = env_str("PROM_URL", "http://prometheus:9090");
    let greycat = env_str("GREYCAT_URL", "http://greycat:8080");
    let power_query = env_str("POWER_QUERY", "sum by (namespace, pod) (mockpower_pod_watts)");
    let interval = env_f64("INTERVAL", 30.0);

    let cli = Client::in_cluster().expect("kubernetes in-cluster client");
    log::info!("feeder: prom {prom_url}, greycat {greycat}");

    let mut last = Instant::now();
    loop {
        thread::sleep(Duration::from_secs_f64(interval));
        let elapsed = last.elapsed().as_secs_f64();
        last = Instant::now();

        match snapshot(&cli, &prom_url, &power_query, elapsed) {
            Ok(payload) => {
                let pods = payload.pods.len();
                let resp = ureq::post(&format!("{greycat}/twin::ingest"))
                    .set("Accept", "application/json")
                    .timeout(Duration::from_secs(10))
                    .send_json(serde_json::json!([payload]));
                match resp {
                    Ok(_) => log::info!("ingested {pods} pods"),
                    Err(e) => log::warn!("ingest failed: {e}"),
                }
            }
            Err(e) => log::warn!("snapshot failed: {e}"),
        }
    }
}
