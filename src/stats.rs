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

            // 逐事件计费：有实际费用（如 DeepSeek GUI 的 costUsd）就用实际值，
            // 否则按该事件模型定价估算。两者均为 USD，统一求和后折算人民币。
            // 不再"有实际费用就只算实际费用"——那样会丢掉无 actual_cost 的事件。
            let raw_cost: f64 = tool_events.iter()
                .map(|e| e.actual_cost.unwrap_or_else(|| {
                    FunMetrics::estimate_cost(
                        e.input_tokens,
                        e.output_tokens,
                        e.cache_read_tokens,
                        e.model_name.as_deref(),
                    )
                }))
                .sum();
            // 统一折算为人民币：日志原始计费与按价估算均为 USD，按汇率换算
            let estimated_cost = raw_cost * crate::models::get_usd_to_rmb();
            // 代码行数只用输出 token 估算（输入/缓存是读进来的上下文，非生成代码）
            let code_lines = FunMetrics::tokens_to_code_lines(total_output);
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

    /// 重新聚合所有有原始事件的日期（改价后重算费用）
    pub fn recompute_all(storage: &Storage) -> anyhow::Result<usize> {
        let dates = storage.get_all_raw_event_dates()?;
        let mut count = 0;
        for date in dates {
            Self::aggregate_daily(storage, &date)?;
            count += 1;
        }
        info!("重新聚合完成，共 {} 个日期", count);
        Ok(count)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_storage() -> (Storage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Storage::open(&db_path).unwrap();
        (storage, dir)
    }

    #[test]
    fn test_recompute_all() {
        let (storage, _dir) = create_test_storage();
        let events = vec![
            RawEvent {
                id: None, tool_name: "claude_code".to_string(),
                timestamp: "2026-06-08T10:00:00".to_string(),
                input_tokens: 1000, output_tokens: 500, cache_read_tokens: 300,
                model_name: Some("claude-sonnet-4".to_string()),
                actual_cost: None, raw_line: Some("test".to_string()),
            },
            RawEvent {
                id: None, tool_name: "claude_code".to_string(),
                timestamp: "2026-06-09T11:00:00".to_string(),
                input_tokens: 2000, output_tokens: 1000, cache_read_tokens: 600,
                model_name: Some("claude-sonnet-4".to_string()),
                actual_cost: None, raw_line: Some("test2".to_string()),
            },
        ];
        storage.insert_raw_events(&events).unwrap();

        // 先手动聚合 06-08，06-09 尚未聚合
        let first = StatsEngine::aggregate_daily(&storage,
            &NaiveDate::parse_from_str("2026-06-08", "%Y-%m-%d").unwrap()).unwrap();
        let cost_0608 = first[0].estimated_cost;
        assert!(cost_0608 > 0.0, "claude-sonnet-4 应有非零估算费用");
        assert!(!storage.has_daily_stats(
            &NaiveDate::parse_from_str("2026-06-09", "%Y-%m-%d").unwrap(),
            "claude_code"));

        // recompute_all 应聚合所有有原始事件的日期
        let count = StatsEngine::recompute_all(&storage).unwrap();
        assert_eq!(count, 2, "应聚合 2 个日期");

        // 06-09 现在应有统计；06-08 重算后费用应保持一致（幂等）
        assert!(storage.has_daily_stats(
            &NaiveDate::parse_from_str("2026-06-09", "%Y-%m-%d").unwrap(),
            "claude_code"));
        let recomputed = StatsEngine::aggregate_daily(&storage,
            &NaiveDate::parse_from_str("2026-06-08", "%Y-%m-%d").unwrap()).unwrap();
        assert!((recomputed[0].estimated_cost - cost_0608).abs() < 1e-9,
            "重算后 06-08 费用应与首次一致");
    }

    #[test]
    fn test_recompute_reflects_price_change() {
        // 串行化所有触碰全局覆盖表的测试，避免 set_pricing_overrides 互相覆盖
        let _guard = crate::models::PRICING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (storage, _dir) = create_test_storage();
        // price-test-model 不匹配任何硬编码分支 → 兜底价 2.5/10/1.25；1M input → ¥18.0（$2.5 × 汇率）
        let events = vec![
            RawEvent {
                id: None, tool_name: "claude_code".to_string(),
                timestamp: "2026-06-08T10:00:00".to_string(),
                input_tokens: 1_000_000, output_tokens: 0, cache_read_tokens: 0,
                model_name: Some("price-test-model".to_string()),
                actual_cost: None, raw_line: Some("test".to_string()),
            },
        ];
        storage.insert_raw_events(&events).unwrap();

        let date = NaiveDate::parse_from_str("2026-06-08", "%Y-%m-%d").unwrap();
        let cost_default = StatsEngine::aggregate_daily(&storage, &date).unwrap()[0].estimated_cost;
        assert!((cost_default - 2.5 * crate::models::DEFAULT_USD_TO_RMB).abs() < 1e-9, "默认兜底价下 1M input 应为 ¥{:.2}", 2.5 * crate::models::DEFAULT_USD_TO_RMB);

        // 设覆盖价 100/0/0，recompute_all 应反映新价 → ¥720（$100 × 汇率）
        crate::models::set_pricing_overrides(std::collections::HashMap::from([(
            "price-test-model".to_string(),
            crate::models::ModelPricing {
                input_per_mtok: 100.0, output_per_mtok: 0.0, cache_read_per_mtok: 0.0,
            },
        )]));
        StatsEngine::recompute_all(&storage).unwrap();
        let cost_new = StatsEngine::aggregate_daily(&storage, &date).unwrap()[0].estimated_cost;
        assert!(cost_new > cost_default, "改价后费用应增大");
        assert!((cost_new - 100.0 * crate::models::DEFAULT_USD_TO_RMB).abs() < 1e-9, "覆盖价 100/0/0 下 1M input 应为 ¥{:.2}", 100.0 * crate::models::DEFAULT_USD_TO_RMB);

        // 清理全局表，避免污染其他测试
        crate::models::clear_pricing_overrides();
    }
}
