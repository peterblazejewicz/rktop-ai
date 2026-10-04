---
name: rockchip-embedded
description: Specialized cheat-sheet, sysfs telemetry paths, hardware quirks, and diagnostic runbook for Rockchip embedded platforms (RK3576, RK3588, and RK1828 PCIe AI accelerators).
---

# Rockchip Embedded Platform Runbook & Hardware Cheat-Sheet

## 1. Supported Hardware Matrix

| Platform / Subsystem | Architecture / Core Configuration | Key Monitoring Interfaces |
| :--- | :--- | :--- |
| **RK3576 Host SoC** | 4x Cortex-A72 @ 2.2 GHz + 4x Cortex-A53 @ 1.8 GHz | `/sys/devices/system/cpu/cpu*/cpufreq` |
| **RK3576 GPU** | ARM Mali-G52 MC3 (Panthor / Bifrost) | `/sys/class/devfreq/*27800000.gpu/` or `*.gpu/` |
| **RK3576 NPU** | Rockchip RKNPU2 (~6 TOPS, dual-core) | `/sys/class/devfreq/*27700000.npu/`, `/sys/kernel/debug/rknpu/load` |
| **RK3588 Host SoC** | 4x Cortex-A76 @ 2.4 GHz + 4x Cortex-A55 @ 1.8 GHz | `/sys/devices/system/cpu/cpu*/cpufreq` |
| **RK3588 GPU** | ARM Mali-G610 MP4 | `/sys/class/devfreq/fb000000.gpu/` |
| **RK3588 NPU** | Rockchip RKNPU2 (~6 TOPS, tri-core) | `/sys/class/devfreq/fdab0000.npu/`, `/sys/kernel/debug/rknpu/load` |
| **RGA 2D Engine** | RGA 2 / RGA 3 graphics accelerator scheduler | `/sys/kernel/debug/rkrga/load` |
| **RM1828MC0-F Card** | RK1828 dedicated NPU, 5120 MB LPDDR VRAM | `/bin/rknn-smi info -w`, `/dev/pcie-rkep*`, `0000:01:00.0` |

---

## 2. Exact Sysfs & Debugfs Telemetry Paths

### CPU & Clustering
- **Current Frequency**: `/sys/devices/system/cpu/cpu{N}/cpufreq/scaling_cur_freq` (values in kHz)
- **Min / Max Limits**: `/sys/devices/system/cpu/cpu{N}/cpufreq/cpuinfo_min_freq`, `cpuinfo_max_freq` (kHz)
- **Governor**: `/sys/devices/system/cpu/cpu{N}/cpufreq/scaling_governor` (e.g. `schedutil`, `performance`)
- **System Stat**: `/proc/stat` (aggregate user, nice, system, idle, iowait, irq, softirq)

### GPU (Mali Panthor / Bifrost)
- **Dynamic devfreq discovery**: Search `/sys/class/devfreq/` for directories ending in `.gpu`:
  - RK3576: `/sys/class/devfreq/27800000.gpu/`
  - RK3588: `/sys/class/devfreq/fb000000.gpu/` or `/sys/devices/platform/fb000000.gpu-panthor/devfreq/fb000000.gpu-panthor/`
- **Current Frequency**: `<gpu_dir>/cur_freq` (values in Hz; divide by `1_000_000` for MHz)
- **GPU Load (devfreq)**: `<gpu_dir>/load`
  - Typical format: `0@300000000Hz` or single integer percentage `0`..`100`
- **Panthor Driver Load**: `/sys/kernel/debug/panthor/` or devfreq busy/total times

### NPU (RKNPU2)
- **Dynamic devfreq discovery**: Search `/sys/class/devfreq/` for directories ending in `.npu`:
  - RK3576: `/sys/class/devfreq/27700000.npu/cur_freq` (Hz)
  - RK3588: `/sys/class/devfreq/fdab0000.npu/cur_freq` (Hz)
- **Core Load Table (Debugfs)**: `/sys/kernel/debug/rknpu/load`
  - Format output:
    ```
    NPU load: Core0:  15%, Core1:  20%
    ```
    or for RK3588:
    ```
    NPU load: Core0:  10%, Core1:   5%, Core2:  12%
    ```
  - Parse with regex: `Core(\d+):\s*(\d+)%`

### Thermal Zones
- **Base directory**: `/sys/class/thermal/thermal_zone*/`
- **Sensor Type**: `thermal_zone{N}/type`
  - Common labels: `soc-thermal`, `cpu-thermal`, `gpu-thermal`, `npu-thermal`, `littlecore-thermal`, `bigcore-thermal`
  - Clean labels: Strip `_thermal` and `-thermal` suffixes
- **Temperature**: `thermal_zone{N}/temp` (value in millidegrees Celsius; divide by `1000`)

### RGA 2D Accelerator
- **Debugfs Load**: `/sys/kernel/debug/rkrga/load`
- Reports load across schedulers (e.g. `rga3_0`, `rga3_1`, `rga2_2`).

### AI PCIe Accelerator (RM1828MC0-F / RK1828)
- **Vendor Tool**: `/bin/rknn-smi`
- **Static Info**: `/bin/rknn-smi info`
  ```
  +------------------------+---------------+---------------+----------------------+
  | Device        Status   | Health        | Power(mW)     | Npu(%)               |
  | Chip          Name     | Bus-Id        | Temp(C)       | Memory-Usage(MB)     |
  +========================+===============+===============+======================+
  | 0             Online   | OK            | NA            | 0                    |
  | 0             RK1828   | 0000:01:00.0  | 42            | 1599 / 5120          |
  +------------------------+---------------+---------------+----------------------+
  ```
- **Real-Time Watch Stream**: `/bin/rknn-smi info -w`
  - Streamed line format:
    ```
    0           0            NA      43     0% - 1000        0% - 850         32% - 5120           OK      
    ```
  - Columns:
    1. Device Idx (`0`)
    2. Chip Idx (`0`)
    3. Power in mW (`NA` or integer, e.g. `1200`)
    4. Temp in °C (`43`)
    5. CPU Load & Freq (`0% - 1000` MHz)
    6. NPU Load & Freq (`0% - 850` MHz)
    7. Memory % & Total MB (`32% - 5120`)
    8. Health Status (`OK`)
- **PCIe Endpoint Character Device**: `/dev/pcie-rkep*` (e.g. `/dev/pcie-rkep0`)
- **PCIe Sysfs Driver**: `/sys/bus/pci/drivers/pcie-rkep`

---

## 3. Known Hardware Quirks & Solutions

### Quirk 1: `rknn-smi` Exits With Status 251 (0xFB)
- **Problem**: When invoking `/bin/rknn-smi info` or `/bin/rknn-smi info -w`, the process may exit with returncode `251` rather than `0`, despite printing valid table output. Standard commands checking `exit_status.success()` will treat this as an error.
- **Workaround in Rust**:
  Inspect the stdout content directly instead of requiring exit code `0`. If stdout contains valid headers or chip name matches (`RK1828`), accept the output.

### Quirk 2: Streaming Watch Mode vs Process Spawning Overhead
- **Problem**: Spawning `/bin/rknn-smi info` every 1 second consumes 10-25% CPU overhead on an ARM Cortex-A53/A72 core due to dynamic linker startup, PCIe bus communication initialization, and text table formatting.
- **Workaround in Rust**:
  Launch a single long-lived thread executing `/bin/rknn-smi info -w`. Read its stdout pipe incrementally line-by-line via `BufReader::lines()`, updating a shared `Arc<RwLock<Option<AcceleratorMetrics>>>`. When `AcceleratorMonitor` drops, set an `AtomicBool` and kill the child process.

### Quirk 3: DFRobot ACM3576 Device Tree Model Detection
- **Problem**: Some DFRobot boards and BSP device trees report model strings such as `DFRobot ACM3576 Development Board` or `/proc/device-tree/compatible` as `rockchip,rk3576`. Generic substring matchers looking only for `RK\d+` will miss `ACM3576`.
- **Workaround in Rust**:
  In `get_rk_model()`:
  1. Inspect `/proc/device-tree/compatible` for `rockchip,rk3576`.
  2. Apply regex pattern `(?i)\bACM(\d+)\b` to transform `ACM3576` into `RK3576`.
  3. Fallback to `/sys/firmware/devicetree/base/model` or `/proc/cpuinfo`.

### Quirk 4: PCIe Endpoint Node Permissions
- **Problem**: Accessing `/dev/pcie-rkep*` or `/sys/kernel/debug/` may require root privileges or specific capabilities.
- **Workaround**:
  Run `rktop-ai` with `sudo` or grant capabilities:
  ```bash
  sudo setcap cap_dac_read_search,cap_sys_ptrace=eip target/aarch64-unknown-linux-gnu/release/rktop-ai
  ```

---

## 4. On-Board Diagnostic Runbook

Execute these commands via SSH on the target board to verify hardware nodes:

```bash
# 1. Identify Board and SoC
cat /proc/device-tree/model
tr '\0' '\n' < /proc/device-tree/compatible

# 2. Check Host CPU Frequencies & Governors
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq
cat /sys/devices/system/cpu/cpu4/cpufreq/scaling_cur_freq

# 3. Check GPU Devfreq
ls -ld /sys/class/devfreq/*gpu*
cat /sys/class/devfreq/*.gpu/cur_freq

# 4. Check Host NPU Devfreq & Debugfs Load
ls -ld /sys/class/devfreq/*npu*
cat /sys/class/devfreq/*.npu/cur_freq
sudo cat /sys/kernel/debug/rknpu/load

# 5. Check PCIe AI Accelerator
lspci -tv
ls -l /dev/pcie-rkep*
/bin/rknn-smi info
/bin/rknn-smi info -w
```
