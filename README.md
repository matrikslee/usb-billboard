# USB Billboard Debug Tool

![Rust](https://img.shields.io/badge/Language-Rust%202024-orange.svg)
![Runtime](https://img.shields.io/badge/Runtime-Smol-blueviolet.svg)
![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20Linux-lightgrey.svg)
![License](https://img.shields.io/badge/License-MIT-blue.svg)

**USB Billboard Debug Tool** 是一个高性能、轻量级的 USB 上位机实用工具。它通过 USB Billboard Class 设备的 **Vendor-Specific（厂商自定义）** 接口，提供实时日志监控和底层寄存器读写功能。

本项目基于 **Rust 2024 Edition** 和极简异步运行时 **`smol`** 构建，采用 **并发任务架构** 实现收发分离。

## ✨ 功能特性

*   **双向交互控制台 (Interactive Console)**：
    *   **实时日志**：通过 `GET_DBG_MSG` (0x10) 请求，配合环形缓冲区与短包机制，实现不丢字、低延迟的日志流监控。
    *   **命令发送**：复用 `SET_DBG_MSG` (0x22) 请求，支持从标准输入（Stdin）发送字符串命令到下位机，实现类似 Shell 的交互体验。
    *   **并发架构**：后台任务接收日志，前台任务处理键盘输入，互不阻塞。
*   **寄存器调试 (Register REPL)**：
    *   提供独立的交互环境，支持 `r` (read) 和 `w` (write) 指令。
    *   智能参数解析：支持十六进制自动识别（如 `10` 等同于 `0x10`）。
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
* **日志显示：**程序会自动初始化并持续打印下位机输出的日志。

* **发送命令：**直接在终端输入字符串并回车，程序会将整行内容发送给下位机。

* **退出：**按 Ctrl+C。

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

| 功能 | 方向 | Req ID | 接收者 (Recipient) | wValue | wIndex | wLength | 数据(Data) |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **开启日志** | OUT | `0x22` | Interface/Device | 0 | Interface (0) | 0 | (Empty) |
| **发送命令** | OUT | `0x22` | Interface/Device | 0 | Interface (0) | * | Cmd String Bytes |
| **获取日志** | IN | `0x10` | Interface/Device | 0 | Interface (0) | 64 | Log String Bytes |
| **读寄存器** | IN | `0x12` | **Device*** | `Addr` | `Offset` | 8 | Reg Data |
| **写寄存器** | IN** | `0x11` | **Device*** | `(Addr<<8) \| Val` | `Offset` | 8 | (Empty) |

### 协议说明
1.  **Request `0x22` 复用**：下位机通过 `wLength` 区分功能。长度为 0 表示开启日志功能；长度 > 0 表示接收 Console 命令字符串。
2.  **Request `0x11` 写寄存器**：尽管是写操作，但定义为 IN 请求，数值通过 `wValue` 传递，下位机返回状态码。
3.  **WinUSB 限制规避 (*)**：
    *   在 Windows 下，`Recipient::Interface` 请求强制要求 `wIndex` 低字节为接口号。
    *   为了支持任意 Offset 的寄存器访问，寄存器相关指令将 Recipient 修改为 **`Device`**，从而绕过驱动检查。

## 🏗️ 核心技术

*   **Smol**: 使用 `smol::spawn` 运行后台日志任务，使用 `smol::Unblock` 实现 Stdin 的异步非阻塞读取。
*   **Nusb**: 纯 Rust USB 栈，通过 `Interface` 和 `Device` Recipient 的灵活切换解决驱动兼容性问题。
*   **Optimization**:
    ```toml
    [profile.release]
    strip = true
    lto = true
    codegen-units = 1
    panic = "abort"
    ```

## 📄 License

MIT License
