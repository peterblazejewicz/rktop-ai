# rktop-ai — Rockchip System & AI Accelerator Monitor

A high-performance system monitoring tool for Rockchip SoC devices (RK3576, RK3588, RK3399) and dedicated PCIe AI accelerator cards (RK1828, RK1820, RM1828MC0-F), written in Rust using the Ratatui TUI framework.

![rktop screenshot](screenshots/rktop.png)

### Extended Capabilities:
- **Host SoC Telemetry**: Real-time per-core CPU, Mali GPU, onboard RKNPU2, memory, RGA, and thermal sensors across RK3576/RK3588/RK3399 platforms.
- **Dedicated AI Coprocessors**: Real-time NPU utilization, dedicated DDR/VRAM usage, coprocessor CPU load, frequency, operating temperature, and power for Rockchip PCIe accelerator cards via `rknn-smi`.
- **Process & System Stats**: Interactive process sorting, system load averages, network I/O, and disk telemetry with <0.5% CPU overhead.

---

## Features

### Hardware Monitoring
- **CPU**: Per-core usage, frequencies, and time breakdown (user/system/iowait/idle)
- **GPU (Mali)**: Utilization percentage and frequency via Panthor or Bifrost devfreq/debugfs
- **Host NPU (RKNPU2)**: Per-core load (0–2 cores on RK3576, 0–3 cores on RK3588) and dynamic frequency
- **PCIe AI Accelerator (RK1828 / RM1828MC0-F)**:
  - Real-time NPU utilization percentage, clock frequency, and sparkline history
  - Dedicated onboard VRAM gauge (e.g. `1600 MB / 5120 MB` LPDDR)
  - Coprocessor CPU load and operating frequency
  - Board temperature, power consumption (mW), PCIe bus ID (`0000:01:00.0`), and card health
- **RGA**: 2D graphics accelerator scheduler load across all RGA cores
- **Memory**: RAM, Swap, and ZRAM usage with compression ratio and breakdown
- **Temperatures**: Thermal sensors for CPU, GPU, NPU, and ambient zones
- **Network & Disk I/O**: Real-time throughput rates per network adapter and storage device

### Process Information
- **Interactive Sorting**: Sort by CPU, Memory, PID, or Name (ascending/descending)
- **Detailed Metrics**: PID, User, Nice level, CPU core affinity, Runtime, CPU%, Memory%
- **Dynamic Display**: Adaptive layout fitting any terminal dimensions

### System Statistics & Version Probing
- System uptime, hostname, kernel release, and load average (1/5/15 min)
- CPU governor and frequency ranges (per cluster)
- Context switches, interrupts, and softirqs per second
- Active TCP connection count
- NPU and RGA kernel driver versions
- Host RKNN and RKLLM runtime library versions
- Dedicated AI Accelerator model and PCIe bus address

---

## Performance

Engineered for zero interference with edge AI workloads and zero thermal penalties:
- **<0.5% CPU Overhead**: Zero busy-loops; events synchronised with terminal ticks.
- **Cached File Descriptors (`src/file_cache.rs`)**: Retains open handles for sysfs and procfs nodes, avoiding repetitive `open()` / `close()` kernel context switches.
- **Streaming Telemetry Daemon (`src/accelerator.rs`)**: Spawns `/bin/rknn-smi info -w` once in a long-lived background thread instead of spawning expensive processes repeatedly.
- **Kernel-Guaranteed Process Safety**: Uses Unix `libc::prctl(PR_SET_PDEATHSIG, SIGTERM)` and explicit PID management to ensure child processes are never orphaned or left as zombies.
- **Dynamic Devfreq Path Scanning**: Discovers SoC frequency and load nodes once at startup with `OnceLock` caching.

---

## Installation

### Prerequisites
- Rockchip SoC device (RK3576, RK3588, RK3399, etc.)
- Linux with sysfs and debugfs mounted
- Rust toolchain (or prebuilt cross-compiled binary)

### Building from Source

```bash
# Clone the repository
git clone https://github.com/ajokela/rktop.git
cd rktop-ai

# Build optimized release binary
cargo build --release

# Binary will be at target/release/rktop-ai
```

### Cross-Compiling for aarch64 (Linux / WSL2)

Cross-compilation from Ubuntu/WSL2 targeting 64-bit ARM:

```bash
# Install GCC aarch64 toolchain
sudo apt-get install -y gcc-aarch64-linux-gnu

# Add Rust target
rustup target add aarch64-unknown-linux-gnu

# Build and strip
cargo build --target aarch64-unknown-linux-gnu --release
aarch64-linux-gnu-strip target/aarch64-unknown-linux-gnu/release/rktop-ai
```

### System-Wide Installation

```bash
# Copy to system path
sudo cp target/release/rktop-ai /usr/local/bin/
sudo chmod +x /usr/local/bin/rktop-ai
```

---

## Usage

### Basic Usage

```bash
# Run with root privileges (required for debugfs access)
sudo rktop-ai
```

### Keyboard Controls

| Key | Action |
|:---:|:---|
| `q`, `Q`, `Esc` | Quit application |
| `c` | Toggle CPU sort (ascending / descending) |
| `m` | Toggle Memory sort (ascending / descending) |
| `p` | Toggle PID sort (ascending / descending) |
| `n` | Toggle Name sort (ascending / descending) |

### Running Without Root (Optional)

You can grant specific capabilities to run without `sudo`:

```bash
sudo setcap cap_dac_read_search,cap_sys_ptrace=eip /usr/local/bin/rktop-ai
rktop-ai
```

---

## Display Panels

### CPU Panel
- Per-core usage bars with real-time frequency
- CPU time breakdown (User, System, IOWait, Idle percentages)
- Cluster frequency ranges (big.LITTLE)
- Process state counts, context switches, interrupts, and softirqs

### Memory Panel
- RAM usage (used + cached / total)
- Swap and ZRAM usage with real-time compression ratio

### GPU Panel (if available)
- Mali GPU utilization percentage and clock frequency (Panthor / Bifrost)

### Host NPU Panel (if available)
- Per-core onboard RKNPU2 load and frequency

### AI Accelerator Panel (if PCIe card detected)
- Dedicated NPU load bar with clock frequency
- Dedicated VRAM gauge (e.g. `1599 / 5120 MB`)
- Coprocessor CPU utilization and frequency
- Card temperature, PCIe bus ID, power draw, and health status
- Historical sparkline activity graph

### System Info Panel
- Board name, SoC model, hostname, kernel release, and CPU architecture
- NPU driver, RGA driver, RKNN runtime, and RKLLM runtime versions
- AI Card model (`RK1828`) and PCIe bus address (`0000:01:00.0`)

---

## Architecture

### Multi-Module Design

- **`src/main.rs`** — Ratatui TUI rendering, event loop, double-buffered terminal layouts, and state management.
- **`src/accelerator.rs`** — PCIe AI accelerator monitor, async stream parser, zero-orphan process lifecycle management.
- **`src/hardware.rs`** — Dynamic devfreq scanning, Rockchip SoC probing, RKNPU2, GPU, and RGA telemetry.
- **`src/file_cache.rs`** — Open file descriptor pool eliminating repetitive `open()` / `close()` syscalls.
- **`src/sysinfo_ext.rs`** — Extended system metrics (per-process stats, ZRAM, TCP connections).

---

## Supported Hardware

| Hardware | Support Details |
|:---|:---|
| **Rockchip RK3576** | Octa-core ARM (4x A72 + 4x A53), Mali-G52 MC3 (`27800000.gpu`), dual-core RKNPU2 (`27700000.npu`). Tested on DFRobot ACM3576. |
| **Rockchip RK3588 / RK3588S** | Octa-core ARM (4x A76 + 4x A55), Mali-G610 MC4 (`fb000000.gpu`), triple-core RKNPU2 (`fdab0000.npu`). Tested on Orange Pi 5 Max. |
| **Rockchip RK3399** | Hexa-core ARM (2x A72 + 4x A53), Mali-T860 MP4. |
| **RM1828MC0-F PCIe AI Card** | Dedicated RK1828 NPU chip, 5120 MB LPDDR VRAM, PCIe endpoint `/dev/pcie-rkep*`, vendor `/bin/rknn-smi` interface. |

---

## License

This project is licensed under the BSD 3-Clause License - see the [LICENSE](LICENSE) file for details.
