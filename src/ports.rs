//! One-shot socket inventory. No privileged helpers, DNS lookups or idle polling.
use crate::monitor::{self, ProcessIdentity, ProcessUsage};
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeSet,
    io::Read,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Protocol {
    Tcp,
    Udp,
}
impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Socket {
    pub protocol: Protocol,
    pub address: String,
    pub port: u16,
    pub pid: u32,
    pub name: String,
    pub process: Option<ProcessUsage>,
}
impl Socket {
    pub fn matches(&self, query: &str) -> bool {
        let port = self.port.to_string();
        let pid = self.pid.to_string();
        query.split_whitespace().all(|word| {
            // Numeric queries select exact port/PID, rather than matching 3000 in 13000.
            if word.bytes().all(|c| c.is_ascii_digit()) {
                return word == port || word == pid;
            }
            let word = word.to_lowercase();
            self.name.to_lowercase().contains(&word)
                || self.address.to_lowercase().contains(&word)
                || self.protocol.label().to_lowercase().contains(&word)
                || self
                    .process
                    .as_ref()
                    .and_then(|p| p.members.first())
                    .and_then(|p| p.exe.as_ref())
                    .is_some_and(|p| p.to_string_lossy().to_lowercase().contains(&word))
        })
    }
    pub fn can_terminate(&self) -> bool {
        self.pid > 1 && self.pid != std::process::id() && self.process.is_some()
    }
}

pub fn scan(cancelled: &AtomicBool) -> Result<Vec<Socket>> {
    if cancelled.load(Ordering::Relaxed) {
        bail!("扫描已取消");
    }
    let mut before = System::new();
    before.refresh_processes_specifics(
        ProcessesToUpdate::All,
        ProcessRefreshKind::new().with_exe(UpdateKind::Always),
    );
    #[cfg(target_os = "macos")]
    let mut rows = {
        let mut command = Command::new("/usr/sbin/lsof");
        command.args(["-nP", "-iTCP", "-sTCP:LISTEN", "-iUDP", "-FpcfPnt"]);
        parse_lsof(&run(command, cancelled, true)?)?
    };
    #[cfg(target_os = "windows")]
    let mut rows = {
        let root = std::env::var_os("SystemRoot").context("SystemRoot 未设置")?;
        let mut command = Command::new(std::path::PathBuf::from(root).join("System32/netstat.exe"));
        command.args(["-ano"]);
        parse_netstat(&run(command, cancelled, false)?)?
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut rows: Vec<Socket> = { bail!("当前系统暂不支持端口扫描") };
    if cancelled.load(Ordering::Relaxed) {
        bail!("扫描已取消");
    }
    let pids: Vec<_> = rows
        .iter()
        .map(|r| Pid::from_u32(r.pid))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut after = System::new();
    if !pids.is_empty() {
        after.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            ProcessRefreshKind::new().with_exe(UpdateKind::Always),
        );
    }
    for row in &mut rows {
        let pid = Pid::from_u32(row.pid);
        if let (Some(old), Some(current)) = (before.process(pid), after.process(pid))
            && current.start_time() != 0
            && current.start_time() == old.start_time()
            && current.exe().is_some()
            && current.exe() == old.exe()
        {
            row.name = current.name().to_string_lossy().into_owned();
            let exe = current.exe().unwrap().to_path_buf();
            row.process = Some(ProcessUsage {
                id: format!("port-process:{}:{}", row.pid, current.start_time()),
                name: row.name.clone(),
                path: Some(monitor::app_bundle(&exe).unwrap_or_else(|| exe.clone())),
                process_count: 1,
                members: vec![ProcessIdentity {
                    pid: row.pid,
                    start_time: current.start_time(),
                    exe: Some(exe),
                }],
                cpu_percent: 0.,
                memory: 0,
            });
        }
    }
    rows.sort_by(|a, b| {
        (a.port, a.protocol, a.pid, &a.address).cmp(&(b.port, b.protocol, b.pid, &b.address))
    });
    rows.dedup_by(|a, b| {
        a.port == b.port && a.protocol == b.protocol && a.pid == b.pid && a.address == b.address
    });
    Ok(rows)
}

// Spooling into private temporary files avoids pipe deadlocks and unbounded
// output allocations. Every timeout/cancellation path reaps the child.
fn run(mut command: Command, cancelled: &AtomicBool, empty_status_one: bool) -> Result<String> {
    const LIMIT: u64 = 4 * 1024 * 1024;
    let output = tempfile::tempfile()?;
    let error = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(error.try_clone()?);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(command.spawn().context("无法启动系统端口查询工具")?);
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if cancelled.load(Ordering::Relaxed)
            || Instant::now() >= deadline
            || output.metadata()?.len() > LIMIT
            || error.metadata()?.len() > LIMIT
        {
            bail!("端口扫描已取消、超时或结果过大，请重试");
        }
        match child.0.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                return Err(e.into());
            }
        }
    };
    use std::io::{Seek, SeekFrom};
    let mut output = output;
    let mut error = error;
    output.seek(SeekFrom::Start(0))?;
    error.seek(SeekFrom::Start(0))?;
    let mut stdout = String::new();
    let mut stderr = String::new();
    output.take(LIMIT + 1).read_to_string(&mut stdout)?;
    error.take(LIMIT + 1).read_to_string(&mut stderr)?;
    if stdout.len() as u64 > LIMIT || stderr.len() as u64 > LIMIT {
        bail!("端口扫描结果过大");
    }
    if !status.success()
        && !(empty_status_one
            && status.code() == Some(1)
            && stderr.trim().is_empty()
            && stdout.is_empty())
    {
        bail!(
            "端口扫描失败：{}",
            if stderr.trim().is_empty() {
                status.to_string()
            } else {
                stderr.chars().take(500).collect()
            }
        );
    }
    // lsof can succeed with partial output and a permission warning. Surface
    // that failure rather than silently representing it as complete inventory.
    if !stderr.trim().is_empty() {
        bail!(
            "端口信息未完整读取：{}",
            stderr.chars().take(500).collect::<String>()
        );
    }
    Ok(stdout)
}

fn endpoint(value: &str) -> Option<(String, u16)> {
    let local = value.split("->").next()?;
    let (_, port) = local.rsplit_once(':')?;
    Some((local.into(), port.parse().ok()?))
}

#[cfg(any(target_os = "macos", test))]
fn parse_lsof(value: &str) -> Result<Vec<Socket>> {
    let mut rows = Vec::new();
    let mut pid = None;
    let mut name = String::new();
    let mut protocol = None;
    for line in value.lines() {
        let Some((field, rest)) = line.as_bytes().split_first() else {
            continue;
        };
        let rest = std::str::from_utf8(rest)?;
        match *field {
            b'p' => {
                pid = Some(rest.parse::<u32>().context("系统返回了无效的进程 ID")?);
                name.clear();
                protocol = None;
            }
            b'c' => name = rest.into(),
            b'f' => protocol = None,
            b'P' => {
                protocol = match rest {
                    "TCP" => Some(Protocol::Tcp),
                    "UDP" => Some(Protocol::Udp),
                    _ => None,
                }
            }
            b'n' => {
                if let (Some(pid), Some(protocol), Some((address, port))) =
                    (pid, protocol, endpoint(rest))
                {
                    rows.push(Socket {
                        protocol,
                        address,
                        port,
                        pid,
                        name: name.clone(),
                        process: None,
                    });
                }
            }
            _ => {}
        }
    }
    if !value.trim().is_empty() && pid.is_none() {
        bail!("无法识别系统返回的端口清单");
    }
    Ok(rows)
}

#[cfg(any(target_os = "windows", test))]
fn parse_netstat(value: &str) -> Result<Vec<Socket>> {
    let mut rows = Vec::new();
    for line in value.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let (protocol, pid) = match fields.as_slice() {
            ["TCP", _, remote, state, pid]
                if *state == "LISTENING" || endpoint(remote).is_some_and(|(_, p)| p == 0) =>
            {
                (Protocol::Tcp, *pid)
            }
            ["UDP", _, _, pid] => (Protocol::Udp, *pid),
            _ => continue,
        };
        let pid = pid.parse::<u32>().context("系统返回了无效的进程 ID")?;
        if let Some((address, port)) = endpoint(fields[1]) {
            rows.push(Socket {
                protocol,
                address,
                port,
                pid,
                name: format!("进程 {pid}"),
                process: None,
            });
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_ipv4_ipv6_udp_and_multiple_sockets_without_leaking_fields() {
        let rows = parse_lsof("p123\ncnode\nf20\ntIPv6\nPTCP\nn[::1]:3000\nf21\ntIPv4\nPUDP\nn127.0.0.1:5353->127.0.0.1:53\np456\ncother\nf8\nPTCP\nn*:8080\n").unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].port, 3000);
        assert_eq!(rows[1].protocol, Protocol::Udp);
        assert_eq!(rows[1].address, "127.0.0.1:5353");
        assert_eq!(rows[2].pid, 456);
        assert!(rows[0].matches("3000 node"));
        assert!(!rows[0].matches("300"));
        assert!(!rows[0].can_terminate());
    }
    #[test]
    fn windows_ignores_outgoing_tcp_connections() {
        let rows = parse_netstat("TCP 0.0.0.0:80 0.0.0.0:0 LISTENING 123\nTCP [::]:3000 [::]:0 LISTENING 456\nTCP 127.0.0.1:9000 127.0.0.1:9001 ESTABLISHED 789\nUDP [::]:5353 *:* 789").unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].port, 3000);
        assert_eq!(rows[2].protocol, Protocol::Udp);
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn discovers_an_owned_listener_and_protects_starter() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let rows = scan(&AtomicBool::new(false)).unwrap();
        let own = rows
            .iter()
            .find(|r| r.port == port && r.pid == std::process::id())
            .expect("owned listener missing");
        assert!(own.process.is_some());
        assert!(!own.can_terminate());
    }
}
