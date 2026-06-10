use log::{info, warn};

use crate::models::{FunMetrics, NotificationMessage};

/// 跨平台通知发送器
pub struct Notifier;

impl Notifier {
    /// 发送系统通知
    pub fn send(message: &NotificationMessage) -> anyhow::Result<()> {
        info!("发送通知: {} - {}", message.title, message.body);

        #[cfg(target_os = "macos")]
        {
            Self::send_macos(message)?;
        }

        #[cfg(target_os = "windows")]
        {
            Self::send_windows(message)?;
        }

        #[cfg(target_os = "linux")]
        {
            Self::send_linux(message)?;
        }

        Ok(())
    }

    /// 使用 notify-rust 发送跨平台通知
    fn send_cross_platform(message: &NotificationMessage) -> anyhow::Result<()> {
        use notify_rust::Notification;

        Notification::new()
            .summary(&message.title)
            .body(&message.body)
            .timeout(10000)
            .show()?;

        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn send_macos(message: &NotificationMessage) -> anyhow::Result<()> {
        // 优先使用 osascript 实现更丰富的通知
        let escaped_body = message.body.replace('"', "\\\"");
        let escaped_title = message.title.replace('"', "\\\"");
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escaped_body, escaped_title
        );

        let result = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output();

        match result {
            Ok(_) => {
                info!("macOS 通知发送成功");
                Ok(())
            }
            Err(e) => {
                warn!("osascript 通知失败: {}, 回退到 notify-rust", e);
                Self::send_cross_platform(message)
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn send_windows(message: &NotificationMessage) -> anyhow::Result<()> {
        // Windows 使用 notify-rust (底层调用 Windows Toast)
        match Self::send_cross_platform(message) {
            Ok(_) => {
                info!("Windows Toast 通知发送成功");
                Ok(())
            }
            Err(e) => {
                warn!("Toast 通知失败: {}, 尝试 PowerShell 回退", e);
                // 回退方案：使用 PowerShell 弹窗
                let escaped_body = message.body.replace("'", "''");
                let escaped_title = message.title.replace("'", "''");
                let ps_script = format!(
                    "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.MessageBox]::Show('{}', '{}', 'OK', 'Information')",
                    escaped_body, escaped_title
                );

                std::process::Command::new("powershell")
                    .arg("-NoProfile")
                    .arg("-Command")
                    .arg(&ps_script)
                    .spawn()?;

                Ok(())
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn send_linux(message: &NotificationMessage) -> anyhow::Result<()> {
        // 优先使用 notify-send
        let result = std::process::Command::new("notify-send")
            .arg(&message.title)
            .arg(&message.body)
            .arg("-t")
            .arg("10000")
            .output();

        match result {
            Ok(_) => {
                info!("Linux notify-send 通知发送成功");
                Ok(())
            }
            Err(e) => {
                warn!("notify-send 失败: {}, 回退到 notify-rust", e);
                Self::send_cross_platform(message)
            }
        }
    }

    /// 生成并发送晨间报告通知
    pub fn send_morning_report(
        date: &chrono::NaiveDate,
        daily_stats: &[crate::models::DailyStats],
    ) -> anyhow::Result<()> {
        if daily_stats.is_empty() {
            info!("没有统计数据，跳过通知");
            return Ok(());
        }

        let total_cost: f64 = daily_stats.iter().map(|s| s.estimated_cost).sum();
        let total_code_lines: i64 = daily_stats.iter().map(|s| s.code_lines_equivalent).sum();
        let total_opus4: f64 = daily_stats.iter().map(|s| s.opus4_equivalent).sum();

        let tool_stats: Vec<(String, i64, f64)> = daily_stats
            .iter()
            .map(|s| (s.tool_name.clone(), s.code_lines_equivalent, s.estimated_cost))
            .collect();

        let message = FunMetrics::format_morning_notification(
            date,
            &tool_stats,
            total_cost,
            total_code_lines,
            total_opus4,
        );

        Self::send(&message)
    }
}
