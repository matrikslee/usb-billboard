use clap::{Parser, Subcommand};
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use smol::io::AsyncBufReadExt;
use std::io::{self, Write};
use std::time::Duration;

// --- 厂商请求定义 ---
// IN Requests
const REQ_GET_DBG_MSG: u8 = 0x10;
const REQ_GET_WR_REG: u8 = 0x11;
const REQ_GET_RD_REG: u8 = 0x12;

// OUT Requests
const REQ_SET_DBG_MSG: u8 = 0x22;

// --- USB 常量定义 ---
const READ_BUFFER_SIZE: u16 = 8;

// ================= CLI 结构定义 =================

#[derive(Parser)]
#[command(name = "usb-billboard")]
#[command(about = "USB Billboard 调试工具", long_about = None)]
struct Cli {
    /// 目标设备 VID (Hex格式, e.g. 0x343C)
    #[arg(long, value_parser = parse_hex_u16, default_value = "0x343C", global = true)]
    vid: u16,

    /// 目标设备 PID (Hex格式, e.g. 0x5361)
    #[arg(long, value_parser = parse_hex_u16, default_value = "0x5361", global = true)]
    pid: u16,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 读取实时调试日志
    Log,

    /// 进入寄存器交互模式 (支持 r/w 指令)
    Reg,
    // 可以在这里扩展其他子命令，例如：
    // Info { ... },
    // Reset,
}

/// 辅助函数：解析 16 进制字符串 (支持 "0x1234" 或 "1234")
fn parse_hex_u16(s: &str) -> Result<u16, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("输入为空".to_string());
    }
    // 移除可能存在的 0x 或 0X 前缀
    let clean_s = if s.to_lowercase().starts_with("0x") {
        &s[2..]
    } else {
        s
    };

    // 强制使用基数 16 进行解析
    u16::from_str_radix(clean_s, 16)
        .map_err(|e: std::num::ParseIntError| format!("无法解析为十六进制数 '{}': {}", s, e))
}

fn main() {
    // 捕获 Ctrl-C 以便优雅退出 REPL
    ctrlc::set_handler(move || {
        println!("\nExiting...");
        std::process::exit(0);
    })
    .ok();

    // 使用 smol::block_on 启动异步世界
    smol::block_on(async_main());
}

async fn async_main() {
    let cli = Cli::parse();

    // 统一查找和打开设备逻辑
    let interface = match find_and_open_device(cli.vid, cli.pid).await {
        Ok(i) => i,
        Err(e) => {
            eprintln!("错误: {}", e);
            std::process::exit(1);
        }
    };

    // 根据子命令分发
    let result = match cli.command {
        Commands::Log => run_log_console(&interface).await,
        Commands::Reg => run_reg_shell(&interface).await, // 进入交互模式
    };

    if let Err(e) = result {
        eprintln!("执行失败: {}", e);
        std::process::exit(1);
    }
}

// ================= 设备连接辅助 =================
async fn find_and_open_device(vid: u16, pid: u16) -> io::Result<nusb::Interface> {
    println!("正在连接设备 VID:0x{:04X} PID:0x{:04X}...", vid, pid);

    // 1. 查找设备
    let device_info = nusb::list_devices()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .find(|d| d.vendor_id() == vid && d.product_id() == pid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "未找到指定的 USB 设备"))?;

    // 2. 打开设备
    let device = device_info
        .open()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let interface = device
        .claim_interface(0)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(interface)
}

// ================= 业务逻辑实现 =================

// 1. 日志 + Console 输入模式
async fn run_log_console(interface: &nusb::Interface) -> io::Result<()> {
    println!("发送初始化请求 (SET_DBG_MSG, Len=0)...");
    // 初始化调用，payload 为空
    if let Err(e) = req_set_dbg_msg_init(interface).await {
        eprintln!("警告: 初始化失败 ({})，尝试直接进入模式...", e);
    } else {
        println!("初始化成功！");
    }

    println!("--- 进入 Console 模式 ---");
    println!(" [提示] 直接输入命令并回车发送，按 Ctrl+C 退出");
    println!("---------------------------------------");

    let log_interface = interface.clone();

    // 任务 A: 后台接收日志
    let logger_task = smol::spawn(async move {
        loop {
            match req_get_dbg_msg(&log_interface).await {
                Ok(data) => {
                    let valid_len = data.iter().position(|&x| x == 0).unwrap_or(data.len());
                    // 获取有效切片 (Slice)
                    let valid_bytes = &data[0..valid_len];
                    // 只有当有效字节不为空时才打印
                    if !valid_bytes.is_empty() {
                        let text = String::from_utf8_lossy(valid_bytes);
                        print!("{}", text);
                        let _ = io::stdout().flush();
                    } else {
                        smol::future::yield_now().await
                    }
                }
                Err(_) => {
                    smol::Timer::after(Duration::from_secs(1)).await;
                }
            }
        }
    });

    // 任务 B: 前台读取键盘输入
    let stdin = smol::Unblock::new(std::io::stdin());
    let mut reader = smol::io::BufReader::new(stdin);
    let mut input_line = String::new();

    loop {
        input_line.clear();
        // 异步读取一行
        match reader.read_line(&mut input_line).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                let cmd_bytes = input_line.as_bytes();
                let cmd_bytes = if cmd_bytes.ends_with(&[b'\n']) {
                    &cmd_bytes[..cmd_bytes.len() - 1]
                } else {
                    cmd_bytes
                };

                // 发送给下位机 (复用 0x22，带数据)
                match req_send_console_cmd(interface, cmd_bytes).await {
                    Ok(_) => {} // 发送成功
                    Err(e) => eprintln!("\n[CMD ERROR] 发送失败: {}", e),
                }
            }
            Err(e) => {
                eprintln!("Stdin Error: {}", e);
                break;
            }
        }
    }

    logger_task.cancel().await;
    Ok(())
}

// 2. 寄存器交互 Shell
async fn run_reg_shell(interface: &nusb::Interface) -> io::Result<()> {
    println!("--- 寄存器交互模式 ---");
    println!("用法:");
    println!("  r <addr> <offset>          读取寄存器 (Addr + Offset)");
    println!("  w <addr> <offset> <value>  写入寄存器 (Addr + Offset)");
    println!("  exit / quit                退出");
    println!("------------------------");

    let stdin = io::stdin();
    let mut input_buf = String::new();

    loop {
        print!("> ");
        io::stdout().flush()?;
        input_buf.clear();

        // 读取一行输入
        if stdin.read_line(&mut input_buf).is_err() {
            break;
        }

        let line = input_buf.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            // 解析读指令
            ["r", addr_str, offset_str] => {
                let addr = match parse_hex_u16(addr_str) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("Read Err: {}", e);
                        continue;
                    }
                };
                let offset = match parse_hex_u16(offset_str) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("Read Err: {}", e);
                        continue;
                    }
                };

                match req_read_reg(interface, addr, offset).await {
                    Ok(data) => {
                        print!("[READ {:02X}::{:04X}] ", addr, offset);
                        for b in data {
                            print!("{:02X} ", b);
                        }
                        println!();
                    }
                    Err(e) => eprintln!("Read Error: {}", e),
                }
            }
            // 解析写指令
            ["w", addr_str, offset_str, val_str] => {
                let addr = match parse_hex_u16(addr_str) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("{}", e);
                        continue;
                    }
                };
                let offset = match parse_hex_u16(offset_str) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("Write Error: {}", e);
                        continue;
                    }
                };
                let val = match parse_hex_u16(val_str) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("Write Error: {}", e);
                        continue;
                    }
                };

                // Value 只取低 8 位 (因为下位机是 & 0x00FF)
                // Addr 只取低 8 位 (因为下位机是 & 0xFF00 >> 8，虽然我们传u16，但会被截断)
                match req_write_reg(interface, addr, offset, val as u8).await {
                    Ok(_) => {
                        println!(
                            "[WRITE {:02X}::{:04X} <= {:02X}] Done",
                            addr, offset, val
                        );
                    }
                    Err(e) => eprintln!("Write Error: {}", e),
                }
            }
            ["exit"] | ["quit"] | ["q"] => {
                println!("Bye!");
                break;
            }
            _ => {
                eprintln!("格式错误: r <addr> <offset> 或 w <addr> <offset> <value>");
            }
        }
    }
    Ok(())
}

// ================= 底层 USB 请求封装 =================

// [OUT] 0x22: 发送 Console 命令 (复用 SET_DBG_MSG)
// 区别：Payload 不为空
async fn req_send_console_cmd(interface: &nusb::Interface, data: &[u8]) -> io::Result<()> {
    let req = ControlOut {
        control_type: ControlType::Vendor,
        recipient: Recipient::Interface,
        request: REQ_SET_DBG_MSG, // 0x22
        value: 0,
        index: 0,
        data: data, // 发送字符串字节
    };
    interface
        .control_out(req, Duration::from_millis(200))
        .await
        .map(|_| ())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

// [OUT] 0x22: 初始化日志
// 区别：Payload 为空
async fn req_set_dbg_msg_init(interface: &nusb::Interface) -> io::Result<()> {
    let req = ControlOut {
        control_type: ControlType::Vendor,
        recipient: Recipient::Interface,
        request: REQ_SET_DBG_MSG, // 0x22
        value: 0,
        index: 0,
        data: &[], // 空数据
    };
    interface
        .control_out(req, Duration::from_millis(200))
        .await
        .map(|_| ())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

// [IN] 0x12: 读寄存器
// C代码: bus_map(setup->wValue, setup->wIndex)
// 映射: wValue = Addr, wIndex = Offset
// 返回: PACKET_LEN (8 bytes)
async fn req_read_reg(interface: &nusb::Interface, addr: u16, offset: u16) -> io::Result<Vec<u8>> {
    let req = ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: REQ_GET_RD_REG, // 0x12
        value: addr,
        index: offset,
        length: 8,
    };
    println!("addr{addr}, offset{offset}");
    interface
        .control_in(req, Duration::from_millis(200))
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

// [IN] 0x11: 写寄存器
// C代码: write_reg((setup->wValue & 0xFF00) >> 8, setup->wIndex, setup->wValue & 0x00FF)
// 映射: wValue High=Addr, wValue Low=Value, wIndex=Offset
async fn req_write_reg(
    interface: &nusb::Interface,
    addr: u16,
    offset: u16,
    val: u8,
) -> io::Result<Vec<u8>> {
    // 构造 wValue: 高8位是地址，低8位是值
    let w_value = ((addr & 0xFF) << 8) | (val as u16);
    println!("w_value{w_value}, offset{offset}");

    let req = ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: REQ_GET_WR_REG, // 0x11
        value: w_value,          // 组合 Addr 和 Value
        index: offset,           // Offset
        length: 0,
    };
    interface
        .control_in(req, Duration::from_millis(200))
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

// [IN] 0x10: 获取日志流
async fn req_get_dbg_msg(interface: &nusb::Interface) -> io::Result<Vec<u8>> {
    let req = ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: REQ_GET_DBG_MSG,
        value: 0,
        index: 0,
        length: READ_BUFFER_SIZE,
    };
    interface
        .control_in(req, Duration::from_millis(200))
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}
