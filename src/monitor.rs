//! On-demand resource sampling. CPU is normalized to the whole machine;
//! application memory is summed resident memory and can include shared pages.
use async_channel::Receiver;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Disks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

mod actions;
pub use actions::{Action, perform};

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub sampled_at: Instant,
    pub cpu_percent: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub processes: Vec<ProcessUsage>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessUsage {
    pub id: String,
    pub name: String,
    pub path: Option<PathBuf>,
    pub process_count: usize,
    /// Exact processes represented by this row, captured with the sample.
    pub members: Vec<ProcessIdentity>,
    pub cpu_percent: f32,
    pub memory: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Unix epoch seconds, as reported by sysinfo on both supported platforms.
    pub start_time: u64,
    pub exe: Option<PathBuf>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Cpu,
    Memory,
}

impl Snapshot {
    pub fn ranked(&self, query: &str, sort: Sort) -> Vec<usize> {
        let query = query.trim().to_lowercase();
        let mut rows: Vec<_> = self
            .processes
            .iter()
            .enumerate()
            .filter(|(_, row)| query.is_empty() || row.name.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect();
        rows.sort_unstable_by(|&a, &b| {
            let a = &self.processes[a];
            let b = &self.processes[b];
            match sort {
                Sort::Cpu => b
                    .cpu_percent
                    .total_cmp(&a.cpu_percent)
                    .then_with(|| b.memory.cmp(&a.memory)),
                Sort::Memory => b
                    .memory
                    .cmp(&a.memory)
                    .then_with(|| b.cpu_percent.total_cmp(&a.cpu_percent)),
            }
            .then_with(|| a.id.cmp(&b.id))
        });
        rows
    }
}

/// Disabling blocks the worker without polling. Dropping the command sender
/// shuts it down. CPU is warmed again after each idle period.
pub fn start() -> (Sender<bool>, Receiver<Snapshot>) {
    let (commands, receiver) = mpsc::channel();
    let (events, snapshots) = async_channel::bounded(2);
    thread::Builder::new()
        .name("starter-monitor".into())
        .spawn(move || {
            let mut system = System::new();
            let mut disks = Disks::new();
            let mut enabled = false;
            let mut disk_sample = Instant::now();
            loop {
                if !enabled {
                    match receiver.recv() {
                        Ok(true) => {
                            enabled = true;
                            refresh(&mut system);
                            disks.refresh_list();
                            disk_sample = Instant::now();
                        }
                        Ok(false) => continue,
                        Err(_) => break,
                    }
                }
                match receiver.recv_timeout(SAMPLE_INTERVAL) {
                    Ok(true) => continue,
                    Ok(false) => {
                        enabled = false;
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                refresh(&mut system);
                if disk_sample.elapsed() >= Duration::from_secs(15) {
                    disks.refresh();
                    disk_sample = Instant::now();
                }
                let snapshot = snapshot(&system, &disks);
                match events.try_send(snapshot) {
                    Ok(()) | Err(async_channel::TrySendError::Full(_)) => {}
                    Err(async_channel::TrySendError::Closed(_)) => break,
                }
            }
        })
        .expect("Cannot start the monitor worker");
    (commands, snapshots)
}

fn refresh(system: &mut System) {
    system.refresh_memory();
    system.refresh_cpu_usage();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        ProcessRefreshKind::new()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet),
    );
}

fn snapshot(system: &System, disks: &Disks) -> Snapshot {
    let cores = system.cpus().len().max(1) as f32;
    let rows = system.processes().values().map(|process| {
        let bundle = process.exe().and_then(app_bundle);
        let path = bundle.or_else(|| process.exe().map(Path::to_path_buf));
        let name = if let Some(bundle) = path
            .as_ref()
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
        {
            bundle
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            process.name().to_string_lossy().into_owned()
        };
        ProcessUsage {
            // Full bundle/executable path is identity. Equal display names in
            // separate locations must not merge. Inaccessible processes use PID.
            id: path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("pid:{}", process.pid())),
            name,
            path,
            process_count: 1,
            members: vec![ProcessIdentity {
                pid: process.pid().as_u32(),
                start_time: process.start_time(),
                exe: process.exe().map(Path::to_path_buf),
            }],
            cpu_percent: process.cpu_usage() / cores,
            memory: process.memory(),
        }
    });
    let (disk_used, disk_total) = system_disk(disks);
    Snapshot {
        sampled_at: Instant::now(),
        cpu_percent: system.global_cpu_usage().clamp(0.0, 100.0),
        memory_used: system.used_memory(),
        memory_total: system.total_memory(),
        disk_used,
        disk_total,
        processes: aggregate(rows),
    }
}

fn aggregate(rows: impl Iterator<Item = ProcessUsage>) -> Vec<ProcessUsage> {
    let mut groups = BTreeMap::<String, ProcessUsage>::new();
    for row in rows {
        if let Some(group) = groups.get_mut(&row.id) {
            group.cpu_percent += row.cpu_percent;
            group.memory = group.memory.saturating_add(row.memory);
            group.process_count += row.process_count;
            group.members.extend(row.members);
        } else {
            groups.insert(row.id.clone(), row);
        }
    }
    groups.into_values().collect()
}

fn app_bundle(path: &Path) -> Option<PathBuf> {
    // Helpers can be nested .app bundles; use the outermost owning bundle.
    path.ancestors()
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .last()
        .map(Path::to_path_buf)
}

fn system_disk(disks: &Disks) -> (u64, u64) {
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("SystemRoot").map(PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let root = Some(PathBuf::from("/"));
    let disk = disks
        .list()
        .iter()
        .filter(|disk| {
            root.as_ref()
                .is_some_and(|root| root.starts_with(disk.mount_point()))
        })
        .max_by_key(|disk| disk.mount_point().components().count());
    let Some(disk) = disk else { return (0, 0) };
    let total = disk.total_space();
    (total.saturating_sub(disk.available_space()), total)
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_filters_all_rows_and_switches_between_cpu_and_memory() {
        let snapshot = Snapshot {
            sampled_at: Instant::now(),
            cpu_percent: 20.0,
            memory_used: 1000,
            memory_total: 2000,
            disk_used: 1000,
            disk_total: 4000,
            processes: (0..20)
                .map(|i| ProcessUsage {
                    id: format!("app:{i}"),
                    name: format!("App {i}"),
                    path: None,
                    process_count: 1,
                    members: vec![],
                    cpu_percent: i as f32,
                    memory: 20 - i,
                })
                .collect(),
        };
        assert_eq!(snapshot.ranked("", Sort::Cpu).len(), 20);
        assert_eq!(
            snapshot.processes[snapshot.ranked("", Sort::Cpu)[0]].name,
            "App 19"
        );
        assert_eq!(
            snapshot.processes[snapshot.ranked("", Sort::Memory)[0]].name,
            "App 0"
        );
        assert_eq!(
            snapshot.processes[snapshot.ranked(" APP 0 ", Sort::Cpu)[0]].memory,
            20
        );
        assert!(snapshot.ranked("missing", Sort::Memory).is_empty());
    }

    #[test]
    fn worker_is_idle_until_enabled_and_publishes_real_samples_then_stops() {
        let (commands, snapshots) = start();
        thread::sleep(Duration::from_millis(100));
        assert!(snapshots.try_recv().is_err());
        commands.send(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let sample = loop {
            if let Ok(sample) = snapshots.try_recv() {
                break sample;
            }
            assert!(
                Instant::now() < deadline,
                "monitor did not publish a sample"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert!(sample.memory_total > 0);
        assert!(sample.memory_used <= sample.memory_total);
        assert!((0.0..=100.0).contains(&sample.cpu_percent));
        assert!(sample.disk_used <= sample.disk_total);
        assert!(sample.processes.iter().any(|p| p.memory > 0));
        commands.send(false).unwrap();
        thread::sleep(SAMPLE_INTERVAL + Duration::from_millis(200));
        assert!(
            snapshots.try_recv().is_err(),
            "sampling continued while idle"
        );
        commands.send(true).unwrap();
        drop(commands);
        assert!(
            snapshots.recv_blocking().is_err(),
            "worker did not shut down"
        );
    }

    #[test]
    fn nested_helpers_share_outer_bundle_but_same_names_in_other_locations_do_not() {
        let paths = [
            "/Applications/Example.app/Contents/MacOS/Main",
            "/Applications/Example.app/Contents/Frameworks/Helper.app/Contents/MacOS/Helper",
            "/Users/test/Example.app/Contents/MacOS/Main",
        ];
        let rows = paths.iter().map(|p| {
            let path = app_bundle(Path::new(p)).unwrap();
            ProcessUsage {
                id: path.display().to_string(),
                name: "Example".into(),
                path: Some(path),
                process_count: 1,
                members: vec![ProcessIdentity {
                    pid: 100 + p.len() as u32,
                    start_time: 1,
                    exe: Some(PathBuf::from(p)),
                }],
                cpu_percent: 10.0,
                memory: 100,
            }
        });
        let groups = aggregate(rows);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].process_count, 2);
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(groups[0].cpu_percent, 20.0);
        assert_eq!(groups[0].memory, 200);
        assert_eq!(app_bundle(Path::new("/usr/bin/worker")), None);
    }
    #[test]
    fn capacities_keep_their_correct_units() {
        assert_eq!(format_bytes(16 * 1024_u64.pow(3)), "16.0 GiB");
        assert_eq!(format_bytes(512 * 1024_u64.pow(2)), "512.0 MiB");
        assert_eq!(format_bytes(0), "0 B");
    }
}
