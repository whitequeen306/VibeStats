use std::path::Path;

use actix_files as fs;
use actix_web::{web, App, HttpServer, HttpResponse};
use log::info;

use crate::config::Config;
use crate::models::{AggregatedStats, CacheStatsResponse, CacheToolStats, CacheTotals, TrendResponse, ToolTrendSeries, TrendPointValue};
use crate::storage::Storage;

const TOOL_COLORS: &[(&str, &str)] = &[
    ("claude_code", "#7C3AED"),
    ("deepseek_gui", "#00B4D8"),
    ("cursor", "#F59E0B"),
    ("codex", "#10B981"),
    ("copilot_jb", "#6366F1"),
    ("trae_cn", "#EF4444"),
    ("lingma", "#EC4899"),
    ("opencoder", "#8B5CF6"),
    ("windsurf", "#14B8A6"),
    ("aider", "#F97316"),
    ("cline", "#06B6D4"),
    ("roo_code", "#84CC16"),
    ("continue_dev", "#A855F7"),
    ("github_copilot", "#3B82F6"),
    ("amazon_q", "#FF6B35"),
];

#[allow(dead_code)]
fn get_tool_color(tool_name: &str) -> &'static str {
    TOOL_COLORS.iter()
        .find(|(name, _)| *name == tool_name)
        .map(|(_, color)| *color)
        .unwrap_or("#94A3B8")
}

pub struct Dashboard;

impl Dashboard {
    pub async fn serve(config: &Config, storage: Storage) -> anyhow::Result<()> {
        let port = config.serve_port;
        let data_dir = Config::data_dir();
        info!("启动 Web Dashboard，地址: http://localhost:{}", port);

        let storage_data = web::Data::new(std::sync::Mutex::new(storage));
        let config_data = web::Data::new(config.clone());
        let data_dir_clone = data_dir.clone();

        HttpServer::new(move || {
            App::new()
                .app_data(storage_data.clone())
                .app_data(config_data.clone())
                .route("/api/stats", web::get().to(get_stats))
                .route("/api/stats/range", web::get().to(get_stats_range))
                .route("/api/trend", web::get().to(get_trend))
                .route("/api/tools", web::get().to(get_tools))
                .route("/api/builtin-tools", web::get().to(get_builtin_tools))
                .route("/api/dates", web::get().to(get_dates))
                .route("/api/cache-stats", web::get().to(get_cache_stats))
                .route("/", web::get().to(index))
                .service(fs::Files::new("/static", data_dir_clone.join("static")).show_files_listing())
        })
        .bind(format!("127.0.0.1:{}", port))?
        .run()
        .await?;
        Ok(())
    }

    pub fn generate_static_report(storage: &Storage, output_path: &Path) -> anyhow::Result<()> {
        let today = chrono::Local::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);
        let aggregated = crate::stats::StatsEngine::get_aggregated_range(storage, &yesterday, &yesterday)?;
        let html = render_dashboard_html(&aggregated)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(output_path, html)?;
        info!("静态报告已生成: {}", output_path.display());
        Ok(())
    }
}

async fn get_stats(storage: web::Data<std::sync::Mutex<Storage>>) -> HttpResponse {
    let storage = storage.lock().unwrap();
    match storage.get_all_daily_stats() {
        Ok(stats) => HttpResponse::Ok().json(stats),
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn get_stats_range(
    storage: web::Data<std::sync::Mutex<Storage>>,
    query: web::Query<RangeQuery>,
) -> HttpResponse {
    let storage = storage.lock().unwrap();
    let start = chrono::NaiveDate::parse_from_str(&query.start, "%Y-%m-%d");
    let end = chrono::NaiveDate::parse_from_str(&query.end, "%Y-%m-%d");
    match (start, end) {
        (Ok(s), Ok(e)) => {
            match crate::stats::StatsEngine::get_aggregated_range(&storage, &s, &e) {
                Ok(stats) => HttpResponse::Ok().json(stats),
                Err(err) => HttpResponse::InternalServerError().body(format!("Error: {}", err)),
            }
        }
        _ => HttpResponse::BadRequest().body("Invalid date format. Use YYYY-MM-DD"),
    }
}

async fn get_trend(
    storage: web::Data<std::sync::Mutex<Storage>>,
    query: web::Query<TrendQuery>,
) -> HttpResponse {
    let storage = storage.lock().unwrap();
    let now = chrono::Local::now();
    let (start, end, granularity) = match query.range.as_str() {
        "1d" => {
            // 近一天 = 今天 00:00:00 到明天 00:00:00（补齐整24小时内的所有时段）
            let today = now.format("%Y-%m-%d").to_string();
            let tomorrow = (now + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
            (format!("{}T00:00", today), format!("{}T00:00", tomorrow), "hour".to_string())
        }
        "1w" => {
            let s = (now - chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
            let e = now.format("%Y-%m-%d").to_string();
            (s, e, "day".to_string())
        }
        "1m" => {
            let s = (now - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
            let e = now.format("%Y-%m-%d").to_string();
            (s, e, "day".to_string())
        }
        _ => {
            let s = (now - chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
            let e = now.format("%Y-%m-%d").to_string();
            (s, e, "day".to_string())
        }
    };

    match storage.get_trend_points(&start, &end, &granularity) {
        Ok(points) => {
            // 收集所有 bucket 和 tool_name
            let mut bucket_set = std::collections::BTreeSet::new();
            let mut tool_set = std::collections::BTreeSet::new();
            for p in &points {
                bucket_set.insert(p.bucket.clone());
                tool_set.insert(p.tool_name.clone());
            }
            let buckets: Vec<String> = bucket_set.into_iter().collect();

            // 按 tool 分组
            let mut series_map: std::collections::HashMap<String, Vec<TrendPointValue>> =
                std::collections::HashMap::new();
            for p in points {
                series_map.entry(p.tool_name.clone()).or_default().push(TrendPointValue {
                    bucket: p.bucket,
                    estimated_cost: p.estimated_cost,
                    input_tokens: p.input_tokens,
                    output_tokens: p.output_tokens,
                    cache_read_tokens: p.cache_read_tokens,
                    event_count: p.event_count,
                });
            }

            let series: Vec<ToolTrendSeries> = tool_set.into_iter().map(|tool_name| {
                let mut pts = series_map.remove(&tool_name).unwrap_or_default();
                pts.sort_by(|a, b| a.bucket.cmp(&b.bucket));
                ToolTrendSeries { tool_name, points: pts }
            }).collect();

            let resp = TrendResponse { granularity, buckets, series };
            HttpResponse::Ok().json(resp)
        }
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn get_tools(storage: web::Data<std::sync::Mutex<Storage>>) -> HttpResponse {
    let storage = storage.lock().unwrap();
    match storage.get_tool_names() {
        Ok(tools) => HttpResponse::Ok().json(tools),
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn get_builtin_tools(config: web::Data<Config>) -> HttpResponse {
    let status = config.all_tools_status();
    HttpResponse::Ok().json(status)
}

async fn get_dates(storage: web::Data<std::sync::Mutex<Storage>>) -> HttpResponse {
    let storage = storage.lock().unwrap();
    match storage.get_dates() {
        Ok(dates) => HttpResponse::Ok().json(dates),
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn get_cache_stats(
    storage: web::Data<std::sync::Mutex<Storage>>,
    query: web::Query<CacheStatsQuery>,
) -> HttpResponse {
    let storage = storage.lock().unwrap();
    let start = &query.start;
    let end = &query.end;

    match storage.get_cache_stats(start, end) {
        Ok(stats) => {
            // 按工具聚合
            let mut tool_map: std::collections::HashMap<String, (i64, i64, i64)> = std::collections::HashMap::new();
            for s in &stats {
                let entry = tool_map.entry(s.tool_name.clone()).or_insert((0, 0, 0));
                entry.0 += s.input_tokens;
                entry.1 += s.output_tokens;
                entry.2 += s.cache_read_tokens;
            }

            let mut total_input = 0i64;
            let mut total_output = 0i64;
            let mut total_cache = 0i64;
            // 仅 Claude Code、DeepSeek GUI、OpenCode 有缓存数据
            let cache_supporting_tools: std::collections::HashSet<&str> =
                ["claude_code", "deepseek_gui", "opencode"].iter().copied().collect();

            let by_tool: Vec<CacheToolStats> = tool_map.into_iter().map(|(tool_name, (input, output, cache))| {
                total_input += input;
                total_output += output;
                total_cache += cache;
                let total_readable = input + cache;
                let hit_rate = if total_readable > 0 {
                    cache as f64 / total_readable as f64 * 100.0
                } else {
                    0.0
                };
                CacheToolStats {
                    tool_name,
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cache,
                    cache_hit_rate: hit_rate,
                }
            }).collect();

            // 总体命中率只计算有缓存数据的工具
            let (cache_input, cache_only) = stats.iter()
                .filter(|s| cache_supporting_tools.contains(s.tool_name.as_str()))
                .fold((0i64, 0i64), |(input_acc, cache_acc), s| {
                    (input_acc + s.input_tokens, cache_acc + s.cache_read_tokens)
                });
            let overall_total_readable = cache_input + cache_only;
            let overall_hit_rate = if overall_total_readable > 0 {
                cache_only as f64 / overall_total_readable as f64 * 100.0
            } else {
                0.0
            };

            let resp = CacheStatsResponse {
                by_tool,
                totals: CacheTotals {
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    total_cache_read_tokens: total_cache,
                    overall_cache_hit_rate: overall_hit_rate,
                },
            };
            HttpResponse::Ok().json(resp)
        }
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

async fn index(storage: web::Data<std::sync::Mutex<Storage>>) -> HttpResponse {
    let storage = storage.lock().unwrap();
    let today = chrono::Local::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);

    match crate::stats::StatsEngine::get_aggregated_range(&storage, &yesterday, &yesterday) {
        Ok(aggregated) => {
            match render_dashboard_html(&aggregated) {
                Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
                Err(e) => HttpResponse::InternalServerError().body(format!("Render error: {}", e)),
            }
        }
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

#[derive(serde::Deserialize)]
struct RangeQuery {
    start: String,
    end: String,
}

#[derive(serde::Deserialize)]
struct TrendQuery {
    range: String,
}

#[derive(serde::Deserialize)]
struct CacheStatsQuery {
    start: String,
    end: String,
}

fn render_dashboard_html(stats: &AggregatedStats) -> anyhow::Result<String> {
    let stats_json = serde_json::to_string(stats)?;
    let color_map_js: Vec<String> = TOOL_COLORS.iter()
        .map(|(name, color)| format!("'{}': '{}'", name, color))
        .collect();
    let color_map_js = color_map_js.join(", ");

    let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let yesterday_str = yesterday.format("%Y-%m-%d").to_string();

    let html = format!(r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>VibeStats - VibeCoding 趣味仪表盘</title>
    <script src="https://cdn.jsdelivr.net/npm/echarts@5/dist/echarts.min.js"></script>
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@400;500;600;700&family=Outfit:wght@300;400;500;600;700;800&family=Noto+Sans+SC:wght@300;400;500;600;700&display=swap" rel="stylesheet">
    <style>
        :root {{
            --bg-deep: #0B0F1A;
            --bg-surface: #111827;
            --bg-elevated: #1A2236;
            --bg-card: rgba(26,34,54,0.7);
            --border-subtle: rgba(255,255,255,0.06);
            --border-medium: rgba(255,255,255,0.1);
            --text-primary: #F0F2F7;
            --text-secondary: #8B95A8;
            --text-muted: #5A6478;
            --accent-cyan: #00E5FF;
            --accent-pink: #FF2E7D;
            --accent-amber: #FFB800;
            --accent-lime: #7EE787;
            --accent-violet: #A78BFA;
            --gradient-hero: linear-gradient(135deg, #00E5FF 0%, #7B61FF 50%, #FF2E7D 100%);
            --gradient-card: linear-gradient(180deg, rgba(255,255,255,0.06) 0%, rgba(255,255,255,0.02) 100%);
            --shadow-glow: 0 0 40px rgba(0,229,255,0.08);
            --radius-lg: 20px;
            --radius-md: 14px;
            --radius-sm: 10px;
        }}

        * {{ margin: 0; padding: 0; box-sizing: border-box; }}

        body {{
            font-family: 'Outfit', 'Noto Sans SC', sans-serif;
            background: var(--bg-deep);
            color: var(--text-primary);
            min-height: 100vh;
            overflow-x: hidden;
        }}

        /* 背景噪点纹理 */
        body::before {{
            content: '';
            position: fixed; inset: 0;
            background-image: url("data:image/svg+xml,%3Csvg viewBox='0 0 256 256' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)' opacity='0.03'/%3E%3C/svg%3E");
            pointer-events: none; z-index: 0;
        }}

        /* 顶部光晕 */
        .hero-glow {{
            position: fixed; top: -200px; left: 50%; transform: translateX(-50%);
            width: 800px; height: 400px;
            background: radial-gradient(ellipse, rgba(0,229,255,0.12) 0%, transparent 70%);
            pointer-events: none; z-index: 0;
        }}

        .app {{ position: relative; z-index: 1; }}

        /* 顶部光晕 */
        .header {{
            text-align: center; padding: 48px 20px 16px;
            position: relative;
        }}
        .header-badge {{
            display: inline-flex; align-items: center; gap: 8px;
            padding: 6px 16px; border-radius: 100px;
            background: rgba(0,229,255,0.08); border: 1px solid rgba(0,229,255,0.15);
            font-size: 0.78em; color: var(--accent-cyan); font-weight: 500;
            letter-spacing: 0.5px; margin-bottom: 16px;
        }}
        .header-badge::before {{
            content: ''; width: 6px; height: 6px; border-radius: 50%;
            background: var(--accent-cyan); animation: pulse 2s ease-in-out infinite;
        }}
        @keyframes pulse {{
            0%, 100% {{ opacity: 1; transform: scale(1); }}
            50% {{ opacity: 0.4; transform: scale(0.7); }}
        }}
        .header h1 {{
            font-family: 'Space Grotesk', 'Outfit', sans-serif;
            font-size: 3.2em; font-weight: 700; letter-spacing: -1.5px;
            background: var(--gradient-hero);
            -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
            line-height: 1.1;
        }}
        .header .tagline {{
            color: var(--text-secondary); margin-top: 10px;
            font-size: 1em; font-weight: 300; letter-spacing: 0.5px;
        }}
        .header .date-label {{
            color: var(--text-muted); margin-top: 14px; font-size: 0.9em;
            font-weight: 400; letter-spacing: 1px; text-transform: uppercase;
        }}

        .container {{ max-width: 1400px; margin: 0 auto; padding: 0 24px 40px; }}

        /* ===== Controls ===== */
        .controls {{
            display: flex; gap: 10px; margin-bottom: 24px;
            justify-content: center; align-items: center; flex-wrap: wrap;
        }}
        .time-btn {{
            padding: 9px 22px; border: 1px solid var(--border-subtle);
            background: var(--bg-elevated); color: var(--text-secondary);
            border-radius: 12px; cursor: pointer; font-size: 0.88em;
            font-family: inherit; font-weight: 500;
            transition: all 0.25s cubic-bezier(0.4, 0, 0.2, 1);
            letter-spacing: 0.3px;
        }}
        .time-btn:hover {{
            border-color: rgba(0,229,255,0.3); color: var(--text-primary);
            transform: translateY(-1px);
            box-shadow: 0 4px 20px rgba(0,229,255,0.08);
        }}
        .time-btn.active {{
            background: linear-gradient(135deg, rgba(0,229,255,0.15), rgba(123,97,255,0.15));
            border-color: rgba(0,229,255,0.4); color: var(--accent-cyan);
            box-shadow: 0 0 20px rgba(0,229,255,0.1), inset 0 1px 0 rgba(255,255,255,0.05);
        }}
        .trend-btn {{
            padding: 6px 16px; border: 1px solid var(--border-subtle);
            background: var(--bg-elevated); color: var(--text-muted);
            border-radius: 10px; cursor: pointer; font-size: 0.8em;
            font-family: inherit; font-weight: 500;
            transition: all 0.25s cubic-bezier(0.4, 0, 0.2, 1);
        }}
        .trend-btn:hover {{
            border-color: rgba(255,46,125,0.3); color: var(--text-primary);
        }}
        .trend-btn.active {{
            background: linear-gradient(135deg, rgba(255,46,125,0.15), rgba(123,97,255,0.15));
            border-color: rgba(255,46,125,0.4); color: var(--accent-pink);
            box-shadow: 0 0 16px rgba(255,46,125,0.08);
        }}
        .date-picker {{
            padding: 9px 16px; border: 1px solid var(--border-subtle);
            background: var(--bg-elevated); color: var(--text-secondary);
            border-radius: 12px; font-size: 0.88em;
            transition: all 0.25s;
        }}
        .date-picker:hover {{ border-color: rgba(0,229,255,0.25); }}
        .date-picker input[type="date"] {{
            background: transparent; color: var(--text-secondary); border: none;
            font-family: inherit; font-size: 0.88em; cursor: pointer; outline: none;
        }}
        .date-picker input[type="date"]::-webkit-calendar-picker-indicator {{
            filter: invert(0.6); cursor: pointer;
        }}

        /* 每日摘要 */
        .daily-report {{
            position: relative;
            background: var(--gradient-card);
            border-radius: var(--radius-lg); padding: 28px 32px;
            border: 1px solid var(--border-medium);
            margin-bottom: 24px;
            overflow: hidden;
        }}
        .daily-report::before {{
            content: ''; position: absolute; top: 0; left: 0; right: 0; height: 2px;
            background: var(--gradient-hero); opacity: 0.6;
        }}
        .daily-report-text {{
            font-size: 1.12em; line-height: 1.9; color: var(--text-primary);
            font-weight: 400;
        }}
        .daily-report-text .highlight {{
            color: var(--accent-cyan); font-weight: 600;
        }}
        .daily-report-text .cost-highlight {{
            color: var(--accent-pink); font-weight: 700;
        }}
        .daily-report-text .tool-highlight {{
            font-weight: 600; padding: 2px 10px; border-radius: 8px;
            background: rgba(0,229,255,0.1); border: 1px solid rgba(0,229,255,0.2);
        }}

        /* 核心指标卡片 */
        .summary-cards {{
            display: grid; grid-template-columns: 1.4fr 1fr 1fr 1fr;
            gap: 16px; margin-bottom: 24px;
        }}
        @media (max-width: 1100px) {{ .summary-cards {{ grid-template-columns: repeat(2, 1fr); }} }}
        @media (max-width: 600px) {{ .summary-cards {{ grid-template-columns: 1fr; }} }}

        .card {{
            position: relative;
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 24px;
            border: 1px solid var(--border-subtle);
            transition: all 0.35s cubic-bezier(0.4, 0, 0.2, 1);
            overflow: hidden;
        }}
        .card.cost {{
            padding: 28px 27px 26px;
            border-radius: 22px;
        }}
        .card::before {{
            content: ''; position: absolute; top: 0; left: 0; right: 0; height: 3px;
            opacity: 0; transition: opacity 0.35s;
        }}
        .card:hover {{
            transform: translateY(-4px);
            border-color: var(--border-medium);
            box-shadow: var(--shadow-glow), 0 20px 40px rgba(0,0,0,0.3);
        }}
        .card:hover::before {{ opacity: 1; }}
        .card .label {{
            font-size: 0.8em; color: var(--text-secondary); margin-bottom: 10px;
            font-weight: 500; letter-spacing: 0.5px; text-transform: uppercase;
        }}
        .card .value {{
            font-size: 2em; font-weight: 700; letter-spacing: -0.5px;
            font-family: 'Space Grotesk', 'Outfit', sans-serif;
        }}
        .card .sub {{
            font-size: 0.78em; color: var(--text-muted); margin-top: 6px;
            font-weight: 400;
        }}
        .card.cost::before {{ background: var(--accent-pink); }}
        .card.cost .value {{ color: var(--accent-pink); }}
        .card.lines::before {{ background: var(--accent-cyan); }}
        .card.lines .value {{ color: var(--accent-cyan); }}
        .card.books::before {{ background: var(--accent-violet); }}
        .card.books .value {{ color: var(--accent-violet); }}
        .card.events::before {{ background: var(--accent-amber); }}
        .card.events .value {{ color: var(--accent-amber); }}

        /* 图表区域 */
        .charts-row {{
            display: grid; grid-template-columns: 1fr 1fr;
            gap: 16px; margin-bottom: 24px;
        }}
        @media (max-width: 900px) {{ .charts-row {{ grid-template-columns: 1fr; }} }}

        .chart-box {{
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 20px;
            border: 1px solid var(--border-subtle);
            transition: all 0.3s;
        }}
        .chart-box:hover {{
            border-color: var(--border-medium);
            box-shadow: 0 8px 32px rgba(0,0,0,0.2);
        }}
        .chart-box h3 {{
            margin-bottom: 14px; color: var(--text-secondary);
            font-size: 0.92em; font-weight: 600; letter-spacing: 0.5px;
            text-transform: uppercase;
        }}

        /* 趣味换算 */
        .fun-metrics {{
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 24px;
            border: 1px solid var(--border-subtle); margin-bottom: 24px;
        }}
        .fun-metrics h3 {{
            color: var(--text-secondary); margin-bottom: 16px;
            font-size: 0.92em; font-weight: 600; letter-spacing: 0.5px;
            text-transform: uppercase;
        }}
        .fun-grid {{
            display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 12px;
        }}
        .fun-item {{
            background: rgba(255,255,255,0.02); border-radius: var(--radius-sm);
            padding: 18px; border: 1px solid var(--border-subtle);
            transition: all 0.3s;
        }}
        .fun-item:hover {{
            border-color: var(--border-medium);
            background: rgba(255,255,255,0.04);
        }}
        .fun-item .fun-label {{
            color: var(--text-muted); font-size: 0.8em; font-weight: 500;
            letter-spacing: 0.5px; text-transform: uppercase;
        }}
        .fun-item .fun-value {{
            font-size: 1.3em; font-weight: 700; color: var(--accent-cyan);
            margin-top: 6px; font-family: 'Outfit', sans-serif;
        }}
        .fun-item .fun-desc {{
            color: var(--text-muted); font-size: 0.76em; margin-top: 4px;
        }}

        /* Agent 注册表 */
        .tools-section {{
            background: var(--gradient-card);
            border-radius: var(--radius-md); padding: 24px;
            border: 1px solid var(--border-subtle); margin-bottom: 24px;
        }}
        .tools-section h3 {{
            color: var(--text-secondary); margin-bottom: 16px;
            font-size: 0.92em; font-weight: 600; letter-spacing: 0.5px;
            text-transform: uppercase;
        }}
        .tools-section > p {{
            color: var(--text-muted); font-size: 0.82em; margin-bottom: 14px;
        }}
        .tools-grid {{
            display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 10px;
        }}
        .tool-card {{
            background: rgba(255,255,255,0.02); border-radius: var(--radius-sm);
            padding: 14px; display: flex; align-items: center; gap: 12px;
            border: 1px solid var(--border-subtle);
            transition: all 0.25s;
        }}
        .tool-card:hover {{
            border-color: var(--border-medium);
            background: rgba(255,255,255,0.04);
            transform: translateX(3px);
        }}
        .tool-card.enabled {{ border-left: 3px solid var(--accent-cyan); }}
        .tool-card.disabled {{ border-left: 3px solid #3A3F4D; opacity: 0.55; }}
        .tool-card .tool-dot {{
            width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0;
            box-shadow: 0 0 8px currentColor;
        }}
        .tool-card .tool-name {{ font-weight: 600; font-size: 0.9em; color: var(--text-primary); }}
        .tool-card .tool-desc {{ font-size: 0.78em; color: var(--text-muted); }}
        .tool-card .tool-status {{
            margin-left: auto; font-size: 0.75em; padding: 3px 10px; border-radius: 100px;
            font-weight: 500;
        }}
        .tool-card .tool-status.on {{
            background: rgba(0,229,255,0.1); color: var(--accent-cyan);
            border: 1px solid rgba(0,229,255,0.2);
        }}
        .tool-card .tool-status.off {{
            background: rgba(255,255,255,0.03); color: var(--text-muted);
            border: 1px solid var(--border-subtle);
        }}

        /* 底部 */
        .footer {{
            text-align: center; padding: 32px 24px;
            color: var(--text-muted); font-size: 0.82em;
            border-top: 1px solid var(--border-subtle); margin-top: 8px;
        }}

        /* 入场动画 */
        @keyframes fadeUp {{
            from {{ opacity: 0; transform: translateY(24px); }}
            to {{ opacity: 1; transform: translateY(0); }}
        }}
        .summary-cards .card {{
            animation: fadeUp 0.6s cubic-bezier(0.4, 0, 0.2, 1) both;
        }}
        .summary-cards .card:nth-child(1) {{ animation-delay: 0.05s; }}
        .summary-cards .card:nth-child(2) {{ animation-delay: 0.12s; }}
        .summary-cards .card:nth-child(3) {{ animation-delay: 0.19s; }}
        .summary-cards .card:nth-child(4) {{ animation-delay: 0.26s; }}
    </style>
</head>
<body>
    <div class="hero-glow"></div>
    <div class="app">
        <div class="header">
            <div class="header-badge">LIVE DASHBOARD</div>
            <h1>VibeStats</h1>
            <p class="tagline">让每一次 Token 燃烧都有迹可循</p>
            <p class="date-label" id="dateLabel">昨日数据</p>
        </div>

        <div class="container">
            <div class="controls">
                <button class="time-btn active" onclick="setRange('yesterday')">昨日</button>
                <button class="time-btn" onclick="setRange('last_week')">上周</button>
                <button class="time-btn" onclick="setRange('last_month')">上月</button>
                <button class="time-btn" onclick="setRange('all_time')">迄今</button>
                <div class="date-picker">
                    <label style="color:var(--text-muted)">跳转到: </label>
                    <input type="date" id="dateInput" value="{yesterday_str}" onchange="jumpToDate(this.value)">
                </div>
            </div>

            <div class="daily-report" id="dailyReport">
                <div class="daily-report-text" id="dailyReportText">加载中...</div>
            </div>

            <div class="summary-cards">
                <div class="card cost">
                    <div class="label">花了多少钱</div>
                    <div class="value" id="totalCost">$0.00</div>
                    <div class="sub">按各工具实际模型定价</div>
                </div>
                <div class="card lines">
                    <div class="label">敲了多少行代码</div>
                    <div class="value" id="totalLines">0</div>
                    <div class="sub">约 15 Token/行</div>
                </div>
                <div class="card books">
                    <div class="label">够写几本书了</div>
                    <div class="value" id="totalBooks">0</div>
                    <div class="sub">按 30,000 行/本书估算</div>
                </div>
                <div class="card events">
                    <div class="label">调了几次接口</div>
                    <div class="value" id="totalEvents">0</div>
                    <div class="sub">原始事件数量</div>
                </div>
            </div>

            <div class="charts-row">
                <div class="chart-box">
                    <h3>各 Agent 消耗对比</h3>
                    <div id="barChart" style="width:100%;height:380px;"></div>
                </div>
                <div class="chart-box">
                    <h3>各 Agent 花费占比</h3>
                    <div id="pieChart" style="width:100%;height:380px;"></div>
                </div>
            </div>

            <div class="charts-row">
                <div class="chart-box" style="grid-column: 1 / -1;">
                    <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;">
                        <h3 style="margin:0">消耗趋势</h3>
                        <div style="display:flex;gap:8px;">
                            <button class="trend-btn active" onclick="setTrendRange('1d')">近一天</button>
                            <button class="trend-btn" onclick="setTrendRange('1w')">近一周</button>
                            <button class="trend-btn" onclick="setTrendRange('1m')">近一月</button>
                        </div>
                    </div>
                    <div id="trendChart" style="width:100%;height:360px;"></div>
                </div>
            </div>

            <div class="charts-row">
                <div class="chart-box">
                    <h3>Token 分布（输入 / 输出 / 缓存命中）</h3>
                    <div id="tokenDistChart" style="width:100%;height:380px;"></div>
                    <div style="color:var(--text-muted);font-size:0.72em;margin-top:6px;text-align:center;">
                        * Cursor / Trae 的缓存命中为 0 是正常的：这些工具的本地日志不记录缓存数据
                    </div>
                </div>
                <div class="chart-box">
                    <h3>缓存命中率（不含 Cursor/Trae，本地无缓存日志）</h3>
                    <div id="cacheRateChart" style="width:100%;height:380px;"></div>
                    <div style="color:var(--text-muted);font-size:0.72em;margin-top:6px;text-align:center;">
                        * 仅 Claude Code / DeepSeek GUI / OpenCode 支持缓存命中统计
                    </div>
                </div>
            </div>

            <div class="fun-metrics">
                <h3>趣味数据换算</h3>
                <div class="fun-grid" id="funMetrics"></div>
            </div>

            <div class="tools-section">
                <h3>内置 Agent 工具注册表</h3>
                <p>在 config.toml 中启用/禁用工具</p>
                <div class="tools-grid" id="toolsGrid"></div>
            </div>
        </div>

        <div class="footer">VibeStats v0.1 · 数据每日自动更新</div>
    </div>

    <script>
        const rawData = {stats_json};
        const toolColors = {{ {color_map_js} }};

        document.addEventListener('DOMContentLoaded', () => {{
            updateSummary();
            renderBarChart();
            renderPieChart();
            loadTrendData('1d');
            loadCacheStats();
            renderFunMetrics();
            renderDailyReport();
            loadBuiltinTools();
            loadAvailableDates();
        }});

        function updateSummary() {{
            const t = rawData.totals;
            document.getElementById('totalCost').textContent = '$' + t.total_estimated_cost.toFixed(2);
            document.getElementById('totalLines').textContent = t.total_code_lines.toLocaleString();
            document.getElementById('totalBooks').textContent = Math.max(1, Math.floor(t.total_code_lines / 30000)).toLocaleString() + ' 本';
            document.getElementById('totalEvents').textContent = t.total_events.toLocaleString();
        }}

        function formatTokens(n) {{
            if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + 'M';
            if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
            return n.toString();
        }}

        function renderDailyReport() {{
            const t = rawData.totals;
            const totalTokens = t.total_input_tokens + t.total_output_tokens + t.total_cache_read_tokens;
            const codeLines = t.total_code_lines;
            const books = Math.max(1, Math.floor(codeLines / 30000));

            // 找出花费最多的工具（按实际花费，不是按 token 数）
            let topTool = {{ name: '无', cost: 0 }};
            rawData.by_tool.forEach(tool => {{
                const toolCost = tool.daily_data.reduce((s, d) => s + d.estimated_cost, 0);
                if (toolCost > topTool.cost) {{
                    topTool = {{ name: tool.tool_name, cost: toolCost }};
                }}
            }});

            const toolDisplayName = {{
                'claude_code': 'Claude Code',
                'deepseek_gui': 'DeepSeek GUI',
                'cursor': 'Cursor',
                'codex': 'Codex',
                'copilot_jb': 'Copilot JB',
                'trae_cn': 'Trae',
                'lingma': 'Lingma',
                'opencoder': 'OpenCode',
                'windsurf': 'Windsurf',
                'aider': 'Aider',
                'cline': 'Cline',
                'roo_code': 'Roo Code',
                'continue_dev': 'Continue',
                'github_copilot': 'GitHub Copilot',
                'amazon_q': 'Amazon Q'
            }};

            const topName = toolDisplayName[topTool.name] || topTool.name;

            if (totalTokens === 0) {{
                document.getElementById('dailyReportText').innerHTML =
                    currentRangeLabel + '没有检测到 AI 编程工具的使用记录，开始 Vibe Coding 吧！';
                return;
            }}

            const html = `${{currentRangeLabel}}你一共消耗了 <span class="highlight">${{formatTokens(totalTokens)}}</span> Token，` +
                `相当于写了 <span class="highlight">${{codeLines.toLocaleString()}}</span> 行代码，` +
                `这些代码相当于写了 <span class="highlight">${{books}}</span> 本书，` +
                `你最喜欢用的是 <span class="tool-highlight" style="color:${{getToolColor(topTool.name)}}">${{topName}}</span>，` +
                `花费约 <span class="cost-highlight">$${{t.total_estimated_cost.toFixed(2)}}</span>`;

            document.getElementById('dailyReportText').innerHTML = html;
        }}

        function getToolColor(name) {{ return toolColors[name] || '#94A3B8'; }}

        function renderBarChart() {{
            const chart = echarts.init(document.getElementById('barChart'));
            const tools = rawData.by_tool.map(t => t.tool_name);
            const costs = rawData.by_tool.map(t => t.daily_data.reduce((s, d) => s + d.estimated_cost, 0));
            const colors = rawData.by_tool.map(t => getToolColor(t.tool_name));

            chart.setOption({{
                tooltip: {{ trigger: 'axis', formatter: function(params) {{
                    let s = params[0].name + '<br/>';
                    params.forEach(p => s += p.marker + ' $' + p.value.toFixed(2));
                    return s;
                }}}},
                grid: {{ left: '3%', right: '4%', bottom: '15%', containLabel: true }},
                xAxis: {{
                    type: 'category',
                    data: tools,
                    axisLabel: {{ color: '#ccc', rotate: 35, fontSize: 11, interval: 0 }}
                }},
                yAxis: {{ type: 'value', axisLabel: {{ color: '#ccc', formatter: v => '$' + v.toFixed(2) }} }},
                series: [{{
                    type: 'bar',
                    data: costs.map((v, i) => ({{ value: v, itemStyle: {{ color: colors[i] }} }})),
                    barWidth: '50%',
                    label: {{ show: true, position: 'top', color: '#ccc', fontSize: 10, formatter: p => '$' + p.value.toFixed(2) }}
                }}]
            }});
            window.addEventListener('resize', () => chart.resize());
        }}

        function renderPieChart() {{
            const chart = echarts.init(document.getElementById('pieChart'));
            const pieData = rawData.by_tool
                .filter(t => t.daily_data.reduce((s, d) => s + d.estimated_cost, 0) > 0)
                .map(t => ({{
                    name: t.tool_name,
                    value: parseFloat(t.daily_data.reduce((s, d) => s + d.estimated_cost, 0).toFixed(4)),
                    itemStyle: {{ color: getToolColor(t.tool_name) }}
                }}));

            chart.setOption({{
                tooltip: {{ trigger: 'item', formatter: p => p.name + ': $' + p.value.toFixed(2) + ' (' + p.percent + '%)' }},
                series: [{{
                    type: 'pie', radius: ['35%', '70%'], avoidLabelOverlap: false,
                    itemStyle: {{ borderRadius: 10, borderColor: '#1a1a2e', borderWidth: 2 }},
                    label: {{ show: true, color: '#ccc', formatter: '{{b}}\n{{d}}%' }},
                    emphasis: {{ label: {{ show: true, fontSize: 16, fontWeight: 'bold' }} }},
                    data: pieData
                }}]
            }});
            window.addEventListener('resize', () => chart.resize());
        }}

        let trendChartInstance = null;
        let currentTrendRange = '1d';

        function setTrendRange(range) {{
            currentTrendRange = range;
            document.querySelectorAll('.trend-btn').forEach(b => b.classList.remove('active'));
            event.target.classList.add('active');
            loadTrendData(range);
        }}

        function loadTrendData(range) {{
            fetch('/api/trend?range=' + range)
                .then(r => r.json())
                .then(data => renderTrendChart(data))
                .catch(() => {{}});
        }}

        function renderTrendChart(data) {{
            if (!data) return;
            if (!trendChartInstance) {{
                trendChartInstance = echarts.init(document.getElementById('trendChart'));
                window.addEventListener('resize', () => trendChartInstance.resize());
            }}

            const buckets = data.buckets;
            const isHourly = data.granularity === 'hour';

            // 格式化 bucket 标签
            const labels = buckets.map(b => {{
                if (isHourly) {{
                    // "2026-06-09T13:00" -> "06/09 13:00"
                    const parts = b.split('T');
                    const d = parts[0].substring(5).replace('-', '/');
                    const t = parts[1] ? parts[1].substring(0, 5) : '';
                    return d + ' ' + t;
                }}
                // "2026-06-09" -> "06/09"
                return b.substring(5).replace('-', '/');
            }});

            const series = data.series.map(s => ({{
                name: s.tool_name, type: 'line', smooth: true, symbol: 'circle', symbolSize: 5,
                lineStyle: {{ color: getToolColor(s.tool_name), width: 2 }},
                itemStyle: {{ color: getToolColor(s.tool_name) }},
                areaStyle: {{ color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
                    {{ offset: 0, color: getToolColor(s.tool_name) + '30' }},
                    {{ offset: 1, color: getToolColor(s.tool_name) + '05' }}
                ])}},
                data: buckets.map(b => {{
                    const found = s.points.find(p => p.bucket === b);
                    return found ? parseFloat(found.estimated_cost.toFixed(4)) : 0;
                }})
            }}));

            trendChartInstance.setOption({{
                tooltip: {{ trigger: 'axis', formatter: function(params) {{
                    let s = params[0].axisValue + '<br/>';
                    params.forEach(p => {{
                        if (p.value > 0) s += p.marker + ' ' + p.seriesName + ': $' + p.value.toFixed(2) + '<br/>';
                    }});
                    return s;
                }}}},
                legend: {{ textStyle: {{ color: '#ccc' }}, top: 0 }},
                grid: {{ left: '3%', right: '4%', bottom: '3%', top: 40, containLabel: true }},
                xAxis: {{
                    type: 'category', data: labels, boundaryGap: false,
                    axisLabel: {{ color: '#ccc', rotate: isHourly ? 30 : 0, fontSize: 11 }}
                }},
                yAxis: {{ type: 'value', axisLabel: {{ color: '#ccc', formatter: v => '$' + v.toFixed(2) }} }},
                series: series
            }}, true);
        }}

        function renderFunMetrics() {{
            const t = rawData.totals;
            const totalTokens = t.total_input_tokens + t.total_output_tokens + t.total_cache_read_tokens;
            const codeLines = t.total_code_lines;

            // 平均值：昨日按每小时，上周/上月按每天，迄今按每天
            const avgLabel = currentDataRange === 'yesterday' ? '平均每小时消耗' : '平均每天消耗';
            const avgDivisor = currentDataRange === 'yesterday' ? 24
                : (currentDataRange === 'last_week' ? 7
                    : (currentDataRange === 'last_month' ? 30
                        : Math.max(1, rawData.dates ? rawData.dates.length : 1)));
            const avgTokens = Math.floor(totalTokens / avgDivisor);

            // 代码连起来围绕地球：假设每行代码显示宽度 15cm
            // 地球周长 = 40,075 km = 40,075,000 m = 267,166,667 行（按 0.15m/行）
            const earthLines = 267_166_667;
            const earthCircles = codeLines / earthLines;
            const earthDisplay = earthCircles >= 10.0
                ? earthCircles.toFixed(1) + ' 圈'
                : (earthCircles >= 1.0
                    ? earthCircles.toFixed(2) + ' 圈'
                    : earthCircles.toFixed(4) + ' 圈');

            // 英语单词：1 token ≈ 0.75 英语单词
            const words = Math.floor(totalTokens * 0.75);
            const wordsDisplay = words >= 1_000_000
                ? (words / 1_000_000).toFixed(2) + 'M'
                : words.toLocaleString();

            const metrics = [
                {{ label: '总 Token 消耗', value: formatTokens(totalTokens), desc: '输入 + 输出 + 缓存命中' }},
                {{ label: avgLabel, value: formatTokens(avgTokens), desc: '=' + avgTokens.toLocaleString() + ' Token' }},
                {{ label: '代码能绕地球', value: earthDisplay, desc: '按每行 15cm 显示宽度' }},
                {{ label: '英语单词当量', value: wordsDisplay + ' 词', desc: '≈ ' + (totalTokens * 0.75 / 1_000_000).toFixed(2) + 'M 词' }},
            ];
            document.getElementById('funMetrics').innerHTML = metrics.map(m => `
                <div class="fun-item">
                    <div class="fun-label">${{m.label}}</div>
                    <div class="fun-value">${{m.value}}</div>
                    <div style="color:#666;font-size:0.78em;margin-top:2px">${{m.desc}}</div>
                </div>
            `).join('');
        }}

        function loadBuiltinTools() {{
            fetch('/api/builtin-tools').then(r => r.json()).then(tools => {{
                document.getElementById('toolsGrid').innerHTML = tools.map(t => `
                    <div class="tool-card ${{t.enabled ? 'enabled' : 'disabled'}}">
                        <div class="tool-dot" style="background:${{getToolColor(t.id)}};color:${{getToolColor(t.id)}}"></div>
                        <div>
                            <div class="tool-name">${{t.display_name}}</div>
                            <div class="tool-desc">${{t.description}}</div>
                        </div>
                        <span class="tool-status ${{t.enabled ? 'on' : 'off'}}">${{t.enabled ? '已启用' : '未启用'}}</span>
                    </div>
                `).join('');
            }}).catch(() => {{}});
        }}

        function loadAvailableDates() {{
            fetch('/api/dates').then(r => r.json()).then(dates => {{
                if (dates.length > 0) {{
                    document.getElementById('dateInput').value = dates[0];
                }}
            }}).catch(() => {{}});
        }}

        // ===== 缓存统计 =====
        let currentCacheStart = '';
        let currentCacheEnd = '';

        function loadCacheStats() {{
            const today = new Date();
            const y = new Date(today); y.setDate(y.getDate() - 1);
            const start = y.toISOString().split('T')[0];
            const end = start;
            currentCacheStart = start;
            currentCacheEnd = end;
            fetchCacheStats(start, end);
        }}

        function fetchCacheStats(start, end) {{
            currentCacheStart = start;
            currentCacheEnd = end;
            fetch('/api/cache-stats?start=' + start + '&end=' + end)
                .then(r => r.json())
                .then(data => renderCacheCharts(data))
                .catch(() => {{}});
        }}

        function renderCacheCharts(data) {{
            if (!data || !data.by_tool) return;

            // === Token 分布堆叠柱状图 ===
            const distChart = echarts.init(document.getElementById('tokenDistChart'));
            const tools = data.by_tool.map(t => t.tool_name);
            const toolDisplayNames = {{
                'claude_code': 'Claude Code', 'deepseek_gui': 'DeepSeek GUI',
                'cursor': 'Cursor', 'trae_cn': 'Trae', 'codex': 'Codex',
                'copilot_jb': 'Copilot JB', 'lingma': 'Lingma'
            }};

            distChart.setOption({{
                tooltip: {{ trigger: 'axis', axisPointer: {{ type: 'shadow' }},
                    formatter: function(params) {{
                        let s = params[0].axisValue + '<br/>';
                        let total = 0;
                        params.forEach(p => {{ total += p.value; s += p.marker + ' ' + p.seriesName + ': ' + formatTokens(p.value) + '<br/>'; }});
                        s += '<b>合计: ' + formatTokens(total) + '</b>';
                        return s;
                    }}
                }},
                legend: {{ textStyle: {{ color: '#ccc' }}, top: 0 }},
                grid: {{ left: '3%', right: '4%', bottom: '15%', containLabel: true }},
                xAxis: {{
                    type: 'category',
                    data: tools.map(t => toolDisplayNames[t] || t),
                    axisLabel: {{ color: '#ccc', rotate: 30, fontSize: 11, interval: 0 }}
                }},
                yAxis: {{ type: 'value', axisLabel: {{ color: '#ccc', formatter: v => formatTokens(v) }} }},
                series: [
                    {{
                        name: '输入 Tokens', type: 'bar', stack: 'total',
                        data: data.by_tool.map(t => t.input_tokens),
                        itemStyle: {{ color: '#00E5FF' }},
                        emphasis: {{ focus: 'series' }}
                    }},
                    {{
                        name: '输出 Tokens', type: 'bar', stack: 'total',
                        data: data.by_tool.map(t => t.output_tokens),
                        itemStyle: {{ color: '#FF2E7D' }},
                        emphasis: {{ focus: 'series' }}
                    }},
                    {{
                        name: '缓存命中 Tokens', type: 'bar', stack: 'total',
                        data: data.by_tool.map(t => t.cache_read_tokens),
                        itemStyle: {{ color: '#7EE787' }},
                        emphasis: {{ focus: 'series' }}
                    }}
                ]
            }});
            window.addEventListener('resize', () => distChart.resize());

            // === 缓存命中率仪表盘 ===
            const rateChart = echarts.init(document.getElementById('cacheRateChart'));
            const overallRate = data.totals.overall_cache_hit_rate;

            // 各工具的缓存命中率
            const rateData = data.by_tool
                .filter(t => (t.input_tokens + t.cache_read_tokens) > 0)
                .map(t => ({{
                    name: toolDisplayNames[t.tool_name] || t.tool_name,
                    value: parseFloat(t.cache_hit_rate.toFixed(1)),
                    color: getToolColor(t.tool_name)
                }}));

            rateChart.setOption({{
                tooltip: {{ formatter: function(p) {{ return p.name + ': ' + p.value + '%' + '<br/><span style=\"font-size:10px;color:#888\">(仅统计 Claude Code / DeepSeek GUI)</span>'; }} }},
                series: [
                    {{
                        name: '总体缓存命中率',
                        type: 'gauge',
                        center: ['50%', '55%'],
                        radius: '75%',
                        startAngle: 200, endAngle: -20,
                        min: 0, max: 100,
                        splitNumber: 10,
                        axisLine: {{
                            lineStyle: {{
                                width: 18,
                                color: [[0.3, '#FF2E7D'], [0.7, '#FFB800'], [1, '#7EE787']]
                            }}
                        }},
                        pointer: {{
                            itemStyle: {{ color: 'auto' }},
                            width: 4, length: '60%'
                        }},
                        axisTick: {{ distance: -18, length: 6, lineStyle: {{ color: '#fff', width: 1 }} }},
                        splitLine: {{ distance: -18, length: 18, lineStyle: {{ color: '#fff', width: 2 }} }},
                        axisLabel: {{ color: '#999', distance: 25, fontSize: 11 }},
                        detail: {{
                            valueAnimation: true,
                            formatter: function(v) {{ return v + '%'; }},
                            color: '#00E5FF',
                            fontSize: 28, fontWeight: 700,
                            offsetCenter: [0, '70%']
                        }},
                        title: {{
                            offsetCenter: [0, '85%'],
                            fontSize: 12,
                            color: '#888',
                        }},
                        data: [{{ value: overallRate, name: '命中率 (Claude Code + DeepSeek GUI)' }}]
                    }}
                ]
            }});
            window.addEventListener('resize', () => rateChart.resize());
        }}

        let currentRangeLabel = '昨日';
        let currentDataRange = 'yesterday';  // yesterday / last_week / last_month / all_time

        function updateCardLabels(rangeLabel) {{
            currentRangeLabel = rangeLabel;
            const prefix = rangeLabel || '昨日';
            document.querySelector('.card.cost .label').textContent = prefix + '花费估算';
            document.querySelector('.card.lines .label').textContent = prefix + '代码行数当量';
            document.querySelector('.card.books .label').textContent = prefix + '写了多少本书';
            document.querySelector('.card.events .label').textContent = prefix + ' API 调用次数';
        }}

        function setRange(range) {{
            currentDataRange = range;
            document.querySelectorAll('.time-btn').forEach(b => b.classList.remove('active'));
            event.target.classList.add('active');

            const today = new Date();
            let start, end, label;

            if (range === 'yesterday') {{
                const y = new Date(today); y.setDate(y.getDate() - 1);
                start = end = y.toISOString().split('T')[0];
                label = '昨日 (' + start + ')';
                updateCardLabels('昨日');
            }} else if (range === 'last_week') {{
                const dow = today.getDay() || 7;
                const lastSunday = new Date(today); lastSunday.setDate(today.getDate() - dow);
                const lastMonday = new Date(lastSunday); lastMonday.setDate(lastSunday.getDate() - 6);
                start = lastMonday.toISOString().split('T')[0];
                end = lastSunday.toISOString().split('T')[0];
                label = '上周 (' + start + ' ~ ' + end + ')';
                updateCardLabels('上周');
            }} else if (range === 'all_time') {{
                // 从 /api/dates 获取最早日期
                start = '2020-01-01';
                end = today.toISOString().split('T')[0];
                label = '迄今 (全部数据)';
                updateCardLabels('累计');
                // 异步获取精确的最早日期
                fetch('/api/dates').then(r => r.json()).then(dates => {{
                    if (dates.length > 0) {{
                        const earliest = dates[dates.length - 1];
                        const latest = dates[0];
                        document.getElementById('dateLabel').textContent = '迄今 (' + earliest + ' ~ ' + latest + ')';
                        fetchData(earliest, latest);
                    }}
                }}).catch(() => {{}});
            }} else {{
                const firstDay = new Date(today.getFullYear(), today.getMonth() - 1, 1);
                const lastDay = new Date(today.getFullYear(), today.getMonth(), 0);
                start = firstDay.toISOString().split('T')[0];
                end = lastDay.toISOString().split('T')[0];
                label = '上月 (' + start + ' ~ ' + end + ')';
                updateCardLabels('上月');
            }}

            document.getElementById('dateLabel').textContent = label;
            document.getElementById('dateInput').value = end;

            if (range !== 'all_time') {{
                fetchData(start, end);
            }}
        }}

        function jumpToDate(date) {{
            if (!date) return;
            document.querySelectorAll('.time-btn').forEach(b => b.classList.remove('active'));
            document.getElementById('dateLabel').textContent = date + ' 数据';
            updateCardLabels(date.slice(5).replace('-', '/') + ' ');
            fetchData(date, date);
        }}

        function fetchData(start, end) {{
            fetch(`/api/stats/range?start=${{start}}&end=${{end}}`)
                .then(r => r.json())
                .then(data => {{
                    rawData.by_tool = data.by_tool;
                    rawData.dates = data.dates;
                    rawData.totals = data.totals;
                    updateSummary();
                    renderBarChart();
                    renderPieChart();
                    renderFunMetrics();
                    renderDailyReport();
                }});
            loadTrendData(currentTrendRange);
            fetchCacheStats(start, end);
        }}
    </script>

    <!-- GSAP ANIMATION: START -->
    <script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.2/gsap.min.js"></script>
    <script>
    (function() {{
        /* GSAP 动画 - 尊重 prefers-reduced-motion */
        if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;

        /* ---- 1. Hero Glow 呼吸动画 ---- */
        gsap.to('.hero-glow', {{
            scale: 1.15,
            opacity: 0.7,
            duration: 4,
            ease: 'sine.inOut',
            yoyo: true,
            repeat: -1
        }});

        /* ---- 2. 入场动画 - 仅 Header ---- */
        gsap.set('.header', {{ opacity: 0, y: -30 }});
        gsap.set('.summary-cards .card', {{ opacity: 0, y: 30 }});

        /* ---- 3. 入场 Timeline ---- */
        var entranceTl = gsap.timeline({{
            defaults: {{ ease: 'power3.out', duration: 0.7 }}
        }});

        entranceTl
            .to('.header', {{ opacity: 1, y: 0 }}, 0.1)
            .to('.summary-cards .card', {{
                opacity: 1, y: 0,
                stagger: 0.1,
                duration: 0.6
            }}, 0.45);

        /* ---- 4. 数字滚动动画 ---- */
        function animateCountUp(el, targetText) {{
            /* 解析目标数值 */
            var prefix = '';
            var suffix = '';
            var numStr = targetText.replace(/[^0-9.]/g, '');
            var targetNum = parseFloat(numStr);

            if (isNaN(targetNum)) return;

            /* 提取前缀和后缀 */
            var match = targetText.match(/^([^0-9]*)([0-9.,]+)([^0-9]*)$/);
            if (match) {{
                prefix = match[1];
                suffix = match[3];
            }}

            var obj = {{ val: 0 }};
            gsap.to(obj, {{
                val: targetNum,
                duration: 1.2,
                ease: 'power2.out',
                delay: 0.6,
                onUpdate: function() {{
                    var current = obj.val;
                    var formatted;
                    if (targetText.indexOf(',') !== -1) {{
                        /* 带千分位格式 */
                        var intPart = Math.floor(current).toLocaleString();
                        var decMatch = numStr.match(/\\.(\d+)/);
                        if (decMatch) {{
                            var decLen = decMatch[1].length;
                            formatted = intPart + '.' + current.toFixed(decLen).split('.')[1];
                        }} else {{
                            formatted = intPart;
                        }}
                    }} else if (numStr.indexOf('.') !== -1) {{
                        var decLen = numStr.split('.')[1].length;
                        formatted = current.toFixed(decLen);
                    }} else {{
                        formatted = Math.floor(current).toLocaleString();
                    }}
                    el.textContent = prefix + formatted + suffix;
                }}
            }});
        }}

        /* 拦截 updateSummary 以触发数字动画 */
        var _origUpdateSummary = window.updateSummary;
        if (typeof _origUpdateSummary === 'function') {{
            window.updateSummary = function() {{
                _origUpdateSummary();
                /* 在数据填充后启动数字滚动 */
                var costEl = document.getElementById('totalCost');
                var linesEl = document.getElementById('totalLines');
                var booksEl = document.getElementById('totalBooks');
                var eventsEl = document.getElementById('totalEvents');
                if (costEl) animateCountUp(costEl, costEl.textContent);
                if (linesEl) animateCountUp(linesEl, linesEl.textContent);
                if (booksEl) animateCountUp(booksEl, booksEl.textContent);
                if (eventsEl) animateCountUp(eventsEl, eventsEl.textContent);
            }};
        }}

        /* ---- 6. Card hover 微交互 ---- */
        document.querySelectorAll('.summary-cards .card').forEach(function(card) {{
            var valueEl = card.querySelector('.value');
            card.addEventListener('mouseenter', function() {{
                gsap.to(valueEl, {{ scale: 1.12, duration: 0.25, ease: 'power2.out' }});
            }});
            card.addEventListener('mouseleave', function() {{
                gsap.to(valueEl, {{ scale: 1, duration: 0.25, ease: 'power2.out' }});
            }});
        }});
    }})();
    </script>
    <!-- GSAP ANIMATION: END -->

</body>
</html>"##, stats_json = stats_json, color_map_js = color_map_js, yesterday_str = yesterday_str);

    Ok(html)
}
