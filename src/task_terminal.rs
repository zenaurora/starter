//! A task-owned terminal session. No graphical terminal or interactive user shell
//! is launched. Output and input remain in memory for the lifetime of the task.
use anyhow::{Context, Result, ensure};
use async_channel::{Receiver, Sender};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

pub use portable_pty::PtySize as Size;

#[derive(Clone, Debug)]
pub struct Request {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl Request {
    pub fn command(&self) -> Result<CommandBuilder> {
        ensure!(self.program.is_absolute(), "任务程序必须使用绝对路径");
        let mut command = CommandBuilder::new(&self.program);
        command.args(&self.args);
        command.env_clear();
        for key in ["HOME", "USER", "LOGNAME", "TMPDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.cwd(dirs::home_dir().context("无法确定用户目录")?);
        command.env("PATH", task_path());
        command.env("LANG", "en_US.UTF-8");
        command.env("LC_CTYPE", "en_US.UTF-8");
        command.env("TERM", "xterm-256color");
        command.env("TERM_PROGRAM", "Starter");
        command.env("HOMEBREW_NO_AUTO_UPDATE", "1");
        command.env("HOMEBREW_NO_ANALYTICS", "1");
        command.env("MOLE_DELETE_MODE", "trash");
        Ok(command)
    }
}

pub fn task_path() -> OsString {
    let mut paths = vec![];
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local/bin"));
    }
    paths.extend(
        [
            "/opt/homebrew/bin",
            "/opt/homebrew/sbin",
            "/usr/local/bin",
            "/usr/local/sbin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .map(PathBuf::from),
    );
    std::env::join_paths(paths).expect("fixed terminal PATH contains no separators")
}

#[derive(Debug)]
pub enum Event {
    Output(Vec<u8>),
    Echo(bool),
    Error(String),
    Exit { code: u32, stopped: bool },
}

enum Command {
    Input(Vec<u8>),
    Resize(Size),
}

pub struct Session {
    commands: mpsc::SyncSender<Command>,
    events: Sender<Event>,
    interrupt: Arc<AtomicBool>,
    finished: Receiver<()>,
}

impl Session {
    pub fn start(request: Request, size: Size) -> Result<(Self, Receiver<Event>)> {
        let pair = native_pty_system()
            .openpty(size)
            .context("无法创建任务终端")?;
        let mut reader = nonblocking_reader(pair.master.as_ref())?;
        let mut writer = pair.master.take_writer()?;
        let mut child = pair
            .slave
            .spawn_command(request.command()?)
            .context("无法启动任务")?;
        drop(pair.slave);
        let group = child.process_id().context("任务进程未返回标识")? as libc::pid_t;
        let mut guard = SpawnGuard {
            group,
            killer: Some(child.clone_killer()),
        };
        let (events, receiver) = async_channel::bounded(128);
        let session_events = events.clone();
        let interrupt = Arc::new(AtomicBool::new(false));
        let stop_requested = interrupt.clone();
        let (finished_sender, finished) = async_channel::bounded(1);
        let output_events = events.clone();
        let output_cancelled = Arc::new(AtomicBool::new(false));
        let read_cancelled = output_cancelled.clone();
        let output = thread::Builder::new()
            .name("starter-terminal-output".into())
            .spawn(move || {
                let mut bytes = [0u8; 8192];
                while !read_cancelled.load(Ordering::Relaxed) {
                    let mut poll = libc::pollfd {
                        fd: reader.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    // reader owns this descriptor until the output thread exits.
                    if unsafe { libc::poll(&mut poll, 1, 50) } <= 0 {
                        continue;
                    }
                    match reader.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(length) => {
                            let mut event = Event::Output(bytes[..length].to_vec());
                            loop {
                                match output_events.try_send(event) {
                                    Ok(()) => break,
                                    Err(async_channel::TrySendError::Closed(_)) => return,
                                    Err(async_channel::TrySendError::Full(pending)) => {
                                        if read_cancelled.load(Ordering::Relaxed) {
                                            return;
                                        }
                                        event = pending;
                                        thread::sleep(Duration::from_millis(5));
                                    }
                                }
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                            ) =>
                        {
                            continue;
                        }
                        // macOS/Linux may report EIO when the slave side closes.
                        Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                        Err(error) => {
                            let _ = output_events
                                .try_send(Event::Error(format!("终端读取失败：{error}")));
                            break;
                        }
                    }
                }
            })?;
        let (commands, queue) = mpsc::sync_channel(32);
        thread::Builder::new()
            .name("starter-terminal-task".into())
            .spawn(move || {
                let master = pair.master;
                let mut stop: Option<Instant> = None;
                let mut last_echo = None;
                let mut pending = std::collections::VecDeque::<u8>::new();
                let exit = loop {
                    if events.is_closed() {
                        let foreground = master.process_group_leader();
                        signal_groups(master.as_ref(), group, libc::SIGTERM);
                        // macOS can keep a killed process in its exit path while
                        // unread PTY output remains. Close all transport handles
                        // before reaping, rather than waiting with the master open.
                        output_cancelled.store(true, Ordering::Relaxed);
                        drop(writer);
                        drop(master);
                        let _ = output.join();
                        let deadline = Instant::now() + Duration::from_millis(500);
                        while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                            thread::sleep(Duration::from_millis(10));
                        }
                        signal_group(group, libc::SIGKILL);
                        if let Some(foreground) = foreground {
                            signal_group(foreground, libc::SIGKILL);
                        }
                        let _ = child.wait();
                        let _ = finished_sender.try_send(());
                        return;
                    }
                    if stop_requested.load(Ordering::Relaxed) && stop.is_none() {
                        signal_groups(master.as_ref(), group, libc::SIGINT);
                        stop = Some(Instant::now());
                    }
                    match child.try_wait() {
                        Ok(Some(status)) => break Some(status.exit_code()),
                        Ok(None) => {}
                        Err(error) => {
                            let _ =
                                events.try_send(Event::Error(format!("无法读取任务状态：{error}")));
                            signal_groups(master.as_ref(), group, libc::SIGKILL);
                            break None;
                        }
                    }
                    if let Some(echo) = echo_enabled(master.as_ref())
                        && last_echo != Some(echo)
                        && events.try_send(Event::Echo(echo)).is_ok()
                    {
                        last_echo = Some(echo);
                    }
                    if !pending.is_empty() && stop.is_none() {
                        let bytes = pending.as_slices().0;
                        match writer.write(bytes) {
                            Ok(length) => {
                                pending.drain(..length);
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::Interrupted
                                ) => {}
                            Err(error) => {
                                pending.clear();
                                let _ =
                                    events.try_send(Event::Error(format!("终端输入失败：{error}")));
                            }
                        }
                    }
                    if let Some(started) = stop {
                        let elapsed = started.elapsed();
                        if elapsed > Duration::from_secs(5) {
                            signal_groups(master.as_ref(), group, libc::SIGKILL);
                            break None;
                        } else if elapsed > Duration::from_secs(3) {
                            signal_groups(master.as_ref(), group, libc::SIGTERM);
                        }
                    }
                    match queue.recv_timeout(Duration::from_millis(20)) {
                        Ok(Command::Input(bytes)) if stop.is_none() => {
                            if pending.len() + bytes.len() <= 1024 * 1024 {
                                pending.extend(bytes);
                            } else {
                                let _ = events.try_send(Event::Error(
                                    "输入过多，请等待任务读取后重试".into(),
                                ));
                            }
                        }
                        Ok(Command::Resize(size)) => {
                            if let Err(error) = master.resize(size) {
                                let _ = events
                                    .try_send(Event::Error(format!("终端调整大小失败：{error}")));
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => continue,
                        Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                    }
                };
                // Terminate remaining task helpers and drain output before reporting exit.
                signal_groups(master.as_ref(), group, libc::SIGTERM);
                let drain_deadline = Instant::now() + Duration::from_millis(500);
                while !output.is_finished() && Instant::now() < drain_deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                if !output.is_finished() {
                    signal_groups(master.as_ref(), group, libc::SIGKILL);
                    output_cancelled.store(true, Ordering::Relaxed);
                }
                signal_groups(master.as_ref(), group, libc::SIGKILL);
                drop(writer);
                drop(master);
                let _ = output.join();
                let exit = exit
                    .unwrap_or_else(|| child.wait().map(|status| status.exit_code()).unwrap_or(1));
                let _ = events.send_blocking(Event::Exit {
                    code: exit,
                    stopped: stop.is_some(),
                });
                let _ = finished_sender.try_send(());
            })?;
        guard.killer = None;
        Ok((
            Self {
                commands,
                events: session_events,
                interrupt,
                finished,
            },
            receiver,
        ))
    }

    pub fn input(&self, bytes: Vec<u8>) -> Result<()> {
        ensure!(bytes.len() <= 65536, "一次最多粘贴 64 KiB 文本");
        self.commands
            .try_send(Command::Input(bytes))
            .context("任务已结束或输入队列已满")
    }

    pub fn resize(&self, size: PtySize) -> Result<()> {
        self.commands
            .try_send(Command::Resize(size))
            .context("任务已结束或输入队列已满")
    }

    pub fn interrupt(&self) {
        self.interrupt.store(true, Ordering::Relaxed);
    }

    pub fn completion(&self) -> Receiver<()> {
        self.finished.clone()
    }

    pub fn close(self) -> Receiver<()> {
        let finished = self.completion();
        drop(self);
        finished
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.events.close();
    }
}

fn nonblocking_reader(master: &dyn MasterPty) -> Result<File> {
    let fd = master.as_raw_fd().context("任务终端缺少文件描述符")?;
    // Duplicates share file status flags. Both reader and writer must remain
    // interruptible even if the child stops reading or a helper holds the PTY open.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        ensure!(
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0,
            "无法设置任务终端模式"
        );
        // Do not leak a master handle into concurrently spawned unrelated jobs:
        // that would keep this task's terminal alive after Starter closes it.
        let reader = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0);
        ensure!(reader >= 0, "无法创建任务输出通道");
        Ok(File::from_raw_fd(reader))
    }
}

struct SpawnGuard {
    group: libc::pid_t,
    killer: Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if let Some(killer) = &mut self.killer {
            // This group belongs to the child spawned into our private PTY.
            unsafe {
                libc::kill(-self.group, libc::SIGKILL);
            }
            let _ = killer.kill();
        }
    }
}

fn signal_groups(master: &dyn MasterPty, group: libc::pid_t, signal: i32) {
    if let Some(foreground) = master.process_group_leader()
        && foreground != group
    {
        signal_group(foreground, signal);
    }
    signal_group(group, signal);
}

fn signal_group(group: libc::pid_t, signal: i32) {
    if group > 0 {
        unsafe {
            libc::kill(-group, signal);
        }
    }
}

fn echo_enabled(master: &dyn MasterPty) -> Option<bool> {
    let fd = master.as_raw_fd()?;
    let mut settings = std::mem::MaybeUninit::<libc::termios>::uninit();
    // The master owns fd for the entire call; settings is initialized on success.
    unsafe {
        if libc::tcgetattr(fd, settings.as_mut_ptr()) == 0 {
            Some(settings.assume_init().c_lflag & libc::ECHO != 0)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(script: &str) -> Request {
        Request {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        }
    }

    fn collect(receiver: &Receiver<Event>, mut on_event: impl FnMut(&Event)) -> (String, u32) {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut output = vec![];
        while Instant::now() < deadline {
            if let Ok(event) = receiver.try_recv() {
                on_event(&event);
                match event {
                    Event::Output(bytes) => output.extend(bytes),
                    Event::Exit { code, .. } => {
                        return (String::from_utf8_lossy(&output).into_owned(), code);
                    }
                    _ => {}
                }
            } else {
                thread::sleep(Duration::from_millis(5));
            }
        }
        panic!("fixture terminal did not finish in time");
    }

    #[test]
    fn child_gets_a_private_controlling_terminal_and_literal_arguments() {
        let literal = "中文 'quotes' $(touch /tmp/should-not-run); spaces";
        let mut request = fixture(
            "test -t 0 && test -t 1 && test -t 2 && printf 'TTY:%s\\n' \"$1\"; stty size; printf 'err' >&2; exit 7",
        );
        request.args.extend(["fixture".into(), literal.into()]);
        let (_session, events) = Session::start(
            request,
            Size {
                rows: 18,
                cols: 88,
                ..Size::default()
            },
        )
        .unwrap();
        let (output, code) = collect(&events, |_| {});
        assert!(output.contains(&format!("TTY:{literal}")));
        assert!(output.contains("18 88"));
        assert!(output.contains("err"));
        assert_eq!(code, 7);
    }

    #[test]
    fn interactive_input_resizing_and_hidden_echo_work_without_a_shell_window() {
        let request = fixture(
            "stty -echo; printf ready; read -r answer; stty size; printf '\\nanswer-length:%s' \"${#answer}\"",
        );
        let (session, events) = Session::start(request, Size::default()).unwrap();
        let mut sent = false;
        let mut hidden = false;
        let (output, code) = collect(&events, |event| {
            if matches!(event, Event::Echo(false)) {
                hidden = true;
            }
            if matches!(event, Event::Output(bytes) if bytes.windows(5).any(|w| w == b"ready"))
                && !sent
            {
                session
                    .resize(Size {
                        rows: 20,
                        cols: 90,
                        ..Size::default()
                    })
                    .unwrap();
                session.input(b"not-recorded-password\r".to_vec()).unwrap();
                sent = true;
            }
        });
        assert_eq!(code, 0);
        assert!(sent && hidden);
        assert!(output.contains("20 90"));
        assert!(output.contains("answer-length:21"));
        assert!(!output.contains("not-recorded-password"));
    }

    #[test]
    fn interrupt_reaches_task_helpers_and_reaps_the_child() {
        let (session, events) = Session::start(
            fixture("trap 'printf stopped; exit 130' INT; printf ready; sleep 30 & wait"),
            Size::default(),
        )
        .unwrap();
        let (_, code) = collect(&events, |event| {
            if matches!(event, Event::Output(bytes) if bytes.windows(5).any(|w| w == b"ready")) {
                session.interrupt();
            }
        });
        assert_eq!(code, 130);
    }

    #[test]
    fn dropping_a_task_with_unread_output_still_terminates_it() {
        let (session, events) = Session::start(
            fixture("trap '' TERM; printf 'PID:%s\\n' $$; yes output"),
            Size::default(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut text = String::new();
        let pid = loop {
            assert!(Instant::now() < deadline, "fixture did not start");
            if let Ok(Event::Output(bytes)) = events.try_recv() {
                text.push_str(&String::from_utf8_lossy(&bytes));
                if let Some(line) = text.lines().find(|line| line.starts_with("PID:")) {
                    break line[4..].trim().parse::<libc::pid_t>().unwrap();
                }
            } else {
                thread::sleep(Duration::from_millis(5));
            }
        };
        thread::sleep(Duration::from_millis(100)); // Fill the bounded output channel.
        drop(session);
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "private task {pid} survived drop; {text}"
        );
    }

    #[test]
    fn a_helper_holding_the_pty_open_cannot_block_the_exit_event() {
        let (session, events) = Session::start(
            fixture("(trap '' TERM HUP; sleep 30) & printf complete; exit 0"),
            Size::default(),
        )
        .unwrap();
        let (output, code) = collect(&events, |_| {});
        assert_eq!(code, 0);
        assert!(output.contains("complete"));
        drop(session);
    }

    #[test]
    fn stopping_a_task_ignoring_interrupts_releases_a_backed_up_pty() {
        let (session, events) = Session::start(
            fixture("trap '' INT TERM; printf ready; yes output"),
            Size::default(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline, "fixture did not start");
            if matches!(events.try_recv(), Ok(Event::Output(bytes)) if bytes.windows(5).any(|bytes| bytes == b"ready"))
            {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        session.interrupt();
        // Let the output queue fill while SIGINT/SIGTERM are ignored.
        thread::sleep(Duration::from_millis(5400));
        let mut stopped = false;
        let (_, code) = collect(&events, |event| {
            if matches!(event, Event::Exit { stopped: true, .. }) {
                stopped = true;
            }
        });
        assert!(stopped);
        assert_ne!(code, 0);
    }
}
