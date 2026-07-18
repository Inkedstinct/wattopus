//! tiny in-cluster Kubernetes client: reads the mounted service-account token
//! and CA, talks to the apiserver 
use std::sync::Arc;

use serde_json::Value;

const TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
const CA_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";

pub struct Client {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl Client {
    /// builds from the in-cluster environment
    pub fn in_cluster() -> Result<Self, String> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST")
            .map_err(|_| "not in cluster: KUBERNETES_SERVICE_HOST unset".to_string())?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".into());
        let token = std::fs::read_to_string(TOKEN_PATH).map_err(|e| format!("token: {e}"))?;
        let ca = std::fs::read(CA_PATH).map_err(|e| format!("ca: {e}"))?;

        let mut roots = ureq::rustls::RootCertStore::empty();
        for c in rustls_pemfile::certs(&mut std::io::BufReader::new(&ca[..])).flatten() {
            let _ = roots.add(c);
        }
        let tls = ureq::rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        let agent = ureq::AgentBuilder::new().tls_config(Arc::new(tls)).build();
        Ok(Client { base: format!("https://{host}:{port}"), token: token.trim().to_string(), agent })
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        self.agent
            .get(&format!("{}{}", self.base, path))
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| format!("GET {path}: {e}"))?
            .into_json()
            .map_err(|e| format!("GET {path}: decode: {e}"))
    }

    pub fn list(&self, api: &str, kind_plural: &str) -> Result<Vec<Value>, String> {
        let body = self.get(&format!("{api}/{kind_plural}"))?;
        Ok(body["items"].as_array().cloned().unwrap_or_default())
    }

    pub fn patch_scale(&self, namespace: &str, deployment: &str, replicas: i64) -> Result<(), String> {
        let path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{deployment}/scale");
        let patch = serde_json::json!({"spec": {"replicas": replicas}});
        self.agent
            .request("PATCH", &format!("{}{}", self.base, path))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/merge-patch+json")
            .send_json(patch)
            .map_err(|e| format!("PATCH scale: {e}"))?;
        Ok(())
    }
}
