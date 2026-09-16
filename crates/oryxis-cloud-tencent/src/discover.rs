//! CVM instance discovery: `tccli cvm DescribeInstances --output json`,
//! parsed into the `DiscoveredEc2` family shared with the AWS provider
//! (individual-VM import). Paginated with `Offset` / `Limit` against
//! `TotalCount`.

use serde::Deserialize;

use oryxis_cloud::{CloudError, DiscoveredEc2};

use crate::{TencentConfig, run_tccli};

/// Page size for `DescribeInstances`; 100 is the API maximum.
const PAGE_SIZE: usize = 100;
/// Upper bound on pages per discovery, so a `TotalCount` that never
/// reconciles with the pages served cannot spin the task forever.
const MAX_PAGES: usize = 200;

/// One `DescribeInstances` response page. With `--output json` tccli
/// prints the response fields flat (no `Response` envelope); only the
/// fields we map are declared.
#[derive(Debug, Deserialize)]
struct DescribeInstances {
    #[serde(rename = "TotalCount", default)]
    total_count: usize,
    #[serde(rename = "InstanceSet", default)]
    instance_set: Vec<CvmInstance>,
}

#[derive(Debug, Deserialize)]
struct CvmInstance {
    #[serde(rename = "InstanceId")]
    instance_id: String,
    #[serde(rename = "InstanceName", default)]
    instance_name: String,
    /// `RUNNING` / `STOPPED` / `STARTING` / `STOPPING` / `PENDING` / ...
    #[serde(rename = "InstanceState", default)]
    instance_state: String,
    /// Operating system name (`Ubuntu Server 22.04 LTS 64bit`,
    /// `TencentOS Server 3.1`, `Windows Server 2022 ...`).
    #[serde(rename = "OsName", default)]
    os_name: String,
    /// May be JSON `null` for an instance with no public IP, hence the
    /// `Option` around the list.
    #[serde(rename = "PublicIpAddresses", default)]
    public_ip_addresses: Option<Vec<String>>,
    #[serde(rename = "PrivateIpAddresses", default)]
    private_ip_addresses: Option<Vec<String>>,
    #[serde(rename = "Placement", default)]
    placement: Placement,
}

#[derive(Debug, Default, Deserialize)]
struct Placement {
    /// Availability zone (`ap-guangzhou-3`).
    #[serde(rename = "Zone", default)]
    zone: String,
}

/// First non-blank entry of an optional address list.
fn first_ip(list: &Option<Vec<String>>) -> Option<String> {
    list.as_ref()?
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

/// The region a zone belongs to: `ap-guangzhou-3` -> `ap-guangzhou`.
/// Tencent zones are always the region plus a trailing `-<n>`; a zone
/// with no numeric suffix is returned as-is.
fn region_of_zone(zone: &str) -> Option<String> {
    let z = zone.trim();
    if z.is_empty() {
        return None;
    }
    match z.rsplit_once('-') {
        Some((region, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            Some(region.to_string())
        }
        _ => Some(z.to_string()),
    }
}

/// The login user a Tencent Cloud public image provisions: `ubuntu` on
/// Ubuntu images, `root` on every other Linux image (TencentOS, CentOS,
/// Debian, openSUSE, ...). Windows authenticates as `Administrator`
/// over RDP, not SSH, so no SSH user is inferred there. The editor lets
/// the user override either way.
fn default_username(os_name: &str) -> Option<String> {
    let s = os_name.to_lowercase();
    if s.contains("windows") {
        None
    } else if s.contains("ubuntu") {
        Some("ubuntu".to_string())
    } else {
        Some("root".to_string())
    }
}

/// Parse one `DescribeInstances` page. Pure, so it is unit-tested against
/// fixture JSON without a live `tccli`.
fn parse_page(json: &[u8]) -> Result<DescribeInstances, CloudError> {
    serde_json::from_slice(json)
        .map_err(|e| CloudError::Other(format!("parsing tccli cvm JSON: {e}")))
}

fn map_instance(i: CvmInstance, configured_region: Option<&str>) -> DiscoveredEc2 {
    // The configured region is authoritative when set (the listing was
    // scoped to it); otherwise the zone names the profile's default
    // region.
    let region = configured_region
        .map(str::to_string)
        .or_else(|| region_of_zone(&i.placement.zone))
        .unwrap_or_default();
    DiscoveredEc2 {
        instance_id: i.instance_id,
        region,
        name: non_empty(&i.instance_name),
        public_dns: None,
        private_dns: None,
        public_ip: first_ip(&i.public_ip_addresses),
        private_ip: first_ip(&i.private_ip_addresses),
        // Normalize `RUNNING` to the AWS provider's lowercase convention.
        state: i.instance_state.trim().to_lowercase(),
        default_username: default_username(&i.os_name),
    }
}

/// Discover every CVM instance in the configured region (or the
/// profile's default region when none is configured).
pub(crate) async fn discover_instances(
    cfg: &TencentConfig,
) -> Result<Vec<DiscoveredEc2>, CloudError> {
    let mut out = Vec::new();
    let limit = PAGE_SIZE.to_string();
    let region = cfg.region();
    for page in 0..MAX_PAGES {
        let offset = (page * PAGE_SIZE).to_string();
        let bytes = run_tccli(
            cfg,
            &[
                "cvm",
                "DescribeInstances",
                "--Limit",
                &limit,
                "--Offset",
                &offset,
            ],
        )
        .await?;
        let page = parse_page(&bytes)?;
        let got = page.instance_set.len();
        out.extend(
            page.instance_set
                .into_iter()
                .map(|i| map_instance(i, region)),
        );
        // Stop on the last page: everything counted is in, or a short /
        // empty page says the count was stale.
        if got == 0 || got < PAGE_SIZE || out.len() >= page.total_count {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_of_zone_strips_the_zone_index() {
        assert_eq!(
            region_of_zone("ap-guangzhou-3").as_deref(),
            Some("ap-guangzhou")
        );
        assert_eq!(
            region_of_zone("na-siliconvalley-1").as_deref(),
            Some("na-siliconvalley")
        );
        // No numeric suffix: returned as-is rather than truncated.
        assert_eq!(
            region_of_zone("ap-guangzhou").as_deref(),
            Some("ap-guangzhou")
        );
        assert_eq!(region_of_zone(""), None);
    }

    #[test]
    fn default_username_follows_the_image_family() {
        assert_eq!(
            default_username("Ubuntu Server 22.04 LTS 64bit").as_deref(),
            Some("ubuntu")
        );
        assert_eq!(
            default_username("TencentOS Server 3.1 (TK4)").as_deref(),
            Some("root")
        );
        assert_eq!(
            default_username("CentOS 7.9 64bit").as_deref(),
            Some("root")
        );
        assert_eq!(
            default_username("Windows Server 2022 DataCenter 64bit CN"),
            None
        );
    }

    #[test]
    fn parses_a_running_instance_with_both_ips() {
        let json = br#"{
          "TotalCount": 1,
          "InstanceSet": [ {
            "Placement": { "Zone": "ap-guangzhou-3", "ProjectId": 0 },
            "InstanceId": "ins-r8hr2upy",
            "InstanceType": "S5.MEDIUM4",
            "InstanceName": "web-1",
            "InstanceState": "RUNNING",
            "OsName": "Ubuntu Server 22.04 LTS 64bit",
            "PrivateIpAddresses": [ "10.0.0.8" ],
            "PublicIpAddresses": [ "43.129.1.2" ]
          } ],
          "RequestId": "x"
        }"#;
        let page = parse_page(json).unwrap();
        assert_eq!(page.total_count, 1);
        let h = map_instance(page.instance_set.into_iter().next().unwrap(), None);
        assert_eq!(h.instance_id, "ins-r8hr2upy");
        assert_eq!(h.name.as_deref(), Some("web-1"));
        // No configured region: derived from the zone.
        assert_eq!(h.region, "ap-guangzhou");
        assert_eq!(h.state, "running");
        assert_eq!(h.public_ip.as_deref(), Some("43.129.1.2"));
        assert_eq!(h.private_ip.as_deref(), Some("10.0.0.8"));
        assert_eq!(h.default_username.as_deref(), Some("ubuntu"));
    }

    #[test]
    fn null_public_ips_and_windows_are_handled() {
        let json = br#"{
          "TotalCount": 1,
          "InstanceSet": [ {
            "Placement": { "Zone": "ap-singapore-1" },
            "InstanceId": "ins-2",
            "InstanceName": "",
            "InstanceState": "STOPPED",
            "OsName": "Windows Server 2022 DataCenter 64bit",
            "PrivateIpAddresses": [ "10.0.1.2", "10.0.1.3" ],
            "PublicIpAddresses": null
          } ]
        }"#;
        let page = parse_page(json).unwrap();
        let h = map_instance(
            page.instance_set.into_iter().next().unwrap(),
            Some("ap-singapore"),
        );
        assert_eq!(h.public_ip, None);
        assert_eq!(h.private_ip.as_deref(), Some("10.0.1.2"));
        assert_eq!(h.name, None);
        assert_eq!(h.region, "ap-singapore");
        assert_eq!(h.state, "stopped");
        assert_eq!(h.default_username, None);
    }

    #[test]
    fn empty_page_is_ok() {
        let page = parse_page(br#"{"TotalCount":0,"InstanceSet":[],"RequestId":"x"}"#).unwrap();
        assert!(page.instance_set.is_empty());
    }
}
