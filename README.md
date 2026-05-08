# Codex Provider Sync Full

一个可直接二改的完整模板：React + Vite + Tauri 桌面应用，支持 macOS 与 Windows 打包。

## 当前功能

- 扫描 `.codex` 目录
- 统计 `sessions` / `archived_sessions` / `state_5.sqlite` / `config.toml` / `.codex-global-state.json`
- 扫描 `rollout-*.jsonl` 首行 `session_meta.payload.model_provider`
- 统计含 `encrypted_content` 的历史会话，并提示跨 provider/account 风险
- 诊断 rollout 中用户消息线程与 cwd 线程
- 诊断 Codex Desktop 项目侧可见性，包括最近 50 条首屏命中和 rank
- 启动时显示版本说明、使用说明与 QQ 群引导弹窗
- 定向同步 SQLite `threads.model_provider`
- 修复 SQLite `threads.has_user_event` 与 `threads.cwd`
- 泛化扫描并同步 SQLite 中其它名为 `provider` 或 `model_provider` 的列
- 修复 `.codex-global-state.json` 中 workspace roots 相关路径
- 以当前 `config.toml` 根级 `model_provider` 为同步目标，不修改 `config.toml`
- 自动备份
- 列出备份
- 从备份恢复

## 本地开发

```bash
pnpm install
pnpm dev
```

## 本地打包

```bash
pnpm bundle:mac
pnpm bundle:win
```

## GitHub Actions

1. 推送到 GitHub。
2. 打开 Actions。
3. 运行 `build-desktop`。
4. 下载 artifacts。

## 注意

- 如果使用 CCS 切换账号/provider，请先在 CCS 中切到目标账号，再打开本工具扫描和同步。
- 本工具不会修改 `config.toml`，也不会覆盖自定义 URL、API key 或 auth 配置；它只把历史会话、SQLite provider 元数据和 workspace roots 同步到当前配置。
- 当前 QQ 群弹窗用于引导加入交流群，不强制校验进群状态；群号 `484630263`，加群链接为 `https://qm.qq.com/q/ZSq3H3Iu0q`。
- 如果跳转链接打不开，可在弹窗中复制 QQ 群号后手动搜索加入。
- 备份目录为 `~/.codex/backups_state/provider-sync/<timestamp>`，并兼容读取旧的 `.provider-sync-backups`。
- 备份包含 `sessions`、`archived_sessions`、`state_5.sqlite`、`state_5.sqlite-wal`、`state_5.sqlite-shm`、`config.toml`、`.codex-global-state.json`。
- 含 `encrypted_content` 的历史会话跨 provider/account 后，通常只能恢复列表可见性，继续对话或 compact 仍可能失败。
- 建议执行同步或恢复前关闭 Codex / Codex App / app-server，避免 SQLite 或 rollout 文件被占用。
- 当前实现已通过前端 `pnpm -C packages/desktop build`；后端 Tauri/Rust 构建需要本机安装 Rust 工具链后再运行验证。
