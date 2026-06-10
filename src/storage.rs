use std::path::Path;

use chrono::NaiveDate;
use log::{debug, info};
use rusqlite::{params, Connection, Result as SqlResult};

use crate::models::{CacheStats, DailyStats, RawEvent, TrendPoint};

/// SQLite 存储管理器
pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        let mut storage = Self { conn };
        storage.init_tables()?;
        storage.migrate()?;
        Ok(storage)
    }

    fn init_tables(&mut self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS raw_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tool_name TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                model_name TEXT,
                actual_cost REAL,
                raw_line TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                UNIQUE(timestamp, tool_name, input_tokens, output_tokens)
            );

            CREATE INDEX IF NOT EXISTS idx_raw_events_timestamp ON raw_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_raw_events_tool ON raw_events(tool_name);

            CREATE TABLE IF NOT EXISTS daily_stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                date TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                estimated_cost REAL NOT NULL DEFAULT 0.0,
                code_lines_equivalent INTEGER NOT NULL DEFAULT 0,
                opus4_equivalent REAL NOT NULL DEFAULT 0.0,
                event_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT DEFAULT (datetime('now')),
                UNIQUE(date, tool_name)
            );

            CREATE INDEX IF NOT EXISTS idx_daily_stats_date ON daily_stats(date);
            CREATE INDEX IF NOT EXISTS idx_daily_stats_tool ON daily_stats(tool_name);
            "
        )?;
        Ok(())
    }

    /// 数据库迁移：添加缺失的列
    fn migrate(&mut self) -> anyhow::Result<()> {
        // 添加 actual_cost 列（如果不存在）
        let has_actual_cost: bool = self.conn
            .prepare("SELECT actual_cost FROM raw_events LIMIT 0")
            .is_ok();
        if !has_actual_cost {
            self.conn.execute_batch(
                "ALTER TABLE raw_events ADD COLUMN actual_cost REAL;"
            )?;
        }
        Ok(())
    }

    /// 批量插入原始事件（去重）
    pub fn insert_raw_events(&self, events: &[RawEvent]) -> anyhow::Result<usize> {
        let mut inserted = 0;
        let tx = self.conn.unchecked_transaction()?;

        for event in events {
            let result = tx.execute(
                "INSERT OR IGNORE INTO raw_events (tool_name, timestamp, input_tokens, output_tokens, cache_read_tokens, model_name, actual_cost, raw_line)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    event.tool_name,
                    event.timestamp,
                    event.input_tokens,
                    event.output_tokens,
                    event.cache_read_tokens,
                    event.model_name,
                    event.actual_cost,
                    event.raw_line,
                ],
            )?;
            inserted += result;
        }

        tx.commit()?;
        debug!("插入了 {} 条原始事件", inserted);
        Ok(inserted)
    }

    pub fn upsert_daily_stats(&self, stats: &DailyStats) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO daily_stats (date, tool_name, total_input_tokens, total_output_tokens, total_cache_read_tokens, estimated_cost, code_lines_equivalent, opus4_equivalent, event_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(date, tool_name) DO UPDATE SET
                total_input_tokens = excluded.total_input_tokens,
                total_output_tokens = excluded.total_output_tokens,
                total_cache_read_tokens = excluded.total_cache_read_tokens,
                estimated_cost = excluded.estimated_cost,
                code_lines_equivalent = excluded.code_lines_equivalent,
                opus4_equivalent = excluded.opus4_equivalent,
                event_count = excluded.event_count",
            params![
                stats.date,
                stats.tool_name,
                stats.total_input_tokens,
                stats.total_output_tokens,
                stats.total_cache_read_tokens,
                stats.estimated_cost,
                stats.code_lines_equivalent,
                stats.opus4_equivalent,
                stats.event_count,
            ],
        )?;
        Ok(())
    }

    pub fn has_daily_stats(&self, date: &NaiveDate, tool_name: &str) -> bool {
        let date_str = date.format("%Y-%m-%d").to_string();
        let result: Result<i64, _> = self.conn.query_row(
            "SELECT COUNT(*) FROM daily_stats WHERE date = ?1 AND tool_name = ?2",
            params![date_str, tool_name],
            |row| row.get(0),
        );
        result.unwrap_or(0) > 0
    }

    /// 清空所有原始事件和每日统计，用于全量重建（--rebuild 使用 drop + 删除文件方式替代）
    #[allow(dead_code)]
    pub fn clear_all_data(&self) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM raw_events", [])?;
        self.conn.execute("DELETE FROM daily_stats", [])?;
        info!("已清空所有原始事件和每日统计数据");
        Ok(())
    }

    pub fn get_daily_stats_range(
        &self,
        start_date: &NaiveDate,
        end_date: &NaiveDate,
    ) -> anyhow::Result<Vec<DailyStats>> {
        let start_str = start_date.format("%Y-%m-%d").to_string();
        let end_str = end_date.format("%Y-%m-%d").to_string();
        let mut stmt = self.conn.prepare(
            "SELECT id, date, tool_name, total_input_tokens, total_output_tokens,
                    total_cache_read_tokens, estimated_cost, code_lines_equivalent,
                    opus4_equivalent, event_count
             FROM daily_stats
             WHERE date BETWEEN ?1 AND ?2
             ORDER BY date, tool_name"
        )?;

        let stats = stmt.query_map(params![start_str, end_str], |row| {
            Ok(DailyStats {
                id: row.get(0)?,
                date: row.get(1)?,
                tool_name: row.get(2)?,
                total_input_tokens: row.get(3)?,
                total_output_tokens: row.get(4)?,
                total_cache_read_tokens: row.get(5)?,
                estimated_cost: row.get(6)?,
                code_lines_equivalent: row.get(7)?,
                opus4_equivalent: row.get(8)?,
                event_count: row.get(9)?,
            })
        })?.collect::<SqlResult<Vec<_>>>()?;

        Ok(stats)
    }

    pub fn get_all_daily_stats(&self) -> anyhow::Result<Vec<DailyStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, date, tool_name, total_input_tokens, total_output_tokens,
                    total_cache_read_tokens, estimated_cost, code_lines_equivalent,
                    opus4_equivalent, event_count
             FROM daily_stats
             ORDER BY date DESC, tool_name"
        )?;

        let stats = stmt.query_map([], |row| {
            Ok(DailyStats {
                id: row.get(0)?,
                date: row.get(1)?,
                tool_name: row.get(2)?,
                total_input_tokens: row.get(3)?,
                total_output_tokens: row.get(4)?,
                total_cache_read_tokens: row.get(5)?,
                estimated_cost: row.get(6)?,
                code_lines_equivalent: row.get(7)?,
                opus4_equivalent: row.get(8)?,
                event_count: row.get(9)?,
            })
        })?.collect::<SqlResult<Vec<_>>>()?;

        Ok(stats)
    }

    pub fn get_raw_events_by_date(
        &self,
        date: &NaiveDate,
    ) -> anyhow::Result<Vec<RawEvent>> {
        let date_str = date.format("%Y-%m-%d").to_string();
        let date_pattern = format!("{}%", date_str);
        let mut stmt = self.conn.prepare(
            "SELECT id, tool_name, timestamp, input_tokens, output_tokens,
                    cache_read_tokens, model_name, actual_cost, raw_line
             FROM raw_events
             WHERE timestamp LIKE ?1
             ORDER BY timestamp"
        )?;

        let events = stmt.query_map(params![date_pattern], |row| {
            let actual_cost: Option<f64> = row.get(7)?;
            Ok(RawEvent {
                id: row.get(0)?,
                tool_name: row.get(1)?,
                timestamp: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                model_name: row.get(6)?,
                actual_cost,
                raw_line: row.get(8)?,
            })
        })?.collect::<SqlResult<Vec<_>>>()?;

        Ok(events)
    }

    pub fn get_trend_points(
        &self,
        start: &str,
        end: &str,
        granularity: &str,
    ) -> anyhow::Result<Vec<TrendPoint>> {
        let bucket_expr = if granularity == "hour" {
            "substr(timestamp, 1, 13) || ':00'"
        } else {
            "substr(timestamp, 1, 10)"
        };

        let sql = format!(
            "SELECT {bucket_expr} as bucket, tool_name,
                    SUM(input_tokens) as input_tokens,
                    SUM(output_tokens) as output_tokens,
                    SUM(cache_read_tokens) as cache_read_tokens,
                    SUM(COALESCE(actual_cost, 0.0)) as actual_cost_sum,
                    COUNT(*) as event_count,
                    MAX(model_name) as model_name
             FROM raw_events
             WHERE timestamp >= ?1 AND timestamp < ?2
             GROUP BY bucket, tool_name
             ORDER BY bucket, tool_name"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let points = stmt.query_map(params![start, end], |row| {
            let input_tokens: i64 = row.get(2)?;
            let output_tokens: i64 = row.get(3)?;
            let cache_read_tokens: i64 = row.get(4)?;
            let actual_cost_sum: f64 = row.get(5)?;
            let model_name: Option<String> = row.get(7)?;
            let estimated_cost = if actual_cost_sum > 0.0 {
                actual_cost_sum
            } else {
                crate::models::FunMetrics::estimate_cost(
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    model_name.as_deref(),
                )
            };

            Ok(TrendPoint {
                bucket: row.get(0)?,
                tool_name: row.get(1)?,
                estimated_cost,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                event_count: row.get(6)?,
            })
        })?.collect::<SqlResult<Vec<_>>>()?;

        Ok(points)
    }

    pub fn get_tool_names(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT tool_name FROM daily_stats ORDER BY tool_name"
        )?;
        let names = stmt.query_map([], |row| row.get(0))?.collect::<SqlResult<Vec<_>>>()?;
        Ok(names)
    }

    pub fn get_dates(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT date FROM daily_stats ORDER BY date DESC"
        )?;
        let dates = stmt.query_map([], |row| row.get(0))?.collect::<SqlResult<Vec<_>>>()?;
        Ok(dates)
    }

    /// 获取指定日期范围的缓存统计数据（从 daily_stats 聚合）
    pub fn get_cache_stats(
        &self,
        start_date: &str,
        end_date: &str,
    ) -> anyhow::Result<Vec<CacheStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT tool_name, date, total_input_tokens, total_output_tokens, total_cache_read_tokens
             FROM daily_stats
             WHERE date BETWEEN ?1 AND ?2
             ORDER BY tool_name, date"
        )?;

        let stats = stmt.query_map(params![start_date, end_date], |row| {
            Ok(CacheStats {
                tool_name: row.get(0)?,
                date: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_read_tokens: row.get(4)?,
            })
        })?.collect::<SqlResult<Vec<_>>>()?;

        Ok(stats)
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

    // ============================================================
    // 数据库初始化测试
    // ============================================================

    #[test]
    fn test_storage_open_creates_tables() {
        let (storage, _dir) = create_test_storage();
        let result = storage.conn.execute("SELECT 1 FROM raw_events LIMIT 1", []);
        assert!(result.is_ok(), "raw_events 表应存在");

        let result = storage.conn.execute("SELECT 1 FROM daily_stats LIMIT 1", []);
        assert!(result.is_ok(), "daily_stats 表应存在");
    }

    // ============================================================
    // 原始事件插入测试
    // ============================================================

    #[test]
    fn test_insert_raw_events() {
        let (storage, _dir) = create_test_storage();
        let events = vec![
            RawEvent {
                id: None,
                tool_name: "claude_code".to_string(),
                timestamp: "2026-06-09T10:00:00".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 30,
                model_name: Some("claude-sonnet".to_string()),
                actual_cost: None,
                raw_line: Some("test".to_string()),
            },
        ];

        let inserted = storage.insert_raw_events(&events).unwrap();
        assert_eq!(inserted, 1, "应插入 1 条事件");
    }

    #[test]
    fn test_insert_raw_events_dedup() {
        let (storage, _dir) = create_test_storage();
        let events = vec![
            RawEvent {
                id: None,
                tool_name: "claude_code".to_string(),
                timestamp: "2026-06-09T10:00:00".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 30,
                model_name: Some("claude-sonnet".to_string()),
                actual_cost: None,
                raw_line: Some("test".to_string()),
            },
        ];

        storage.insert_raw_events(&events).unwrap();
        let inserted2 = storage.insert_raw_events(&events).unwrap();
        assert_eq!(inserted2, 0, "重复事件应被忽略");
    }

    #[test]
    fn test_insert_raw_events_with_actual_cost() {
        let (storage, _dir) = create_test_storage();
        let events = vec![
            RawEvent {
                id: None,
                tool_name: "deepseek_gui".to_string(),
                timestamp: "2026-06-09T10:00:00".to_string(),
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: 200,
                model_name: Some("deepseek-v4-pro".to_string()),
                actual_cost: Some(0.05),
                raw_line: Some("test".to_string()),
            },
        ];

        let inserted = storage.insert_raw_events(&events).unwrap();
        assert_eq!(inserted, 1);

        let events = storage.get_raw_events_by_date(
            &NaiveDate::parse_from_str("2026-06-09", "%Y-%m-%d").unwrap()
        ).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actual_cost, Some(0.05));
    }

    // ============================================================
    // 每日统计测试
    // ============================================================

    #[test]
    fn test_upsert_daily_stats() {
        let (storage, _dir) = create_test_storage();
        let stats = DailyStats {
            id: None,
            date: "2026-06-09".to_string(),
            tool_name: "claude_code".to_string(),
            total_input_tokens: 1000,
            total_output_tokens: 500,
            total_cache_read_tokens: 300,
            estimated_cost: 0.15,
            code_lines_equivalent: 120,
            opus4_equivalent: 0.375,
            event_count: 5,
        };

        storage.upsert_daily_stats(&stats).unwrap();
        assert!(storage.has_daily_stats(
            &NaiveDate::parse_from_str("2026-06-09", "%Y-%m-%d").unwrap(),
            "claude_code"
        ));
    }

    #[test]
    fn test_upsert_daily_stats_update() {
        let (storage, _dir) = create_test_storage();
        let stats = DailyStats {
            id: None,
            date: "2026-06-09".to_string(),
            tool_name: "claude_code".to_string(),
            total_input_tokens: 1000,
            total_output_tokens: 500,
            total_cache_read_tokens: 300,
            estimated_cost: 0.15,
            code_lines_equivalent: 120,
            opus4_equivalent: 0.375,
            event_count: 5,
        };

        storage.upsert_daily_stats(&stats).unwrap();

        let updated = DailyStats {
            id: None,
            date: "2026-06-09".to_string(),
            tool_name: "claude_code".to_string(),
            total_input_tokens: 2000,
            total_output_tokens: 1000,
            total_cache_read_tokens: 600,
            estimated_cost: 0.30,
            code_lines_equivalent: 240,
            opus4_equivalent: 0.75,
            event_count: 10,
        };

        storage.upsert_daily_stats(&updated).unwrap();

        let result = storage.get_daily_stats_range(
            &NaiveDate::parse_from_str("2026-06-09", "%Y-%m-%d").unwrap(),
            &NaiveDate::parse_from_str("2026-06-09", "%Y-%m-%d").unwrap(),
        ).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].total_input_tokens, 2000, "upsert 应更新已有记录");
    }

    #[test]
    fn test_get_cache_stats() {
        let (storage, _dir) = create_test_storage();

        let stats = vec![
            DailyStats {
                id: None, date: "2026-06-09".to_string(), tool_name: "claude_code".to_string(),
                total_input_tokens: 1000, total_output_tokens: 500, total_cache_read_tokens: 800,
                estimated_cost: 0.15, code_lines_equivalent: 153, opus4_equivalent: 0.375, event_count: 5,
            },
            DailyStats {
                id: None, date: "2026-06-09".to_string(), tool_name: "cursor".to_string(),
                total_input_tokens: 2000, total_output_tokens: 1000, total_cache_read_tokens: 0,
                estimated_cost: 0.10, code_lines_equivalent: 200, opus4_equivalent: 0.25, event_count: 3,
            },
        ];

        for s in &stats {
            storage.upsert_daily_stats(s).unwrap();
        }

        let cache_stats = storage.get_cache_stats("2026-06-09", "2026-06-09").unwrap();
        assert_eq!(cache_stats.len(), 2, "应返回 2 个工具的缓存统计");

        let claude_stats: Vec<&CacheStats> = cache_stats.iter()
            .filter(|s| s.tool_name == "claude_code").collect();
        assert_eq!(claude_stats.len(), 1);
        assert_eq!(claude_stats[0].cache_read_tokens, 800);
        assert_eq!(claude_stats[0].input_tokens, 1000);
    }

    #[test]
    fn test_get_raw_events_by_date() {
        let (storage, _dir) = create_test_storage();
        let events = vec![
            RawEvent {
                id: None, tool_name: "claude_code".to_string(),
                timestamp: "2026-06-09T10:00:00".to_string(),
                input_tokens: 100, output_tokens: 50, cache_read_tokens: 30,
                model_name: Some("claude-sonnet".to_string()),
                actual_cost: None, raw_line: Some("test".to_string()),
            },
            RawEvent {
                id: None, tool_name: "deepseek_gui".to_string(),
                timestamp: "2026-06-09T11:00:00".to_string(),
                input_tokens: 200, output_tokens: 100, cache_read_tokens: 50,
                model_name: Some("deepseek-v4-pro".to_string()),
                actual_cost: Some(0.05), raw_line: Some("test".to_string()),
            },
        ];

        storage.insert_raw_events(&events).unwrap();

        let result = storage.get_raw_events_by_date(
            &NaiveDate::parse_from_str("2026-06-09", "%Y-%m-%d").unwrap()
        ).unwrap();

        assert_eq!(result.len(), 2, "应返回 2 条事件");
    }

    #[test]
    fn test_get_tool_names() {
        let (storage, _dir) = create_test_storage();

        let stats = vec![
            DailyStats {
                id: None, date: "2026-06-09".to_string(), tool_name: "claude_code".to_string(),
                total_input_tokens: 100, total_output_tokens: 50, total_cache_read_tokens: 30,
                estimated_cost: 0.01, code_lines_equivalent: 12, opus4_equivalent: 0.025, event_count: 1,
            },
            DailyStats {
                id: None, date: "2026-06-09".to_string(), tool_name: "cursor".to_string(),
                total_input_tokens: 200, total_output_tokens: 100, total_cache_read_tokens: 0,
                estimated_cost: 0.02, code_lines_equivalent: 20, opus4_equivalent: 0.05, event_count: 1,
            },
        ];

        for s in &stats {
            storage.upsert_daily_stats(s).unwrap();
        }

        let tools = storage.get_tool_names().unwrap();
        assert!(tools.contains(&"claude_code".to_string()), "应包含 claude_code");
        assert!(tools.contains(&"cursor".to_string()), "应包含 cursor");
    }

    #[test]
    fn test_has_daily_stats_negative() {
        let (storage, _dir) = create_test_storage();
        let result = storage.has_daily_stats(
            &NaiveDate::parse_from_str("2026-06-09", "%Y-%m-%d").unwrap(),
            "nonexistent_tool"
        );
        assert!(!result, "不存在的工具/日期应返回 false");
    }

    #[test]
    fn test_get_dates() {
        let (storage, _dir) = create_test_storage();

        let stats = vec![
            DailyStats {
                id: None, date: "2026-06-08".to_string(), tool_name: "claude_code".to_string(),
                total_input_tokens: 100, total_output_tokens: 50, total_cache_read_tokens: 30,
                estimated_cost: 0.01, code_lines_equivalent: 12, opus4_equivalent: 0.025, event_count: 1,
            },
            DailyStats {
                id: None, date: "2026-06-09".to_string(), tool_name: "claude_code".to_string(),
                total_input_tokens: 200, total_output_tokens: 100, total_cache_read_tokens: 60,
                estimated_cost: 0.02, code_lines_equivalent: 24, opus4_equivalent: 0.05, event_count: 2,
            },
        ];

        for s in &stats {
            storage.upsert_daily_stats(s).unwrap();
        }

        let dates = storage.get_dates().unwrap();
        assert_eq!(dates.len(), 2, "应有 2 个日期");
        assert_eq!(dates[0], "2026-06-09", "日期应降序排列");
        assert_eq!(dates[1], "2026-06-08");
    }
}
