use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::System;

use crate::accelerator::AcceleratorMetrics;
use crate::hardware::{
    get_dmc_available_frequencies, get_dmc_available_governors, get_dmc_frequency,
    get_dmc_governor, get_gpu_frequency, get_gpu_usage, get_npu_frequency, get_npu_load,
    get_rga_load, get_thermal_cached,
};
use crate::sysinfo_ext::{get_tracked_processes, get_zram_info, ProcessAffinityProfile, ZramInfo};
use crate::AppState;

#[derive(Serialize)]
pub struct DmcSnapshot {
    pub freq_mhz: u32,
    pub governor: String,
    pub available_frequencies_mhz: Vec<u32>,
    pub available_governors: Vec<String>,
}

#[derive(Serialize)]
pub struct SystemSnapshot {
    pub timestamp_unix: u64,
    pub host: HostInfo,
    pub cpu: CpuSnapshot,
    pub memory: MemorySnapshot,
    pub dmc: Option<DmcSnapshot>,
    pub gpu: Option<GpuSnapshot>,
    pub host_npu: Option<NpuSnapshot>,
    pub accelerator: Option<AcceleratorMetrics>,
    pub rga: Option<HashMap<String, f32>>,
    pub thermals_celsius: HashMap<String, i32>,
    pub stats: StatsSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracked_processes: Option<Vec<ProcessAffinityProfile>>,
}

#[derive(Serialize)]
pub struct HostInfo {
    pub board: String,
    pub soc: String,
    pub hostname: String,
    pub kernel: String,
    pub architecture: String,
    pub npu_driver: String,
    pub rga_driver: String,
    pub rknn_runtime: String,
    pub rkllm_runtime: String,
}

#[derive(Serialize)]
pub struct CpuSnapshot {
    pub total_load_pct: f32,
    pub per_core_pct: Vec<f32>,
    pub per_core_freq_mhz: Vec<u32>,
    pub user_pct: f64,
    pub system_pct: f64,
    pub iowait_pct: f64,
    pub idle_pct: f64,
}

#[derive(Serialize)]
pub struct MemorySnapshot {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub zram: Option<ZramInfo>,
}

#[derive(Serialize)]
pub struct GpuSnapshot {
    pub usage_pct: f32,
    pub freq_mhz: Option<u32>,
}

#[derive(Serialize)]
pub struct NpuSnapshot {
    pub cores_pct: Vec<u8>,
    pub freq_mhz: Option<u32>,
}

#[derive(Serialize)]
pub struct StatsSnapshot {
    pub uptime_secs: u64,
    pub load_avg_1m: f64,
    pub load_avg_5m: f64,
    pub load_avg_15m: f64,
    pub running_processes: u64,
    pub blocked_processes: u64,
}

/// Collect a complete snapshot of all host and accelerator metrics
pub fn collect_snapshot(
    sys: &mut System,
    app_state: &mut AppState,
    tracked_patterns: &[String],
) -> SystemSnapshot {
    // Refresh CPU stats with two measurements to compute accurate usage
    sys.refresh_cpu_all();
    std::thread::sleep(Duration::from_millis(250));
    sys.refresh_cpu_all();
    sys.refresh_memory();
    if !tracked_patterns.is_empty() {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    }

    app_state.update_cpu_stats();
    app_state.update_stats();

    // Give background accelerator monitor time to collect first line if just spawned
    if let Some(ref monitor) = app_state.accelerator {
        if monitor.is_available() && monitor.get_metrics().is_none() {
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(50));
                if monitor.get_metrics().is_some() {
                    break;
                }
            }
        }
    }

    build_snapshot_internal(sys, app_state, tracked_patterns)
}

/// Fast snapshot builder without sleep (used by continuous streaming loop)
pub fn collect_snapshot_continuous(
    sys: &System,
    app_state: &AppState,
    tracked_patterns: &[String],
) -> SystemSnapshot {
    build_snapshot_internal(sys, app_state, tracked_patterns)
}

fn build_snapshot_internal(
    sys: &System,
    app_state: &AppState,
    tracked_patterns: &[String],
) -> SystemSnapshot {
    let timestamp_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let per_core_pct: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
    let per_core_freq_mhz: Vec<u32> = sys.cpus().iter().map(|c| c.frequency() as u32).collect();
    let total_load_pct = if !per_core_pct.is_empty() {
        per_core_pct.iter().sum::<f32>() / per_core_pct.len() as f32
    } else {
        (100.0 - app_state.cpu_idle_pct as f32).clamp(0.0, 100.0)
    };

    let host = HostInfo {
        board: app_state.board_name.clone(),
        soc: app_state.rk_model.clone(),
        hostname: app_state.hostname.clone(),
        kernel: app_state.kernel_version.clone(),
        architecture: app_state.cpu_arch.clone(),
        npu_driver: app_state.npu_version.clone(),
        rga_driver: app_state.rga_version.clone(),
        rknn_runtime: app_state.rknn_version.clone(),
        rkllm_runtime: app_state.rkllm_version.clone(),
    };

    let cpu = CpuSnapshot {
        total_load_pct,
        per_core_pct,
        per_core_freq_mhz,
        user_pct: app_state.cpu_user_pct,
        system_pct: app_state.cpu_system_pct,
        iowait_pct: app_state.cpu_iowait_pct,
        idle_pct: app_state.cpu_idle_pct,
    };

    let memory = MemorySnapshot {
        total_bytes: sys.total_memory(),
        used_bytes: sys.used_memory(),
        free_bytes: sys.free_memory(),
        swap_total_bytes: sys.total_swap(),
        swap_used_bytes: sys.used_swap(),
        zram: get_zram_info(),
    };

    let dmc = get_dmc_frequency().map(|freq_mhz| DmcSnapshot {
        freq_mhz,
        governor: get_dmc_governor().unwrap_or_else(|| "unknown".to_string()),
        available_frequencies_mhz: get_dmc_available_frequencies(),
        available_governors: get_dmc_available_governors(),
    });

    let gpu = get_gpu_usage().map(|usage_pct| GpuSnapshot {
        usage_pct,
        freq_mhz: get_gpu_frequency(),
    });

    let npu_loads = get_npu_load();
    let host_npu = if !npu_loads.is_empty() {
        Some(NpuSnapshot {
            cores_pct: npu_loads,
            freq_mhz: get_npu_frequency(),
        })
    } else {
        None
    };

    let accelerator = app_state
        .accelerator
        .as_ref()
        .and_then(|m| m.get_metrics());

    let rga = get_rga_load().map(|list| list.into_iter().collect::<HashMap<String, f32>>());

    let thermals_celsius: HashMap<String, i32> =
        get_thermal_cached(&app_state.thermal_zone_paths).into_iter().collect();

    let load_avg = System::load_average();
    let stats = StatsSnapshot {
        uptime_secs: System::uptime(),
        load_avg_1m: load_avg.one,
        load_avg_5m: load_avg.five,
        load_avg_15m: load_avg.fifteen,
        running_processes: app_state.running_procs,
        blocked_processes: app_state.blocked_procs,
    };

    let tracked_processes = if !tracked_patterns.is_empty() {
        let list = get_tracked_processes(sys, tracked_patterns);
        if !list.is_empty() {
            Some(list)
        } else {
            None
        }
    } else {
        None
    };

    SystemSnapshot {
        timestamp_unix,
        host,
        cpu,
        memory,
        dmc,
        gpu,
        host_npu,
        accelerator,
        rga,
        thermals_celsius,
        stats,
        tracked_processes,
    }
}

/// Write string to stdout, silently exiting with 0 on BrokenPipe (e.g. piped into head, jq failure)
fn write_to_stdout(content: &str) {
    use std::io::{self, Write};
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if let Err(e) = handle.write_all(content.as_bytes()).and_then(|_| handle.flush()) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("Error writing to stdout: {}", e);
        std::process::exit(1);
    }
}

/// Print formatted JSON snapshot to stdout and exit
pub fn print_json_snapshot(sys: &mut System, app_state: &mut AppState, tracked_patterns: &[String]) {
    let snapshot = collect_snapshot(sys, app_state, tracked_patterns);
    if let Ok(mut json_str) = serde_json::to_string_pretty(&snapshot) {
        json_str.push('\n');
        write_to_stdout(&json_str);
    } else {
        eprintln!("Error: Failed to serialize telemetry snapshot to JSON");
        std::process::exit(1);
    }
}

/// Stream continuous JSON snapshots at specified interval (NDJSON, sub-second capable)
pub fn stream_json_snapshots(
    sys: &mut System,
    app_state: &mut AppState,
    interval: Duration,
    tracked_patterns: &[String],
) {
    // Initial measurement
    sys.refresh_cpu_all();
    sys.refresh_memory();
    if !tracked_patterns.is_empty() {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    }
    app_state.update_cpu_stats();
    app_state.update_stats();

    // Give accelerator monitor time to collect initial line
    if let Some(ref monitor) = app_state.accelerator {
        if monitor.is_available() && monitor.get_metrics().is_none() {
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(50));
                if monitor.get_metrics().is_some() {
                    break;
                }
            }
        }
    }

    let find_pids = |system: &System| -> Vec<sysinfo::Pid> {
        let mut pids = Vec::new();
        for (pid, process) in system.processes() {
            let name = process.name().to_string_lossy();
            let cmdline = process.cmd().iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ");
            let matches = tracked_patterns.iter().any(|pat| {
                let pat_lower = pat.to_lowercase();
                name.to_lowercase().contains(&pat_lower) || cmdline.to_lowercase().contains(&pat_lower)
            });
            if matches {
                pids.push(*pid);
            }
        }
        pids
    };

    let mut tracked_pids = if !tracked_patterns.is_empty() {
        find_pids(sys)
    } else {
        Vec::new()
    };
    let mut last_full_proc_refresh = Instant::now();

    loop {
        std::thread::sleep(interval);

        sys.refresh_cpu_all();
        sys.refresh_memory();
        if !tracked_patterns.is_empty() {
            // Full process scan every 2 seconds to discover newly spawned processes without CPU spikes.
            // On sub-second ticks, refresh ONLY the specific tracked PIDs.
            if last_full_proc_refresh.elapsed() >= Duration::from_secs(2) || tracked_pids.is_empty() {
                sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
                tracked_pids = find_pids(sys);
                last_full_proc_refresh = Instant::now();
            } else {
                sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&tracked_pids), true);
            }
        }
        app_state.update_cpu_stats();
        app_state.update_stats();

        let snapshot = collect_snapshot_continuous(sys, app_state, tracked_patterns);
        if let Ok(mut json_str) = serde_json::to_string(&snapshot) {
            json_str.push('\n');
            write_to_stdout(&json_str);
        }
    }
}

/// Print clean ASCII plaintext snapshot to stdout and exit
pub fn print_oneshot_snapshot(sys: &mut System, app_state: &mut AppState, tracked_patterns: &[String]) {
    use std::fmt::Write as FmtWrite;
    let snapshot = collect_snapshot(sys, app_state, tracked_patterns);
    let mut out = String::new();

    let _ = writeln!(out, "================================================================================");
    let _ = writeln!(
        out,
        "rktop-ai System Snapshot | Host: {} | SoC: {} | Kernel: {}",
        snapshot.host.hostname, snapshot.host.soc, snapshot.host.kernel
    );
    let _ = writeln!(out, "Board: {}", snapshot.host.board);
    let _ = writeln!(out, "================================================================================");

    // CPU
    let cores_str: Vec<String> = snapshot
        .cpu
        .per_core_pct
        .iter()
        .map(|p| format!("{:.0}%", p))
        .collect();
    let _ = writeln!(
        out,
        "CPU:       {:.1}% ({} cores: {})",
        snapshot.cpu.total_load_pct,
        snapshot.cpu.per_core_pct.len(),
        cores_str.join(" ")
    );
    let _ = writeln!(
        out,
        "           User: {:.1}%  System: {:.1}%  IOWait: {:.1}%  Idle: {:.1}%",
        snapshot.cpu.user_pct, snapshot.cpu.system_pct, snapshot.cpu.iowait_pct, snapshot.cpu.idle_pct
    );

    // Memory
    let ram_used_gb = snapshot.memory.used_bytes as f64 / 1_073_741_824.0;
    let ram_total_gb = snapshot.memory.total_bytes as f64 / 1_073_741_824.0;
    let ram_pct = if ram_total_gb > 0.0 {
        (ram_used_gb / ram_total_gb) * 100.0
    } else {
        0.0
    };
    let _ = writeln!(
        out,
        "RAM:       {:.1} GB / {:.1} GB ({:.1}%)",
        ram_used_gb, ram_total_gb, ram_pct
    );

    // DMC (Host DDR)
    if let Some(ref dmc) = snapshot.dmc {
        let mut dmc_str = format!("{} MHz (governor: {})", dmc.freq_mhz, dmc.governor);
        if !dmc.available_frequencies_mhz.is_empty() {
            let freqs: Vec<String> = dmc.available_frequencies_mhz.iter().map(|f| f.to_string()).collect();
            dmc_str.push_str(&format!(" [available: {} MHz]", freqs.join(", ")));
        }
        let _ = writeln!(out, "DMC (DDR): {}", dmc_str);
    }

    // GPU
    if let Some(gpu) = snapshot.gpu {
        let freq_str = gpu.freq_mhz.map(|f| format!(" @ {} MHz", f)).unwrap_or_default();
        let _ = writeln!(out, "GPU:       Mali {:.2}%{}", gpu.usage_pct, freq_str);
    }

    // Host NPU
    if let Some(npu) = snapshot.host_npu {
        let freq_str = npu.freq_mhz.map(|f| format!(" @ {} MHz", f)).unwrap_or_default();
        let cores: Vec<String> = npu
            .cores_pct
            .iter()
            .enumerate()
            .map(|(i, &p)| format!("Core {}: {}%", i, p))
            .collect();
        let _ = writeln!(out, "Host NPU:  {}{}", cores.join(" | "), freq_str);
    }

    // PCIe AI Accelerator
    if let Some(acc) = snapshot.accelerator {
        let _ = writeln!(out, "--------------------------------------------------------------------------------");
        let _ = writeln!(
            out,
            "AI Card:   {} (PCIe: {}) - Status: {}",
            acc.chip_name, acc.bus_id, acc.health
        );
        let _ = writeln!(
            out,
            "           NPU:  {:>3}% @ {} MHz",
            acc.npu_load_pct, acc.npu_freq_mhz
        );
        let _ = writeln!(
            out,
            "           VRAM: {:>4} / {:>4} MB ({:.1}%)",
            acc.memory_used_mb,
            acc.memory_total_mb,
            if acc.memory_total_mb > 0 {
                (acc.memory_used_mb as f64 / acc.memory_total_mb as f64) * 100.0
            } else {
                0.0
            }
        );
        let _ = writeln!(
            out,
            "           CPU:  {:>3}% @ {} MHz",
            acc.cpu_load_pct, acc.cpu_freq_mhz
        );
        let temp_str = acc.temp_celsius.map(|t| format!("{}°C", t)).unwrap_or_else(|| "N/A".to_string());
        let power_str = acc.power_mw.map(|p| format!("{:.2}W", p as f64 / 1000.0)).unwrap_or_else(|| "N/A".to_string());
        let _ = writeln!(out, "           Temp: {} | Power: {}", temp_str, power_str);
    }

    // Thermals
    if !snapshot.thermals_celsius.is_empty() {
        let _ = writeln!(out, "--------------------------------------------------------------------------------");
        let mut sorted_thermals: Vec<(&String, &i32)> = snapshot.thermals_celsius.iter().collect();
        sorted_thermals.sort_by_key(|(k, _)| (*k).clone());
        let thermals_str: Vec<String> = sorted_thermals
            .iter()
            .map(|(name, temp)| format!("{}: {}°C", name, temp))
            .collect();
        let _ = writeln!(out, "Thermals:  {}", thermals_str.join("  "));
    }

    // Tracked Processes & Thread Core Placement
    if let Some(ref procs) = snapshot.tracked_processes {
        let _ = writeln!(out, "--------------------------------------------------------------------------------");
        let _ = writeln!(out, "Tracked Processes & Thread-to-Core Placement:");
        for p in procs {
            let _ = writeln!(
                out,
                "  [PID {:>5}] {:<18} | CPU: {:>5.1}% | RSS: {:>4} MB | Threads: {}",
                p.pid, p.name, p.cpu_pct, p.rss_mb, p.thread_count
            );
            let mut dist_str = Vec::new();
            for core_id in 0..8 {
                let key = format!("core{}", core_id);
                if let Some(&cnt) = p.core_distribution.get(&key) {
                    if cnt > 0 {
                        dist_str.push(format!("{}: {}", key, cnt));
                    }
                }
            }
            let _ = writeln!(
                out,
                "             Cores: A72 (big): {} | A53 (little): {} [{}]",
                p.big_cores_count, p.little_cores_count, dist_str.join(", ")
            );
            if !p.threads.is_empty() && p.threads.len() <= 16 {
                let th_strs: Vec<String> = p
                    .threads
                    .iter()
                    .map(|t| format!("TID {} ({}) -> Core {}", t.tid, t.name, t.cpu_core))
                    .collect();
                let _ = writeln!(out, "             {}", th_strs.join(" | "));
            }
        }
    }

    let _ = writeln!(out, "================================================================================");
    write_to_stdout(&out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_snapshot_serialization() {
        let snapshot = SystemSnapshot {
            timestamp_unix: 1728000000,
            host: HostInfo {
                board: "DFRobot ACM3576".to_string(),
                soc: "RK3576".to_string(),
                hostname: "rockchip".to_string(),
                kernel: "6.1.75".to_string(),
                architecture: "aarch64".to_string(),
                npu_driver: "0.9.8".to_string(),
                rga_driver: "1.2.3".to_string(),
                rknn_runtime: "2.3.0".to_string(),
                rkllm_runtime: "1.1.4".to_string(),
            },
            cpu: CpuSnapshot {
                total_load_pct: 12.5,
                per_core_pct: vec![10.0, 15.0, 20.0, 5.0],
                per_core_freq_mhz: vec![1800, 1800, 2200, 2200],
                user_pct: 8.0,
                system_pct: 4.5,
                iowait_pct: 0.0,
                idle_pct: 87.5,
            },
            memory: MemorySnapshot {
                total_bytes: 8589934592,
                used_bytes: 2147483648,
                free_bytes: 6442450944,
                swap_total_bytes: 0,
                swap_used_bytes: 0,
                zram: None,
            },
            dmc: Some(DmcSnapshot {
                freq_mhz: 1560,
                governor: "performance".to_string(),
                available_frequencies_mhz: vec![528, 1066, 1560, 2112],
                available_governors: vec!["performance".to_string(), "dmc_ondemand".to_string()],
            }),
            gpu: Some(GpuSnapshot {
                usage_pct: 0.0,
                freq_mhz: Some(300),
            }),
            host_npu: Some(NpuSnapshot {
                cores_pct: vec![0, 0],
                freq_mhz: Some(1000),
            }),
            accelerator: Some(AcceleratorMetrics {
                device_id: 0,
                chip_name: "RK1828".to_string(),
                bus_id: "0000:01:00.0".to_string(),
                health: "OK".to_string(),
                npu_load_pct: 0,
                npu_freq_mhz: 1000,
                memory_used_mb: 1638,
                memory_total_mb: 5120,
                cpu_load_pct: 0,
                cpu_freq_mhz: 1000,
                temp_celsius: Some(45),
                power_mw: None,
                work_mode: Some("PERFORMANCE".to_string()),
                ddr_freq_mhz: Some(1560),
                available_npu_freqs_mhz: vec![500, 650, 800, 850],
            }),
            rga: None,
            thermals_celsius: HashMap::from([
                ("soc".to_string(), 42),
                ("npu".to_string(), 40),
            ]),
            stats: StatsSnapshot {
                uptime_secs: 3600,
                load_avg_1m: 0.15,
                load_avg_5m: 0.25,
                load_avg_15m: 0.10,
                running_processes: 2,
                blocked_processes: 0,
            },
            tracked_processes: None,
        };

        let json = serde_json::to_string(&snapshot).expect("JSON serialization failed");
        assert!(json.contains("\"soc\":\"RK3576\""));
        assert!(json.contains("\"chip_name\":\"RK1828\""));
        assert!(json.contains("\"memory_total_mb\":5120"));
        assert!(json.contains("\"npu\":40"));

        let val: serde_json::Value = serde_json::from_str(&json).expect("JSON deserialization failed");
        assert_eq!(val["host"]["soc"], "RK3576");
        assert_eq!(val["accelerator"]["chip_name"], "RK1828");
        assert_eq!(val["cpu"]["total_load_pct"], 12.5);
        assert_eq!(val["dmc"]["freq_mhz"], 1560);
        assert_eq!(val["dmc"]["governor"], "performance");
    }

    #[test]
    fn test_tracked_processes_serialization() {
        use crate::sysinfo_ext::ThreadPlacement;

        let profile = ProcessAffinityProfile {
            pid: 1234,
            name: "rkllm3-server".to_string(),
            cmdline: "/usr/bin/rkllm3-server --threads 4".to_string(),
            cpu_pct: 98.5,
            rss_bytes: 1939865600,
            rss_mb: 1850,
            thread_count: 4,
            little_cores_count: 0,
            big_cores_count: 4,
            core_distribution: HashMap::from([
                ("core4".to_string(), 1),
                ("core5".to_string(), 1),
                ("core6".to_string(), 1),
                ("core7".to_string(), 1),
            ]),
            threads: vec![
                ThreadPlacement { tid: 1234, name: "rkllm3-server".to_string(), cpu_core: 4 },
                ThreadPlacement { tid: 1235, name: "worker-0".to_string(), cpu_core: 5 },
                ThreadPlacement { tid: 1236, name: "worker-1".to_string(), cpu_core: 6 },
                ThreadPlacement { tid: 1237, name: "worker-2".to_string(), cpu_core: 7 },
            ],
        };

        let json = serde_json::to_string(&profile).expect("Serialization failed");
        assert!(json.contains("\"name\":\"rkllm3-server\""));
        assert!(json.contains("\"big_cores_count\":4"));
        assert!(json.contains("\"little_cores_count\":0"));
        assert!(json.contains("\"cpu_core\":7"));
    }
}
