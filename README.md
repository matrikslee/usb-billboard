# USB Billboard Debug Tool

![Rust](https://img.shields.io/badge/Language-Rust%202024-orange.svg)
![Runtime](https://img.shields.io/badge/Runtime-Smol-blueviolet.svg)
![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20Linux-lightgrey.svg)
![License](https://img.shields.io/badge/License-MIT-blue.svg)

**USB Billboard Debug Tool** 是一个高性能、轻量级的 USB 上位机实用工具。它通过 USB Billboard Class 设备的 **Vendor-Specific（厂商自定义）** 接口，提供实时日志监控和底层寄存器读写功能。

本项目基于 **Rust 2024 Edition** 和极简异步运行时 **`smol`** 构建。

## ✨ 功能特性

*   **实时日志流 (Log Streaming)**：
    *   使用 `GET_DBG_MSG` (0x10) 请求。
    *   支持下位机环形缓冲区，实现弹性读取，不丢字、不刷屏。
*   **交互式寄存器调试 (Register REPL)**：
    *   提供类似 Shell 的交互环境，支持 `r` (read) 和 `w` (write) 指令。
    *   智能参数解析：支持十六进制自动识别（如 `10` 等同于 `0x10`）。
*   **设备初始化**：发送 `SET_DBG_MSG` (0x22) 激活调试模式。
*   **灵活性**：运行时指定 VID/PID，适配不同固件版本。

## 🛠️ 环境要求

### Windows 用户 (⚠️ 核心步骤)
Windows 系统默认会为 Billboard 设备加载微软自带的 `BbUsb.sys` 驱动，这会阻止 Vendor 接口的访问。**必须手动更换驱动**：

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

### 1. 实时日志监控
默认连接 VID: `0x343C`, PID: `0x5361`。
```bash
usb-billboard log
```
*程序会自动发送初始化命令，然后进入监听模式。按 `Ctrl+C` 退出。*

### 2. 寄存器交互模式 (REPL)
进入命令行交互环境，进行寄存器读写。
```bash
usb-billboard reg
```

**交互命令示例：**
> **注意**：所有数值参数均默认视为十六进制（无需加 `0x` 前缀）。

*   **读取寄存器**：`r <addr> <offset>`
    ```text
    > r 0 100
    [RESULT] 0x1E 0x04 0x00 0x00 0x00 0x00 0x00 0x00
    ```
*   **写入寄存器**：`w <addr> <offset> <value>`
    ```text
    > w 0 2 0xFF
    [RESULT] Write Done
    ```
*   **退出**：`q` 或 `exit`

### 3. 指定设备 VID/PID
```bash
# 连接 VID=0x1234, PID=0xABCD 的设备
usb-billboard.exe --vid 1234 --pid abcd log
```

## 🔌 协议定义 (Protocol)

本工具基于 USB Control Transfer 实现。

| 功能 | 方向 | Req ID | 接收者 (Recipient) | wValue | wIndex | wLength |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **开启日志** | OUT | `0x22` | Interface/Device | 0 | Interface (0) | 0 |
| **获取日志** | IN | `0x10` | Interface/Device | 0 | Interface (0) | 64 |
| **读寄存器** | IN | `0x12` | **Device*** | `Addr` | `Offset` | 8 |
| **写寄存器** | IN** | `0x11` | **Device*** | `(Addr<<8) \| Val` | `Offset` | 8 |

### * 说明：WinUSB 限制
在 Windows WinUSB 驱动中，如果 Request Recipient 是 `Interface`，驱动强制要求 `wIndex` 的低 8 位必须等于接口号（0）。这导致无法通过 `wIndex` 传递任意的 Register Offset。
*   **解决方案**：上位机将读写寄存器的 Recipient 修改为 **`Device`**。
*   **原理**：Windows 不会校验 Device 请求的 `wIndex`；而下位机固件通常只校验 Request ID，忽略 Recipient 类型，从而成功绕过限制。

### ** 写寄存器特殊说明
虽然是写操作，但协议定义为 `REQ_GET_WR_REG` (0x11)，这是一个 IN 请求。要写入的值通过 `wValue` 的低 8 位传递，下位机执行写入后返回状态数据。

## 📄 License

MIT License
