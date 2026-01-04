# USB Billboard Debug Tool

![Rust](https://img.shields.io/badge/Language-Rust%202024-orange.svg)
![Runtime](https://img.shields.io/badge/Runtime-Smol-blueviolet.svg)
![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20Linux-lightgrey.svg)
![License](https://img.shields.io/badge/License-MIT-blue.svg)

**USB Billboard Debug Tool** 是一个高性能、轻量级的 USB 上位机实用工具。它通过 USB Billboard Class 设备的 **Vendor-Specific（厂商自定义）** 接口，提供实时日志监控、控制台命令下发以及底层寄存器读写功能。

本项目基于 **Rust 2024 Edition** 和极简异步运行时 **`smol`** 构建，采用 **并发任务架构** 实现收发分离和完全异步的 I/O 模型，在 Windows 和 Linux 平台上均能稳定运行。

## ✨ 核心特性

*   **实时日志与控制台 (Log & Console)**：
    *   **双向交互**：后台自动轮询获取下位机日志，前台支持从标准输入发送字符串命令。
    *   **高效传输**：利用 `smol` 异步运行时处理并发任务，输入与输出互不阻塞。
    *   **智能显示**：自动处理 UTF-8 解码与缓冲区清洗。
*   **寄存器调试 (Register REPL)**：
    *   提供交互式 Shell，支持 `r` (read) 和 `w` (write) 指令。
    *   **参数解析**：支持十六进制自动识别（`10` 与 `0x10` 等效）。
*   **灵活性**：命令行参数支持指定目标 VID/PID，适配不同固件版本。

## 🛠️ 环境准备

### Windows 用户 (⚠️ 必须执行)
Windows 系统默认会为 Billboard 设备加载微软自带的 `BbUsb.sys` 驱动，导致 Vendor 接口无法访问。**必须更换驱动**：

1.  下载并运行 [Zadig](https://zadig.akeo.ie/)。
2.  菜单栏选择 `Options` -> `List All Devices`。
3.  选中您的 USB Billboard 设备。
4.  将驱动选择为 **WinUSB** (v6.1 或更高)。
5.  点击 **Replace Driver**。

### Linux 用户
通常无需驱动安装。如遇权限问题，请配置 udev 规则或使用 `sudo`。

## 📦 构建

确保安装了支持 Edition 2024 的 Rust 工具链。

```bash
# 1. 克隆项目
git clone https://github.com/matrikslee/usb-billboard.git
cd usb-billboard

# 2. 编译 (Release 模式已配置极致体积优化)
cargo build --release
```

可执行文件位于 `target/release/usb-billboard{.exe}`。

## 📖 使用指南

### 1. 日志控制台模式 (Log)
连接设备，查看实时日志，并支持发送命令。
```bash
# 使用默认 VID(0x343C) PID(0x5361)
usb-billboard log
```

*   **操作**：直接在终端输入命令并回车，
*   **退出**：按 `Ctrl+C`。

### 2. 寄存器交互模式 (REPL)
进入命令行交互环境，进行寄存器读写。
```bash
usb-billboard reg
```

**交互命令示例：**
> **注意**：所有数值参数均默认视为十六进制。

*   **读取寄存器**：`r <addr> <offset>`
    ```text
    > r 0 100
    [READ 00::0100] 0x1E 0x04 0x00 0x00 0x00 0x00 0x00 0x00
    ```
*   **写入寄存器**：`w <addr> <offset> <value>`
    ```text
    > w 0 2 FF
    [WRITE 00::0002 <= FF] Done
    ```
*   **退出**：`q` 或 `exit`

### 3. 指定设备 VID/PID
```bash
# 连接 VID=0x1234, PID=0xABCD 的设备
usb-billboard.exe --vid 1234 --pid abcd log
```

## 🔌 协议定义 (Protocol)

本工具基于 USB Control Transfer 实现。

| 功能 | 方向 | Req ID | 接收者 (Recipient) | wValue | wIndex | wLength | 说明 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **开启日志** | OUT | `0x22` | Interface | 0 | 0 | 0 | Data 长度为 0 |
| **发送命令** | OUT | `0x22` | Interface | 0 | 0 | Len | Data 为命令字符串 |
| **获取日志** | IN | `0x10` | **Device** | 0 | 0 | 8 | 返回日志流 |
| **读寄存器** | IN | `0x12` | **Device** | `Addr` | `Offset` | 8 | 返回寄存器数据 |
| **写寄存器** | IN | `0x11` | **Device** | `(Addr<<8) \| Val` | `Offset` | 0 | 写操作，无返回数据 |

### 协议细节说明
1.  **写寄存器 (`0x11`)**：
    *   这是一个特殊的 **IN** 请求（或无数据阶段请求）。
    *   **Addr** (8-bit) 放在 `wValue` 的高 8 位。
    *   **Value** (8-bit) 放在 `wValue` 的低 8 位。
    *   **Offset** (16-bit) 放在 `wIndex` 中。
    *   由于使用了 `Recipient::Device`，WinUSB 不会校验 `wIndex`，允许任意 Offset。
2.  **读寄存器 (`0x12`)**：
    *   **Addr** 放在 `wValue`。
    *   **Offset** 放在 `wIndex`。
3.  **超时机制**：所有 USB 请求均设置了 200ms 的超时时间，防止程序挂死。

## 🏗️ 依赖配置

`Cargo.toml` 关键依赖：

```toml
[dependencies]
nusb = { version = "0.2", features = ["smol"] } # 纯 Rust USB 栈
smol = "2.0"       # 极简异步运行时
clap = { ... }     # 命令行解析
ctrlc = "3.4"      # 信号处理
```

## 📄 License

MIT License