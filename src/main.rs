use clap::{Parser, Subcommand};
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use smol::channel::{Receiver, unbounded};
use smol::io::AsyncBufReadExt;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

#[derive(Subcommand, Clone, Copy)]
enum Commands {
    /// 读取实时调试日志
    Log,

    /// 进入寄存器交互模式 (支持 r/w 指令)
    Reg,
}

/// 辅助函数：解析 16 进制字符串
fn parse_hex_u16(s: &str) -> Result<u16, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("输入为空".to_string());
    }
    let clean_s = if s.to_lowercase().starts_with("0x") {
        &s[2..]
    } else {
        s
    };
    u16::from_str_radix(clean_s, 16)
        .map_err(|e: std::num::ParseIntError| format!("无法解析为十六进制数 '{}': {}", s, e))
}

fn main() {
    ctrlc::set_handler(move || {
        println!("\nExiting...");
        std::process::exit(0);
    })
    .ok();

    smol::block_on(async_main());
}

async fn async_main() {
    let cli = Cli::parse();

    let (stdin_tx, stdin_rx) = unbounded::<String>();

    // 启动一个脱离生命周期的后台任务，专门读键盘
    smol::spawn(async move {
        let stdin = smol::Unblock::new(std::io::stdin());
        let mut reader = smol::io::BufReader::new(stdin);
        let mut line = String::new();
        loop {
            line.clear();
            // 这里会一直阻塞等待输入
            if reader.read_line(&mut line).await.is_ok() {
                // 读取成功，发送给主逻辑
                if stdin_tx.send(line.clone()).await.is_err() {
                    break; // 主程序退出了
                }
            }
        }
    })
    .detach(); // detach 表示这个任务在后台一直运行

    // 外层循环：支持断线重连
    loop {
        // 1. 尝试连接设备
        let interface = match find_and_open_device(cli.vid, cli.pid).await {
            Ok(i) => i,
            Err(_) => {
                smol::Timer::after(Duration::from_millis(1000)).await;
                continue;
            }
        };

        // 2. 根据子命令执行业务逻辑
        // 如果函数返回 Err，说明发生了致命错误（如设备断开），则进入下一次重连循环
        let result = match cli.command {
            Commands::Log => run_log_console(&interface, stdin_rx.clone()).await,
            Commands::Reg => run_reg_shell(&interface).await,
        };

        // 3. 处理会话结束结果
        match result {
            Ok(_) => {
                // 用户主动退出 (如输入 exit 或 Ctrl+C 的逻辑分支)
                println!("会话结束。");
                break;
            }
            Err(_) => {
                eprintln!("\n设备连接中断或发生错误。等待重连...");
            }
        }
    }
}

// ================= 设备连接辅助 =================
async fn find_and_open_device(vid: u16, pid: u16) -> io::Result<nusb::Interface> {
    let device_info = nusb::list_devices()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .find(|d| d.vendor_id() == vid && d.product_id() == pid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "未找到指定的 USB 设备"))?;

    let device = device_info
        .open()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let interface = device
        .claim_interface(0)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    println!("设备已连接！");
    Ok(interface)
}

// ================= 业务逻辑实现 =================

// 1. 日志 + Console 输入模式
async fn run_log_console(
    interface: &nusb::Interface,
    stdin_receiver: Receiver<String>,
) -> io::Result<()> {
    // 初始化调用
    if let Err(e) = req_set_dbg_msg_init(interface).await {
        eprintln!("警告: 初始化失败 ({})", e);
    }

    println!("--- 进入 Log Console 模式 ---");
    println!(" [提示] 输入命令回车发送，按 Ctrl+C 退出");
    println!("---------------------------------------");

    // 用于回显去重的共享标志
    // true: 表示刚发送了命令，正在等待下位机回显结束（收到 \r\n 之前不打印）
    // false: 正常打印日志
    let suppress_echo = Arc::new(AtomicBool::new(false));

    // 任务 A: 日志读取
    let log_interface = interface.clone();
    let log_suppress = suppress_echo.clone();

    let logger_task = async move {
        // 记录上一个字符是否为 \r，用于跨包检测 \r\n
        let mut last_char_was_cr = false;

        loop {
            match req_get_dbg_msg(&log_interface).await {
                Ok(data) => {
                    let valid_len = data.iter().position(|&x| x == 0).unwrap_or(data.len());
                    let valid_bytes = &data[0..valid_len];

                    if !valid_bytes.is_empty() {
                        // 准备一个缓冲区存放需要打印的字符
                        let mut output_buffer = Vec::new();

                        for &b in valid_bytes {
                            // 检查当前是否处于“静默回显”状态
                            if log_suppress.load(Ordering::SeqCst) {
                                // 处于静默状态，检测是否收到 \r\n 结束符
                                if b == b'\n' && last_char_was_cr {
                                    // 找到了 \r\n，回显结束，解除静默
                                    log_suppress.store(false, Ordering::SeqCst);
                                    // 注意：这个 \n 本身也是回显的一部分，所以也不打印
                                }
                            } else {
                                // 非静默状态，正常收集字符
                                output_buffer.push(b);
                            }
                            // 更新状态，并丢弃当前字符 b
                            last_char_was_cr = b == b'\r';
                        }

                        // 批量打印有效字符
                        if !output_buffer.is_empty() {
                            print!("{}", String::from_utf8_lossy(&output_buffer));
                            let _ = io::stdout().flush();
                        }
                    } else {
                        smol::future::yield_now().await;
                    }
                }
                Err(e) => {
                    if e.kind() == io::ErrorKind::TimedOut {
                        smol::Timer::after(Duration::from_millis(100)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    };

    // 任务 B: 键盘输入
    let input_suppress = suppress_echo.clone();
    let input_task = async move {
        loop {
            match stdin_receiver.recv().await {
                Ok(input_line) => {
                    let cmd_clean = input_line.trim().to_string();

                    let mut cmd_to_send = cmd_clean;
                    cmd_to_send.push('\r'); // 追加 \r 触发下位机执行

                    // 1. 开启静默模式 (过滤下位机回显)
                    input_suppress.store(true, Ordering::SeqCst);

                    // 2. 发送命令
                    // 如果发送失败，返回错误以中断 race，触发外层重连
                    if let Err(e) = req_send_console_cmd(interface, cmd_to_send.as_bytes()).await {
                        // 发送失败也应该重置 flag，不过既然要重连了，不重置也没关系
                        return Err(e);
                    }
                }
                Err(_) => return Ok(()),
            }
        }
    };

    smol::future::race(logger_task, input_task).await
}

// 2. 寄存器交互 Shell
async fn run_reg_shell(interface: &nusb::Interface) -> io::Result<()> {
    println!("--- 寄存器交互模式 ---");
    println!("  r <addr> <offset>");
    println!("  w <addr> <offset> <value>");
    println!("  exit / quit");
    println!("------------------------");

    let stdin = io::stdin();
    let mut input_buf = String::new();

    loop {
        print!("> ");
        io::stdout().flush()?;
        input_buf.clear();

        if stdin.read_line(&mut input_buf).is_err() {
            break;
        }

        let line = input_buf.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            ["r", addr_s, off_s] => {
                if let (Ok(addr), Ok(off)) = (parse_hex_u16(addr_s), parse_hex_u16(off_s)) {
                    match req_read_reg(interface, addr, off).await {
                        Ok(data) => {
                            print!("[READ {:02X}::{:04X}] ", addr, off);
                            for b in data {
                                print!("{:02X} ", b);
                            }
                            println!();
                        }
                        Err(e) => {
                            eprintln!("Read Error: {}", e);
                            // 如果是严重错误，抛出以触发重连
                            if is_fatal_error(&e) {
                                return Err(e);
                            }
                        }
                    }
                }
            }
            ["w", addr_s, off_s, val_s] => {
                if let (Ok(addr), Ok(off), Ok(val)) = (
                    parse_hex_u16(addr_s),
                    parse_hex_u16(off_s),
                    parse_hex_u16(val_s),
                ) {
                    match req_write_reg(interface, addr, off, val as u8).await {
                        Ok(_) => println!("[WRITE] Done"),
                        Err(e) => {
                            eprintln!("Write Error: {}", e);
                            if is_fatal_error(&e) {
                                return Err(e);
                            }
                        }
                    }
                }
            }
            ["exit"] | ["quit"] | ["q"] => return Ok(()),
            _ => eprintln!("指令格式错误"),
        }
    }
    Ok(())
}

fn is_fatal_error(e: &io::Error) -> bool {
    // 根据实际情况判断，BrokenPipe 还可以是 ConnectionReset 等
    match e.kind() {
        io::ErrorKind::BrokenPipe
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::NotConnected => true,
        _ => false,
    }
}

// ================= 底层 USB 请求封装 =================

async fn req_send_console_cmd(interface: &nusb::Interface, data: &[u8]) -> io::Result<()> {
    let req = ControlOut {
        control_type: ControlType::Vendor,
        recipient: Recipient::Interface,
        request: REQ_SET_DBG_MSG,
        value: 0,
        index: 0,
        data,
    };
    interface
        .control_out(req, Duration::from_millis(500))
        .await
        .map(|_| ())
        .map_err(to_io_err)
}

async fn req_set_dbg_msg_init(interface: &nusb::Interface) -> io::Result<()> {
    let req = ControlOut {
        control_type: ControlType::Vendor,
        recipient: Recipient::Interface,
        request: REQ_SET_DBG_MSG,
        value: 0,
        index: 0,
        data: &[],
    };
    interface
        .control_out(req, Duration::from_millis(500))
        .await
        .map(|_| ())
        .map_err(to_io_err)
}

async fn req_read_reg(interface: &nusb::Interface, addr: u16, offset: u16) -> io::Result<Vec<u8>> {
    let req = ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: REQ_GET_RD_REG,
        value: addr,
        index: offset,
        length: 8,
    };
    interface
        .control_in(req, Duration::from_millis(500))
        .await
        .map_err(to_io_err)
}

async fn req_write_reg(
    interface: &nusb::Interface,
    addr: u16,
    offset: u16,
    val: u8,
) -> io::Result<Vec<u8>> {
    let w_value = ((addr & 0xFF) << 8) | (val as u16);
    let req = ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: REQ_GET_WR_REG,
        value: w_value,
        index: offset,
        length: 0,
    };
    interface
        .control_in(req, Duration::from_millis(500))
        .await
        .map_err(to_io_err)
}

async fn req_get_dbg_msg(interface: &nusb::Interface) -> io::Result<Vec<u8>> {
    let req = ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: REQ_GET_DBG_MSG,
        value: 0,
        index: 0,
        length: READ_BUFFER_SIZE,
    };
    // 缩短超时时间以便更快响应，应用层 loop 会处理重试
    interface
        .control_in(req, Duration::from_millis(100))
        .await
        .map_err(to_io_err)
}

fn to_io_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}
