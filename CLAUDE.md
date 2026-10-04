# CLAUDE.md - Board Optimization & Runtime Agent Guide for rktop-ai

@AGENTS.md

## 1. Role in Multi-Agent Architecture

You are the **Board Optimization Agent** operating in the `rktop-ai` cross-agent workflow:
- **Claude Session (Board Target)**: Direct interaction with the Rockchip target (RK3576 / RK3588 with RM1828MC0-F PCIe AI card via SSH or local board shell). Responsible for live telemetry verification, profiling, validating sysfs paths, verifying kernel quirks, and hardware load testing.
- **Antigravity Session (Workstation)**: Windows 11 / WSL2 workstation handling full codebase refactoring, Ratatui UI components, cross-compilation (`aarch64-unknown-linux-gnu`), and unit tests.

---

## 2. CRITICAL SAFETY GATE: Zero-Surprise Deployment Policy

> [!CAUTION]
> **NEVER transfer files, scp binaries, run rsync, or modify board files without explicit user confirmation.**
> - Always show the command, target host, destination path, and file size to the user first.
> - Wait for explicit confirmation before deploying any binary or modifying `/etc/` or `/sys/` configurations on the board.

---

## 3. Platform Architecture

- **Host SoC**: Rockchip RK3576 (DFRobot ACM3576 / generic RK3576)
  - Octa-core ARM (4x Cortex-A72 @ 2.2 GHz, 4x Cortex-A53 @ 1.8 GHz)
  - GPU: ARM Mali-G52 MC3 (Panthor / Bifrost devfreq driver)
  - NPU: RKNPU2 (up to 6 TOPS, dual-core, debugfs at `/sys/kernel/debug/rknpu/load`)
- **PCIe AI Accelerator**:
  - Model: RM1828MC0-F expansion card
  - NPU Chip: Rockchip RK1828
  - Dedicated VRAM: 5120 MB (5 GB) LPDDR
  - Telemetry: `/bin/rknn-smi info -w`
  - Bus ID: `0000:01:00.0` (endpoint `/dev/pcie-rkep*`, `/sys/bus/pci/drivers/pcie-rkep`)

---

## 4. Key Workflows & Commands

### Cross-Compilation (from Workstation / WSL2)
```bash
# Debug build
cargo build --target aarch64-unknown-linux-gnu

# Optimized release build
cargo build --target aarch64-unknown-linux-gnu --release

# Strip binary
aarch64-linux-gnu-strip target/aarch64-unknown-linux-gnu/release/rktop-ai
```

### Running Tests
```bash
# Tests require a Unix environment (WSL2 or on-board)
cargo test
```

### On-Board Validation (when on device)
```bash
# Verify accelerator hardware presence
ls -l /dev/pcie-rkep*
/bin/rknn-smi info

# Test rknn-smi watch mode stream (exit code 251 quirk applies)
/bin/rknn-smi info -w

# Check NPU load in debugfs
sudo cat /sys/kernel/debug/rknpu/load

# Run rktop-ai (requires root for debugfs or cap_dac_read_search)
sudo ./rktop-ai
```

---

## 5. Performance Directives

1. **Sub-0.5% CPU Target**: Keep the monitoring overhead under 0.5% CPU on the RK3576 host.
2. **File Descriptor Caching**: Never introduce repeated `fs::read_to_string` on sysfs nodes. Use `crate::file_cache::read_cached_file`.
3. **Stream Worker**: Keep `/bin/rknn-smi info -w` in the background streaming worker (`AcceleratorMonitor`). Never spawn subprocesses in the frame draw loop.
4. **Error Handling**: Gracefully handle missing drivers, missing accelerator card, or non-root permissions without crashing or panicking.
