//! Actions operate on the processes in the selected sample, never on names or
//! newly discovered children. Recheck executable and start time before acting:
//! a process may have exited and its PID may already have been reused.
use super::{ProcessIdentity, ProcessUsage, app_bundle};
use anyhow::{Result, bail};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Quit,
    Terminate,
    ForceQuit,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Quit => "退出",
            Self::Terminate => "终止进程",
            Self::ForceQuit => "强制退出",
        }
    }
}

/// A successful result means the OS accepted a request, not that the process
/// already exited. A normal quit can display a save dialog or be cancelled.
pub fn perform(action: Action, row: &ProcessUsage) -> Result<String> {
    protect_target(row)?;
    let pids: Vec<_> = row.members.iter().map(|p| Pid::from_u32(p.pid)).collect();
    let mut system = System::new();
    // Start time is collected by sysinfo independently of refresh flags. Avoid
    // CPU/memory sampling and force a fresh image path for this action only.
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        ProcessRefreshKind::new().with_exe(UpdateKind::Always),
    );
    let mut verified = Vec::new();
    let mut skipped = 0;
    for expected in &row.members {
        let current = system.process(Pid::from_u32(expected.pid));
        if current.is_some_and(|current| {
            matches_identity(row, expected, current.start_time(), current.exe())
        }) {
            verified.push(expected);
        } else {
            skipped += 1;
        }
    }
    if verified.is_empty() {
        bail!("所选进程已退出或身份已变化，请等待监控刷新后重试");
    }
    let mut report = Report::default();
    match action {
        Action::Quit => quit(row, &verified, &mut report)?,
        Action::Terminate | Action::ForceQuit => {
            for process in verified {
                report.record(process.pid, signal(action, process));
            }
        }
    }
    report.message(action, &row.name, skipped)
}

fn group_path(exe: &Path) -> PathBuf {
    app_bundle(exe).unwrap_or_else(|| exe.to_path_buf())
}

fn protect_target(row: &ProcessUsage) -> Result<()> {
    if row.members.is_empty() {
        bail!("没有可操作的进程，请等待监控刷新后重试");
    }
    if row.members.iter().any(|p| p.pid <= 1) {
        bail!("不能操作系统核心进程");
    }
    let own_pid = std::process::id();
    let own_group = std::env::current_exe().ok().map(|exe| group_path(&exe));
    if row.members.iter().any(|p| p.pid == own_pid)
        || own_group
            .as_ref()
            .is_some_and(|own| row.path.as_ref() == Some(own))
    {
        bail!("请通过 Starter 菜单退出自身，监控中不能终止 Starter");
    }
    Ok(())
}

fn matches_identity(
    row: &ProcessUsage,
    expected: &ProcessIdentity,
    start_time: u64,
    exe: Option<&Path>,
) -> bool {
    // Unknown identity is not permission to act on a bare PID.
    expected.start_time != 0
        && expected.start_time == start_time
        && expected.exe.as_deref().is_some_and(|expected_exe| {
            exe == Some(expected_exe) && row.path.as_ref() == Some(&group_path(expected_exe))
        })
}

#[derive(Default)]
struct Report {
    sent: BTreeSet<u32>,
    errors: Vec<String>,
}

impl Report {
    fn record(&mut self, pid: u32, result: Result<()>) {
        match result {
            Ok(()) => {
                self.sent.insert(pid);
            }
            Err(error) => self.errors.push(format!("PID {pid}: {error}")),
        }
    }

    fn message(self, action: Action, name: &str, skipped: usize) -> Result<String> {
        if self.sent.is_empty() {
            bail!("{}", self.errors.join("；"));
        }
        let mut message = format!(
            "已向 {name} 的 {} 个进程发送{}请求",
            self.sent.len(),
            action.label()
        );
        if action == Action::Quit {
            message.push_str("；应用可能提示保存或取消退出");
        }
        if skipped > 0 {
            message.push_str(&format!("；跳过 {skipped} 个已退出或身份变化的进程"));
        }
        if !self.errors.is_empty() {
            message.push_str(&format!("；{}", self.errors.join("；")));
        }
        Ok(message)
    }
}

#[cfg(target_os = "macos")]
fn quit(row: &ProcessUsage, processes: &[&ProcessIdentity], report: &mut Report) -> Result<()> {
    use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
    use objc2_foundation::NSURL;

    let Some(bundle) = row
        .path
        .as_ref()
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
    else {
        bail!("后台进程不支持应用退出，请选择“终止进程”");
    };
    for process in processes {
        // Use the sampled, validated PID, not a bundle identifier (which may
        // refer to another installed copy). Helper bundles are not quit here:
        // the main application owns its normal shutdown and save workflow.
        let application: Option<Retained<AnyObject>> = unsafe {
            msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: process.pid as i32]
        };
        let Some(application) = application else {
            continue;
        };
        let url: Option<Retained<NSURL>> = unsafe { msg_send![&*application, bundleURL] };
        let path = url
            .and_then(|url| url.path())
            .map(|path| PathBuf::from(path.to_string()));
        if path.as_ref() != Some(bundle) {
            continue;
        }
        let accepted: bool = unsafe { msg_send![&*application, terminate] };
        report.record(
            process.pid,
            if accepted {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "系统未接受退出请求，应用可能已退出或拒绝退出"
                ))
            },
        );
    }
    if report.sent.is_empty() && report.errors.is_empty() {
        bail!("未找到该应用的主进程，不能正常退出；可选择“终止进程”");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn signal(action: Action, process: &ProcessIdentity) -> Result<()> {
    let signal = match action {
        Action::Terminate => libc::SIGTERM,
        Action::ForceQuit => libc::SIGKILL,
        Action::Quit => unreachable!(),
    };
    if unsafe { libc::kill(process.pid as i32, signal) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, HWND, LPARAM},
        System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, TerminateProcess,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible,
            PostMessageW, WM_CLOSE,
        },
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    fn process_handle(process: &ProcessIdentity, terminate: bool) -> Result<Handle> {
        let rights =
            PROCESS_QUERY_LIMITED_INFORMATION | if terminate { PROCESS_TERMINATE } else { 0 };
        let handle = Handle(unsafe { OpenProcess(rights, 0, process.pid) });
        if handle.0.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
            == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let started = ((u64::from(created.dwHighDateTime) << 32)
            | u64::from(created.dwLowDateTime))
            / 10_000_000;
        let mut image = vec![0u16; 32768];
        let mut length = image.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle.0, 0, image.as_mut_ptr(), &mut length) } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let exe = PathBuf::from(OsString::from_wide(&image[..length as usize]));
        if started.checked_sub(11_644_473_600) != Some(process.start_time)
            || process.exe.as_ref() != Some(&exe)
        {
            bail!("进程身份已变化");
        }
        Ok(handle)
    }

    pub(super) fn signal(action: Action, process: &ProcessIdentity) -> Result<()> {
        if action == Action::Terminate {
            bail!("Windows 不支持 SIGTERM，请使用“退出”或“强制退出”");
        }
        // Retain the verified kernel handle through termination so PID reuse
        // between checking and acting cannot redirect the request.
        let handle = process_handle(process, true)?;
        if unsafe { TerminateProcess(handle.0, 1) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub(super) fn quit(
        _: &ProcessUsage,
        processes: &[&ProcessIdentity],
        report: &mut Report,
    ) -> Result<()> {
        struct Context<'a> {
            processes: &'a [&'a ProcessIdentity],
            report: &'a mut Report,
        }
        unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> i32 {
            let context = unsafe { &mut *(data as *mut Context<'_>) };
            if unsafe { IsWindowVisible(window) } == 0
                || !unsafe { GetWindow(window, GW_OWNER) }.is_null()
            {
                return 1;
            }
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(window, &mut pid) };
            let Some(process) = context.processes.iter().find(|p| p.pid == pid) else {
                return 1;
            };
            let result = (|| {
                let _handle = process_handle(process, false)?;
                // Recheck the window's owner immediately before posting.
                let mut current_pid = 0;
                unsafe { GetWindowThreadProcessId(window, &mut current_pid) };
                if current_pid != pid {
                    bail!("窗口所属进程已变化");
                }
                if unsafe { PostMessageW(window, WM_CLOSE, 0, 0) } == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                Ok(())
            })();
            context.report.record(pid, result);
            1
        }
        let mut context = Context { processes, report };
        if unsafe { EnumWindows(Some(visit), &mut context as *mut _ as LPARAM) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if context.report.sent.is_empty() && context.report.errors.is_empty() {
            bail!("未找到可关闭的应用窗口，请使用“强制退出”");
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
use windows::{quit, signal};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn quit(_: &ProcessUsage, _: &[&ProcessIdentity], _: &mut Report) -> Result<()> {
    bail!("当前平台不支持退出应用");
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn signal(_: Action, _: &ProcessIdentity) -> Result<()> {
    bail!("当前平台不支持终止进程");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(process: ProcessIdentity) -> ProcessUsage {
        let path = process.exe.as_deref().map(group_path);
        ProcessUsage {
            id: path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            name: "Test process".into(),
            path,
            process_count: 1,
            members: vec![process],
            cpu_percent: 0.0,
            memory: 0,
        }
    }

    #[test]
    fn identity_checks_start_time_image_and_row_group() {
        let image = PathBuf::from("/Applications/Test.app/Contents/MacOS/Main");
        let process = ProcessIdentity {
            pid: 100,
            start_time: 500,
            exe: Some(image.clone()),
        };
        let mut target = row(process.clone());
        assert!(matches_identity(&target, &process, 500, Some(&image)));
        assert!(!matches_identity(&target, &process, 501, Some(&image)));
        assert!(!matches_identity(
            &target,
            &process,
            500,
            Some(Path::new("/bin/other"))
        ));
        assert!(!matches_identity(&target, &process, 500, None));
        target.path = Some(PathBuf::from("/Applications/Other.app"));
        assert!(!matches_identity(&target, &process, 500, Some(&image)));
        let unknown = ProcessIdentity {
            pid: 100,
            start_time: 0,
            exe: None,
        };
        assert!(!matches_identity(&row(unknown.clone()), &unknown, 0, None));
    }

    #[test]
    fn self_and_core_processes_are_refused_before_any_system_action() {
        for pid in [0, 1, std::process::id()] {
            let target = row(ProcessIdentity {
                pid,
                start_time: 1,
                exe: Some("/bin/test".into()),
            });
            assert!(protect_target(&target).is_err());
        }
        let own_image = std::env::current_exe().unwrap();
        let target = row(ProcessIdentity {
            pid: std::process::id() + 1,
            start_time: 1,
            exe: Some(own_image),
        });
        assert!(protect_target(&target).is_err());
    }

    #[test]
    fn reports_distinguish_accepted_requests_and_partial_failures() {
        let mut report = Report::default();
        report.record(10, Ok(()));
        report.record(11, Err(anyhow::anyhow!("permission denied")));
        let message = report.message(Action::Quit, "Editor", 2).unwrap();
        assert!(message.contains("发送退出请求"));
        assert!(message.contains("保存或取消"));
        assert!(message.contains("跳过 2 个"));
        assert!(message.contains("PID 11: permission denied"));
        assert!(
            Report::default()
                .message(Action::ForceQuit, "Editor", 0)
                .is_err()
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    struct Fixture(std::process::Child);

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn fixture() -> (Fixture, ProcessUsage) {
        #[cfg(target_os = "macos")]
        let child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .unwrap();
        #[cfg(target_os = "windows")]
        let child = std::process::Command::new("ping.exe")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let fixture = Fixture(child);
        let mut system = System::new();
        let pid = Pid::from_u32(fixture.0.id());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                ProcessRefreshKind::new().with_exe(UpdateKind::Always),
            );
            if let Some(process) = system
                .process(pid)
                .filter(|p| p.start_time() != 0 && p.exe().is_some())
            {
                let target = row(ProcessIdentity {
                    pid: pid.as_u32(),
                    start_time: process.start_time(),
                    exe: process.exe().map(Path::to_path_buf),
                });
                return (fixture, target);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fixture process was not sampled"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn stale_snapshot_does_not_signal_a_live_fixture() {
        let (mut fixture, mut target) = fixture();
        target.members[0].start_time += 1;
        assert!(perform(Action::ForceQuit, &target).is_err());
        assert!(fixture.0.try_wait().unwrap().is_none());
        target.members[0].start_time -= 1;
        target.members[0].exe = Some(PathBuf::from("/not-the-selected-executable"));
        assert!(perform(Action::ForceQuit, &target).is_err());
        assert!(fixture.0.try_wait().unwrap().is_none());
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn force_quit_terminates_only_a_selected_child_fixture() {
        let (mut selected, mut target) = fixture();
        let (mut other, other_target) = fixture();
        let mut stale_member = other_target.members[0].clone();
        stale_member.start_time += 1;
        target.members.push(stale_member);
        target.process_count += 1;
        let message = perform(Action::ForceQuit, &target).unwrap();
        assert!(message.contains("发送强制退出请求"));
        assert!(message.contains("跳过 1 个"));
        assert!(other.0.try_wait().unwrap().is_none());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while selected.0.try_wait().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline, "fixture did not exit");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn terminate_sends_sigterm_and_quit_refuses_background_fixture() {
        let (mut fixture, target) = fixture();
        assert!(perform(Action::Quit, &target).is_err());
        assert!(fixture.0.try_wait().unwrap().is_none());
        assert!(
            perform(Action::Terminate, &target)
                .unwrap()
                .contains("发送终止进程请求")
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while fixture.0.try_wait().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline, "fixture did not exit");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(perform(Action::ForceQuit, &target).is_err());
    }
}
