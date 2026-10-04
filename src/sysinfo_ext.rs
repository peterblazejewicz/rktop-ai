use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use sysinfo::{Process, System};
use crate::ProcessSortMode;

// Global cache for UID to username mappings
static USER_CACHE: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub user: String,
    pub cpu: f32,
    pub mem: f32,
    pub nice: i32,
    pub runtime: u64, // in seconds
    pub cpu_core: u32, // Which CPU core process is running on
    pub is_thread: bool, // Is this a thread of another process?
    pub thread_group_id: u32, // TGID - the main process ID for threads
    pub state: char, // Process state: R, S, D, Z, T, etc.
    pub num_threads: u32, // Number of threads in this process
}

#[derive(Debug, Clone, Serialize)]
pub struct ZramInfo {
    pub orig_data_size: u64,
    pub compr_data_size: u64,
    pub used: u64,
    pub limit: u64,
}

impl ZramInfo {
    /// Calculate compression ratio (original / compressed)
    pub fn compression_ratio(&self) -> f64 {
        if self.compr_data_size > 0 {
            self.orig_data_size as f64 / self.compr_data_size as f64
        } else {
            0.0
        }
    }
}

/// Get top processes with configurable sorting
pub fn get_top_processes(sys: &System, count: usize, sort_mode: ProcessSortMode) -> Vec<ProcessInfo> {
    // First pass: collect minimal info and sort
    let mut minimal_processes: Vec<_> = sys
        .processes()
        .iter()
        .map(|(pid, process)| {
            (
                pid.as_u32(),
                process,
                process.cpu_usage(),
                process.memory() as f32 / sys.total_memory() as f32 * 100.0,
            )
        })
        .collect();

    // Sort the minimal list based on selected mode
    // Use unwrap_or(Equal) to safely handle potential NaN values in CPU/memory percentages
    use std::cmp::Ordering;
    match sort_mode {
        ProcessSortMode::CpuDesc => {
            minimal_processes.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal));
        }
        ProcessSortMode::CpuAsc => {
            minimal_processes.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(Ordering::Equal));
        }
        ProcessSortMode::MemoryDesc => {
            minimal_processes.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(Ordering::Equal));
        }
        ProcessSortMode::MemoryAsc => {
            minimal_processes.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(Ordering::Equal));
        }
        ProcessSortMode::PidAsc => {
            minimal_processes.sort_by_key(|p| p.0);
        }
        ProcessSortMode::PidDesc => {
            minimal_processes.sort_by_key(|a| std::cmp::Reverse(a.0));
        }
        ProcessSortMode::NameAsc => {
            minimal_processes.sort_by(|a, b| {
                a.1.name().to_string_lossy().to_lowercase()
                    .cmp(&b.1.name().to_string_lossy().to_lowercase())
            });
        }
        ProcessSortMode::NameDesc => {
            minimal_processes.sort_by(|a, b| {
                b.1.name().to_string_lossy().to_lowercase()
                    .cmp(&a.1.name().to_string_lossy().to_lowercase())
            });
        }
    }

    // Second pass: only read detailed info for top N processes
    minimal_processes
        .into_iter()
        .take(count)
        .map(|(pid_u32, process, cpu, mem)| {
            let name = process.name().to_string_lossy().to_string();
            let user = get_process_user(process);
            let runtime = process.run_time();

            // Only read extended info for top N processes
            let nice = get_process_nice(pid_u32);
            let cpu_core = get_process_cpu_core(pid_u32);
            let (thread_group_id, is_thread, num_threads, state, _num_fds) =
                get_process_extended_info(pid_u32);

            ProcessInfo {
                pid: pid_u32,
                name,
                user,
                cpu,
                mem,
                nice,
                runtime,
                cpu_core,
                is_thread,
                thread_group_id,
                state,
                num_threads,
            }
        })
        .collect()
}

/// Helper to extract fields after the command name from /proc/[pid]/stat content.
/// The command name in field 2 is enclosed in parentheses and may contain spaces or parentheses.
/// Returns the remaining whitespace-separated fields starting at field 3 (state).
pub fn parse_stat_after_comm(stat_content: &str) -> Option<Vec<&str>> {
    let rparen = stat_content.rfind(')')?;
    let rest = &stat_content[rparen + 1..];
    Some(rest.split_whitespace().collect())
}

fn get_process_nice(pid: u32) -> i32 {
    // Read nice level from /proc/<pid>/stat (field 19 -> index 16 after comm)
    let stat_path = format!("/proc/{}/stat", pid);
    if let Ok(content) = fs::read_to_string(&stat_path) {
        if let Some(fields) = parse_stat_after_comm(&content) {
            if fields.len() > 16 {
                if let Ok(nice) = fields[16].parse::<i32>() {
                    return nice;
                }
            }
        }
    }
    0 // Default nice value
}

fn get_process_cpu_core(pid: u32) -> u32 {
    // Read current CPU core from /proc/<pid>/stat (field 39 -> index 36 after comm)
    let stat_path = format!("/proc/{}/stat", pid);
    if let Ok(content) = fs::read_to_string(&stat_path) {
        if let Some(fields) = parse_stat_after_comm(&content) {
            if fields.len() > 36 {
                if let Ok(cpu_core) = fields[36].parse::<u32>() {
                    return cpu_core;
                }
            }
        }
    }
    0 // Default to core 0
}

/// Get consolidated process info from /proc/[pid]/status and /proc/[pid]/stat
/// Returns (tgid, is_thread, num_threads, state, num_fds)
/// This reads files once instead of multiple times
fn get_process_extended_info(pid: u32) -> (u32, bool, u32, char, u32) {
    let mut tgid = pid;
    let mut num_threads = 1;
    let mut state = 'U';

    // Read /proc/[pid]/status once for TGID and thread count
    let status_path = format!("/proc/{}/status", pid);
    if let Ok(content) = fs::read_to_string(&status_path) {
        for line in content.lines() {
            if line.starts_with("Tgid:") {
                if let Some(tgid_str) = line.split_whitespace().nth(1) {
                    if let Ok(parsed_tgid) = tgid_str.parse::<u32>() {
                        tgid = parsed_tgid;
                    }
                }
            } else if line.starts_with("Threads:") {
                if let Some(threads_str) = line.split_whitespace().nth(1) {
                    if let Ok(threads) = threads_str.parse::<u32>() {
                        num_threads = threads;
                    }
                }
            }
        }
    }

    let is_thread = pid != tgid;

    // Read /proc/[pid]/stat once for state
    let stat_path = format!("/proc/{}/stat", pid);
    if let Ok(content) = fs::read_to_string(&stat_path) {
        if let Some(fields) = parse_stat_after_comm(&content) {
            if let Some(state_char) = fields.first().and_then(|s| s.chars().next()) {
                state = state_char;
            }
        }
    }

    // Skip file descriptor counting entirely - it's very expensive
    // Counting FDs requires opening and iterating /proc/[pid]/fd directory
    // For a system with 200+ processes, this adds significant overhead
    let num_fds = 0;

    (tgid, is_thread, num_threads, state, num_fds)
}

fn get_process_user(process: &Process) -> String {
    if let Some(uid) = process.user_id() {
        let uid_num = uid.to_string().parse::<u32>().unwrap_or(0);

        // Try to get from cache first (safely recovering if lock was poisoned)
        let mut cache = USER_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if cache.is_none() {
            *cache = Some(HashMap::new());
        }

        if let Some(ref mut map) = *cache {
            // Check cache
            if let Some(username) = map.get(&uid_num) {
                return username.clone();
            }

            // Not in cache, read from /etc/passwd (faster than spawning 'id' command)
            if let Ok(passwd_content) = fs::read_to_string("/etc/passwd") {
                for line in passwd_content.lines() {
                    let parts: Vec<&str> = line.split(':').collect();
                    if parts.len() >= 3 {
                        if let Ok(line_uid) = parts[2].parse::<u32>() {
                            if line_uid == uid_num {
                                let username = parts[0].to_string();
                                map.insert(uid_num, username.clone());
                                return username;
                            }
                        }
                    }
                }
            }

            // Failed to resolve, cache the UID as string
            let uid_str = uid.to_string();
            map.insert(uid_num, uid_str.clone());
            return uid_str;
        }
    }
    "unknown".to_string()
}

/// Read ZRAM statistics
pub fn get_zram_info() -> Option<ZramInfo> {
    let path = "/sys/block/zram0/mm_stat";
    if let Ok(content) = fs::read_to_string(path) {
        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.len() >= 4 {
            return Some(ZramInfo {
                orig_data_size: parts[0].parse().ok()?,
                compr_data_size: parts[1].parse().ok()?,
                used: parts[2].parse().ok()?,
                limit: parts[3].parse().ok()?,
            });
        }
    }
    None
}

#[derive(Debug, Clone, Default)]
pub struct CpuStats {
    pub context_switches: u64,
    pub interrupts: u64,
    pub softirqs: u64,
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub running_procs: u64,
    pub blocked_procs: u64,
}

/// Read CPU statistics from /proc/stat
pub fn get_cpu_stats() -> CpuStats {
    let mut stats = CpuStats::default();

    if let Ok(content) = fs::read_to_string("/proc/stat") {
        for line in content.lines() {
            if line.starts_with("cpu ") {
                // Parse aggregate CPU time: user nice system idle iowait irq softirq...
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 8 {
                    stats.user = parts[1].parse().unwrap_or(0);
                    stats.nice = parts[2].parse().unwrap_or(0);
                    stats.system = parts[3].parse().unwrap_or(0);
                    stats.idle = parts[4].parse().unwrap_or(0);
                    stats.iowait = parts[5].parse().unwrap_or(0);
                    stats.irq = parts[6].parse().unwrap_or(0);
                    stats.softirq = parts[7].parse().unwrap_or(0);
                }
            } else if line.starts_with("ctxt ") {
                if let Some(value) = line.split_whitespace().nth(1) {
                    stats.context_switches = value.parse().unwrap_or(0);
                }
            } else if line.starts_with("intr ") {
                if let Some(value) = line.split_whitespace().nth(1) {
                    stats.interrupts = value.parse().unwrap_or(0);
                }
            } else if line.starts_with("softirq ") {
                if let Some(value) = line.split_whitespace().nth(1) {
                    stats.softirqs = value.parse().unwrap_or(0);
                }
            } else if line.starts_with("procs_running ") {
                if let Some(value) = line.split_whitespace().nth(1) {
                    stats.running_procs = value.parse().unwrap_or(0);
                }
            } else if line.starts_with("procs_blocked ") {
                if let Some(value) = line.split_whitespace().nth(1) {
                    stats.blocked_procs = value.parse().unwrap_or(0);
                }
            }
        }
    }

    stats
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadPlacement {
    pub tid: u32,
    pub name: String,
    pub cpu_core: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessAffinityProfile {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    pub cpu_pct: f32,
    pub rss_bytes: u64,
    pub rss_mb: u64,
    pub thread_count: usize,
    pub little_cores_count: usize, // Cores 0-3 (e.g. Cortex-A53)
    pub big_cores_count: usize,    // Cores 4-7 (e.g. Cortex-A72 / A76)
    pub core_distribution: HashMap<String, usize>,
    pub threads: Vec<ThreadPlacement>,
}

/// Read thread-level placement and affinity for a specific PID
pub fn get_process_threads_placement(pid: u32) -> (Vec<ThreadPlacement>, HashMap<String, usize>, usize, usize) {
    let mut threads = Vec::new();
    let mut distribution = HashMap::new();
    let mut little_count = 0;
    let mut big_count = 0;

    let task_dir = format!("/proc/{}/task", pid);
    if let Ok(entries) = fs::read_dir(task_dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let tid_str = file_name.to_string_lossy();
            if let Ok(tid) = tid_str.parse::<u32>() {
                let comm_path = entry.path().join("comm");
                let name = fs::read_to_string(&comm_path)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "unknown".to_string());

                let stat_path = entry.path().join("stat");
                let mut cpu_core = 0;
                if let Ok(stat_content) = fs::read_to_string(&stat_path) {
                    if let Some(fields) = parse_stat_after_comm(&stat_content) {
                        if fields.len() > 36 {
                            if let Ok(core) = fields[36].parse::<u32>() {
                                cpu_core = core;
                            }
                        }
                    }
                }

                if cpu_core < 4 {
                    little_count += 1;
                } else {
                    big_count += 1;
                }

                let core_key = format!("core{}", cpu_core);
                *distribution.entry(core_key).or_insert(0) += 1;

                threads.push(ThreadPlacement {
                    tid,
                    name,
                    cpu_core,
                });
            }
        }
    }

    threads.sort_by_key(|t| t.tid);
    (threads, distribution, little_count, big_count)
}

/// Parse /proc/[pid]/status content to determine if this PID is a secondary thread (i.e. Tgid != pid)
pub fn parse_tgid_is_thread(status_content: &str, pid: u32) -> bool {
    for line in status_content.lines() {
        if line.starts_with("Tgid:") {
            if let Some(tgid_str) = line.split_whitespace().nth(1) {
                if let Ok(tgid) = tgid_str.parse::<u32>() {
                    return pid != tgid;
                }
            }
            break;
        }
    }
    false
}

/// Check if a PID is a secondary thread (i.e. Tgid != PID in /proc/[pid]/status)
pub fn is_secondary_thread(pid: u32) -> bool {
    let status_path = format!("/proc/{}/status", pid);
    if let Ok(content) = fs::read_to_string(&status_path) {
        return parse_tgid_is_thread(&content, pid);
    }
    false
}

/// Check if a process matches any of the filter patterns.
/// Evaluates process name (comm), executable filename, and ALL command line arguments (e.g. script name or -m module).
/// Automatically excludes self (rktop-ai), wrapper tools (sudo, timeout, grep), and secondary threads.
pub fn process_matches_patterns(process: &Process, patterns: &[String], current_pid: u32) -> bool {
    let pid_u32 = process.pid().as_u32();

    // 1. Exclude self
    if pid_u32 == current_pid {
        return false;
    }

    // 2. Exclude secondary threads (group them under main process)
    if is_secondary_thread(pid_u32) {
        return false;
    }

    let name = process.name().to_string_lossy();
    let name_lower = name.to_lowercase();

    // 3. Exclude wrapper tools that might pass the pattern as an argument
    if name_lower == "sudo" || name_lower == "timeout" || name_lower == "rktop-ai" || name_lower == "grep" {
        return false;
    }

    let exe_name = process
        .exe()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    patterns.iter().any(|pat| {
        let pat_lower = pat.to_lowercase();
        // Check process name (comm) or executable filename
        if name_lower.contains(&pat_lower) || exe_name.to_lowercase().contains(&pat_lower) {
            return true;
        }
        // Check ALL command line arguments (matches e.g. "python3.13 -s -m functiongemma_rkllm", module names, or script paths)
        for arg in process.cmd() {
            if arg.to_string_lossy().to_lowercase().contains(&pat_lower) {
                return true;
            }
        }
        false
    })
}

/// Find matching processes by pattern (e.g. "rkllm", "proxy", "functiongemma_rkllm") and inspect their CPU, RSS, and thread affinity.
/// Groups threads under their main process PID and filters out wrapper commands (sudo, timeout, rktop-ai itself).
pub fn get_tracked_processes(sys: &System, patterns: &[String]) -> Vec<ProcessAffinityProfile> {
    if patterns.is_empty() {
        return Vec::new();
    }

    let mut profiles = Vec::new();
    let current_pid = std::process::id();

    for (pid, process) in sys.processes() {
        if process_matches_patterns(process, patterns, current_pid) {
            let pid_u32 = pid.as_u32();
            let name = process.name().to_string_lossy().to_string();
            let cmdline = process.cmd().iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ");
            let (threads, core_distribution, little_cores_count, big_cores_count) =
                get_process_threads_placement(pid_u32);
            let thread_count = if !threads.is_empty() { threads.len() } else { 1 };
            let rss_bytes = process.memory();
            let rss_mb = rss_bytes / (1024 * 1024);

            profiles.push(ProcessAffinityProfile {
                pid: pid_u32,
                name,
                cmdline,
                cpu_pct: process.cpu_usage(),
                rss_bytes,
                rss_mb,
                thread_count,
                little_cores_count,
                big_cores_count,
                core_distribution,
                threads,
            });
        }
    }

    profiles.sort_by(|a, b| b.cpu_pct.partial_cmp(&a.cpu_pct).unwrap_or(std::cmp::Ordering::Equal));
    profiles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stat_after_comm_standard() {
        // Standard /proc/pid/stat: pid comm(no space) state ppid ...
        // Index 0: state ('S')
        // Index 16: nice (field 19)
        // Index 36: processor (field 39)
        let sample = "12345 (bash) S 1234 12345 12345 34816 12345 4194304 382 0 0 0 2 1 0 0 20 0 1 0 123456 1234567 123 18446744073709551615 0 0 0 0 0 0 0 2147483647 0 0 0 0 17 6 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let fields = parse_stat_after_comm(sample).expect("should parse");
        assert_eq!(fields[0], "S"); // state
        assert_eq!(fields[16], "0"); // nice (field 19)
        assert_eq!(fields[36], "6"); // processor (field 39) -> core 6
    }

    #[test]
    fn test_parse_stat_after_comm_with_spaces_and_nested_parens() {
        // Process name with spaces and nested parentheses: (rkllm (worker) 0)
        let sample = "8812 (rkllm (worker) 0) R 8800 8812 8812 0 -1 4194304 50 0 0 0 150 20 0 0 10 -5 4 0 50000 90000 500 18446744073709551615 0 0 0 0 0 0 0 2147483647 0 0 0 0 17 5 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let fields = parse_stat_after_comm(sample).expect("should parse");
        assert_eq!(fields[0], "R"); // state is Running
        assert_eq!(fields[16], "-5"); // nice is -5
        assert_eq!(fields[36], "5"); // processor is core 5 (big core)
    }

    #[test]
    fn test_parse_stat_after_comm_malformed() {
        assert!(parse_stat_after_comm("invalid string without parens").is_none());
        assert!(parse_stat_after_comm("").is_none());
    }

    #[test]
    fn test_parse_tgid_is_thread() {
        // Main process: PID == TGID
        let status_main = "Name:\trkllm3-server\nUmask:\t0022\nState:\tS (sleeping)\nTgid:\t27719\nNgid:\t0\nPid:\t27719\n";
        assert!(!parse_tgid_is_thread(status_main, 27719));

        // Secondary worker thread: PID != TGID
        let status_thread = "Name:\trkllm3-server\nUmask:\t0022\nState:\tR (running)\nTgid:\t27719\nNgid:\t0\nPid:\t27899\n";
        assert!(parse_tgid_is_thread(status_thread, 27899));
    }
}
