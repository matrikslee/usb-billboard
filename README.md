
# USB Billboard Debug Tool

![Rust](https://img.shields.io/badge/Language-Rust%202024-orange.svg)
![Runtime](https://img.shields.io/badge/Runtime-Smol-blueviolet.svg)
![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20Linux-lightgrey.svg)
![License](https://img.shields.io/badge/License-MIT-blue.svg)

**USB Billboard Debug Tool** 是一个轻量级、高性能的 USB 上位机实用工具。它基于 USB Billboard Class 设备的 **Vendor-Specific（厂商自定义）** 接口，实现与下位机的高速通信、调试日志实时获取以及设备控制。

## ✨ 功能特性

*   **实时日志流读取**：通过 `GET_DBG_MSG` (0x10) 厂商请求，配合下位机环形缓冲区，实现不丢字的日志监控。
*   **灵活性**：完全命令行驱动，支持运行时指定目标设备的 **VID** 和 **PID**。
*   **轻量级架构**：使用 `smol` 替代重量级运行时，结合 LTO 和 Strip 优化，极低资源占用。

## 🛠️ 环境要求

### Windows 用户 (⚠️ 核心步骤)
Windows 系统默认会为 Billboard 设备加载微软自带的 `BbUsb.sys` 驱动，导致普通应用程序无法通过 WinUSB 访问底层 Vendor 接口。**必须手动更换驱动**：

1.  下载并运行 [Zadig](https://zadig.akeo.ie/)。
2.  在菜单栏选择 `Options` -> `List All Devices`。
3.  在下拉列表中选中您的 USB Billboard 设备。
4.  将目标驱动（Driver）选择为 **WinUSB** (v6.1 或更高)。
5.  点击 **Replace Driver** (或 Install Driver)。

### Linux 用户
通常无需安装驱动。如果遇到权限问题（Permission Denied），请配置 `udev` 规则或使用 `sudo` 运行。

## 📦 构建与安装

确保您已安装最新的 [Rust 工具链](https://rustup.rs/) (需支持 Edition 2024)。

1.  **克隆项目**
    ```bash
    git clone https://github.com/matrikslee/usb-billboard.git
    cd usb-billboard
    ```

2.  **构建优化配置**
    本项目在 `Cargo.toml` 中配置了激进的体积优化策略，以确保 Windows 下产物保持在 ~600KB：
    ```toml
    [profile.release]
    strip = true        # 剥离符号表
    lto = true          # 开启链接时优化
    codegen-units = 1   # 降低并行度以换取更优的代码生成
    panic = "abort"     # 禁用栈展开 (Unwind)，移除 .eh_frame
    ```

3.  **编译**
    ```bash
    cargo build --release
    ```

4.  **运行**
    可执行文件位于 `target/release/usb-billboard{.exe}`。

## 📖 使用指南

程序内置命令行参数解析，支持子命令模式。

### 1. 查看帮助信息
```bash
usb-billboard --help
```

### 2. 读取调试日志 (使用默认 VID/PID)
默认连接目标：VID `0x343C`, PID `0x5361`。
```bash
usb-billboard read-log
```
*程序会自动发送初始化命令，然后进入监听模式。按 `Ctrl+C` 退出。*

### 3. 指定设备 VID/PID
适用于固件 ID 变更或多设备场景（支持 Hex 格式）：

```bash
# 连接 VID=0x1234, PID=0xABCD 的设备
usb-billboard --vid 0x1234 --pid 0xABCD read-log

# 简写方式 (自动识别 Hex)
usb-billboard --vid 1234 --pid abcd
```
