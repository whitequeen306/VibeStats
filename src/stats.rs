use chrono::NaiveDate;
use log::info;
use std::collections::HashMap;

use crate::models::{DailyStats, FunMetrics, RawEvent};
use crate::storage::Storage;

/// 统计引擎：负责聚合计算
pub struct StatsEngine;

impl StatsEngine {
    /// 对指定日期的原始事件进行聚合，生成每日统计
    pub fn aggregate_daily(
        storage: &Storage,
        date: &NaiveDate,
    ) -> anyhow::Result<Vec<DailyStats>> {
        let events = storage.get_raw_events_by_date(date)?;
        if events.is_empty() {
            info!("日期 {} 没有原始事件", date);
            return Ok(vec![]);
        }

        // 按工具名分组
        let mut grouped: HashMap<String, Vec<&RawEvent>> = HashMap::new();
        for event in &events {
            grouped
                .entry(event.tool_name.clone())
                .or_default()
                .push(event);
        }

        let date_str = date.format("%Y-%m-%d").to_string();
        let mut results = Vec::new();

        for (tool_name, tool_events) in grouped {
            let total_input: i64 = tool_events.iter().map(|e| e.input_tokens).sum();
            let total_output: i64 = tool_events.iter().map(|e| e.output_tokens).sum();
            let total_cache: i64 = tool_events.iter().map(|e| e.cache_read_tokens).sum();
            let event_count = tool_events.len() as i64;

            // 优先使用实际费用（如 DeepSeek GUI 的 costUsd），否则按模型定价估算
            let total_actual_cost: f64 = tool_events.iter()
                .filter_map(|e| e.actual_cost)
                .sum();
            let estimated_cost = if total_actual_cost > 0.0 {
                total_actual_cost
            } else {
                // 按每个事件的模型分别计算费用再求和（不同模型定价不同）
                tool_events.iter().map(|e| {
                    FunMetrics::estimate_cost(
                        e.input_tokens,
                        e.output_tokens,
                        e.cache_read_tokens,
                        e.model_name.as_deref(),
                    )
                }).sum()
            };
            let total_tokens = total_input + total_output + total_cache;
            let code_lines = FunMetrics::tokens_to_code_lines(total_tokens);
            let opus4_eq = FunMetrics::cost_to_opus4_equivalent(estimated_cost);

            let stats = DailyStats {
                id: None,
                date: date_str.clone(),
                tool_name,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                total_cache_read_tokens: total_cache,
                estimated_cost,
                code_lines_equivalent: code_lines,
                opus4_equivalent: opus4_eq,
                event_count,
            };

            storage.upsert_daily_stats(&stats)?;
            results.push(stats);
        }

        info!("日期 {} 聚合完成，共 {} 个工具", date, results.len());
        Ok(results)
    }

    /// 对所有未统计的日期进行补偿聚合
    pub fn compensate_missing_days(storage: &Storage) -> anyhow::Result<Vec<DailyStats>> {
        // 获取所有有原始事件但缺少统计的日期
        let dates = Self::find_unaggregated_dates(storage)?;
        let mut all_stats = Vec::new();

        for date in dates {
            info!("补偿聚合日期: {}", date);
            let stats = Self::aggregate_daily(storage, &date)?;
            all_stats.extend(stats);
        }

        Ok(all_stats)
    }

    /// 查找未聚合的日期
    fn find_unaggregated_dates(storage: &Storage) -> anyhow::Result<Vec<NaiveDate>> {
        // 简单实现：检查最近 30 天
        let today = chrono::Local::now().date_naive();
        let mut missing = Vec::new();

        for i in 1..=30 {
            let date = today - chrono::Duration::days(i);
            let events = storage.get_raw_events_by_date(&date)?;
            if !events.is_empty() {
                let tool_names: Vec<String> = events
                    .iter()
                    .map(|e| e.tool_name.clone())
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();

                for tool in tool_names {
                    if !storage.has_daily_stats(&date, &tool) {
                        if !missing.contains(&date) {
                            missing.push(date);
                        }
                    }
                }
            }
        }

        Ok(missing)
    }

    /// 获取指定日期范围的聚合数据（用于 Dashboard）
    pub fn get_aggregated_range(
        storage: &Storage,
        start_date: &NaiveDate,
        end_date: &NaiveDate,
    ) -> anyhow::Result<crate::models::AggregatedStats> {
        let stats = storage.get_daily_stats_range(start_date, end_date)?;

        let mut dates: Vec<String> = Vec::new();
        let mut by_tool: HashMap<String, Vec<crate::models::SingleDayStats>> = HashMap::new();
        let mut total_input = 0i64;
        let mut total_output = 0i64;
        let mut total_cache = 0i64;
        let mut total_cost = 0.0f64;
        let mut total_code_lines = 0i64;
        let mut total_opus4 = 0.0f64;
        let mut total_events = 0i64;

        for s in &stats {
            if !dates.contains(&s.date) {
                dates.push(s.date.clone());
            }

            let day_stat = crate::models::SingleDayStats {
                date: s.date.clone(),
                input_tokens: s.total_input_tokens,
                output_tokens: s.total_output_tokens,
                cache_read_tokens: s.total_cache_read_tokens,
                estimated_cost: s.estimated_cost,
                code_lines_equivalent: s.code_lines_equivalent,
                opus4_equivalent: s.opus4_equivalent,
                event_count: s.event_count,
            };

            by_tool
                .entry(s.tool_name.clone())
                .or_default()
                .push(day_stat);

            total_input += s.total_input_tokens;
            total_output += s.total_output_tokens;
            total_cache += s.total_cache_read_tokens;
            total_cost += s.estimated_cost;
            total_code_lines += s.code_lines_equivalent;
            total_opus4 += s.opus4_equivalent;
            total_events += s.event_count;
        }

        let tool_stats: Vec<crate::models::ToolDailyStats> = by_tool
            .into_iter()
            .map(|(name, data)| crate::models::ToolDailyStats {
                tool_name: name,
                daily_data: data,
            })
            .collect();

        Ok(crate::models::AggregatedStats {
            dates,
            by_tool: tool_stats,
            totals: crate::models::DailyTotals {
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                total_cache_read_tokens: total_cache,
                total_estimated_cost: total_cost,
                total_code_lines: total_code_lines,
                total_opus4_equivalent: total_opus4,
                total_events: total_events,
            },
        })
    }
}
