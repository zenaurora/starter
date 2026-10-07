mod install;

use crate::version::Version;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub use install::{acknowledge_startup, run_helper, take_report};
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/zenaurora/starter/releases/latest";
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        url: String,
        assets: Vec<Asset>,
    },
    Downloading {
        received: u64,
        total: u64,
    },
    Preparing,
    Installing,
    Cancelled,
    Updated(String),
    Failed(String),
}

impl Status {
    pub fn busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Preparing | Self::Installing
        )
    }
    pub fn label(&self) -> String {
        match self {
            Self::Idle => "尚未检查更新".into(),
            Self::Checking => "正在检查更新…".into(),
            Self::UpToDate => "已是最新版本".into(),
            Self::Available { version, .. } => format!("发现新版本 {version}"),
            Self::Downloading { total: 0, .. } => "正在准备下载…".into(),
            Self::Downloading { received, total } => format!(
                "正在下载… {}%（{:.1} / {:.1} MiB）",
                received * 100 / total.max(&1),
                *received as f64 / 1048576.,
                *total as f64 / 1048576.
            ),
            Self::Preparing => "下载已校验，正在准备安装…".into(),
            Self::Cancelled => "下载已取消，可重新更新".into(),
            Self::Installing => "正在安装，即将重新启动…".into(),
            Self::Updated(version) => format!("已更新到 {version}"),
            Self::Failed(error) => format!("更新失败：{error}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

pub fn check() -> Result<Status> {
    check_at(LATEST_RELEASE_API, CURRENT_VERSION)
}

fn agent(timeout: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout)))
        .build()
        .into()
}

fn check_at(endpoint: &str, current: &str) -> Result<Status> {
    let mut response = agent(15)
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
        assets: release.assets,
    })
}

fn select_asset<'a>(
    assets: &'a [Asset],
    version: &str,
    os: &str,
    arch: &str,
    installed: bool,
) -> Result<&'a Asset> {
    ensure!(
        Version::parse(version)?.to_string() == version,
        "版本号不规范"
    );
    let (platform, extension) = match (os, arch, installed) {
        ("macos", "aarch64", _) => ("macos-arm64", "dmg"),
        ("macos", "x86_64", _) => ("macos-x86_64", "dmg"),
        ("windows", "x86_64", true) => ("windows-x86_64", "msi"),
        ("windows", "x86_64", false) => ("windows-x86_64", "zip"),
        _ => anyhow::bail!("暂不支持当前系统架构自动更新"),
    };
    let name = format!("Starter-{version}-{platform}.{extension}");
    let mut matches = assets.iter().filter(|asset| asset.name == name);
    let asset = matches
        .next()
        .context("发布尚未提供当前平台的安装包，请稍后重试")?;
    ensure!(matches.next().is_none(), "发布包含重复的安装包");
    ensure!(
        asset.size > 0 && asset.size <= MAX_DOWNLOAD,
        "安装包大小异常"
    );
    ensure!(
        asset.browser_download_url
            == format!("https://github.com/zenaurora/starter/releases/download/v{version}/{name}"),
        "安装包地址不属于官方发布"
    );
    let digest = asset
        .digest
        .as_deref()
        .and_then(|s| s.strip_prefix("sha256:"))
        .context("发布缺少 SHA-256 校验值")?;
    ensure!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "安装包校验值格式异常"
    );
    Ok(asset)
}

fn copy_verified(
    mut reader: impl Read,
    mut writer: impl Write,
    asset: &Asset,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(Status),
) -> Result<()> {
    let mut hash = Sha256::new();
    let mut received = 0u64;
    let mut last_percent = u64::MAX;
    let mut buffer = [0; 64 * 1024];
    loop {
        ensure!(!cancelled.load(Ordering::Relaxed), "下载已取消");
        let count = reader.read(&mut buffer).context("下载中断，请重试")?;
        if count == 0 {
            break;
        }
        received += count as u64;
        ensure!(
            received <= asset.size && received <= MAX_DOWNLOAD,
            "下载内容超出安装包大小"
        );
        writer.write_all(&buffer[..count])?;
        hash.update(&buffer[..count]);
        let percent = received * 100 / asset.size.max(1);
        if percent != last_percent {
            progress(Status::Downloading {
                received,
                total: asset.size,
            });
            last_percent = percent;
        }
    }
    ensure!(received == asset.size, "安装包下载不完整");
    let actual = format!("sha256:{:x}", hash.finalize());
    ensure!(
        asset
            .digest
            .as_deref()
            .is_some_and(|digest| digest.eq_ignore_ascii_case(&actual)),
        "安装包 SHA-256 校验失败，请重试"
    );
    writer.flush()?;
    ensure!(!cancelled.load(Ordering::Relaxed), "下载已取消");
    Ok(())
}

fn download(
    asset: &Asset,
    path: &Path,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(Status),
) -> Result<()> {
    progress(Status::Downloading {
        received: 0,
        total: asset.size,
    });
    let mut response = agent(300)
        .get(&asset.browser_download_url)
        .header("User-Agent", concat!("Starter/", env!("CARGO_PKG_VERSION")))
        .call()
        .context("无法下载安装包")?;
    let file = std::fs::File::create(path)?;
    copy_verified(
        response.body_mut().as_reader(),
        file,
        asset,
        cancelled,
        progress,
    )
}

/// Downloads and prepares on a worker; returns only after the helper is ready.
/// The caller may then quit. On any preparation failure, the running app stays intact.
pub fn prepare_and_launch(
    version: &str,
    assets: &[Asset],
    cancelled: &AtomicBool,
    mut progress: impl FnMut(Status),
) -> Result<()> {
    install::prepare_and_launch(version, assets, cancelled, &mut progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, net::TcpListener};

    fn asset(version: &str, platform: &str, extension: &str, bytes: &[u8]) -> Asset {
        let name = format!("Starter-{version}-{platform}.{extension}");
        Asset {
            browser_download_url: format!(
                "https://github.com/zenaurora/starter/releases/download/v{version}/{name}"
            ),
            name,
            size: bytes.len() as u64,
            digest: Some(format!("sha256:{:x}", Sha256::digest(bytes))),
        }
    }

    #[test]
    fn chooses_exact_architecture_and_installation_type() {
        let assets = vec![
            asset("0.4.0", "macos-arm64", "dmg", b"mac"),
            asset("0.4.0", "macos-x86_64", "dmg", b"intel"),
            asset("0.4.0", "windows-x86_64", "msi", b"msi"),
            asset("0.4.0", "windows-x86_64", "zip", b"zip"),
        ];
        for (os, arch, installed, index) in [
            ("macos", "aarch64", false, 0),
            ("macos", "x86_64", false, 1),
            ("windows", "x86_64", true, 2),
            ("windows", "x86_64", false, 3),
        ] {
            assert_eq!(
                select_asset(&assets, "0.4.0", os, arch, installed).unwrap(),
                &assets[index]
            );
        }
        assert!(select_asset(&assets, "0.4.1", "macos", "aarch64", false).is_err());
        let mut tampered = assets.clone();
        tampered[0].browser_download_url = "https://example.com/evil".into();
        assert!(select_asset(&tampered, "0.4.0", "macos", "aarch64", false).is_err());
        tampered = assets.clone();
        tampered[0].digest = None;
        assert!(select_asset(&tampered, "0.4.0", "macos", "aarch64", false).is_err());
    }

    #[test]
    fn rejects_corrupt_truncated_oversized_or_cancelled_downloads() {
        let asset = asset("0.4.0", "macos-arm64", "dmg", b"verified");
        let cancel = AtomicBool::new(false);
        let mut output = Vec::new();
        copy_verified(
            Cursor::new(b"verified"),
            &mut output,
            &asset,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(output, b"verified");
        for bytes in [b"corrupt!".as_slice(), b"short", b"verified-long"] {
            assert!(
                copy_verified(Cursor::new(bytes), Vec::new(), &asset, &cancel, &mut |_| {})
                    .is_err()
            );
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(
            copy_verified(
                Cursor::new(b"verified"),
                Vec::new(),
                &asset,
                &cancel,
                &mut |_| {}
            )
            .is_err()
        );
    }

    #[test]
    fn compares_versions_and_ignores_unstable_releases() {
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
                assert!(
                    matches!(result.unwrap(), Status::Available { version, .. } if version == "0.2.0")
                );
            }
        }
    }
}
