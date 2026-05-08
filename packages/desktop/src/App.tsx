import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

const QQ_GROUP_NAME = "Codex Provider Sync 用户交流群";
const QQ_GROUP_NUMBER = "484630263";
const QQ_GROUP_JOIN_URL = "https://qm.qq.com/q/ZSq3H3Iu0q";
const QQ_GROUP_NUMBER_READY = QQ_GROUP_NUMBER.trim().length > 0;
const APP_VERSION = "2.2.3";
const UPSTREAM_PROJECT_URL = "https://github.com/Dailin521/codex-provider-sync";

type ProviderStat = { provider: string; count: number; source: string };
type ScanResult = {
  codex_home: string;
  exists: boolean;
  sessions_count: number;
  archived_sessions_count: number;
  state_db_exists: boolean;
  config_exists: boolean;
  global_state_exists: boolean;
  current_provider: string;
  current_provider_source: string;
  configured_providers: string[];
  reserved_provider_conflicts: string[];
  config_warnings: string[];
  provider_stats: ProviderStat[];
  encrypted_content_stats: ProviderStat[];
  locked_rollout_files: string[];
  user_event_thread_count: number;
  thread_cwd_count: number;
  sqlite_repair_stats?: {
    user_event_rows_needing_repair: number;
    cwd_rows_needing_repair: number;
  } | null;
  project_visibility: ProjectVisibility[];
};

type BackupInfo = { id: string; path: string; created_at: string };
type ClearToolDataResult = { removed_paths: string[] };
type ConfigRepairResult = {
  backup_path: string;
  renamed_providers: { from: string; to: string }[];
};
type WorkspaceSyncResult = {
  present: boolean;
  updated: boolean;
  updated_workspace_roots: number;
  saved_workspace_root_count: number;
};
type SyncResult = {
  backup_id?: string | null;
  target_provider: string;
  changed_rollout_files: number;
  changed_rollout_values: number;
  skipped_rollout_files: string[];
  changed_sqlite_rows: number;
  changed_sqlite_provider_rows: number;
  changed_sqlite_generic_rows: number;
  changed_sqlite_user_event_rows: number;
  changed_sqlite_cwd_rows: number;
  changed_config: boolean;
  workspace_roots: WorkspaceSyncResult;
  protected_encrypted_rollout_files: string[];
  protected_encrypted_thread_count: number;
  encrypted_content_warning?: string | null;
};
type ProjectVisibility = {
  root: string;
  interactive_threads: number;
  first_page_threads: number;
  exact_cwd_matches: number;
  verbatim_cwd_rows: number;
  top_rank?: number | null;
  rank_preview: string;
  provider_counts: Record<string, number>;
};

export default function App() {
  const [path, setPath] = useState("");
  const [autoBackup, setAutoBackup] = useState(true);
  const [showStartupNotice, setShowStartupNotice] = useState(true);
  const [copyMessage, setCopyMessage] = useState("");
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [deleteTarget, setDeleteTarget] = useState<BackupInfo | null>(null);
  const [showClearToolDataDialog, setShowClearToolDataDialog] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const hasProviderConflict = Boolean(scan?.reserved_provider_conflicts.length);
  const canSync = Boolean(scan && scan.config_exists && scan.current_provider && !hasProviderConflict);

  useEffect(() => {
    void refreshBackups();
  }, []);

  async function run<T>(fn: () => Promise<T>, options: { resetStatus?: boolean } = {}) {
    const resetStatus = options.resetStatus ?? true;
    setBusy(true);
    if (resetStatus) {
      setError("");
      setMessage("");
    }
    try {
      return await fn();
    } catch (e) {
      setError(String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }

  async function doScan(options: { silent?: boolean; preserveStatus?: boolean } = {}) {
    const data = await run(() =>
      invoke<ScanResult>("scan_codex_home", { codexHome: path.trim() || null })
    , { resetStatus: !options.preserveStatus });
    setScan(data);
    if (!options.silent) {
      setMessage("扫描完成");
    }
    await refreshBackups();
    return data;
  }

  async function doBackup() {
    const data = await run(() =>
      invoke<BackupInfo>("create_backup", { codexHome: path.trim() || null })
    );
    setMessage(`备份完成：${data.id}`);
    await refreshBackups();
  }

  async function doSync() {
    const data = await run(() =>
      invoke<SyncResult>("sync_provider", {
        codexHome: path.trim() || null,
        autoBackup
      })
    );
    setMessage(
      [
        `同步完成：目标 provider ${data.target_provider}`,
        `rollout ${data.changed_rollout_files} 个`,
        `SQLite ${data.changed_sqlite_rows} 行`,
        `provider 可见性 ${data.changed_sqlite_provider_rows} 行`,
        `workspace roots ${data.workspace_roots.updated ? "已更新" : "未变化"}`,
        data.protected_encrypted_thread_count > 0
          ? `保护 ${data.protected_encrypted_thread_count} 个 encrypted rollout 未硬改，但已尝试同步 SQLite 可见性`
          : "",
        "config.toml 未修改",
        data.skipped_rollout_files.length > 0 ? `跳过 ${data.skipped_rollout_files.length} 个占用/变化文件` : ""
      ].filter(Boolean).join("，")
    );
    if (data.encrypted_content_warning) {
      setError(data.encrypted_content_warning);
    }
    await doScan({ silent: true, preserveStatus: true });
  }

  async function doRepairProviderConflicts() {
    const data = await run(() =>
      invoke<ConfigRepairResult>("repair_reserved_provider_conflicts", {
        codexHome: path.trim() || null
      })
    );
    const renamed = data.renamed_providers.map((item) => `${item.from} -> ${item.to}`).join("，");
    setMessage(`已修复 provider 命名冲突：${renamed}。原 config.toml 已备份到 ${data.backup_path}`);
    await doScan({ silent: true, preserveStatus: true });
  }

  async function refreshBackups() {
    const data = await invoke<BackupInfo[]>("list_backups", {
      codexHome: path.trim() || null
    });
    setBackups(data);
  }

  async function doRestore(id: string) {
    await run(() =>
      invoke("restore_backup", { codexHome: path.trim() || null, backupId: id })
    );
    setMessage(`恢复完成：${id}`);
    await doScan({ silent: true, preserveStatus: true });
  }

  async function doDeleteBackup() {
    if (!deleteTarget) {
      return;
    }
    const id = deleteTarget.id;
    await run(() =>
      invoke("delete_backup", { codexHome: path.trim() || null, backupId: id })
    );
    setDeleteTarget(null);
    setMessage(`备份已删除：${id}`);
    await refreshBackups();
  }

  async function doClearToolData() {
    const data = await run(() =>
      invoke<ClearToolDataResult>("clear_tool_data", { codexHome: path.trim() || null })
    );
    setShowClearToolDataDialog(false);
    setBackups([]);
    setMessage(
      data.removed_paths.length > 0
        ? `本工具数据已清理：${data.removed_paths.length} 个目录`
        : "没有发现需要清理的本工具数据"
    );
  }

  async function copyQQGroupNumber() {
    if (!QQ_GROUP_NUMBER_READY) {
      setCopyMessage("请先在源码中填写真实QQ群号");
      return;
    }
    await navigator.clipboard.writeText(QQ_GROUP_NUMBER);
    setCopyMessage("QQ群号已复制");
  }

  async function copyQQJoinUrl() {
    await navigator.clipboard.writeText(QQ_GROUP_JOIN_URL);
    setCopyMessage("加群链接已复制");
  }

  async function joinQQGroup() {
    await run(() => invoke("open_external_url", { url: QQ_GROUP_JOIN_URL }));
  }

  return (
    <div style={wrap}>
      {showStartupNotice && (
        <StartupNotice
          copyMessage={copyMessage}
          onCopyGroupNumber={copyQQGroupNumber}
          onCopyJoinUrl={copyQQJoinUrl}
          onJoinGroup={joinQQGroup}
          onClose={() => setShowStartupNotice(false)}
        />
      )}

      {deleteTarget && (
        <DeleteBackupDialog
          backup={deleteTarget}
          busy={busy}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={doDeleteBackup}
        />
      )}

      {showClearToolDataDialog && (
        <ClearToolDataDialog
          busy={busy}
          onCancel={() => setShowClearToolDataDialog(false)}
          onConfirm={doClearToolData}
        />
      )}

      <h1 style={{ marginBottom: 6 }}>Codex Provider Sync Full</h1>
      <p style={{ color: "#4b5563", marginTop: 0 }}>
        扫描、本地 provider 同步、自动备份、恢复，一套打通。
      </p>

      <Card title="路径与同步策略">
        <div style={{ display: "grid", gap: 12 }}>
          <input
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="留空自动用 ~/.codex，或手动输入完整路径"
            style={input}
          />
          <div style={strategyBox}>
            <div>
              <div style={{ color: "#6b7280", fontSize: 12 }}>同步目标</div>
              <div style={{ marginTop: 6, fontWeight: 700 }}>
                当前 provider：{scan?.current_provider || "请先扫描"}
              </div>
              {scan?.current_provider_source && (
                <div style={{ marginTop: 4, color: "#2563eb", fontSize: 13 }}>
                  来源：{scan.current_provider_source}
                </div>
              )}
              <div style={{ marginTop: 6, color: "#4b5563", lineHeight: 1.6 }}>
                以当前 provider 为准，同步历史会话的 SQLite 线程可见性与 workspace roots；未加密 rollout 会同步 provider，含 encrypted_content 的 rollout 只同步可见性，不硬改会话本体。不会修改 API key、自定义 URL 或账号授权。
              </div>
            </div>
            <label style={{ whiteSpace: "nowrap" }}>
              <input type="checkbox" checked={autoBackup} onChange={(e) => setAutoBackup(e.target.checked)} /> 执行前自动备份
            </label>
          </div>
          <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
            <button style={btn} disabled={busy} onClick={() => doScan()}>扫描</button>
            <button style={btn} disabled={busy} onClick={doBackup}>立即备份</button>
            <button style={btnPrimary} disabled={busy || !canSync} onClick={doSync}>执行同步</button>
          </div>
          {hasProviderConflict && (
            <div style={dangerPanel}>
              <div style={{ fontWeight: 800 }}>发现 Codex 保留 provider 名冲突</div>
              <div style={{ marginTop: 6, lineHeight: 1.7 }}>
                当前 config.toml 里存在 {scan?.reserved_provider_conflicts.map((id) => `[model_providers.${id}]`).join("，")}。
                Codex 官方内置 provider 名不能被自定义 provider 覆盖，否则会出现“Invalid configuration: reserved built-in provider IDs”的报错。
              </div>
              <div style={{ marginTop: 12 }}>
                <button style={btnDanger} disabled={busy} onClick={doRepairProviderConflicts}>
                  备份并改名为 openai-custom
                </button>
              </div>
            </div>
          )}
        </div>
      </Card>

      {message && <Alert color="#166534" bg="#f0fdf4" border="#bbf7d0">{message}</Alert>}
      {error && <Alert color="#b91c1c" bg="#fef2f2" border="#fecaca">{error}</Alert>}

      {scan && (
        <Card title="扫描结果">
          <Grid>
            <KV k="Codex 目录" v={scan.codex_home} />
            <KV k="目录存在" v={String(scan.exists)} />
            <KV k="当前 provider" v={scan.current_provider} />
            <KV k="provider 来源" v={scan.current_provider_source || "-"} />
            <KV k="sessions 文件数" v={String(scan.sessions_count)} />
            <KV k="archived_sessions 文件数" v={String(scan.archived_sessions_count)} />
            <KV k="state_5.sqlite" v={String(scan.state_db_exists)} />
            <KV k="config.toml" v={String(scan.config_exists)} />
            <KV k="global state" v={String(scan.global_state_exists)} />
            <KV k="配置 provider" v={scan.configured_providers.join(", ") || "-"} />
            <KV k="rollout 用户消息线程" v={String(scan.user_event_thread_count)} />
            <KV k="rollout cwd 线程" v={String(scan.thread_cwd_count)} />
            <KV
              k="SQLite 待修复"
              v={
                scan.sqlite_repair_stats
                  ? `user_event ${scan.sqlite_repair_stats.user_event_rows_needing_repair} / cwd ${scan.sqlite_repair_stats.cwd_rows_needing_repair}`
                  : "-"
              }
            />
          </Grid>
          {scan.config_warnings.length > 0 && (
            <div style={{ marginTop: 18 }}>
              <Alert color="#92400e" bg="#fffbeb" border="#fde68a">
                <div style={{ fontWeight: 800, marginBottom: 6 }}>配置提醒</div>
                <List items={scan.config_warnings} />
              </Alert>
            </div>
          )}
          <h3 style={{ marginTop: 18 }}>Provider 分布</h3>
          <ProviderTable rows={scan.provider_stats} empty="未发现 provider 相关字段" />

          <h3 style={{ marginTop: 18 }}>Encrypted Content 风险</h3>
          <ProviderTable rows={scan.encrypted_content_stats} empty="未发现 encrypted_content" />

          {scan.locked_rollout_files.length > 0 && (
            <>
              <h3 style={{ marginTop: 18 }}>被占用的 rollout 文件</h3>
              <List items={scan.locked_rollout_files} />
            </>
          )}

          {scan.project_visibility.length > 0 && (
            <>
              <h3 style={{ marginTop: 18 }}>项目可见性诊断</h3>
              <table style={table}>
                <thead>
                  <tr>
                    <th style={th}>Project Root</th>
                    <th style={th}>Threads</th>
                    <th style={th}>First 50</th>
                    <th style={th}>Ranks</th>
                    <th style={th}>Providers</th>
                  </tr>
                </thead>
                <tbody>
                  {scan.project_visibility.map((row) => (
                    <tr key={row.root}>
                      <td style={td}>{row.root}</td>
                      <td style={td}>{row.interactive_threads}</td>
                      <td style={td}>{row.first_page_threads}/50</td>
                      <td style={td}>{row.rank_preview || "-"}</td>
                      <td style={td}>{formatCounts(row.provider_counts)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}
        </Card>
      )}

      <Card title="备份列表">
        <table style={table}>
          <thead>
            <tr>
              <th style={th}>ID</th>
              <th style={th}>备份日期</th>
              <th style={th}>路径</th>
              <th style={th}>操作</th>
            </tr>
          </thead>
          <tbody>
            {backups.length === 0 ? (
              <tr><td style={td} colSpan={4}>暂无备份</td></tr>
            ) : backups.map((b) => (
              <tr key={b.id}>
                <td style={td}>{b.id}</td>
                <td style={td}>{formatDateTime(b.created_at)}</td>
                <td style={td}>{b.path}</td>
                <td style={td}>
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                    <button style={btn} disabled={busy} onClick={() => doRestore(b.id)}>恢复</button>
                    <button style={btnDanger} disabled={busy} onClick={() => setDeleteTarget(b)}>删除</button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>

      <Card title="维护与卸载">
        <div style={maintenanceBox}>
          <div>
            <div style={{ fontWeight: 700 }}>清理本工具数据</div>
            <div style={{ marginTop: 6, color: "#4b5563", lineHeight: 1.6 }}>
              只删除本工具创建的 provider-sync 备份目录和旧备份目录，不会删除 Codex 的 config.toml、auth.json、sessions、archived_sessions 或 state_5.sqlite。
            </div>
          </div>
          <button style={btnDanger} disabled={busy} onClick={() => setShowClearToolDataDialog(true)}>清理本工具数据</button>
        </div>
      </Card>
    </div>
  );
}

function StartupNotice({
  copyMessage,
  onCopyGroupNumber,
  onCopyJoinUrl,
  onJoinGroup,
  onClose
}: {
  copyMessage: string;
  onCopyGroupNumber: () => void;
  onCopyJoinUrl: () => void;
  onJoinGroup: () => void;
  onClose: () => void;
}) {
  return (
    <div style={modalBackdrop} role="dialog" aria-modal="true" aria-labelledby="startup-notice-title">
      <div style={modalCard}>
        <div style={{ color: "#1d4ed8", fontSize: 13, fontWeight: 700, letterSpacing: 0.4 }}>启动说明</div>
        <h2 id="startup-notice-title" style={{ margin: "8px 0 10px" }}>Codex Provider Sync v{APP_VERSION}</h2>

        <div style={noticePanel}>
          <div style={noticeLabel}>版本信息</div>
          <div style={{ fontWeight: 700 }}>当前版本：v{APP_VERSION}</div>
          <div style={{ marginTop: 6, color: "#4b5563" }}>桌面版支持 macOS 与 Windows，用于修复 Codex 历史会话 provider 元数据。</div>
        </div>

        <div style={noticePanel}>
          <div style={noticeLabel}>说明</div>
          <div style={{ color: "#374151", lineHeight: 1.7 }}>
            如果你使用 CCS 切换账号/provider，请先在 CCS 中切好当前账号。本工具以当前 config.toml
            根级 model_provider 为同步目标，不修改 config.toml、自定义 URL、API key 或 auth 配置。
          </div>
        </div>

        <div style={noticePanel}>
          <div style={noticeLabel}>开源致谢</div>
          <div style={{ color: "#374151", lineHeight: 1.7 }}>
            本项目是在 GitHub 开源项目 Dailin521/codex-provider-sync 提供的思路基础上二次修改而来。
            感谢第一代作者的探索与开源分享：{UPSTREAM_PROJECT_URL}
          </div>
        </div>

        <div style={groupBox}>
          <div>
            <div style={{ color: "#1d4ed8", fontSize: 12, fontWeight: 700 }}>邀请你加入</div>
            <div style={{ marginTop: 6, fontWeight: 700 }}>{QQ_GROUP_NAME}</div>
            <div style={{ marginTop: 4, fontSize: 22, fontWeight: 800 }}>{QQ_GROUP_NUMBER}</div>
            <div style={{ marginTop: 6, color: "#4b5563" }}>反馈问题、获取更新、交流 CCS 与 Codex 配置经验。</div>
          </div>
          <div style={{ display: "flex", gap: 10, flexWrap: "wrap", justifyContent: "flex-end" }}>
            <button style={btn} disabled={!QQ_GROUP_NUMBER_READY} onClick={onCopyGroupNumber}>复制群号</button>
            <button style={btn} onClick={onCopyJoinUrl}>复制链接</button>
          </div>
        </div>
        {copyMessage && <div style={{ marginTop: 10, color: "#166534", fontWeight: 600 }}>{copyMessage}</div>}
        <div style={modalActions}>
          <button style={btn} onClick={onClose}>关闭</button>
          <button style={btnPrimary} onClick={onJoinGroup}>加入交流群</button>
        </div>
      </div>
    </div>
  );
}

function DeleteBackupDialog({
  backup,
  busy,
  onCancel,
  onConfirm
}: {
  backup: BackupInfo;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div style={modalBackdrop} role="dialog" aria-modal="true" aria-labelledby="delete-backup-title">
      <div style={smallModalCard}>
        <div style={{ color: "#b91c1c", fontSize: 13, fontWeight: 700, letterSpacing: 0.4 }}>删除确认</div>
        <h2 id="delete-backup-title" style={{ margin: "8px 0 10px" }}>确定删除这个备份吗？</h2>
        <div style={{ color: "#374151", lineHeight: 1.7 }}>
          删除后无法从这个备份恢复，请确认你不再需要它。
        </div>
        <div style={deleteInfoBox}>
          <div><strong>ID：</strong>{backup.id}</div>
          <div><strong>日期：</strong>{formatDateTime(backup.created_at)}</div>
          <div style={{ wordBreak: "break-all" }}><strong>路径：</strong>{backup.path}</div>
        </div>
        <div style={modalActions}>
          <button style={btn} disabled={busy} onClick={onCancel}>取消</button>
          <button style={btnDanger} disabled={busy} onClick={onConfirm}>确认删除</button>
        </div>
      </div>
    </div>
  );
}

function ClearToolDataDialog({
  busy,
  onCancel,
  onConfirm
}: {
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div style={modalBackdrop} role="dialog" aria-modal="true" aria-labelledby="clear-tool-data-title">
      <div style={smallModalCard}>
        <div style={{ color: "#b91c1c", fontSize: 13, fontWeight: 700, letterSpacing: 0.4 }}>清理确认</div>
        <h2 id="clear-tool-data-title" style={{ margin: "8px 0 10px" }}>清理本工具数据？</h2>
        <div style={{ color: "#374151", lineHeight: 1.7 }}>
          这会删除本工具创建的备份目录，包括 provider-sync 当前备份目录和旧版备份目录。不会删除 Codex 账号、key、配置或历史会话。
        </div>
        <div style={deleteInfoBox}>
          <div>将清理：~/.codex/backups_state/provider-sync</div>
          <div>将清理：~/.codex/.provider-sync-backups</div>
          <div>不会清理：config.toml、auth.json、sessions、archived_sessions、state_5.sqlite</div>
        </div>
        <div style={modalActions}>
          <button style={btn} disabled={busy} onClick={onCancel}>取消</button>
          <button style={btnDanger} disabled={busy} onClick={onConfirm}>确认清理</button>
        </div>
      </div>
    </div>
  );
}

function ProviderTable({ rows, empty }: { rows: ProviderStat[]; empty: string }) {
  return (
    <table style={table}>
      <thead>
        <tr>
          <th style={th}>Provider</th>
          <th style={th}>Count</th>
          <th style={th}>Source</th>
        </tr>
      </thead>
      <tbody>
        {rows.length === 0 ? (
          <tr><td style={td} colSpan={3}>{empty}</td></tr>
        ) : rows.map((row, i) => (
          <tr key={`${row.provider}-${row.source}-${i}`}>
            <td style={td}>{row.provider}</td>
            <td style={td}>{row.count}</td>
            <td style={td}>{row.source}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function List({ items }: { items: string[] }) {
  return (
    <ul style={{ margin: 0, paddingLeft: 18 }}>
      {items.map((item) => <li key={item} style={{ marginBottom: 6, wordBreak: "break-all" }}>{item}</li>)}
    </ul>
  );
}

function formatCounts(counts: Record<string, number>) {
  const entries = Object.entries(counts);
  return entries.length === 0 ? "-" : entries.map(([k, v]) => `${k}: ${v}`).join(", ");
}

function formatDateTime(value: string) {
  if (!value || value === "-") {
    return "-";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString();
}

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section style={{ border: "1px solid #e5e7eb", borderRadius: 14, padding: 18, background: "#fff", marginBottom: 16 }}>
      <h2 style={{ marginTop: 0 }}>{title}</h2>
      {children}
    </section>
  );
}

function Alert({ children, color, bg, border }: { children: React.ReactNode; color: string; bg: string; border: string }) {
  return <div style={{ marginBottom: 16, padding: 12, color, background: bg, border: `1px solid ${border}`, borderRadius: 10 }}>{children}</div>;
}

function KV({ k, v }: { k: string; v: string }) {
  return <div style={{ padding: 12, border: "1px solid #eef2f7", borderRadius: 10 }}><div style={{ color: "#6b7280", fontSize: 12 }}>{k}</div><div style={{ marginTop: 6, fontWeight: 600 }}>{v}</div></div>;
}

function Grid({ children }: { children: React.ReactNode }) {
  return <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 12 }}>{children}</div>;
}

const wrap: React.CSSProperties = { maxWidth: 1100, margin: "28px auto", padding: 20, fontFamily: 'Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif', color: "#111827", background: "#f8fafc" };
const input: React.CSSProperties = { width: "100%", padding: "10px 12px", border: "1px solid #d1d5db", borderRadius: 8, fontSize: 14, boxSizing: "border-box" };
const strategyBox: React.CSSProperties = { display: "flex", justifyContent: "space-between", alignItems: "center", gap: 16, padding: 14, border: "1px solid #dbeafe", borderRadius: 12, background: "#eff6ff" };
const dangerPanel: React.CSSProperties = { padding: 14, borderRadius: 12, border: "1px solid #fecaca", background: "#fef2f2", color: "#7f1d1d" };
const btn: React.CSSProperties = { padding: "10px 14px", borderRadius: 8, border: "1px solid #d1d5db", background: "#fff", cursor: "pointer" };
const btnPrimary: React.CSSProperties = { ...btn, background: "#111827", color: "#fff", border: "1px solid #111827" };
const btnDanger: React.CSSProperties = { ...btn, color: "#b91c1c", border: "1px solid #fecaca", background: "#fef2f2" };
const modalBackdrop: React.CSSProperties = { position: "fixed", inset: 0, zIndex: 50, display: "flex", alignItems: "center", justifyContent: "center", padding: 20, background: "rgba(15, 23, 42, 0.46)", backdropFilter: "blur(6px)" };
const modalCard: React.CSSProperties = { width: "min(560px, 100%)", borderRadius: 20, padding: 24, background: "#ffffff", boxShadow: "0 24px 80px rgba(15, 23, 42, 0.28)", border: "1px solid rgba(191, 219, 254, 0.9)" };
const smallModalCard: React.CSSProperties = { width: "min(500px, 100%)", borderRadius: 20, padding: 24, background: "#ffffff", boxShadow: "0 24px 80px rgba(15, 23, 42, 0.28)", border: "1px solid #fecaca" };
const groupBox: React.CSSProperties = { display: "flex", justifyContent: "space-between", alignItems: "center", gap: 16, marginTop: 16, padding: 16, borderRadius: 16, background: "#eff6ff", border: "1px solid #bfdbfe" };
const maintenanceBox: React.CSSProperties = { display: "flex", justifyContent: "space-between", alignItems: "center", gap: 16, padding: 14, borderRadius: 14, background: "#f9fafb", border: "1px solid #eef2f7" };
const noticePanel: React.CSSProperties = { marginTop: 14, padding: 14, borderRadius: 14, background: "#f9fafb", border: "1px solid #eef2f7" };
const deleteInfoBox: React.CSSProperties = { display: "grid", gap: 8, marginTop: 14, padding: 14, borderRadius: 14, background: "#fef2f2", border: "1px solid #fecaca", color: "#374151" };
const noticeLabel: React.CSSProperties = { marginBottom: 6, color: "#6b7280", fontSize: 12, fontWeight: 700 };
const modalActions: React.CSSProperties = { display: "flex", justifyContent: "flex-end", gap: 10, marginTop: 22 };
const table: React.CSSProperties = { width: "100%", borderCollapse: "collapse", fontSize: 14 };
const th: React.CSSProperties = { textAlign: "left", padding: 10, borderBottom: "1px solid #e5e7eb", background: "#f9fafb" };
const td: React.CSSProperties = { padding: 10, borderBottom: "1px solid #f3f4f6", verticalAlign: "top" };
