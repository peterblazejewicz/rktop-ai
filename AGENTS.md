# AGENTS.md - Cross-Agent Development Guide for rktop-ai

## 1. Multi-Agent Collaboration Model (x-Agent Workflow)

`rktop-ai` is developed using a paired cross-agent (x-agent) workflow between **Antigravity (Agy)** and **Claude**:

```
+-------------------------------------------------------------------------+
|                  Antigravity (Agy) Session                              |
|  - Host Environment: Windows 11 / WSL2 (Ubuntu 24.04 Noble)             |
|  - Role: Codebase architect, core Rust development, cross-compiler     |
|  - Responsibilities:                                                    |
|      * Maintain and optimize Ratatui TUI architecture                   |
|      * Cross-compile release binaries for target aarch64-linux-gnu      |
|      * Run unit tests via WSL2 and manage git repositories              |
+-------------------------------------------------------------------------+
                                    │
                                    │ Artifacts: target/aarch64-.../rktop-ai
                                    │ [CRITICAL SAFETY GATE: User Confirmation Required]
                                    ▼
+-------------------------------------------------------------------------+
|                    Claude Session (Board Target)                        |
|  - Host Environment: Rockchip RK3576 / RK3588 (SSH / Direct Terminal)   |
|  - Role: Live target profiling, hardware validation, board optimization |
|  - Responsibilities:                                                    |
|      * Validate sysfs paths and devfreq nodes on live kernel            |
|      * Verify RM1828MC0-F PCIe AI accelerator telemetry stream          |
|      * Profile CPU footprint (ensuring < 0.5% CPU overhead)             |
|      * Report board discrepancies back to codebase developer            |
+-------------------------------------------------------------------------+
```

---

## 2. Crucial Safety Gate (Deployment Policy)

> [!CAUTION]
> **SAFETY GATE: NEVER TRANSFER FILES, SCP BINARIES, OR MODIFY BOARD FILES WITHOUT EXPLICIT USER CONFIRMATION.**
> - Neither Antigravity nor Claude may automatically execute `scp`, `rsync`, `ssh`, or file deployment to the target board without asking for and receiving explicit user approval.
> - Always present the command, destination, and payload size to the user first.
> - Do not modify `/etc/systemd`, bootloaders, u-boot, or root partition configuration on the board without explicit confirmation.

---

## 3. Target Platform Architecture

`rktop-ai` is specialized for high-performance monitoring on Rockchip embedded platforms:

| Component | Specification | Details & Monitoring Interfaces |
| :--- | :--- | :--- |
| **Host SoC** | **Rockchip RK3576** | Octa-core ARM (4x Cortex-A72 @ 2.2 GHz + 4x Cortex-A53 @ 1.8 GHz) |
| **Device Model** | **DFRobot ACM3576** / generic | Compatible strings: `rockchip,rk3576`, board string parsed with `ACM(\d+)` fallback |
| **Host GPU** | **ARM Mali-G52 MC3** | Devfreq at `/sys/class/devfreq/*.gpu/cur_freq` and `load` (Panthor / Bifrost) |
| **Host NPU** | **Rockchip RKNPU2** | Up to 6 TOPS, dual-core; load at `/sys/kernel/debug/rknpu/load`, devfreq at `/sys/class/devfreq/*.npu/cur_freq` |
| **AI Accelerator** | **RM1828MC0-F** | PCIe expansion card with **RK1828** dedicated NPU chip |
| **Accelerator RAM**| **5120 MB LPDDR** | 5 GB dedicated VRAM reported via `/bin/rknn-smi` |
| **PCIe Bus / Dev** | `0000:01:00.0` | `/dev/pcie-rkep*`, `/sys/bus/pci/drivers/pcie-rkep` |

---

## 4. Toolchain and Cross-Compilation Runbook

Cross-compilation is executed on the Windows workstation using **WSL2 (Ubuntu 24.04 LTS Noble)**:

### Prerequisites (inside WSL2)
```bash
# Install GCC aarch64 cross-compiler and libc
sudo apt-get update && sudo apt-get install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu

# Add Rust aarch64 target
rustup target add aarch64-unknown-linux-gnu
```

### Cargo Linker Configuration
Configured in `.cargo/config.toml`:
```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
```

### Build Commands
From Windows PowerShell:
```powershell
# Build debug binary
wsl bash -c "cd /mnt/d/develop/rktop-ai && cargo build --target aarch64-unknown-linux-gnu"

# Build optimized release binary
wsl bash -c "cd /mnt/d/develop/rktop-ai && cargo build --target aarch64-unknown-linux-gnu --release"

# Strip binary for minimal footprint
wsl bash -c "aarch64-linux-gnu-strip /mnt/d/develop/rktop-ai/target/aarch64-unknown-linux-gnu/release/rktop-ai"
```

Resulting binary: `target/aarch64-unknown-linux-gnu/release/rktop-ai` (~2.4 MB stripped).

---

## 5. Rust Development & Performance Guidelines

To run continuously on edge devices without skewing performance benchmarks or generating thermal throttling, `rktop-ai` adheres to strict performance constraints:

### 1. Target CPU Footprint: < 0.5% Overhead
- No busy-loops or tight polling.
- Thread sleeps synchronize to terminal event polling intervals.
- Low-frequency metrics (ZRAM, disks, networks, driver versions) are polled at tiered intervals (2s, 3s, 5s, or once at startup).

### 2. Double-Buffered Ratatui TUI
- Use `ratatui` with the `crossterm` backend.
- Double-buffering compares existing terminal cells with the newly drawn buffer, transmitting only ANSI escape diffs.
- Clear full-screen layouts only when resizing (`Terminal::draw`).

### 3. Cached Sysfs File Descriptors (`src/file_cache.rs`)
- Repeated `open()` / `close()` system calls on `/sys/devices/...` and `/proc/...` generate kernel context switches and CPU spikes.
- `read_cached_file(path)` retains open `std::fs::File` handles in a global mutex cache:
  ```rust
  // Instead of fs::read_to_string:
  file.seek(SeekFrom::Start(0))?;
  file.read_to_string(&mut contents)?;
  ```

### 4. Background Stream Worker vs Process Spawning (`src/accelerator.rs`)
- **NEVER** spawn `/bin/rknn-smi` inside the render loop (spawning a process every 1 second takes 10-25% CPU on embedded ARM).
- `AcceleratorMonitor` spawns a single long-lived thread executing `/bin/rknn-smi info -w`.
- The streaming stdout is parsed line-by-line via `BufReader::lines()` and stored into `Arc<RwLock<Option<AcceleratorMetrics>>>`.
- If `/bin/rknn-smi` is not installed or the accelerator is absent, the monitor disables itself with zero runtime cost.
- **Quirk**: `rknn-smi info` exits with status `251` (`0xFB`) even on successful execution. Process checks must inspect stdout rather than asserting exit status `0`.

---

## 6. Testing Workflow

The `nix` crate and Unix virtual filesystem interfaces require a Unix target environment. All tests must be executed via WSL2 or on the target board:

```powershell
# Run unit tests from Windows via WSL2
wsl bash -c "cd /mnt/d/develop/rktop-ai && cargo test"
```

Unit tests validate:
- Accelerator telemetry watch line parser (`parse_watch_line`) with live format (`0% - 1000MHz`, `32% - 5120MB`, `OK`)
- Accelerator load line parser (`parse_watch_line_with_load`)
- Skip header, table border, and blank line parsing
- Static table chip identification (`RK1828`) and PCIe bus ID (`0000:01:00.0`)
