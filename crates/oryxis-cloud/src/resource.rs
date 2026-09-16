use serde::{Deserialize, Serialize};

/// Result of a one-shot wizard discovery, the user picks a subset and
/// imports it. Discovered EC2s / VMs become individual hosts; discovered
/// ECS services / K8s workloads become dynamic groups; discovered GKE /
/// AKS clusters become Kubernetes accounts (get-credentials then a k8s
/// profile) the user can discover workloads in, and ACK / TKE clusters
/// do the same through a kubeconfig the provider returns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveryResult {
    pub ec2: Vec<DiscoveredEc2>,
    pub ecs_services: Vec<DiscoveredEcsService>,
    pub k8s_workloads: Vec<DiscoveredK8sWorkload>,
    /// GKE (managed Kubernetes) clusters, populated by the GCP provider.
    /// `#[serde(default)]` so an older host / a non-GCP plugin that never
    /// sends this field still deserializes, and an older host ignores it
    /// from a newer plugin (same forward/back rule as the sync payloads).
    #[serde(default)]
    pub gke_clusters: Vec<DiscoveredGkeCluster>,
    /// AKS (managed Kubernetes) clusters, populated by the Azure provider.
    /// Same managed-cluster flow as GKE (get-credentials then a k8s
    /// profile); `#[serde(default)]` for the same forward/back reason.
    #[serde(default)]
    pub aks_clusters: Vec<DiscoveredAksCluster>,
    /// Managed Kubernetes clusters whose provider RETURNS the kubeconfig
    /// instead of writing it (ACK on Alibaba Cloud, TKE on Tencent Cloud;
    /// see [`DiscoveredManagedCluster`]). One list for every such family,
    /// so a new provider of this shape adds an entry, not a section.
    /// `#[serde(default)]` for the same forward/back reason.
    #[serde(default)]
    pub managed_clusters: Vec<DiscoveredManagedCluster>,
}

/// A GKE (managed Kubernetes) cluster surfaced by GCP discovery. Not a
/// host and not a workload: "adding" it runs
/// `gcloud container clusters get-credentials` and creates a Kubernetes
/// account (profile) pointed at the resulting kubeconfig context, after
/// which the normal K8s workload discovery / dynamic-group flow applies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredGkeCluster {
    /// Cluster name (the handle `get-credentials` takes).
    pub name: String,
    /// Region or zone the cluster lives in (`--location`).
    pub location: String,
    /// `RUNNING`, `PROVISIONING`, ... as the provider emits it.
    pub status: String,
    /// Total node count across the cluster's node pools.
    pub node_count: u32,
    /// The kubeconfig context `get-credentials` will create
    /// (`gke_<project>_<location>_<name>`), so the UI can dup-check
    /// against existing k8s profiles before adding.
    pub context: String,
}

/// An AKS (managed Kubernetes) cluster surfaced by Azure discovery. Like
/// [`DiscoveredGkeCluster`], "adding" it runs
/// `az aks get-credentials --resource-group <rg> --name <name>` and
/// creates a Kubernetes account (profile) pointed at the resulting
/// kubeconfig context, after which the normal K8s workload discovery /
/// dynamic-group flow applies. AKS needs the `resource_group` (not a
/// location) to fetch credentials, hence its own struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredAksCluster {
    /// Cluster name (the `--name` handle `get-credentials` takes).
    pub name: String,
    /// Resource group the cluster lives in (the `--resource-group`
    /// handle `get-credentials` takes).
    pub resource_group: String,
    /// Azure region (`location`), informational.
    pub location: String,
    /// `Running`, `Stopped`, ... as the provider emits it.
    pub status: String,
    /// Total node count across the cluster's agent pools.
    pub node_count: u32,
    /// The kubeconfig context `get-credentials` will create (the cluster
    /// name, which is what `az aks get-credentials` writes by default),
    /// so the UI can dup-check against existing k8s profiles before adding.
    pub context: String,
}

/// A managed Kubernetes cluster whose provider hands back the kubeconfig
/// as CONTENT (`provider.cluster_kubeconfig`) rather than merging it into
/// `~/.kube/config` the way `gcloud` / `az` do. Alibaba Cloud's ACK
/// (`DescribeClusterUserKubeconfig`) and Tencent Cloud's TKE
/// (`DescribeClusterKubeconfig`) are both this shape. "Adding" one writes
/// the returned YAML to a file of its own under the app's data directory
/// and creates a Kubernetes account pointed at that file, after which
/// the normal K8s workload discovery / dynamic-group flow applies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredManagedCluster {
    /// Product family the cluster belongs to: `"ack"` or `"tke"`. Drives
    /// the section header and the account label; an app older than a
    /// plugin may see a value it does not know and falls back to
    /// showing it verbatim.
    pub family: String,
    /// Cluster id, the handle `cluster_kubeconfig` takes
    /// (`c3fb96524f9274b4...` on ACK, `cls-abc12345` on TKE).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Region the cluster lives in, informational.
    pub region: String,
    /// `running`, `Running`, `Creating`, ... as the provider emits it.
    pub status: String,
    /// Kubernetes version as the provider reports it, informational.
    #[serde(default)]
    pub version: String,
    /// Node count as the provider reports it.
    #[serde(default)]
    pub node_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredEc2 {
    pub instance_id: String,
    pub region: String,
    pub name: Option<String>,
    pub public_dns: Option<String>,
    pub private_dns: Option<String>,
    pub public_ip: Option<String>,
    pub private_ip: Option<String>,
    pub state: String,
    /// Default username inferred from the AMI when known (`ec2-user`,
    /// `ubuntu`, `admin`…). The editor lets the user override.
    pub default_username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredEcsService {
    pub region: String,
    pub cluster: String,
    pub service: String,
    pub container: String,
    /// Number of currently running tasks, purely informational, shown
    /// in the wizard so the user can tell empty services from active ones.
    pub running_task_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredK8sWorkload {
    pub context: String,
    pub namespace: String,
    /// `Deployment` / `StatefulSet` / `DaemonSet`.
    pub kind: String,
    pub name: String,
    pub container: String,
    pub running_pod_count: u32,
    /// `spec.selector.matchLabels` of the workload. Import turns this into
    /// a `PodSelector::Labels`, which resolves to the workload's pods with a
    /// single `kubectl get pods -l ...` call regardless of workload kind.
    #[serde(default)]
    pub match_labels: std::collections::BTreeMap<String, String>,
}

/// Resolved live host returned by `resolve_query`, used by dynamic
/// groups to render their current children.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredHost {
    /// Display label rendered in the sidebar tree.
    pub label: String,
    /// Identifier the transport needs to actually connect (taskId for
    /// ECS, podName for K8s, instance-id for EC2).
    pub resource_id: String,
    /// Optional per-child overrides surfaced by the provider (zone,
    /// node, etc.), shown as a subtitle on the row.
    pub subtitle: Option<String>,
    /// Container name when this host represents a container inside a
    /// task. None for non-ECS / non-K8s resources. ECS resolve fills
    /// this with the container chosen at import; future iterations may
    /// expand a multi-container task into N rows.
    #[serde(default)]
    pub container_name: Option<String>,
    /// Task definition `family:revision` for ECS resources (e.g.
    /// `my-app:42`). None for non-ECS / when DescribeTaskDefinition
    /// failed.
    #[serde(default)]
    pub task_definition: Option<String>,
    /// Upstream lifecycle status (ECS LastStatus, K8s pod phase):
    /// `RUNNING`, `PENDING`, `STOPPED`, etc. Drives the colour of the
    /// status pill in the row.
    #[serde(default)]
    pub status: Option<String>,
    /// When the resource entered its current running state. Rendered
    /// as a relative timestamp (`2h ago`).
    #[serde(default)]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Private IPv4 surfaced inline (split out of `subtitle` so the
    /// view can render it as its own column / chip).
    #[serde(default)]
    pub private_ip: Option<String>,
    /// Availability zone / node name (`us-east-1a`, `ip-10-0-1-23`).
    #[serde(default)]
    pub availability_zone: Option<String>,
    /// AWS region (or K8s context-region equivalent) the resolver
    /// found this resource in. Exposed so UI actions like
    /// "copy `aws ecs execute-command`" can fill the `--region`
    /// flag without re-deriving it from the profile config.
    #[serde(default)]
    pub region: Option<String>,
}
