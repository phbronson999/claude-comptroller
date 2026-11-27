//! The Commands panel's record: every command the admin terminal runs, written the way an
//! admin would type it, with its result. The terminal mostly calls Windows and HTTP directly;
//! each entry is the hand-typed equivalent, so anyone can copy it and check for themselves.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const KEEP: usize = 200;
/// How far back a finished command with the same result is merged into instead of repeated.
const MERGE_WINDOW: usize = 12;

#[derive(Clone, Debug)]
pub struct Outcome {
    pub ok: bool,
    pub summary: String,
    pub took: Duration,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    /// Local time it last ran, `HH:MM:SS`.
    pub clock: String,
    pub command: String,
    pub started: Instant,
    pub outcome: Option<Outcome>,
    /// How many times it ran with this same result.
    pub repeats: u32,
    /// A routine status check, not something the admin asked for.
    pub background: bool,
}

thread_local! {
    static BACKGROUND: Cell<bool> = const { Cell::new(false) };
}

/// Marks everything this thread records as a routine background check.
pub fn mark_thread_background() {
    BACKGROUND.with(|b| b.set(true));
}

#[derive(Default)]
struct Log {
    next: u64,
    entries: VecDeque<Entry>,
}

fn log() -> &'static Mutex<Log> {
    static LOG: OnceLock<Mutex<Log>> = OnceLock::new();
    LOG.get_or_init(Mutex::default)
}

fn clock() -> String {
    // SAFETY: fills a plain struct.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

pub fn start(command: impl Into<String>) -> u64 {
    let mut log = log().lock().unwrap_or_else(|p| p.into_inner());
    log.next += 1;
    let id = log.next;
    let background = BACKGROUND.with(Cell::get);
    log.entries.push_back(Entry { id, clock: clock(), command: command.into(), started: Instant::now(), outcome: None, repeats: 1, background });
    while log.entries.len() > KEEP {
        log.entries.pop_front();
    }
    id
}

pub fn finish(id: u64, ok: bool, summary: impl Into<String>) {
    let summary = summary.into();
    let mut log = log().lock().unwrap_or_else(|p| p.into_inner());
    let Some(index) = log.entries.iter().position(|e| e.id == id) else { return };
    let took = log.entries[index].started.elapsed();
    let command = log.entries[index].command.clone();

    // The same check with the same result folds into the earlier line.
    let background = log.entries[index].background;
    let earlier = (index.saturating_sub(MERGE_WINDOW)..index).rev().find(|&i| {
        let e = &log.entries[i];
        e.command == command && e.background == background && e.outcome.as_ref().is_some_and(|o| o.ok == ok && o.summary == summary)
    });
    match earlier {
        Some(i) => {
            let now = clock();
            log.entries.remove(index);
            let entry = &mut log.entries[i];
            entry.repeats += 1;
            entry.clock = now;
            if let Some(outcome) = &mut entry.outcome {
                outcome.took = took;
            }
        }
        None => log.entries[index].outcome = Some(Outcome { ok, summary, took }),
    }
}

/// Runs `work`, recording `command` and a one-line summary of what came back.
pub fn track<T, E: ToString>(command: impl Into<String>, work: impl FnOnce() -> Result<T, E>, summary: impl FnOnce(&T) -> String) -> Result<T, E> {
    let id = start(command);
    let result = work();
    match &result {
        Ok(value) => finish(id, true, summary(value)),
        Err(error) => finish(id, false, error.to_string()),
    }
    result
}

/// The newest `count` entries, oldest first.
pub fn recent(count: usize) -> Vec<Entry> {
    let log = log().lock().unwrap_or_else(|p| p.into_inner());
    log.entries.iter().skip(log.entries.len().saturating_sub(count)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mine(prefix: &str) -> Vec<Entry> {
        recent(KEEP).into_iter().filter(|e| e.command.starts_with(prefix)).collect()
    }

    #[test]
    fn repeated_checks_fold_into_one_line_until_the_result_changes() {
        for _ in 0..3 {
            let _ = track("test-fold sc.exe query x", || Ok::<_, String>("RUNNING"), |s| s.to_string());
        }
        let _ = track("test-fold sc.exe query x", || Ok::<_, String>("STOPPED"), |s| s.to_string());
        let _ = track("test-fold sc.exe query x", || Err::<&str, _>("access denied"), |s| s.to_string());

        let entries = mine("test-fold");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].repeats, 3);
        assert_eq!(entries[1].outcome.as_ref().unwrap().summary, "STOPPED");
        assert!(!entries[2].outcome.as_ref().unwrap().ok);
    }

    #[test]
    fn a_command_still_running_has_no_outcome() {
        let id = start("test-running sc.exe start x");
        assert!(mine("test-running")[0].outcome.is_none());
        finish(id, true, "started");
        assert!(mine("test-running")[0].outcome.as_ref().unwrap().ok);
    }
}
