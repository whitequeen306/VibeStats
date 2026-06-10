# VibeStats

> 让每一次 Token 燃烧都有迹可循

本地轻量级 AI 编程工具 Token 消耗统计与趣味看板。支持 Claude Code、Cursor、DeepSeek GUI、Trae 等主流 AI 编程工具，自动解析本地日志，按实际模型定价统计费用，生成赛博朋克风格的数据可视化 Dashboard。

![Dashboard Preview](https://via.placeholder.com/800x450/0B0F1A/00E5FF?text=VibeStats+Dashboard)

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)

## 🚀 快速开始

### 下载即用（推荐）

从 [Releases](https://github.com/whitequeen306/VibeStats/releases) 下载对应平台的可执行文件：

- **Windows**: `vibestats.exe`
- **macOS**: `vibestats`
- **Linux**: `vibestats`

**使用步骤**：
1. 双击运行 `vibestats.exe`
2. 浏览器自动打开 http://localhost:7890
3. 查看你的 AI 编程消耗统计

首次运行会自动生成配置文件，默认启用常见工具。关闭窗口后程序最小化到系统托盘，在后台继续运行。

### 从源码构建

需要 Rust 1.75+。

```bash
git clone https://github.com/whitequeen306/VibeStats.git
cd vibestats
cargo build --release
```

编译产物在 `target/release/vibestats`。

## ✨ 功能特性

### 📊 数据统计
- **多工具支持**：Claude Code、Cursor、DeepSeek GUI、Trae、Codex 等
- **精确计费**：按各工具实际使用的模型定价（Claude Sonnet、GLM-5.1、DeepSeek-V4 等）
- **T+1 定时任务**：每日凌晨自动统计前一天数据，支持补偿机制
- **增量解析**：记录文件指针，避免重复解析

### 🎯 数据看板
- **实时 Dashboard**：赛博朋克风格可视化界面
- **消耗趋势**：近一天/近一周/近一月折线图
- **工具对比**：各 Agent 花费对比柱状图 + 占比扇形图
- **Token 分布**：输入/输出/缓存命中堆叠柱状图
- **缓存命中率**：仪表盘显示总体命中率

### 🎮 趣味数据
- **代码行数当量**：按 15 Token/行估算
- **写了多少本书**：按 30,000 行/本书换算
- **代码绕地球**：按 15cm/行显示宽度，计算能绕地球几圈
- **英语单词当量**：1 Token ≈ 0.75 英语单词
- **平均消耗**：昨日按每小时，上周/上月按每天

### 🔔 系统通知
- 每日上午 8 点推送昨日消耗报告
- 跨平台支持（Windows Toast / macOS Notification Center / Linux notify-send）

### 🛠️ 系统托盘
- 关闭窗口最小化到托盘
- 托盘菜单：显示仪表盘 / 退出 VibeStats
- Token Ring 风格 Logo

## 📦 支持的工具

| 工具 | 类型 | Token 数据 | 费用计算 |
|------|------|-----------|---------|
| **Claude Code** | CLI | ✅ 完整 | ✅ 精确 |
| **DeepSeek GUI** | 桌面端 | ✅ 完整 | ✅ 精确（日志含 costUsd） |
| **Cursor** | AI IDE | ⚠️ 估算（请求数） | ✅ 按模型定价 |
| **Trae CN** | AI IDE | ⚠️ 估算（日志解析） | ✅ 按模型定价 |
| **Codex** | CLI | ❌ 仅事件 | ⚠️ 默认定价 |
| **Copilot (JetBrains)** | 插件 | ❌ 仅事件 | ⚠️ 默认定价 |
| **Windsurf** | AI IDE | ❌ 仅事件 | ⚠️ 默认定价 |
| **Cline / Roo Code** | VS Code 扩展 | ❌ 仅事件 | ⚠️ 默认定价 |
| **GitHub Copilot** | VS Code 扩展 | ❌ 仅事件 | ⚠️ 默认定价 |
| **Continue** | VS Code 扩展 | ❌ 仅事件 | ⚠️ 默认定价 |
| **Amazon Q** | 插件 | ❌ 仅事件 | ⚠️ 默认定价 |
| **OpenCoder** | CLI | ❌ 仅事件 | ⚠️ 默认定价 |
| **通义灵码** | 插件 | ❌ 仅事件 | ⚠️ 默认定价 |

> ✅ = 精确数据 | ⚠️ = 估算 | ❌ = 仅记录事件次数

## 📖 CLI 用法

```bash
# 默认：启动桌面窗口 + 后台调度器
./vibestats

# 仅启动 HTTP 服务器（无窗口，适合服务器）
./vibestats --serve

# 仅后台调度器（无窗口无 HTTP）
./vibestats --headless

# 立即执行一次统计
./vibestats --run-now

# 全量重建（清空数据库重新解析所有日志）
./vibestats --rebuild

# 仅解析日志并打印
./vibestats --parse-only
```

## ⚙️ 配置

配置文件位于：
- Windows: `%LOCALAPPDATA%\vibestats\config.toml`
- macOS: `~/Library/Application Support/vibestats/config.toml`
- Linux: `~/.local/share/vibestats/config.toml`

示例配置见 [config.example.toml](config.example.toml)。

```toml
# 启用的工具列表
enabled_tools = ["claude_code", "cursor", "deepseek_gui", "trae_cn"]

# 每日统计时间（24 小时制）
schedule_time = "00:30"

# HTTP 服务器端口
serve_port = 7890

# 自定义日志路径（可选，覆盖默认路径）
[custom_paths]
cursor = "D:/MyLogs/Cursor/logs"
claude_code = "D:/AI/Claude/logs"
```

## 🔒 数据隐私

**所有数据纯本地处理**：
- ✅ 日志文件从本地磁盘读取，不上传
- ✅ SQLite 数据库存放在本地数据目录
- ✅ 不连接任何远程 API
- ✅ 不开启任何网络请求

## 🏗️ 技术栈

- **后端**: Rust + actix-web + rusqlite
- **桌面窗口**: wry + tao（原生 WebView，不是浏览器）
- **前端**: ECharts + 原生 HTML/CSS
- **调度**: 自定义 T+1 定时任务 + 补偿机制
- **系统托盘**: tray-icon + Windows API

## 📝 更新日志

### v0.1.0 (2026-06-10)
- ✨ 初始版本发布
- 📊 支持 13 种主流 AI 编程工具
- 🎯 按实际模型定价计算费用
- 🎮 趣味数据换算（代码行数、绕地球、英语单词）
- 🔔 跨平台系统通知
- 🛠️ 系统托盘支持

## 🤝 贡献

欢迎提交 Issue 和 Pull Request！

## 📄 License

MIT License - 详见 [LICENSE](LICENSE) 文件
