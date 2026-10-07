use super::{Asset, Status, download, select_asset};
use crate::config;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum Mode {
    Bundle,
    Msi,
    Portable,
}

#[derive(Debug, Deserialize, Serialize)]
struct Plan {
    mode: Mode,
    pid: u32,
    version: String,
    destination: PathBuf,
    payload: PathBuf,
    backup: PathBuf,
    workspace: PathBuf,
    staging_root: Option<PathBuf>,
    report: PathBuf,
}

#[derive(Deserialize, Serialize)]
struct Report {
    version: String,
    error: Option<String>,
    workspace: PathBuf,
}

fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command
}

#[cfg(target_os = "macos")]
fn success(command: &mut Command) -> Result<()> {
    ensure!(command.status()?.success(), "系统安装命令失败");
    Ok(())
}

pub(super) fn prepare_and_launch(
    version: &str,
    assets: &[Asset],
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(Status),
) -> Result<()> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let (mode, destination) = installation(&executable)?;
    let asset = select_asset(
        assets,
        version,
        std::env::consts::OS,
        std::env::consts::ARCH,
        matches!(mode, Mode::Msi),
    )?;
    let workspace = tempfile::Builder::new()
        .prefix("starter-update-")
        .tempdir()?;
    let package = workspace.path().join(&asset.name);
    download(asset, &package, cancelled, progress)?;
    progress(Status::Preparing);
    ensure!(!cancelled.load(Ordering::Relaxed), "下载已取消");
    let (payload, staging) =
        prepare_payload(mode, &package, &destination, version, workspace.path())?;
    ensure!(!cancelled.load(Ordering::Relaxed), "下载已取消");
    let backup = staging.as_ref().map_or_else(
        || destination.with_extension("starter-backup.exe"),
        |dir| {
            dir.path().join(if matches!(mode, Mode::Bundle) {
                "Previous.app"
            } else {
                "Previous.exe"
            })
        },
    );
    let plan = Plan {
        mode,
        pid: std::process::id(),
        version: version.into(),
        destination,
        payload,
        backup,
        workspace: workspace.path().to_owned(),
        staging_root: staging.as_ref().map(|dir| dir.path().to_owned()),
        report: config::config_path()?.with_file_name("update-result.json"),
    };
    let plan_path = workspace.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&plan)?)?;
    let helper = workspace.path().join(if cfg!(target_os = "windows") {
        "update-helper.exe"
    } else {
        "update-helper"
    });
    fs::copy(executable, &helper).context("无法创建更新助手")?;
    let mut child = command(&helper)
        .arg("--apply-update")
        .arg(&plan_path)
        .spawn()
        .context("无法启动更新助手")?;
    let started = Instant::now();
    while !workspace.path().join("ready").exists() {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("更新助手启动失败：{status}");
        }
        if started.elapsed() > Duration::from_secs(10) || cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("更新助手启动超时或已取消，请重试");
        }
        thread::sleep(Duration::from_millis(50));
    }
    // The helper now owns these locations and cleans up after replacement.
    let _ = workspace.keep();
    if let Some(staging) = staging {
        let _ = staging.keep();
    }
    progress(Status::Installing);
    Ok(())
}

fn installation(executable: &Path) -> Result<(Mode, PathBuf)> {
    #[cfg(target_os = "macos")]
    {
        let bundle = executable
            .ancestors()
            .find(|path| path.extension().is_some_and(|ext| ext == "app"))
            .context("开发进程不能原地更新；请从 Starter.app 启动")?;
        ensure!(
            executable == bundle.join("Contents/MacOS/starter"),
            "应用包结构异常"
        );
        let plist = bundle.join("Contents/Info.plist");
        ensure!(
            plist_value(&plist, "CFBundleIdentifier")? == "dev.starter.launcher",
            "当前应用标识不正确"
        );
        Ok((Mode::Bundle, bundle.to_owned()))
    }
    #[cfg(target_os = "windows")]
    {
        let installed = dirs::data_local_dir()
            .context("无法定位用户应用目录")?
            .join("Programs/Starter/Starter.exe");
        let is_installed = installed
            .canonicalize()
            .is_ok_and(|path| path == executable);
        Ok((
            if is_installed {
                Mode::Msi
            } else {
                Mode::Portable
            },
            executable.to_owned(),
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = executable;
        anyhow::bail!("当前平台暂不支持自动安装");
    }
}

fn prepare_payload(
    mode: Mode,
    package: &Path,
    destination: &Path,
    version: &str,
    workspace: &Path,
) -> Result<(PathBuf, Option<tempfile::TempDir>)> {
    #[cfg(target_os = "macos")]
    {
        let _ = mode;
        let staging = tempfile::Builder::new()
            .prefix(".starter-update-")
            .tempdir_in(destination.parent().context("应用路径异常")?)
            .context("无法写入应用所在目录；请把 Starter 移到可写的 Applications 目录后重试")?;
        let mount = workspace.join("mounted");
        fs::create_dir(&mount)?;
        success(
            command("/usr/bin/hdiutil")
                .arg("attach")
                .arg(package)
                .args(["-readonly", "-nobrowse", "-mountpoint"])
                .arg(&mount),
        )
        .context("无法挂载下载的安装包")?;
        let result = (|| {
            let source = mount.join("Starter.app");
            let payload = staging.path().join("Starter.app");
            success(command("/usr/bin/ditto").arg(&source).arg(&payload))
                .context("无法准备新应用")?;
            let plist = payload.join("Contents/Info.plist");
            ensure!(
                plist_value(&plist, "CFBundleIdentifier")? == "dev.starter.launcher",
                "下载应用的标识不正确"
            );
            ensure!(
                plist_value(&plist, "CFBundleShortVersionString")? == version,
                "下载应用的版本不一致"
            );
            ensure!(
                payload.join("Contents/MacOS/starter").is_file(),
                "下载的应用缺少可执行文件"
            );
            success(
                command("/usr/bin/codesign")
                    .args(["--verify", "--deep", "--strict"])
                    .arg(&payload),
            )
            .context("下载应用的代码签名校验失败")?;
            Ok(payload)
        })();
        let detached = success(command("/usr/bin/hdiutil").arg("detach").arg(&mount));
        let payload = result?;
        detached.context("无法卸载安装包，请稍后重试")?;
        Ok((payload, Some(staging)))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = version;
        if matches!(mode, Mode::Msi) {
            return Ok((package.to_owned(), None));
        }
        // Portable packages contain one executable plus documentation. Extract
        // only that file: no archive paths or symlinks are ever materialized.
        let mut archive = zip::ZipArchive::new(fs::File::open(package)?)?;
        let mut executable = archive
            .by_name("Starter.exe")
            .context("安装包缺少 Starter.exe")?;
        ensure!(
            executable.size() > 0 && executable.size() <= super::MAX_DOWNLOAD,
            "可执行文件大小异常"
        );
        let staging = tempfile::Builder::new()
            .prefix(".starter-update-")
            .tempdir_in(destination.parent().context("应用路径异常")?)
            .context("应用目录不可写")?;
        let payload = staging.path().join("Starter.exe");
        let mut output = fs::File::create(&payload)?;
        std::io::copy(
            &mut std::io::Read::take(&mut executable, super::MAX_DOWNLOAD + 1),
            &mut output,
        )?;
        output.sync_all()?;
        let _ = workspace;
        Ok((payload, Some(staging)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (mode, package, destination, version, workspace);
        anyhow::bail!("当前平台暂不支持自动安装");
    }
}

#[cfg(target_os = "macos")]
fn plist_value(plist: &Path, key: &str) -> Result<String> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg(format!("Print :{key}"))
        .arg(plist)
        .output()?;
    ensure!(output.status.success(), "应用缺少 {key}");
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Invoked before GPUI initializes, so the helper holds no window or tray.
pub fn run_helper() -> bool {
    let mut args = std::env::args_os();
    args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--apply-update")) {
        return false;
    }
    let result = (|| -> Result<()> {
        let path = PathBuf::from(args.next().context("更新计划缺失")?);
        let plan: Plan = serde_json::from_slice(&fs::read(path)?)?;
        fs::write(plan.workspace.join("ready"), b"ready")?;
        let exited = wait_for_exit(plan.pid);
        let parent_exited = exited.is_ok();
        let result = exited.and_then(|()| apply(&plan));
        let error = result.as_ref().err().map(|error| format!("{error:#}"));
        let report = Report {
            version: plan.version.clone(),
            error,
            workspace: plan.workspace.clone(),
        };
        if let Err(error) = config::atomic_write(&plan.report, &serde_json::to_vec(&report)?) {
            eprintln!("Cannot save update report: {error:#}");
        }
        if parent_exited && result.is_err() {
            let _ = launch(&plan.destination, None);
        }
        // Never delete a backup if restoring it failed.
        if !plan.backup.exists()
            && let Some(staging) = &plan.staging_root
        {
            let _ = fs::remove_dir_all(staging);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = fs::remove_dir_all(&plan.workspace);
        }
        result
    })();
    if let Err(error) = result {
        eprintln!("Starter update: {error:#}");
    }
    true
}

fn wait_for_exit(pid: u32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let start = Instant::now();
        while unsafe { libc::kill(pid as i32, 0) } == 0 {
            ensure!(
                start.elapsed() < Duration::from_secs(60),
                "Starter 未能退出，原应用未被替换"
            );
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return Ok(());
        }
        let status = unsafe { WaitForSingleObject(handle, 60000) };
        unsafe {
            CloseHandle(handle);
        }
        ensure!(status == 0, "Starter 未能退出，原应用未被替换");
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = pid;
        anyhow::bail!("当前平台不支持更新助手");
    }
}

fn apply(plan: &Plan) -> Result<()> {
    match plan.mode {
        Mode::Bundle | Mode::Portable => {
            replace(&plan.destination, &plan.payload, &plan.backup, || {
                start_and_verify(plan)
            })
        }
        Mode::Msi => {
            #[cfg(target_os = "windows")]
            {
                let log = plan.report.with_file_name("update-msi.log");
                let status = command("msiexec.exe")
                    .arg("/i")
                    .arg(&plan.payload)
                    .args(["/qn", "/norestart", "/L*v"])
                    .arg(&log)
                    .status()?;
                ensure!(
                    matches!(status.code(), Some(0 | 3010)),
                    "MSI 安装失败（{}），日志：{}",
                    status,
                    log.display()
                );
                start_and_verify(plan)
            }
            #[cfg(not(target_os = "windows"))]
            {
                anyhow::bail!("当前平台不能安装 MSI");
            }
        }
    }
}

/// Rename on the destination filesystem, keeping the previous executable until
/// the replacement has opened a window and acknowledged a successful startup.
fn replace(
    destination: &Path,
    payload: &Path,
    backup: &Path,
    activate: impl FnOnce() -> Result<()>,
) -> Result<()> {
    ensure!(
        !backup.exists(),
        "检测到上次更新的备份，请先恢复或移走 {}",
        backup.display()
    );
    fs::rename(destination, backup).context("无法移动旧应用，原应用保留")?;
    let result = fs::rename(payload, destination)
        .context("无法安装新应用")
        .and_then(|()| activate());
    if let Err(error) = result {
        if destination.exists() {
            fs::rename(destination, payload).with_context(|| {
                format!("无法移走失败的新应用；旧版本仍保留在 {}", backup.display())
            })?;
        }
        fs::rename(backup, destination).with_context(|| {
            format!(
                "无法恢复旧版本，备份在 {}；原错误：{error:#}",
                backup.display()
            )
        })?;
        return Err(error);
    }
    let cleaned = if backup.is_dir() {
        fs::remove_dir_all(backup)
    } else {
        fs::remove_file(backup)
    };
    if let Err(error) = cleaned {
        eprintln!("Update backup retained at {}: {error}", backup.display());
    }
    Ok(())
}

fn start_and_verify(plan: &Plan) -> Result<()> {
    let health = plan.workspace.join("healthy");
    let _ = fs::remove_file(&health);
    let mut child = launch(&plan.destination, Some(&health))?;
    let start = Instant::now();
    while !health.exists() {
        if let Some(child) = &mut child
            && let Some(status) = child.try_wait()?
        {
            anyhow::bail!("新应用提前退出：{status}");
        }
        if start.elapsed() >= Duration::from_secs(30) {
            if let Some(child) = &mut child {
                let _ = child.kill();
                let _ = child.wait();
            }
            anyhow::bail!("新应用启动超时，尝试恢复旧版本");
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn launch(destination: &Path, health: Option<&Path>) -> Result<Option<std::process::Child>> {
    #[cfg(target_os = "macos")]
    {
        let mut open = command("/usr/bin/open");
        open.arg("-n").arg(destination);
        if let Some(health) = health {
            open.arg("--args").arg("--update-health").arg(health);
        }
        success(&mut open)?;
        Ok(None)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut app = command(destination);
        if let Some(health) = health {
            app.arg("--update-health").arg(health);
        }
        Ok(Some(app.spawn()?))
    }
}

/// Called only once the new process has successfully opened its GPUI window.
pub fn acknowledge_startup() {
    let mut args = std::env::args_os();
    args.next();
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--update-health"))
        && let Some(path) = args.next()
    {
        let _ = fs::write(PathBuf::from(path), b"healthy");
    }
}

pub fn take_report() -> Option<Status> {
    let path = config::config_path()
        .ok()?
        .with_file_name("update-result.json");
    let report: Report = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    let _ = fs::remove_file(path);
    // On Windows the helper executable was locked during its own cleanup.
    // Only clean directories generated by this updater.
    if report
        .workspace
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("starter-update-"))
        && report.workspace.parent() == Some(std::env::temp_dir().as_path())
    {
        let _ = fs::remove_dir_all(&report.workspace);
    }
    match report.error {
        Some(error) => Some(Status::Failed(error)),
        None if report.version == super::CURRENT_VERSION => Some(Status::Updated(report.version)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn prepares_a_signed_dmg_and_rejects_wrong_bundle_version() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("image/Starter.app");
        fs::create_dir_all(source.join("Contents/MacOS")).unwrap();
        fs::copy(
            std::env::current_exe().unwrap(),
            source.join("Contents/MacOS/starter"),
        )
        .unwrap();
        fs::write(
            source.join("Contents/Info.plist"),
            r#"<?xml version="1.0"?><plist version="1.0"><dict>
            <key>CFBundleIdentifier</key><string>dev.starter.launcher</string>
            <key>CFBundleShortVersionString</key><string>0.4.0</string>
            <key>CFBundleExecutable</key><string>starter</string>
            <key>CFBundlePackageType</key><string>APPL</string>
            </dict></plist>"#,
        )
        .unwrap();
        success(
            command("/usr/bin/codesign")
                .args(["--force", "--deep", "--sign", "-"])
                .arg(&source),
        )
        .unwrap();
        let package = dir.path().join("fixture.dmg");
        success(
            command("/usr/bin/hdiutil")
                .args(["create", "-format", "UDZO", "-srcfolder"])
                .arg(dir.path().join("image"))
                .arg(&package),
        )
        .unwrap();
        let destination = dir.path().join("Starter.app");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("unchanged"), b"old").unwrap();
        for version in ["0.4.0", "0.5.0"] {
            let workspace = tempfile::tempdir().unwrap();
            let result = prepare_payload(
                Mode::Bundle,
                &package,
                &destination,
                version,
                workspace.path(),
            );
            if version == "0.4.0" {
                assert!(result.unwrap().0.join("Contents/MacOS/starter").is_file());
            } else {
                assert!(result.is_err());
            }
            assert_eq!(fs::read(destination.join("unchanged")).unwrap(), b"old");
            assert!(
                !workspace.path().join("mounted/Starter.app").exists(),
                "DMG must be detached even after validation failure"
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn portable_preparation_extracts_only_the_executable() {
        use std::io::Write;
        use zip::{ZipWriter, write::SimpleFileOptions};
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("package.zip");
        let mut archive = ZipWriter::new(fs::File::create(&package).unwrap());
        archive
            .start_file("../escape.txt", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"ignored").unwrap();
        archive
            .start_file("Starter.exe", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"new executable").unwrap();
        archive.finish().unwrap();
        let destination = dir.path().join("Starter.exe");
        fs::write(&destination, b"old").unwrap();
        let (payload, _stage) =
            prepare_payload(Mode::Portable, &package, &destination, "0.4.0", dir.path()).unwrap();
        assert_eq!(fs::read(&payload).unwrap(), b"new executable");
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn replacement_rolls_back_on_install_or_startup_failure() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("current");
        let payload = dir.path().join("next");
        let backup = dir.path().join("previous");
        fs::write(&destination, b"old").unwrap();
        assert!(replace(&destination, &payload, &backup, || Ok(())).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        fs::write(&payload, b"new").unwrap();
        assert!(
            replace(&destination, &payload, &backup, || anyhow::bail!(
                "startup failed"
            ))
            .is_err()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        assert_eq!(fs::read(&payload).unwrap(), b"new");
        replace(&destination, &payload, &backup, || {
            assert_eq!(fs::read(&destination)?, b"new");
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!backup.exists());
    }

    #[test]
    fn replaces_entire_bundle_and_preserves_existing_backup() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("Starter.app");
        let payload = dir.path().join("New.app");
        let backup = dir.path().join("Previous.app");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("version"), b"old").unwrap();
        fs::create_dir(&payload).unwrap();
        fs::write(payload.join("version"), b"new").unwrap();
        replace(&destination, &payload, &backup, || Ok(())).unwrap();
        assert_eq!(fs::read(destination.join("version")).unwrap(), b"new");
        fs::create_dir(&backup).unwrap();
        assert!(replace(&destination, &payload, &backup, || Ok(())).is_err());
        assert_eq!(fs::read(destination.join("version")).unwrap(), b"new");
    }
}
