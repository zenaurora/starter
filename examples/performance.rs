//! Reproducible search timings and Rust heap usage; no GUI or user files are touched.
//! Run with `cargo run --release --locked --example performance -- 10000`.
use starter::{
    catalog::Application,
    config::Config,
    search::{self, Candidate},
    system_commands,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    hint::black_box,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    time::Instant,
};

struct Heap;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

// SAFETY: all allocations retain System's layout/alignment; counters only observe sizes.
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
            ALLOCATED.fetch_add(layout.size(), Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(pointer, layout, size) };
        if !next.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            let live = LIVE.fetch_add(size, Relaxed) + size;
            PEAK.fetch_max(live, Relaxed);
            ALLOCATED.fetch_add(size, Relaxed);
        }
        next
    }
}

#[global_allocator]
static HEAP: Heap = Heap;

fn measure(label: &str, rounds: usize, mut search: impl FnMut() -> Vec<Candidate>) {
    // Warm filesystem/library initialization before timing identical requests.
    black_box(search());
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    ALLOCATED.store(0, Relaxed);
    let start = Instant::now();
    for _ in 0..rounds {
        black_box(search());
    }
    let elapsed = start.elapsed();
    let allocated = ALLOCATED.load(Relaxed);
    let peak = PEAK.load(Relaxed).saturating_sub(live);
    println!(
        "{label}: {:.3} ms/query, {:.1} KiB allocated/query, {:.1} KiB peak extra heap",
        elapsed.as_secs_f64() * 1000.0 / rounds as f64,
        allocated as f64 / rounds as f64 / 1024.0,
        peak as f64 / 1024.0,
    );
}

fn main() -> anyhow::Result<()> {
    let count: usize = std::env::args()
        .nth(1)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(10_000);
    let config = Config::default();
    let usage = BTreeMap::new();
    let apps: Vec<_> = (0..500)
        .map(|index| Application {
            id: format!("app:{index}"),
            name: format!("Example Application {index:03}"),
            path: PathBuf::from(format!("/Applications/Example Application {index:03}.app")),
            aliases: vec![format!("应用 {index:03}")],
        })
        .collect();
    let candidates = search::app_candidates(&apps, &config);
    for query in ["", "example", "exampel", "应用"] {
        measure(&format!("apps/500/{query:?}"), 30, || {
            let commands = if query.is_empty() {
                &[][..]
            } else {
                system_commands::candidates()
            };
            search::rank_apps(
                candidates.iter().chain(commands),
                query,
                &usage,
                &config.favorites,
            )
        });
    }

    let fixture = tempfile::tempdir()?;
    for directory in 0..count.div_ceil(100) {
        let root = fixture
            .path()
            .join(format!("project-{directory:04}/documents"));
        std::fs::create_dir_all(&root)?;
        for file in 0..100.min(count - directory * 100) {
            let name = if file % 10 == 0 {
                format!("项目报告-{file:03}.md")
            } else {
                format!("report-{file:03}-annual-summary.md")
            };
            std::fs::write(root.join(name), "")?;
        }
    }
    let before = LIVE.load(Relaxed);
    let index = search::file_index(&[fixture.path().to_owned()], &AtomicBool::new(false));
    let retained = LIVE.load(Relaxed).saturating_sub(before);
    println!(
        "file index/{} entries: {:.2} MiB retained Rust heap",
        index.len(),
        retained as f64 / 1024.0 / 1024.0,
    );
    for query in ["", "report", "rpeort", "项目", "zzzzzzzz"] {
        measure(&format!("files/{count}/{query:?}"), 3, || {
            index.search(query, &AtomicBool::new(false)).unwrap()
        });
    }
    Ok(())
}
