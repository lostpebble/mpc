pub mod secret_storage;
pub mod triple_storage;

/// Configures storage.
#[derive(Debug, Clone, clap::Parser)]
#[group(id = "storage_options")]
pub struct Options {
    /// GCP project ID.
    #[clap(long, env("MPC_RECOVERY_GCP_PROJECT_ID"))]
    pub gcp_project_id: Option<String>,
    /// GCP Secret Manager ID that will be used to load/store the node's secret key share.
    #[clap(long, env("MPC_RECOVERY_SK_SHARE_SECRET_ID"), requires_all=["gcp_project_id"])]
    pub sk_share_secret_id: Option<String>,
    /// Mostly for integration tests.
    /// GCP Datastore URL that will be used to load/store the node's triples and presignatures.
    #[arg(long, env("MPC_RECOVERY_GCP_DATASTORE_URL"))]
    pub gcp_datastore_url: Option<String>,
    /// env used to suffix datastore table names to differentiate among environments.
    #[clap(long, env("MPC_RECOVERY_ENV"))]
    pub env: Option<String>,
}

impl Options {
    pub fn into_str_args(self) -> Vec<String> {
        let mut opts = Vec::new();

        if let Some(gcp_project_id) = self.gcp_project_id {
            opts.extend(vec!["--gcp-project-id".to_string(), gcp_project_id]);
        }
        if let Some(sk_share_secret_id) = self.sk_share_secret_id {
            opts.extend(vec!["--sk-share-secret-id".to_string(), sk_share_secret_id]);
        }
        if let Some(gcp_datastore_url) = self.gcp_datastore_url {
            opts.extend(vec!["--gcp-datastore-url".to_string(), gcp_datastore_url]);
        }
        if let Some(env) = self.env {
            opts.extend(vec!["--env".to_string(), env]);
        }

        opts
    }
}