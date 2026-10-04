---
name: rust-cross-aarch64
description: Step-by-step workflow for cross-compiling, testing, benchmarking, and packaging Rust TUI applications targeting aarch64 Linux from Windows/WSL.
---

# Rust Cross-Compilation Runbook: Targeting aarch64 Linux from Windows/WSL2

This skill defines the complete pipeline for building, validating, profiling, and packaging `rktop-ai` for ARM64 (aarch64) Linux targets (Rockchip RK3576 / RK3588) from a Windows workstation with WSL2.

---

## 1. Prerequisites & Toolchain Setup

### Host Requirements
- **Host OS**: Windows 10/11 with WSL2 enabled.
- **WSL Distribution**: Ubuntu 24.04 LTS (Noble Numbat).
- **Rust Toolchain**: 1.80+ (managed via `rustup` inside WSL).

### One-Time Toolchain Setup (in WSL2)
```bash
# 1. Update package lists and install GNU aarch64 cross-toolchain
sudo apt-get update && sudo apt-get install -y \
    gcc-aarch64-linux-gnu \
    g++-aarch64-linux-gnu \
    libc6-dev-arm64-cross \
    binutils-aarch64-linux-gnu

# 2. Add aarch64 target to Rustup
rustup target add aarch64-unknown-linux-gnu

# 3. Optional: Install QEMU user emulation for local aarch64 test execution
sudo apt-get install -y qemu-user qemu-user-static
```

### Cargo Cross-Linker Configuration
Ensure `.cargo/config.toml` exists in the repository root:
```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
```

---

## 2. Build & Cross-Compilation Workflow

All commands are executed against the workspace repository via WSL2:

### Debug Build
```powershell
# Fast compilation for syntax and linking validation
wsl bash -c "cd /mnt/d/develop/rktop-ai && cargo build --target aarch64-unknown-linux-gnu"
```

### Release Build (Optimized)
```powershell
# Full LTO and release optimizations
wsl bash -c "cd /mnt/d/develop/rktop-ai && cargo build --target aarch64-unknown-linux-gnu --release"
```

### Strip and Verify Binary
```powershell
# Strip debug symbols to reduce binary size (~2.4 MB)
wsl bash -c "aarch64-linux-gnu-strip /mnt/d/develop/rktop-ai/target/aarch64-unknown-linux-gnu/release/rktop-ai"

# Verify ELF header and architecture
wsl bash -c "file /mnt/d/develop/rktop-ai/target/aarch64-unknown-linux-gnu/release/rktop-ai"
```
Expected output:
```
target/aarch64-unknown-linux-gnu/release/rktop-ai: ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib/ld-linux-aarch64.so.1, for GNU/Linux 3.7.0, stripped
```

---

## 3. Testing & Emulation Workflow

### Host Testing via WSL2
The `nix` crate and Unix sysfs paths require a Unix runtime. Run unit tests directly inside WSL2:
```powershell
wsl bash -c "cd /mnt/d/develop/rktop-ai && cargo test"
```

### Cross-Architecture Testing with QEMU (Optional)
To execute tests compiled directly for `aarch64`:
```powershell
# Set runner in WSL environment and execute
wsl bash -c "cd /mnt/d/develop/rktop-ai && CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER='qemu-aarch64 -L /usr/aarch64-linux-gnu' cargo test --target aarch64-unknown-linux-gnu"
```

---

## 4. Benchmarking & CPU Footprint Profiling

To maintain `rktop-ai`'s design target of **< 0.5% CPU overhead** on edge boards:

### 1. CPU Utilization Measurement
Run on the target board:
```bash
# Monitor rktop-ai process usage over 10 iterations
pid=$(pgrep rktop-ai)
top -b -d 1 -n 10 -p $pid | grep rktop-ai
```
Target: Average CPU usage below 0.5% (single core).

### 2. Syscall Overhead Audit (file_cache Verification)
Verify that `read_cached_file` prevents repeated `openat` / `close` storms:
```bash
# Trace syscall counts for 5 seconds
sudo strace -c -p $(pgrep rktop-ai)
```
Target: `openat` and `close` counts should be near zero during steady-state monitoring (only `read`, `lseek`, and `poll`/`epoll_wait` should occur).

---

## 5. Packaging & Deployment Safety Protocol

> [!CAUTION]
> **CRITICAL SAFETY GATE**: NEVER execute automated deployment (`scp`, `rsync`, or SSH script execution) without EXPLICIT user confirmation.

### Packaging Release Archive
```powershell
wsl bash -c "tar -czvf /mnt/d/develop/rktop-ai/target/rktop-ai-aarch64.tar.gz -C /mnt/d/develop/rktop-ai/target/aarch64-unknown-linux-gnu/release rktop-ai"
```

### Deployment Proposal Template
When the binary is built, present the command to the user for explicit confirmation:

```markdown
Target binary ready for deployment:
- Path: `target/aarch64-unknown-linux-gnu/release/rktop-ai`
- Size: ~2.4 MB (stripped ELF 64-bit aarch64)

Proposed deployment command:
scp target/aarch64-unknown-linux-gnu/release/rktop-ai <user>@<board-ip>:/home/<user>/rktop-ai

Do you confirm proceeding with this file transfer?
```
