use chrono::{Duration, Local, NaiveDate};
use log::{info, warn};
use std::path::PathBuf;

use crate::config::Config;
use crate::notifier::Notifier;
use crate::parser::LogParser;
use crate::stats::StatsEngine;
use crate::storage::Storage;

/// T+1 定时调度器
#[derive(Clone)]
pub struct Scheduler {
    config: Config,
    db_path: PathBuf,
}

impl Scheduler {
    pub fn new(config: Config) -> Self {
        let db_path = config.db_full_path();
        Self { config, db_path }
    }

    /// 打开数据库连接
    fn open_storage(&self) -> anyhow::Result<Storage> {
        Storage::open(&self.db_path)
    }

    /// 启动调度循环
    pub async fn run(&self) -> anyhow::Result<()> {
        // 1. 启动时先检查补偿
        {
            let storage = self.open_storage()?;
            self.check_and_compensate(&storage)?;
        }

        // 1.5 启动实时刷新循环（每 refresh_interval_secs 秒重新解析+重算）
        //    与每日晨报循环共享同一 current_thread runtime：单线程协作调度，
        //    两者均在 await 之间同步完成 DB 写入，不会并发写同一连接。
        //    clone 出独立 Scheduler 实例（各自 open_storage 开独立连接），
        //    使 spawned task 满足 'static 约束。
        let interval = self.config.refresh_interval_secs;
        if interval > 0 {
            let refresh_sched = self.clone();
            tokio::spawn(async move {
                refresh_sched.refresh_loop().await;
            });
            info!("实时刷新已启动，每 {} 秒采集一次用量", interval);
        } else {
            info!("实时刷新已禁用 (refresh_interval_secs=0)，仅靠每日定时任务");
        }

        // 2. 解析调度时间
        let schedule_parts: Vec<u32> = self
            .config
            .schedule_time
            .split(':')
            .filter_map(|s| s.parse().ok())
            .collect();
        let schedule_hour = schedule_parts.first().copied().unwrap_or(0);
        let schedule_minute = schedule_parts.get(1).copied().unwrap_or(30);

        info!(
            "调度器已启动，每日 {:02}:{:02} 执行统计任务",
            schedule_hour, schedule_minute
        );

        // 3. 主循环
        loop {
            let now = Local::now();
            let target_time = now
                .date_naive()
                .and_hms_opt(schedule_hour, schedule_minute, 0)
                .unwrap()
                .and_utc()
                .with_timezone(&Local::now().timezone());

            let next_run = if now < target_time {
                target_time
            } else {
                // 今天的已过，等明天
                (target_time + chrono::Duration::days(1))
                    .with_timezone(&Local::now().timezone())
            };

            let wait_duration = next_run - now;
            info!(
                "下次执行时间: {} (等待 {} 秒)",
                next_run,
                wait_duration.num_seconds()
            );

            tokio::time::sleep(tokio::time::Duration::from_secs(
                wait_duration.num_seconds().max(0) as u64,
            ))
            .await;

            // 执行昨天的统计
            let yesterday = Local::now().date_naive() - Duration::days(1);
            info!("开始执行 {} 的统计任务", yesterday);

            let storage = self.open_storage()?;
            match self.run_daily_stats(&storage, yesterday) {
                Ok(stats) => {
                    if !stats.is_empty() {
                        if let Err(e) = Notifier::send_morning_report(&yesterday, &stats) {
                            warn!("发送通知失败: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("统计任务执行失败: {}", e);
                }
            }
        }
    }

    /// 启动时检查补偿：如果昨天没有统计，立即执行
    pub fn check_and_compensate(&self, storage: &Storage) -> anyhow::Result<()> {
        info!("检查是否需要补偿统计...");

        let today = Local::now().date_naive();
        let yesterday = today - Duration::days(1);

        // 检查昨天是否已统计
        let needs_compensate = self.config.enabled_tools().iter().any(|tool| {
            !storage.has_daily_stats(&yesterday, &tool.name)
        });

        if needs_compensate {
            info!("检测到昨天 ({}) 的统计缺失，开始补偿计算", yesterday);
            self.run_daily_stats(storage, yesterday)?;
        }

        // 同时检查更早的缺失日期
        let missing_stats = StatsEngine::compensate_missing_days(storage)?;
        if !missing_stats.is_empty() {
            info!("补偿完成，共补充 {} 条统计记录", missing_stats.len());
        }

        Ok(())
    }

    /// 执行单日统计流程
    pub fn run_daily_stats(
        &self,
        storage: &Storage,
        date: NaiveDate,
    ) -> anyhow::Result<Vec<crate::models::DailyStats>> {
        info!("开始执行 {} 的统计流程", date);

        // 1. 解析日志
        let tool_configs = self.config.enabled_tools();
        let mut parser = LogParser::new(&Config::data_dir());
        let events = parser.parse_all_logs(&tool_configs);

        // 2. 存储原始事件（按工具日志格式写入：快照式按 (tool,date) 替换 / 增量式去重）
        if !events.is_empty() {
            storage.store_parsed(&events, &tool_configs)?;
        }

        // 3. 保存文件指针
        parser.save_pointers();

        // 4. 聚合统计
        //    快照式解析器（ZCodeSqlite/CodexJsonl/TraeCnLog 等）每次重读全量源数据、
        //    按 (tool,date) 替换 raw_events，这会使“已有 daily_stats 的日期”的
        //    raw_events 在源库后续增长后变大（同一天仍在持续使用、或今天的实时数据）。
        //    若只聚合传入的单个 date，其它已增长日期的 daily_stats 会一直停留在
        //    上次聚合时的旧值 → 与 raw_events 不一致 → 统计偏低。
        //    故此处改为 recompute_all：对所有有 raw_events 的日期全部重算，
        //    保证 daily_stats 始终与 raw_events 一致。增量式解析器同理幂等。
        StatsEngine::recompute_all(storage)?;

        // 5. 取回当天统计（供晨报通知使用）
        let stats = storage.get_daily_stats_range(&date, &date)?;

        info!("{} 统计完成，共 {} 条统计记录", date, stats.len());
        Ok(stats)
    }

    /// 立即执行统计（--run-now 模式）
    pub fn run_now(&self, storage: &Storage) -> anyhow::Result<Vec<crate::models::DailyStats>> {
        let today = Local::now().date_naive();
        info!("立即执行统计模式，统计日期: {}", today);

        // 先执行补偿
        self.check_and_compensate(storage)?;

        // 再统计今天
        self.run_daily_stats(storage, today)
    }

    /// 单次实时刷新：重新解析全部启用工具的日志、按 (tool,date) 替换 raw_events、
    /// 重算所有日期的 daily_stats。不发送通知。失败不抛、由调用方记录。
    /// 与 run_daily_stats 的区别：不取回/返回某日统计、不发晨报，纯数据刷新。
    fn refresh_once(&self) -> anyhow::Result<()> {
        let storage = self.open_storage()?;
        let tool_configs = self.config.enabled_tools();
        let mut parser = LogParser::new(&Config::data_dir());
        let events = parser.parse_all_logs(&tool_configs);
        if !events.is_empty() {
            storage.store_parsed(&events, &tool_configs)?;
        }
        parser.save_pointers();
        StatsEngine::recompute_all(&storage)?;
        Ok(())
    }

    /// 实时刷新循环：每 refresh_interval_secs 秒采集一次，使仪表盘近实时反映用量。
    pub async fn refresh_loop(&self) {
        let interval = self.config.refresh_interval_secs;
        if interval == 0 {
            return;
        }
        let duration = tokio::time::Duration::from_secs(interval);
        loop {
            if let Err(e) = self.refresh_once() {
                warn!("实时刷新失败: {}", e);
            }
            tokio::time::sleep(duration).await;
        }
    }
}
