use crate::version::Version;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/zenaurora/starter/releases/latest";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        url: String,
    },
    Failed(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Self::Idle => "尚未检查更新".into(),
            Self::Checking => "正在检查更新…".into(),
            Self::UpToDate => "已是最新版本".into(),
            Self::Available { version, .. } => format!("发现新版本 {version}"),
            Self::Failed(error) => format!("检查失败：{error}"),
        }
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Runs on a background thread. Checking never downloads or replaces the executable.
pub fn check() -> Result<Status> {
    check_at(LATEST_RELEASE_API, CURRENT_VERSION)
}

fn check_at(endpoint: &str, current: &str) -> Result<Status> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let mut response = agent
        .get(endpoint)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("Starter/", env!("CARGO_PKG_VERSION")))
        .call()
        .context("无法连接 GitHub，请稍后重试")?;
    let body = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .context("无法读取版本信息")?;
    release_status(&body, current)
}

fn release_status(body: &str, current: &str) -> Result<Status> {
    let release: Release = serde_json::from_str(body).context("GitHub 返回了无效的版本信息")?;
    let version = Version::parse(&release.tag_name).context("发布版本号格式不正确")?;
    let current = Version::parse(current).context("当前版本号格式不正确")?;
    if release.draft || release.prerelease || version.is_prerelease() || version <= current {
        return Ok(Status::UpToDate);
    }
    Ok(Status::Available {
        version: version.to_string(),
        url: format!(
            "https://github.com/zenaurora/starter/releases/tag/{}",
            release.tag_name
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[test]
    fn compares_semantic_versions_and_ignores_unstable_releases() {
        for (tag, prerelease, available) in [
            ("v0.1.0", false, false),
            ("v0.0.9", false, false),
            ("v0.10.0", false, true),
            ("v0.2.0-rc1", false, false),
            ("v0.2.0", true, false),
        ] {
            let body = serde_json::json!({"tag_name": tag, "prerelease": prerelease}).to_string();
            assert_eq!(
                matches!(
                    release_status(&body, "0.1.0").unwrap(),
                    Status::Available { .. }
                ),
                available
            );
        }
        assert!(release_status(r#"{"tag_name":"invalid"}"#, "0.1.0").is_err());
        assert!(release_status("not json", "0.1.0").is_err());
    }

    #[test]
    fn checks_http_response_and_reports_network_errors() {
        for (code, body, success) in [
            ("200 OK", r#"{"tag_name":"v0.2.0"}"#, true),
            ("403 Forbidden", "rate limited", false),
            ("200 OK", "invalid", false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/latest", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let size = stream.read(&mut request).unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..size])
                        .to_lowercase()
                        .contains("user-agent: starter/")
                );
                write!(
                    stream,
                    "HTTP/1.1 {code}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            let result = check_at(&endpoint, "0.1.0");
            server.join().unwrap();
            assert_eq!(result.is_ok(), success);
            if success {
                assert_eq!(
                    result.unwrap(),
                    Status::Available {
                        version: "0.2.0".into(),
                        url: "https://github.com/zenaurora/starter/releases/tag/v0.2.0".into(),
                    }
                );
            }
        }
    }
}
