mod autostart;
mod config;
mod dashboard;
mod gui;
mod icon_data;
mod models;
mod notifier;
mod parser;
mod scheduler;
mod stats;
mod storage;

use clap::Parser;
use log::{error, info};

use config::Config;
use storage::Storage;

/// VibeStats - VibeCoding Token 消耗统计与趣味看板
#[derive(Parser, Debug)]
#[command(name = "vibestats", version, about = "VibeCoding Token 消耗统计与趣味看板")]
struct Args {
    /// 立即执行统计，不等待定时任务
    #[arg(long)]
    run_now: bool,

    /// 以命令行模式运行（不弹出窗口，仅启动 HTTP 服务器）
    #[arg(long)]
    serve: bool,

    /// 指定配置文件路径
    #[arg(long)]
    config: Option<String>,

    /// 生成静态 HTML 报告
    #[arg(long)]
    report: bool,

    /// 仅解析日志，不执行统计
    #[arg(long)]
    parse_only: bool,

    /// 无窗口模式（仅后台调度器，不弹出任何界面）
    #[arg(long)]
    headless: bool,

    /// 全量重建：清空数据库和文件指针，重新解析所有日志
    #[arg(long)]
    rebuild: bool,
}

fn main() -> anyhow::Result<()> {
    // 初始化日志
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    // 加载配置
    let config = load_config(&args)?;
    info!("配置加载完成");

    // 确保数据目录存在
    let data_dir = Config::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    info!("数据目录: {}", data_dir.display());

    // 打开数据库
    let db_path = config.db_full_path();
    let mut storage = Storage::open(&db_path)?;
    info!("数据库已打开: {}", db_path.display());

    if args.parse_only {
        // 仅解析模式
        let mut parser = parser::LogParser::new(&data_dir);
        let enabled_tools = config.enabled_tools();
        let events = parser.parse_all_logs(&enabled_tools);
        info!("解析到 {} 条事件", events.len());
        for event in &events {
            println!(
                "[{}] {} - input: {}, output: {}, cache: {}",
                event.tool_name,
                event.timestamp,
                event.input_tokens,
                event.output_tokens,
                event.cache_read_tokens
            );
        }
        if !events.is_empty() {
            storage.insert_raw_events(&events)?;
            parser.save_pointers();
            info!("事件已保存到数据库");
        }
        return Ok(());
    }

    if args.rebuild {
        // 全量重建：删除数据库和文件指针，从头开始
        info!("开始全量重建...");

        // 关闭数据库连接，删除数据库文件
        drop(storage);
        let db_path = config.db_full_path();
        if db_path.exists() {
            std::fs::remove_file(&db_path)?;
            info!("已删除数据库: {}", db_path.display());
        }

        // 删除文件指针
        let pointer_path = data_dir.join("file_pointers.json");
        if pointer_path.exists() {
            std::fs::remove_file(&pointer_path)?;
            info!("已删除文件指针: {}", pointer_path.display());
        }

        // 重新打开数据库（会自动创建）
        storage = Storage::open(&db_path)?;

        // 重新解析所有日志
        let mut parser = parser::LogParser::new(&data_dir);
        let enabled_tools = config.enabled_tools();
        let events = parser.parse_all_logs(&enabled_tools);
        info!("全量解析到 {} 条事件", events.len());

        if !events.is_empty() {
            storage.insert_raw_events(&events)?;
            parser.save_pointers();

            // 重新计算所有每日统计
            let sched = scheduler::Scheduler::new(config.clone());
            let result = sched.run_now(&storage)?;

            println!("\n========== VibeStats 全量重建完成 ==========");
            for stat in &result {
                println!(
                    "[{}] {} - 输入: {}, 输出: {}, 缓存: {}, 花费: ${:.4}",
                    stat.tool_name, stat.date,
                    stat.total_input_tokens, stat.total_output_tokens,
                    stat.total_cache_read_tokens, stat.estimated_cost
                );
            }
            let total_cost: f64 = result.iter().map(|s| s.estimated_cost).sum();
            println!("\n总计: ${:.2}", total_cost);
            println!("============================================\n");
        } else {
            println!("没有找到可统计的数据。");
        }

        if !args.serve {
            return Ok(());
        }
    }

    if args.run_now {
        // 立即执行统计
        let sched = scheduler::Scheduler::new(config.clone());
        let result = sched.run_now(&storage)?;

        if result.is_empty() {
            println!("没有找到可统计的数据。请检查日志路径配置是否正确。");
        } else {
            println!("\n========== VibeStats 统计结果 ==========");
            for stat in &result {
                println!(
                    "[{}] {} - 输入: {}, 输出: {}, 缓存: {}, 花费: ${:.4}, ≈ {} 行代码, ≈ {:.1} 次 Opus4",
                    stat.tool_name,
                    stat.date,
                    stat.total_input_tokens,
                    stat.total_output_tokens,
                    stat.total_cache_read_tokens,
                    stat.estimated_cost,
                    stat.code_lines_equivalent,
                    stat.opus4_equivalent
                );
            }
            let total_cost: f64 = result.iter().map(|s| s.estimated_cost).sum();
            let total_lines: i64 = result.iter().map(|s| s.code_lines_equivalent).sum();
            let total_opus: f64 = result.iter().map(|s| s.opus4_equivalent).sum();
            println!(
                "\n总计: ${:.2} / {} 行代码 / {:.1} 次 Opus4",
                total_cost, total_lines, total_opus
            );
            println!("========================================\n");

            // 发送通知
            let today = chrono::Local::now().date_naive();
            if let Err(e) = notifier::Notifier::send_morning_report(&today, &result) {
                error!("发送通知失败: {}", e);
            }
        }

        // 如果同时指定了 --serve，继续启动服务器
        if !args.serve {
            return Ok(());
        }
    }

    if args.report {
        // 生成静态报告
        let report_path = data_dir.join("report.html");
        dashboard::Dashboard::generate_static_report(&storage, &report_path)?;
        println!("静态报告已生成: {}", report_path.display());
        if !args.serve && !args.headless {
            return Ok(());
        }
    }

    if args.headless {
        // 无窗口模式：仅后台调度器（开机自启动场景）
        autostart::ensure_autostart();
        send_startup_notification(&config, &storage);
        let rt = tokio::runtime::Runtime::new()?;
        let sched = scheduler::Scheduler::new(config);
        rt.block_on(sched.run())?;
        return Ok(());
    }

    if args.serve {
        // 命令行模式：启动 HTTP 服务器（不弹窗）
        let rt = tokio::runtime::Runtime::new()?;

        let sched = scheduler::Scheduler::new(config.clone());
        std::thread::spawn(move || {
            let rt2 = tokio::runtime::Runtime::new().unwrap();
            rt2.block_on(sched.run()).unwrap();
        });

        rt.block_on(dashboard::Dashboard::serve(&config, storage))?;
        return Ok(());
    }

    // 默认模式：启动原生桌面 GUI 窗口
    info!("启动 VibeStats 桌面应用...");

    // 首次运行自动启用开机自启动
    autostart::ensure_autostart();

    // 启动时发送昨日统计通知
    send_startup_notification(&config, &storage);

    gui::launch_gui(&config, storage);

    Ok(())
}

/// 加载配置文件
fn load_config(args: &Args) -> anyhow::Result<Config> {
    let config_path = if let Some(path) = &args.config {
        std::path::PathBuf::from(path)
    } else {
        Config::config_path()
    };

    if config_path.exists() {
        info!("从配置文件加载: {}", config_path.display());
        match Config::load_from_file(&config_path) {
            Ok(config) => Ok(config),
            Err(_) => {
                // 旧配置格式不兼容，用默认配置覆盖
                info!("配置文件格式不兼容，使用默认配置重新生成");
                let config = Config::default();
                config.save_to_file(&config_path)?;
                Ok(config)
            }
        }
    } else {
        info!("配置文件不存在，使用默认配置并创建: {}", config_path.display());
        let config = Config::default();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        config.save_to_file(&config_path)?;
        Ok(config)
    }
}

/// 启动时发送昨日统计通知
fn send_startup_notification(_config: &Config, storage: &Storage) {
    let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);

    let stats = match storage.get_daily_stats_range(&yesterday, &yesterday) {
        Ok(s) => s,
        Err(e) => {
            info!("获取昨日统计失败，跳过启动通知: {}", e);
            return;
        }
    };

    if stats.is_empty() {
        info!("昨日无统计数据，跳过启动通知");
        return;
    }

    let total_input: i64 = stats.iter().map(|s| s.total_input_tokens).sum();
    let total_output: i64 = stats.iter().map(|s| s.total_output_tokens).sum();
    let total_cache: i64 = stats.iter().map(|s| s.total_cache_read_tokens).sum();
    let total_tokens = total_input + total_output + total_cache;
    let total_code_lines: i64 = stats.iter().map(|s| s.code_lines_equivalent).sum();
    let total_cost: f64 = stats.iter().map(|s| s.estimated_cost).sum();

    // 计算相当于多少本书（按 30000 行/本）
    let books = total_code_lines as f64 / 30000.0;

    let message = crate::models::NotificationMessage {
        title: "VibeStats 昨日报告".to_string(),
        body: format!(
            "先生，您昨天消耗了 {} tokens，相当于写了 {} 行代码，相当于写了 {:.1} 本书，花费约 ${:.2}",
            format_tokens(total_tokens),
            total_code_lines,
            books,
            total_cost,
        ),
    };

    if let Err(e) = crate::notifier::Notifier::send(&message) {
        info!("启动通知发送失败: {}", e);
    }
}

/// 格式化 token 数量（带千分位）
fn format_tokens(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
