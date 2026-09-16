//! Map JSON-RPC method calls onto `oryxis-cloud-tencent`.
//!
//! Tencent Cloud implements the discovery surface (credentials, discover,
//! transport-metadata query) plus `provider.cluster_kubeconfig` for TKE.
//! There is no `resolve_query` family (no dynamic-group analog) and no
//! transport operation here: instances are reached over plain SSH from
//! the app, and TKE clusters are served by the Kubernetes provider
//! once their kubeconfig is fetched.

use oryxis_cloud::CloudProvider;
use oryxis_cloud_tencent::TencentProvider;
use oryxis_plugin_protocol::{
    CloudError, ClusterKubeconfigParams, InitializeParams, InitializeResult, JsonRpcResponse,
    ProfileParams, ResolveQueryParams, SUPPORTED_PROTOCOL_VERSIONS, SupportedTransportsParams,
    cloud_error_to_rpc, error_codes, method, negotiate_version,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Provider id this plugin serves. Matches `CloudProvider::id()` and
/// `CloudProfile.provider`.
const PROVIDER_ID: &str = "tencent";

enum OpError {
    BadParams(String),
    Provider(CloudError),
}

pub async fn handle(method_name: &str, id: Value, params: Option<Value>) -> JsonRpcResponse {
    let result: Result<Value, OpError> = match method_name {
        method::INITIALIZE => initialize(params),
        method::TEST_CREDENTIALS => test_credentials(params).await,
        method::DISCOVER => discover(params).await,
        method::RESOLVE_QUERY => resolve_query(params).await,
        method::SUPPORTED_TRANSPORTS => supported_transports(params),
        method::CLUSTER_KUBECONFIG => cluster_kubeconfig(params).await,
        other => {
            return JsonRpcResponse::error(
                id,
                error_codes::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            );
        }
    };
    into_response(id, result)
}

fn into_response(id: Value, result: Result<Value, OpError>) -> JsonRpcResponse {
    match result {
        Ok(value) => JsonRpcResponse::success(id, value),
        Err(OpError::BadParams(msg)) => {
            JsonRpcResponse::error(id, error_codes::INVALID_PARAMS, msg)
        }
        Err(OpError::Provider(err)) => {
            JsonRpcResponse::error_with_data(id, cloud_error_to_rpc(&err))
        }
    }
}

fn parse_params<P: DeserializeOwned>(params: Option<Value>) -> Result<P, OpError> {
    serde_json::from_value(params.unwrap_or(Value::Null))
        .map_err(|e| OpError::BadParams(format!("invalid params: {e}")))
}

fn ok_value<T: Serialize>(value: T) -> Result<Value, OpError> {
    serde_json::to_value(value).map_err(|e| OpError::BadParams(format!("serialize result: {e}")))
}

fn initialize(params: Option<Value>) -> Result<Value, OpError> {
    let p: InitializeParams = parse_params(params)?;
    let protocol_version = negotiate_version(&p.supported_versions, SUPPORTED_PROTOCOL_VERSIONS)
        .ok_or_else(|| {
            OpError::BadParams(format!(
                "no common protocol version: host {:?}, plugin {:?}",
                p.supported_versions, SUPPORTED_PROTOCOL_VERSIONS
            ))
        })?;
    ok_value(InitializeResult {
        protocol_version,
        provider_id: PROVIDER_ID.to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: capabilities(),
    })
}

/// Method names this plugin implements. `resolve_query` is advertised so
/// the host can call it uniformly, though it always returns `Unsupported`.
fn capabilities() -> Vec<String> {
    vec![
        method::TEST_CREDENTIALS.to_string(),
        method::DISCOVER.to_string(),
        method::RESOLVE_QUERY.to_string(),
        method::SUPPORTED_TRANSPORTS.to_string(),
        method::CLUSTER_KUBECONFIG.to_string(),
    ]
}

async fn test_credentials(params: Option<Value>) -> Result<Value, OpError> {
    let p: ProfileParams = parse_params(params)?;
    TencentProvider::new()
        .test_credentials(&p.profile)
        .await
        .map_err(OpError::Provider)?;
    ok_value(())
}

async fn discover(params: Option<Value>) -> Result<Value, OpError> {
    let p: ProfileParams = parse_params(params)?;
    let result = TencentProvider::new()
        .discover(&p.profile)
        .await
        .map_err(OpError::Provider)?;
    ok_value(result)
}

async fn resolve_query(params: Option<Value>) -> Result<Value, OpError> {
    let p: ResolveQueryParams = parse_params(params)?;
    let hosts = TencentProvider::new()
        .resolve_query(&p.profile, &p.query)
        .await
        .map_err(OpError::Provider)?;
    ok_value(hosts)
}

fn supported_transports(params: Option<Value>) -> Result<Value, OpError> {
    let p: SupportedTransportsParams = parse_params(params)?;
    let transports = TencentProvider::new().supported_transports(p.resource_type);
    ok_value(transports)
}

async fn cluster_kubeconfig(params: Option<Value>) -> Result<Value, OpError> {
    let p: ClusterKubeconfigParams = parse_params(params)?;
    let yaml = TencentProvider::new()
        .cluster_kubeconfig(&p.profile, &p.cluster_id)
        .await
        .map_err(OpError::Provider)?;
    ok_value(yaml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initialize_returns_tencent_metadata() {
        let resp = handle(
            method::INITIALIZE,
            Value::from(1),
            Some(serde_json::json!({ "supported_versions": [1] })),
        )
        .await;
        assert!(resp.error.is_none(), "initialize errored: {:?}", resp.error);
        let init: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(init.provider_id, "tencent");
        assert!(
            init.capabilities
                .contains(&method::CLUSTER_KUBECONFIG.to_string())
        );
        assert_eq!(init.protocol_version, 1);
        assert!(init.capabilities.contains(&method::DISCOVER.to_string()));
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let resp = handle("provider.bogus", Value::from(1), None).await;
        assert_eq!(
            resp.error.expect("should reject").code,
            error_codes::METHOD_NOT_FOUND
        );
    }
}
