//! Bounded timing samples. No polling or logging while Lumen is idle.
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

#[derive(Default)]
struct Metrics {
    opening: Option<Instant>,
    query: Option<(String, Instant)>,
    opens: VecDeque<f64>,
    searches: VecDeque<f64>,
    displays: VecDeque<f64>,
    cpu: Option<(Instant, u64)>,
}
fn data() -> &'static Mutex<Metrics> {
    static DATA: OnceLock<Mutex<Metrics>> = OnceLock::new();
    DATA.get_or_init(|| Mutex::new(Metrics::default()))
}
fn push(samples: &mut VecDeque<f64>, value: f64) {
    if samples.len() == 128 {
        samples.pop_front();
    }
    samples.push_back(value);
}
pub fn opening() {
    data().lock().unwrap().opening = Some(Instant::now());
}
pub fn query(text: &str) {
    let mut d = data().lock().unwrap();
    d.query = (!text.is_empty() && !text.starts_with('/')).then(|| (text.into(), Instant::now()));
}
pub fn searched(ms: f64) {
    push(&mut data().lock().unwrap().searches, ms);
}
pub fn painted(query: &str, ready: bool) {
    let mut d = data().lock().unwrap();
    if let Some(start) = d.opening.take() {
        push(&mut d.opens, start.elapsed().as_secs_f64() * 1000.);
    }
    if ready && d.query.as_ref().is_some_and(|(q, _)| q == query) {
        let (_, start) = d.query.take().unwrap();
        push(&mut d.displays, start.elapsed().as_secs_f64() * 1000.);
    }
}
fn summary(samples: &VecDeque<f64>) -> String {
    if samples.is_empty() {
        return "No samples yet".into();
    }
    let mut ordered: Vec<f64> = samples.iter().copied().collect();
    ordered.sort_by(f64::total_cmp);
    format!(
        "Last {:.1} ms · p95 {:.1} ms · {} samples",
        samples.back().unwrap(),
        ordered[(ordered.len() * 95).div_ceil(100).saturating_sub(1)],
        samples.len()
    )
}
pub fn snapshot() -> Vec<(String, String)> {
    let mut d = data().lock().unwrap();
    let mut rows = vec![
        ("Open → drawn frame".into(), summary(&d.opens)),
        ("Local index search".into(), summary(&d.searches)),
        (
            "Query dispatch → drawn results".into(),
            summary(&d.displays),
        ),
    ];
    unsafe {
        let process = GetCurrentProcess();
        let mut memory = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        if GetProcessMemoryInfo(
            process,
            &mut memory,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .is_ok()
        {
            rows.push((
                "Memory in use".into(),
                format!(
                    "{:.1} MB working set",
                    memory.WorkingSetSize as f64 / 1048576.
                ),
            ));
        }
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user).is_ok() {
            let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
            let cpu = ticks(kernel) + ticks(user);
            let now = Instant::now();
            let note = d
                .cpu
                .map(|(at, previous)| {
                    format!(
                        "{:.2}% of one core since last /perf ({:.1}s)",
                        cpu.saturating_sub(previous) as f64
                            / 1e7
                            / now.duration_since(at).as_secs_f64().max(0.001)
                            * 100.,
                        now.duration_since(at).as_secs_f64()
                    )
                })
                .unwrap_or_else(|| "Run /perf again after leaving Lumen idle to sample CPU".into());
            d.cpu = Some((now, cpu));
            rows.push(("CPU usage".into(), note));
        }
    }
    rows
}
