use log::{info, warn};

const REG_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_NAME: &str = "VibeStats";

/// 检查开机自启动是否已启用
#[cfg(target_os = "windows")]
pub fn is_autostart_enabled() -> bool {
    use windows_sys::Win32::System::Registry::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let mut hkey: HKEY = std::ptr::null_mut();
    let key_path_wide: Vec<u16> = OsStr::new("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path_wide.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        )
    };

    if result != 0 {
        return false;
    }

    let app_name_wide: Vec<u16> = OsStr::new(APP_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut data_len: u32 = 0;
    let result = unsafe {
        RegQueryValueExW(
            hkey,
            app_name_wide.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut data_len,
        )
    };

    unsafe { RegCloseKey(hkey); }

    result == 0 && data_len > 0
}

#[cfg(not(target_os = "windows"))]
pub fn is_autostart_enabled() -> bool {
    false
}

/// 启用开机自启动
#[cfg(target_os = "windows")]
pub fn enable_autostart() -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let exe_path = std::env::current_exe()?;
    let exe_path_str = format!("\"{}\" --headless", exe_path.display());
    let value_wide: Vec<u16> = OsStr::new(&exe_path_str)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut hkey: HKEY = std::ptr::null_mut();
    let key_path_wide: Vec<u16> = OsStr::new(REG_KEY_PATH)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path_wide.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };

    if result != 0 {
        anyhow::bail!("无法打开注册表键: 错误码 {}", result);
    }

    let app_name_wide: Vec<u16> = OsStr::new(APP_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        RegSetValueExW(
            hkey,
            app_name_wide.as_ptr(),
            0,
            REG_SZ,
            value_wide.as_ptr() as *const u8,
            (value_wide.len() as u32) * 2,
        )
    };

    unsafe { RegCloseKey(hkey); }

    if result != 0 {
        anyhow::bail!("无法写入注册表: 错误码 {}", result);
    }

    info!("已启用开机自启动: {}", exe_path_str);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn enable_autostart() -> anyhow::Result<()> {
    anyhow::bail!("开机自启动仅支持 Windows")
}

/// 禁用开机自启动
#[cfg(target_os = "windows")]
pub fn disable_autostart() -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let mut hkey: HKEY = std::ptr::null_mut();
    let key_path_wide: Vec<u16> = OsStr::new(REG_KEY_PATH)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path_wide.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };

    if result != 0 {
        anyhow::bail!("无法打开注册表键: 错误码 {}", result);
    }

    let app_name_wide: Vec<u16> = OsStr::new(APP_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        RegDeleteValueW(hkey, app_name_wide.as_ptr())
    };

    unsafe { RegCloseKey(hkey); }

    if result != 0 {
        // 值不存在也算成功
        if result != 2 {
            anyhow::bail!("无法删除注册表值: 错误码 {}", result);
        }
    }

    info!("已禁用开机自启动");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn disable_autostart() -> anyhow::Result<()> {
    anyhow::bail!("开机自启动仅支持 Windows")
}

/// 首次运行时自动启用开机自启动
pub fn ensure_autostart() {
    if !is_autostart_enabled() {
        match enable_autostart() {
            Ok(_) => info!("首次运行，已自动启用开机自启动"),
            Err(e) => warn!("自动启用开机自启动失败: {}", e),
        }
    }
}
