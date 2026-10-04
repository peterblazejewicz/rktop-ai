use regex::Regex;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Telemetry metrics for PCIe AI Accelerator (e.g. RK1828)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AcceleratorMetrics {
    pub device_id: u32,
    pub chip_name: String,       // e.g. "RK1828"
    pub bus_id: String,          // e.g. "0000:01:00.0"
    pub temp_celsius: Option<i32>,
    pub power_mw: Option<u32>,
    pub cpu_load_pct: u8,
    pub cpu_freq_mhz: u32,
    pub npu_load_pct: u8,
    pub npu_freq_mhz: u32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub health: String,          // e.g. "OK"
}

/// Check if a PCIe AI accelerator (RK1828 or similar) is present on the system.
/// Requires both the SMI vendor utility AND physical PCIe device/driver presence.
pub fn is_accelerator_present() -> bool {
    // 1. Must have the vendor SMI utility
    if !Path::new("/bin/rknn-smi").exists() {
        return false;
    }

    // 2. Must have PCIe endpoint device node in /dev
    let has_dev_node = fs::read_dir("/dev")
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name().to_string_lossy().starts_with("pcie-rkep")
            })
        })
        .unwrap_or(false);

    if has_dev_node {
        return true;
    }

    // 3. Or vendor driver bound in sysfs
    if Path::new("/sys/bus/pci/drivers/pcie-rkep").exists() {
        return true;
    }

    false
}

/// Query static accelerator details (Chip Name, Bus-Id) using `rknn-smi info`.
/// Note: rknn-smi info may exit with status 251 (0xFB) despite valid output.
pub fn query_accelerator_static_info() -> Option<(String, String)> {
    let output = Command::new("/bin/rknn-smi")
        .arg("info")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let text = String::from_utf8_lossy(&output.stdout);
    parse_static_info(&text)
}

/// Parse chip name and bus ID from `rknn-smi info` table output.
/// Example line in table:
/// `| 0             RK1828   | 0000:01:00.0  | 42            | 1599 / 5120          |`
pub fn parse_static_info(output: &str) -> Option<(String, String)> {
    static CHIP_RE: OnceLock<Regex> = OnceLock::new();
    let re = CHIP_RE.get_or_init(|| {
        Regex::new(r"\|\s*\d+\s+([A-Za-z0-9_-]+)\s+\|\s*([0-9a-fA-F:.]+)\s*\|").unwrap()
    });

    for line in output.lines() {
        if let Some(caps) = re.captures(line) {
            let chip = caps.get(1)?.as_str().trim().to_string();
            let bus = caps.get(2)?.as_str().trim().to_string();
            return Some((chip, bus));
        }
    }

    None
}

/// Parse a single telemetry line from `/bin/rknn-smi info -w`
/// Example line:
/// `0           0            NA      43     0% - 1000        0% - 850         32% - 5120           OK      `
pub fn parse_watch_line(line: &str, chip_name: &str, bus_id: &str) -> Option<AcceleratorMetrics> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("Device")
        || trimmed.starts_with('+')
        || trimmed.starts_with('=')
        || trimmed.starts_with('|')
    {
        return None;
    }

    // Pattern matching:
    // Device(Idx) Chip(Idx) Power(mW) Temp(C) CPU(%)-Freq(MHz) NPU(%)-Freq(MHz) Memory(%)-Total(GB) Health
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^\s*(\d+)\s+(\d+)\s+(\S+)\s+(\S+)\s+(\d+)%\s*-\s*(\d+)\s+(\d+)%\s*-\s*(\d+)\s+(\d+)%\s*-\s*(\d+)\s+(\S+)"
        ).unwrap()
    });

    if let Some(caps) = re.captures(trimmed) {
        let device_id = caps.get(1)?.as_str().parse::<u32>().ok()?;
        // caps.get(2) is chip_id
        let power_str = caps.get(3)?.as_str();
        let power_mw = power_str.parse::<u32>().ok();
        let temp_str = caps.get(4)?.as_str();
        let temp_celsius = temp_str.parse::<i32>().ok();
        let cpu_load_pct = caps.get(5)?.as_str().parse::<u8>().ok()?;
        let cpu_freq_mhz = caps.get(6)?.as_str().parse::<u32>().ok()?;
        let npu_load_pct = caps.get(7)?.as_str().parse::<u8>().ok()?;
        let npu_freq_mhz = caps.get(8)?.as_str().parse::<u32>().ok()?;
        let memory_pct = caps.get(9)?.as_str().parse::<u64>().ok()?;
        let memory_total_mb = caps.get(10)?.as_str().parse::<u64>().ok()?;
        let health = caps.get(11)?.as_str().to_string();

        let memory_used_mb = memory_pct
            .saturating_mul(memory_total_mb)
            .saturating_add(50)
            / 100;

        Some(AcceleratorMetrics {
            device_id,
            chip_name: chip_name.to_string(),
            bus_id: bus_id.to_string(),
            temp_celsius,
            power_mw,
            cpu_load_pct,
            cpu_freq_mhz,
            npu_load_pct,
            npu_freq_mhz,
            memory_used_mb,
            memory_total_mb,
            health,
        })
    } else {
        // Fallback: tokenized whitespace parser for non-standard whitespace/formatting
        parse_watch_line_tokens(trimmed, chip_name, bus_id)
    }
}

/// Fallback tokenizer for non-standard watch output format
fn parse_watch_line_tokens(trimmed: &str, chip_name: &str, bus_id: &str) -> Option<AcceleratorMetrics> {
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.len() < 8 {
        return None;
    }

    let device_id = tokens[0].parse::<u32>().ok()?;
    let power_mw = tokens[2].parse::<u32>().ok();
    let temp_celsius = tokens[3].parse::<i32>().ok();
    let health = tokens.last()?.to_string();

    let mut pct_freq_pairs = Vec::new();
    let mut i = 4;
    while i < tokens.len() - 1 {
        let tok = tokens[i];
        if tok.ends_with('%') {
            if let Ok(pct) = tok.trim_end_matches('%').parse::<u64>() {
                if i + 1 < tokens.len() && tokens[i + 1] == "-" && i + 2 < tokens.len() {
                    if let Ok(val) = tokens[i + 2].parse::<u64>() {
                        pct_freq_pairs.push((pct, val));
                        i += 3;
                        continue;
                    }
                } else if i + 1 < tokens.len() {
                    if let Ok(val) = tokens[i + 1].trim_start_matches('-').parse::<u64>() {
                        pct_freq_pairs.push((pct, val));
                        i += 2;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }

    if pct_freq_pairs.len() >= 3 {
        let (cpu_load, cpu_freq) = pct_freq_pairs[0];
        let (npu_load, npu_freq) = pct_freq_pairs[1];
        let (mem_pct, mem_total) = pct_freq_pairs[2];
        let memory_used_mb = mem_pct.saturating_mul(mem_total).saturating_add(50) / 100;

        Some(AcceleratorMetrics {
            device_id,
            chip_name: chip_name.to_string(),
            bus_id: bus_id.to_string(),
            temp_celsius,
            power_mw,
            cpu_load_pct: cpu_load as u8,
            cpu_freq_mhz: cpu_freq as u32,
            npu_load_pct: npu_load as u8,
            npu_freq_mhz: npu_freq as u32,
            memory_used_mb,
            memory_total_mb: mem_total,
            health,
        })
    } else {
        None
    }
}

/// Long-lived background monitor streaming telemetry from `/bin/rknn-smi info -w`
pub struct AcceleratorMonitor {
    latest: Arc<RwLock<Option<AcceleratorMetrics>>>,
    running: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    bus_id: String,
    chip_name: String,
    available: bool,
    _worker: Option<JoinHandle<()>>,
}

impl AcceleratorMonitor {
    fn disabled() -> Self {
        Self {
            latest: Arc::new(RwLock::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            child_pid: Arc::new(Mutex::new(None)),
            bus_id: String::new(),
            chip_name: String::new(),
            available: false,
            _worker: None,
        }
    }

    pub fn new() -> Self {
        if !is_accelerator_present() {
            return Self::disabled();
        }

        // Query static info to confirm hardware responsiveness
        let (chip_name, bus_id) = match query_accelerator_static_info() {
            Some(info) => info,
            None => {
                // If SMI command failed or card did not respond, disable monitor
                return Self::disabled();
            }
        };

        let latest = Arc::new(RwLock::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let child_pid = Arc::new(Mutex::new(None));

        let latest_clone = Arc::clone(&latest);
        let running_clone = Arc::clone(&running);
        let child_pid_clone = Arc::clone(&child_pid);
        let chip_name_worker = chip_name.clone();
        let bus_id_worker = bus_id.clone();

        let worker = std::thread::Builder::new()
            .name("rknn-smi-stream".to_string())
            .spawn(move || {
                stream_worker(
                    latest_clone,
                    running_clone,
                    child_pid_clone,
                    chip_name_worker,
                    bus_id_worker,
                );
            })
            .ok();

        Self {
            latest,
            running,
            child_pid,
            bus_id,
            chip_name,
            available: true,
            _worker: worker,
        }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    pub fn bus_id(&self) -> &str {
        &self.bus_id
    }

    pub fn chip_name(&self) -> &str {
        &self.chip_name
    }

    pub fn get_metrics(&self) -> Option<AcceleratorMetrics> {
        self.latest.read().ok().and_then(|guard| guard.clone())
    }
}

impl Drop for AcceleratorMonitor {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        // Terminate child process to unblock reader.lines()
        if let Ok(guard) = self.child_pid.lock() {
            if let Some(pid) = *guard {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
            }
        }

        // Cleanly join worker thread
        if let Some(handle) = self._worker.take() {
            let _ = handle.join();
        }
    }
}

/// Worker loop that maintains the long-lived `rknn-smi info -w` process
fn stream_worker(
    latest: Arc<RwLock<Option<AcceleratorMetrics>>>,
    running: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    chip_name: String,
    bus_id: String,
) {
    let mut consecutive_failures: u32 = 0;

    while running.load(Ordering::Acquire) {
        if consecutive_failures >= 5 {
            // Hardware unresponsive or crash-looping; stop worker to prevent CPU drain
            break;
        }

        let mut cmd = Command::new("/bin/rknn-smi");
        cmd.args(["info", "-w"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                // Ensure kernel sends SIGTERM to child if parent process exits unexpectedly
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
                Ok(())
            });
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => {
                consecutive_failures += 1;
                std::thread::sleep(Duration::from_secs(1 << consecutive_failures.min(4)));
                continue;
            }
        };

        if let Ok(mut pid_guard) = child_pid.lock() {
            *pid_guard = Some(child.id());
        }

        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                if let Ok(mut pid_guard) = child_pid.lock() {
                    *pid_guard = None;
                }
                consecutive_failures += 1;
                std::thread::sleep(Duration::from_secs(1 << consecutive_failures.min(4)));
                continue;
            }
        };

        let reader = BufReader::new(stdout);
        let mut parsed_any_line = false;

        for line in reader.lines() {
            if !running.load(Ordering::Acquire) {
                break;
            }
            match line {
                Ok(line_str) => {
                    if let Some(metrics) = parse_watch_line(&line_str, &chip_name, &bus_id) {
                        if let Ok(mut lock) = latest.write() {
                            *lock = Some(metrics);
                        }
                        parsed_any_line = true;
                    }
                }
                Err(_) => break,
            }
        }

        let _ = child.kill();
        let _ = child.wait();

        if let Ok(mut pid_guard) = child_pid.lock() {
            *pid_guard = None;
        }

        if parsed_any_line {
            consecutive_failures = 0;
        } else {
            consecutive_failures += 1;
        }

        if running.load(Ordering::Acquire) {
            let backoff_secs = if consecutive_failures > 0 {
                1 << consecutive_failures.min(4)
            } else {
                1
            };
            std::thread::sleep(Duration::from_secs(backoff_secs));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_watch_line_actual_live() {
        let line = "0           0            NA      43     0% - 1000        0% - 850         32% - 5120           OK      ";
        let parsed = parse_watch_line(line, "RK1828", "0000:01:00.0").expect("Must parse live line");

        assert_eq!(parsed.device_id, 0);
        assert_eq!(parsed.chip_name, "RK1828");
        assert_eq!(parsed.bus_id, "0000:01:00.0");
        assert_eq!(parsed.temp_celsius, Some(43));
        assert_eq!(parsed.power_mw, None);
        assert_eq!(parsed.cpu_load_pct, 0);
        assert_eq!(parsed.cpu_freq_mhz, 1000);
        assert_eq!(parsed.npu_load_pct, 0);
        assert_eq!(parsed.npu_freq_mhz, 850);
        assert_eq!(parsed.memory_total_mb, 5120);
        assert_eq!(parsed.memory_used_mb, 1638);
        assert_eq!(parsed.health, "OK");
    }

    #[test]
    fn test_parse_watch_line_with_load() {
        let line = "0           0            1200    52    25% - 1200       80% - 850         40% - 5120           OK";
        let parsed = parse_watch_line(line, "RK1828", "0000:01:00.0").expect("Must parse load line");

        assert_eq!(parsed.device_id, 0);
        assert_eq!(parsed.temp_celsius, Some(52));
        assert_eq!(parsed.power_mw, Some(1200));
        assert_eq!(parsed.cpu_load_pct, 25);
        assert_eq!(parsed.cpu_freq_mhz, 1200);
        assert_eq!(parsed.npu_load_pct, 80);
        assert_eq!(parsed.npu_freq_mhz, 850);
        assert_eq!(parsed.memory_used_mb, 2048);
        assert_eq!(parsed.health, "OK");
    }

    #[test]
    fn test_parse_watch_line_na_temp() {
        let line = "0           0            NA      NA     0% - 1000        0% - 850         32% - 5120           OK";
        let parsed = parse_watch_line(line, "RK1828", "0000:01:00.0").expect("Must parse line with NA temp");

        assert_eq!(parsed.temp_celsius, None);
        assert_eq!(parsed.power_mw, None);
        assert_eq!(parsed.health, "OK");
    }

    #[test]
    fn test_parse_watch_line_skip_headers() {
        let header = "Device(Idx) Chip(Idx) Power(mW) Temp(C) CPU(%)-Freq(MHz) NPU(%)-Freq(MHz)  Memory(%)-Total(GB) Health  ";
        assert!(parse_watch_line(header, "RK1828", "0000:01:00.0").is_none());

        let border = "+------------------------+---------------+---------------+----------------------+";
        assert!(parse_watch_line(border, "RK1828", "0000:01:00.0").is_none());

        let empty = "   \n";
        assert!(parse_watch_line(empty, "RK1828", "0000:01:00.0").is_none());
    }

    #[test]
    fn test_parse_static_info() {
        let sample = r#"
+------------------------+---------------+---------------+----------------------+
| rknn-smi      Version: 1.3.0                                                  |
+========================+===============+===============+======================+
| Device        Status   | Health        | Power(mW)     | Npu(%)               |
| Chip          Name     | Bus-Id        | Temp(C)       | Memory-Usage(MB)     |
+========================+===============+===============+======================+
| 0             Online   | OK            | NA            | 0                    |
| 0             RK1828   | 0000:01:00.0  | 42            | 1599 / 5120          |
+========================+===============+===============+======================+
"#;
        let (chip, bus) = parse_static_info(sample).expect("Must parse static info");
        assert_eq!(chip, "RK1828");
        assert_eq!(bus, "0000:01:00.0");
    }

    #[test]
    fn test_parse_watch_line_tokens_fallback() {
        // Line with non-standard spacing that might bypass strict regex
        let line = "0   0   1500   48   10% - 800   50% - 900   20% - 5120   OK";
        let parsed = parse_watch_line(line, "RK1828", "0000:01:00.0").expect("Must parse via fallback or regex");

        assert_eq!(parsed.device_id, 0);
        assert_eq!(parsed.power_mw, Some(1500));
        assert_eq!(parsed.temp_celsius, Some(48));
        assert_eq!(parsed.cpu_load_pct, 10);
        assert_eq!(parsed.cpu_freq_mhz, 800);
        assert_eq!(parsed.npu_load_pct, 50);
        assert_eq!(parsed.npu_freq_mhz, 900);
        assert_eq!(parsed.memory_total_mb, 5120);
        assert_eq!(parsed.memory_used_mb, 1024);
        assert_eq!(parsed.health, "OK");
    }
}
