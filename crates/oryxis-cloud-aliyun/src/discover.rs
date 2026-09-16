//! ECS instance discovery: `aliyun ecs DescribeInstances`, parsed into the
//! `DiscoveredEc2` family shared with the AWS provider (individual-VM
//! import). Paginated with `MaxResults` / `NextToken` (the `PageNumber`
//! pair is deprecated by the API).

use serde::Deserialize;

use oryxis_cloud::{CloudError, DiscoveredEc2};

use crate::{AliyunConfig, run_aliyun};

/// Page size for `DescribeInstances`; 100 is the API maximum.
const PAGE_SIZE: &str = "100";
/// Upper bound on pages per discovery, so a server that keeps handing
/// back the same `NextToken` cannot spin the task forever.
const MAX_PAGES: usize = 200;

/// One `DescribeInstances` response page. The OpenAPI JSON nests the
/// list as `Instances.Instance[]`; only the fields we map are declared.
#[derive(Debug, Deserialize)]
struct DescribeInstances {
    #[serde(rename = "Instances", default)]
    instances: InstanceList,
    /// Empty (or absent) on the last page.
    #[serde(rename = "NextToken", default)]
    next_token: String,
}

#[derive(Debug, Default, Deserialize)]
struct InstanceList {
    #[serde(rename = "Instance", default)]
    instance: Vec<EcsInstance>,
}

#[derive(Debug, Deserialize)]
struct EcsInstance {
    #[serde(rename = "InstanceId")]
    instance_id: String,
    #[serde(rename = "InstanceName", default)]
    instance_name: String,
    #[serde(rename = "HostName", default)]
    host_name: String,
    /// `Running` / `Stopped` / `Starting` / `Stopping` / `Pending`.
    #[serde(rename = "Status", default)]
    status: String,
    #[serde(rename = "RegionId", default)]
    region_id: String,
    /// `linux` / `windows`.
    #[serde(rename = "OSType", default)]
    os_type: String,
    /// Classic public IPs (`PublicIpAddress.IpAddress[]`).
    #[serde(rename = "PublicIpAddress", default)]
    public_ip_address: IpAddressList,
    /// An elastic IP is reported separately from the classic public IP.
    #[serde(rename = "EipAddress", default)]
    eip_address: EipAddress,
    /// VPC private IPs (`VpcAttributes.PrivateIpAddress.IpAddress[]`).
    #[serde(rename = "VpcAttributes", default)]
    vpc_attributes: VpcAttributes,
    /// Classic-network private IPs, the pre-VPC shape.
    #[serde(rename = "InnerIpAddress", default)]
    inner_ip_address: IpAddressList,
}

#[derive(Debug, Default, Deserialize)]
struct IpAddressList {
    #[serde(rename = "IpAddress", default)]
    ip_address: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct EipAddress {
    #[serde(rename = "IpAddress", default)]
    ip_address: String,
}

#[derive(Debug, Default, Deserialize)]
struct VpcAttributes {
    #[serde(rename = "PrivateIpAddress", default)]
    private_ip_address: IpAddressList,
}

/// First non-blank entry of an address list.
fn first_ip(list: &IpAddressList) -> Option<String> {
    list.ip_address
        .iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

/// A blank string is treated as absent.
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The login user an Alibaba Cloud public image provisions: `root` on
/// every Linux image (an `ecs-user` login exists only when chosen at
/// creation, which the API does not report). Windows authenticates as
/// `Administrator` over RDP, not SSH, so no SSH user is inferred there.
/// The editor lets the user override either way.
fn default_username(os_type: &str) -> Option<String> {
    match os_type.trim().to_lowercase().as_str() {
        "linux" => Some("root".to_string()),
        _ => None,
    }
}

/// Parse one `DescribeInstances` page. Pure, so it is unit-tested against
/// fixture JSON without a live `aliyun`.
fn parse_page(json: &[u8]) -> Result<DescribeInstances, CloudError> {
    serde_json::from_slice(json)
        .map_err(|e| CloudError::Other(format!("parsing aliyun ecs JSON: {e}")))
}

fn map_instance(i: EcsInstance, fallback_region: Option<&str>) -> DiscoveredEc2 {
    // A classic public IP wins over an EIP when both are present (an
    // instance carries at most one of the two in practice).
    let public_ip = first_ip(&i.public_ip_address).or_else(|| non_empty(&i.eip_address.ip_address));
    let private_ip =
        first_ip(&i.vpc_attributes.private_ip_address).or_else(|| first_ip(&i.inner_ip_address));
    let region = non_empty(&i.region_id)
        .or_else(|| fallback_region.map(str::to_string))
        .unwrap_or_default();
    DiscoveredEc2 {
        instance_id: i.instance_id,
        region,
        name: non_empty(&i.instance_name).or_else(|| non_empty(&i.host_name)),
        public_dns: None,
        private_dns: None,
        public_ip,
        private_ip,
        // Normalize to the AWS provider's lowercase convention.
        state: i.status.trim().to_lowercase(),
        default_username: default_username(&i.os_type),
    }
}

/// Discover every ECS instance in the configured region (or the
/// profile's default region when none is configured).
pub(crate) async fn discover_instances(
    cfg: &AliyunConfig,
) -> Result<Vec<DiscoveredEc2>, CloudError> {
    let mut out = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut sub: Vec<&str> = vec!["ecs", "DescribeInstances", "--MaxResults", PAGE_SIZE];
        // `--region` picks the endpoint, and the CLI fills the API's own
        // required `RegionId` from the profile's region or that flag
        // (`openapi/invoker.go`, `request.RegionId = profile.RegionId`),
        // which is what makes a blank region work. A configured region is
        // passed as `--RegionId` too so the query names it explicitly.
        let region = cfg.region();
        if let Some(r) = region {
            sub.push("--RegionId");
            sub.push(r);
        }
        if let Some(t) = next_token.as_deref() {
            sub.push("--NextToken");
            sub.push(t);
        }
        let page = parse_page(&run_aliyun(cfg, &sub).await?)?;
        out.extend(
            page.instances
                .instance
                .into_iter()
                .map(|i| map_instance(i, region)),
        );
        if page.next_token.trim().is_empty() {
            break;
        }
        next_token = Some(page.next_token);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_vpc_instance_with_classic_public_ip() {
        let json = br#"{
          "TotalCount": 1,
          "NextToken": "",
          "RequestId": "x",
          "Instances": { "Instance": [ {
            "InstanceId": "i-bp67acfmxazb4p0000",
            "InstanceName": "web-1",
            "HostName": "iZbp67acfmxazb4pZ",
            "Status": "Running",
            "RegionId": "cn-hangzhou",
            "ZoneId": "cn-hangzhou-h",
            "OSName": "Alibaba Cloud Linux 3.2104 LTS 64 bit",
            "OSType": "linux",
            "PublicIpAddress": { "IpAddress": [ "47.96.1.2" ] },
            "EipAddress": { "IpAddress": "", "AllocationId": "" },
            "VpcAttributes": { "PrivateIpAddress": { "IpAddress": [ "172.16.0.10" ] }, "VpcId": "vpc-1" },
            "InnerIpAddress": { "IpAddress": [] }
          } ] }
        }"#;
        let page = parse_page(json).unwrap();
        assert!(page.next_token.is_empty());
        let hosts: Vec<_> = page
            .instances
            .instance
            .into_iter()
            .map(|i| map_instance(i, None))
            .collect();
        assert_eq!(hosts.len(), 1);
        let h = &hosts[0];
        assert_eq!(h.instance_id, "i-bp67acfmxazb4p0000");
        assert_eq!(h.name.as_deref(), Some("web-1"));
        assert_eq!(h.region, "cn-hangzhou");
        assert_eq!(h.state, "running");
        assert_eq!(h.public_ip.as_deref(), Some("47.96.1.2"));
        assert_eq!(h.private_ip.as_deref(), Some("172.16.0.10"));
        assert_eq!(h.default_username.as_deref(), Some("root"));
    }

    #[test]
    fn eip_fills_the_public_slot_when_no_classic_public_ip() {
        let json = br#"{
          "NextToken": "caeba0bb",
          "Instances": { "Instance": [ {
            "InstanceId": "i-2",
            "InstanceName": "",
            "HostName": "db-1",
            "Status": "Stopped",
            "RegionId": "",
            "OSType": "windows",
            "PublicIpAddress": { "IpAddress": [] },
            "EipAddress": { "IpAddress": "8.8.4.4" },
            "VpcAttributes": { "PrivateIpAddress": { "IpAddress": [] } },
            "InnerIpAddress": { "IpAddress": [ "10.0.0.5", "10.0.0.6" ] }
          } ] }
        }"#;
        let page = parse_page(json).unwrap();
        // A non-empty NextToken means another page follows.
        assert_eq!(page.next_token, "caeba0bb");
        let h = map_instance(
            page.instances.instance.into_iter().next().unwrap(),
            Some("cn-beijing"),
        );
        assert_eq!(h.public_ip.as_deref(), Some("8.8.4.4"));
        // Classic-network private IPs are the fallback; the first wins.
        assert_eq!(h.private_ip.as_deref(), Some("10.0.0.5"));
        // A blank InstanceName falls back to the host name.
        assert_eq!(h.name.as_deref(), Some("db-1"));
        // A blank RegionId takes the configured region.
        assert_eq!(h.region, "cn-beijing");
        assert_eq!(h.state, "stopped");
        // Windows: no SSH user inferred.
        assert_eq!(h.default_username, None);
    }

    #[test]
    fn empty_page_is_ok() {
        let page = parse_page(br#"{"TotalCount":0,"Instances":{"Instance":[]}}"#).unwrap();
        assert!(page.instances.instance.is_empty());
        assert!(page.next_token.is_empty());
    }
}
