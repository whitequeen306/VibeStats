use log::{error, info};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder};
use tao::window::WindowBuilder;
use tray_icon::{
    menu::{Menu, MenuItem},
    TrayIconBuilder, TrayIconEvent,
};
use wry::WebViewBuilder;

use crate::config::Config;
use crate::icon_data;
use crate::storage::Storage;

/// 用户自定义事件
#[derive(Debug, Clone)]
enum UserEvent {
    /// 显示主窗口
    ShowWindow,
    /// 退出应用
    Quit,
    /// 托盘图标事件
    TrayEvent(TrayIconEvent),
    /// 菜单事件
    MenuEvent(tray_icon::menu::MenuEvent),
}

/// 在 Windows 上弹出错误对话框
fn show_error_dialog(title: &str, message: &str) {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        let title_wide: Vec<u16> = OsStr::new(title).encode_wide().chain(std::iter::once(0)).collect();
        let msg_wide: Vec<u16> = OsStr::new(message).encode_wide().chain(std::iter::once(0)).collect();
        unsafe {
            windows_dialog(title_wide.as_ptr(), msg_wide.as_ptr());
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("{}: {}", title, message);
    }
}

#[cfg(target_os = "windows")]
unsafe fn windows_dialog(title: *const u16, message: *const u16) {
    windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
        std::ptr::null_mut(),
        message,
        title,
        0x10,
    );
}

/// 在 Windows 上设置窗口图标（任务栏图标）
#[cfg(target_os = "windows")]
fn set_window_icon(window: &tao::window::Window) {
    use tao::platform::windows::WindowExtWindows;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIcon, SendMessageW, WM_SETICON, ICON_BIG, ICON_SMALL,
    };

    let rgba = icon_data::ICON_RGBA;
    let width = icon_data::ICON_WIDTH;
    let height = icon_data::ICON_HEIGHT;

    // CreateIcon 需要 COLORREF (BGR) XOR mask + AND mask
    let mut xor_bits = Vec::with_capacity((width * height * 4) as usize);
    for i in 0..(width * height) {
        let idx = (i * 4) as usize;
        xor_bits.push(rgba[idx + 2]); // B
        xor_bits.push(rgba[idx + 1]); // G
        xor_bits.push(rgba[idx + 0]); // R
        xor_bits.push(rgba[idx + 3]); // A
    }

    // AND mask: 0 = opaque, 1 = transparent
    let and_row_bytes = ((width + 31) / 32) * 4;
    let mut and_bits = vec![0u8; (and_row_bytes * height) as usize];

    unsafe {
        let hicon = CreateIcon(
            std::ptr::null_mut(), // hInstance
            width as i32,
            height as i32,
            1,  // cPlanes
            32, // cBitsPixel
            and_bits.as_ptr(),
            xor_bits.as_ptr(),
        );

        if !hicon.is_null() {
            let hwnd = window.hwnd() as *mut _;
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, hicon as isize);
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, hicon as isize);
            info!("窗口图标已设置");
        } else {
            error!("创建窗口图标失败");
        }
    }
}

/// 加载内嵌图标
fn load_icon() -> Option<tray_icon::Icon> {
    tray_icon::Icon::from_rgba(
        icon_data::ICON_RGBA.to_vec(),
        icon_data::ICON_WIDTH,
        icon_data::ICON_HEIGHT,
    )
    .ok()
}

/// 启动原生桌面 GUI 窗口（带系统托盘）
pub fn launch_gui(config: &Config, storage: Storage) {
    if let Err(e) = launch_gui_inner(config, storage) {
        let err_msg = format!("{:#}", e);
        error!("GUI 启动失败: {}", err_msg);
        show_error_dialog("VibeStats 启动失败", &err_msg);
        std::process::exit(1);
    }
}

fn launch_gui_inner(config: &Config, _storage: Storage) -> anyhow::Result<()> {
    let port = config.serve_port;
    let url = format!("http://127.0.0.1:{}", port);

    // 先在后台启动 HTTP 服务器
    let config_clone = config.clone();
    let storage_for_server = Storage::open(&config.db_full_path())?;

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async {
            if let Err(e) = crate::dashboard::Dashboard::serve(&config_clone, storage_for_server).await {
                error!("HTTP 服务器错误: {}", e);
            }
        });
    });

    // 同时启动调度器（后台线程）
    let config_for_sched = config.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create scheduler runtime");

        rt.block_on(async {
            let sched = crate::scheduler::Scheduler::new(config_for_sched);
            if let Err(e) = sched.run().await {
                error!("调度器错误: {}", e);
            }
        });
    });

    // 等待服务器启动
    info!("等待 HTTP 服务器启动...");
    let mut retries = 0;
    while retries < 50 {
        if check_server_ready(&url) {
            info!("HTTP 服务器已就绪");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        retries += 1;
    }
    if retries >= 50 {
        anyhow::bail!("HTTP 服务器启动超时，请检查端口 {} 是否被占用", port);
    }

    // 创建带用户事件的事件循环
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // 设置托盘图标事件转发
    let proxy_for_tray = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = proxy_for_tray.send_event(UserEvent::TrayEvent(event));
    }));

    let proxy_for_menu = proxy.clone();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy_for_menu.send_event(UserEvent::MenuEvent(event));
    }));

    // 创建系统托盘
    let icon = load_icon();
    let show_item = MenuItem::new("显示仪表盘", true, None);
    let quit_item = MenuItem::new("退出 VibeStats", true, None);
    let tray_menu = Menu::new();
    tray_menu.append(&show_item)?;
    tray_menu.append(&tray_icon::menu::PredefinedMenuItem::separator())?;
    tray_menu.append(&quit_item)?;

    let mut tray_builder = TrayIconBuilder::new()
        .with_tooltip("VibeStats - VibeCoding Token 统计")
        .with_menu(Box::new(tray_menu));

    if let Some(ic) = icon {
        tray_builder = tray_builder.with_icon(ic);
    }

    let _tray = tray_builder.build()?;
    info!("系统托盘已创建");

    // 创建原生窗口
    let window = WindowBuilder::new()
        .with_title("VibeStats - VibeCoding 趣味仪表盘")
        .with_inner_size(tao::dpi::LogicalSize::new(1400, 900))
        .with_min_inner_size(tao::dpi::LogicalSize::new(800, 600))
        .build(&event_loop)
        .map_err(|e| anyhow::anyhow!("无法创建窗口: {}", e))?;

    // 设置窗口图标（Windows 任务栏图标）
    #[cfg(target_os = "windows")]
    {
        set_window_icon(&window);
    }

    // 创建 WebView
    let _webview = WebViewBuilder::new()
        .with_url(&url)
        .with_initialization_script(r#"
            document.addEventListener('DOMContentLoaded', function() {
                document.body.style.overflow = 'auto';
            });
        "#)
        .build(&window)
        .map_err(|e| anyhow::anyhow!("无法创建 WebView: {}。请确保系统已安装 WebView2 运行时。", e))?;

    info!("VibeStats 桌面窗口已启动");

    // 运行事件循环
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(user_event) => match user_event {
                UserEvent::ShowWindow => {
                    window.set_minimized(false);
                    window.set_visible(true);
                    window.set_focus();
                    info!("从托盘恢复窗口");
                }
                UserEvent::Quit => {
                    info!("从托盘退出 VibeStats");
                    *control_flow = ControlFlow::Exit;
                }
                UserEvent::TrayEvent(_event) => {
                    // 左键点击托盘图标时显示窗口
                    // TrayIconEvent 不直接暴露按钮信息，通过菜单处理
                }
                UserEvent::MenuEvent(menu_event) => {
                    if menu_event.id == show_item.id() {
                        window.set_minimized(false);
                        window.set_visible(true);
                        window.set_focus();
                    } else if menu_event.id == quit_item.id() {
                        info!("从托盘菜单退出 VibeStats");
                        *control_flow = ControlFlow::Exit;
                    }
                }
            },
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // 关闭窗口时最小化到托盘而不是退出
                window.set_visible(false);
                info!("窗口最小化到系统托盘");
            }
            Event::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });

    #[allow(unreachable_code)]
    Ok(())
}

/// 检查 HTTP 服务器是否就绪
fn check_server_ready(url: &str) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let addr = url.replace("http://", "");
    let addr = addr.split('/').next().unwrap_or(&addr);

    if let Ok(mut stream) = TcpStream::connect(addr) {
        let request = format!(
            "GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            addr
        );
        if stream.write_all(request.as_bytes()).is_ok() {
            let mut response = [0u8; 64];
            if stream.read(&mut response).is_ok() {
                let resp_str = String::from_utf8_lossy(&response);
                return resp_str.contains("HTTP");
            }
        }
    }
    false
}
