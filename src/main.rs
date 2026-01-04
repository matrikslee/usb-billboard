use clap::{Parser, Subcommand};
use nusb::{
    self, MaybeFuture,
    transfer::{ControlIn, ControlOut, ControlType, Recipient},
};
use std::io::{self, Write};
use std::time::Duration;

// --- 厂商请求定义 (对应 C 代码宏) ---
// IN Requests
const REQ_GET_HARDWARE_STATUS: u8 = 0x01;
const REQ_GET_FIRMWARE_STATUS: u8 = 0x02;
const REQ_GET_FIRMWARE_VERSION: u8 = 0x03;
const REQ_GET_DBG_MSG: u8 = 0x10; // <--- 本次目标
const REQ_GET_WR_REG: u8 = 0x11;
const REQ_GET_RD_REG: u8 = 0x12;

// OUT Requests
const REQ_SET_ERASE_FLASH: u8 = 0x10;
const REQ_SET_UPDATE_DATA: u8 = 0x11;
const REQ_SET_FW_INFO_1: u8 = 0x12;
const REQ_SET_FW_INFO_2: u8 = 0x13;
const REQ_SET_FW_TO_BLDR: u8 = 0x20;
const REQ_SET_DBG_MSG: u8 = 0x22;

// --- USB 常量定义 ---
const DESC_TYPE_STRING: u16 = 0x03;
const DESC_TYPE_BOS: u16 = 0x0F;
const DESC_TYPE_DEVICE_CAPABILITY: u8 = 0x10;
const CAP_TYPE_BILLBOARD: u8 = 0x0D;

const REQ_GET_DESCRIPTOR: u8 = 0x06;

const READ_BUFFER_SIZE: u16 = 64;

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
    // 可以在这里扩展其他子命令，例如：
    // Info { ... },
    // Reset,
}

/// 辅助函数：解析 16 进制字符串 (支持 "0x1234" 或 "1234")
fn parse_hex_u16(s: &str) -> Result<u16, String> {
    let s = s.trim().to_lowercase();
    let s = s.strip_prefix("0x").unwrap_or(&s);
    u16::from_str_radix(s, 16).map_err(|e: std::num::ParseIntError| e.to_string())
}

fn main() {
    // 使用 smol::block_on 启动异步世界
    smol::block_on(async_main());
}

async fn async_main() {
    let cli = Cli::parse();

    println!("目标设备: VID=0x{:04X}, PID=0x{:04X}", cli.vid, cli.pid);

    match cli.command {
        Commands::Log => {
            if let Err(e) = run_read_log(cli.vid, cli.pid).await {
                eprintln!("执行出错: {}", e);
                std::process::exit(1);
            }
        }
    }
}

async fn run_read_log(vid: u16, pid: u16) -> std::io::Result<()> {
    // 1. 查找设备
    let device_info = nusb::list_devices()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .find(|d| d.vendor_id() == vid && d.product_id() == pid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "未找到指定的 USB 设备"))?;

    println!(
        "找到设备: {}",
        device_info.product_string().unwrap_or("未知设备")
    );

    // 2. 打开设备
    let device = device_info
        .open()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    // Windows/WinUSB 必须先认领一个接口才能发送控制传输
    // 通常我们认领接口 0 即可
    println!("正在认领接口 0 以初始化 USB...");
    let interface = device
        .claim_interface(0)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    // 3. 发送初始化命令 (SET_DBG_MSG)
    println!("发送初始化请求 (SET_DBG_MSG)...");
    if let Err(e) = set_dbg_msg(&interface).await {
        eprintln!("初始化失败: {}", e);
        // 根据具体情况，失败了是否还要继续？这里选择继续尝试读取
    } else {
        println!("初始化成功，开始监听日志...");
    }

    // 4. 循环读取调试信息 (GET_DBG_MSG)
    println!("\n--- 开始打印调试日志 按Ctrl-C退出 ---");
    loop {
        match get_dbg_msg(&interface).await {
            Ok(data) => {
                let valid_len = data.iter().position(|&x| x == 0).unwrap_or(data.len());
                // 获取有效切片 (Slice)
                let valid_bytes = &data[0..valid_len];
                // 只有当有效字节不为空时才打印
                if !valid_bytes.is_empty() {
                    let text = String::from_utf8_lossy(valid_bytes);
                    print!("{}", text);
                    let _ = io::stdout().flush();
                }
            }
            Err(e) => {
                // 只有真正的 USB 通信错误才报错，而不是数据内容错误
                eprintln!("通信读取出错: {}", e);
                smol::Timer::after(Duration::from_secs(1)).await;
            }
        }
        // 防止请求过于频繁占用 CPU/USB 带宽
        smol::Timer::after(Duration::from_millis(10)).await;
    }
    // 因为是死循环，这里实际上不可达，但为了满足签名需要一个 Ok(())
    #[allow(unreachable_code)]
    Ok(())
}

// --- 实现 OUT 请求 (主机 -> 下位机) ---
async fn set_dbg_msg(interface: &nusb::Interface) -> std::io::Result<()> {
    let req = ControlOut {
        control_type: ControlType::Vendor,
        recipient: Recipient::Interface, // 必须是 Interface 以匹配 WinUSB 句柄
        request: REQ_SET_DBG_MSG,        // 0x22
        value: 0,                        // wValue 通常为0，除非协议指定需要传参
        index: 0,                        // wIndex 接口号
        data: &[],                       // data: 空数据 (如果协议需要传参，在这里填 &[0x01] 等)
    };

    // 发送请求，忽略返回值(写入字节数)
    interface
        .control_out(req, Duration::from_millis(200))
        .wait()
        .unwrap();
    Ok(())
}

// --- 实现 IN 请求 (下位机 -> 主机) ---
async fn get_dbg_msg(interface: &nusb::Interface) -> std::io::Result<Vec<u8>> {
    // 构造控制传输参数
    // bmRequestType: Dir=IN(0x80) | Type=Vendor(0x40) | Recipient=Device(0x00) => 0xC0
    let req = ControlIn {
        control_type: ControlType::Vendor, // 关键：必须是 Vendor，不是 Standard
        recipient: Recipient::Interface, // 通常是对整个设备的请求。如果不行，尝试 Recipient::Interface
        request: REQ_GET_DBG_MSG,        // 0x10
        value: 0,                        // wValue 通常为0，除非协议另有规定
        index: 0,                        // wIndex 通常为0
        length: READ_BUFFER_SIZE,        // 每次获取 64 字节
    };

    // 发送请求
    let data = interface
        .control_in(req, Duration::from_millis(200))
        .wait()
        .unwrap();

    // 检查长度（可选）
    if data.len() != 8 {
        println!("警告: 预期收到 8 字节，实际收到 {} 字节", data.len());
    }

    Ok(data)
}
