use nusb::{
    self, MaybeFuture,
    transfer::{ControlIn, ControlType, Recipient},
};
use std::time::Duration;

// --- 配置区域 ---
const TARGET_VID: u16 = 0x343c; // 请替换为你的设备 VID
const TARGET_PID: u16 = 0x5361; // 请替换为你的设备 PID

// --- USB 常量定义 ---
const DESC_TYPE_STRING: u16 = 0x03;
const DESC_TYPE_BOS: u16 = 0x0F;
const DESC_TYPE_DEVICE_CAPABILITY: u8 = 0x10;
const CAP_TYPE_BILLBOARD: u8 = 0x0D;

const REQ_GET_DESCRIPTOR: u8 = 0x06;

#[tokio::main]
async fn main() {
    println!(
        "正在查找设备 VID:0x{:04X} PID:0x{:04X}...",
        TARGET_VID, TARGET_PID
    );

    // 1. 查找设备
    // nusb 0.2: list_devices() 返回 Result<Iterator>
    let device_info = match nusb::list_devices()
        .wait()
        .unwrap()
        .find(|d| d.vendor_id() == TARGET_VID && d.product_id() == TARGET_PID)
    {
        Some(d) => d,
        None => {
            eprintln!("错误: 未找到设备。");
            eprintln!("提示: 请检查连接，并确保已使用 Zadig 安装 WinUSB 驱动。");
            return;
        }
    };

    println!(
        "找到设备: {}",
        device_info.product_string().unwrap_or("未知设备")
    );

    // 2. 打开设备
    let device = match device_info.open().wait() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("无法打开设备: {}", e);
            eprintln!("常见原因: 驱动被系统占用或非 WinUSB 驱动。");
            return;
        }
    };

    // Windows/WinUSB 必须先认领一个接口才能发送控制传输
    // 通常我们认领接口 0 即可
    println!("正在认领接口 0 以初始化 WinUSB...");
    let interface = match device.claim_interface(0).await {
        Ok(i) => i,
        Err(e) => {
            eprintln!("认领接口失败: {}", e);
            eprintln!("提示: 即使是读取设备级描述符，WinUSB 也需要认领一个接口。");
            return;
        }
    };

    // 使用 interface 句柄读取设备级的 BOS 描述符
    match get_bos_descriptor(&interface).await {
        Ok(data) => {
            println!("BOS 描述符读取成功 ({} bytes)，开始解析...", data.len());
            // 解析时传入 interface 句柄，因为读取字符串还需要它
            parse_bos_data(&interface, &data).await;
        }
        Err(e) => eprintln!("读取 BOS 描述符失败: {}", e),
    }
}

// 获取完整的 BOS 描述符数据
async fn get_bos_descriptor(interface: &nusb::Interface) -> std::io::Result<Vec<u8>> {
    // 步骤 A: 读取头部 (5字节) 获取 wTotalLength
    let header_req = ControlIn {
        control_type: ControlType::Standard,
        recipient: Recipient::Device,
        request: REQ_GET_DESCRIPTOR,
        value: DESC_TYPE_BOS << 8,
        index: 0,
        length: 5,
    };

    let header = interface
        .control_in(header_req, Duration::from_millis(200))
        .wait()
        .unwrap();

    if header.len() < 5 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "BOS头太短",
        ));
    }

    // wTotalLength 在 index 2 和 3 (Little Endian)
    let total_len = u16::from_le_bytes([header[2], header[3]]) as usize;
    println!("BOS 总长度: {}", total_len);

    // 步骤 B: 读取完整数据
    let full_req = ControlIn {
        control_type: ControlType::Standard,
        recipient: Recipient::Device,
        request: REQ_GET_DESCRIPTOR,
        value: DESC_TYPE_BOS << 8,
        index: 0,
        length: total_len as u16,
    };

    let data = interface
        .control_in(full_req, Duration::from_millis(200))
        .wait()
        .unwrap();
    Ok(data)
}

// 解析 BOS 数据并寻找 Billboard Capability
async fn parse_bos_data(interface: &nusb::Interface, data: &[u8]) {
    let mut offset = 5; // 跳过 BOS Header
    let total_len = data.len();

    while offset < total_len {
        if offset + 3 > total_len {
            break;
        }

        let b_length = data[offset] as usize;
        let b_desc_type = data[offset + 1];
        let b_cap_type = data[offset + 2];

        // 检查是否为 Device Capability (0x10)
        if b_desc_type == DESC_TYPE_DEVICE_CAPABILITY {
            if b_cap_type == CAP_TYPE_BILLBOARD {
                println!(
                    "\n>>> 发现 USB Billboard Capability (Offset: {}) <<<",
                    offset
                );
                // 传入这一段 Capability 的数据进行详细解析
                if offset + b_length <= total_len {
                    process_billboard_cap(interface, &data[offset..offset + b_length]).await;
                }
            }
        }

        offset += b_length;
        if b_length == 0 {
            break;
        }
    }
}

// 解析 Billboard 具体字段
async fn process_billboard_cap(interface: &nusb::Interface, buf: &[u8]) {
    if buf.len() < 40 {
        println!("警告: Billboard 描述符长度不足 (标准至少40字节)");
    }

    let url_index = buf[3];
    let num_alt_modes = buf[4];
    let preferred_mode = buf[5];

    println!("  -> Alternate Modes 数量: {}", num_alt_modes);
    println!("  -> 首选模式索引: {}", preferred_mode);
    println!("  -> URL 字符串索引: {}", url_index);

    if url_index > 0 {
        print!("  -> 读取 URL: ");
        match get_string_descriptor(interface, url_index).await {
            Ok(s) => println!("{}", s),
            Err(_) => println!("[读取失败]"),
        }
    }
}

// 辅助函数：读取字符串描述符
async fn get_string_descriptor(interface: &nusb::Interface, index: u8) -> std::io::Result<String> {
    let lang_id = 0x0409;

    let data = interface
        .control_in(
            ControlIn {
                control_type: ControlType::Standard,
                recipient: Recipient::Device,
                request: REQ_GET_DESCRIPTOR,
                value: (DESC_TYPE_STRING << 8) | (index as u16),
                index: lang_id,
                length: 255,
            },
            Duration::from_millis(200),
        )
        .wait()
        .unwrap();

    if data.len() < 2 {
        return Ok("".to_string());
    }

    // 简单的 UTF-16LE 解析
    let utf16_bytes: Vec<u16> = data[2..]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();

    let s = String::from_utf16(&utf16_bytes).unwrap_or_else(|_| "无效的UTF-16序列".to_string());
    Ok(s)
}
