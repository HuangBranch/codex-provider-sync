import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
  configured_providers: string[];
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

type BackupInfo = { id: string; path: string };
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
  const [target, setTarget] = useState("openai");
  const [updateConfig, setUpdateConfig] = useState(true);
  const [autoBackup, setAutoBackup] = useState(true);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    void refreshBackups();
  }, []);

  async function run<T>(fn: () => Promise<T>) {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      return await fn();
    } catch (e) {
      setError(String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }

  async function doScan() {
    const data = await run(() =>
      invoke<ScanResult>("scan_codex_home", { codexHome: path.trim() || null })
    );
    setScan(data);
    setMessage("扫描完成");
    await refreshBackups();
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
        targetProvider: target,
        updateConfig,
        autoBackup
      })
    );
    setMessage(
      [
        `同步完成：rollout ${data.changed_rollout_files} 个`,
        `SQLite ${data.changed_sqlite_rows} 行`,
        `workspace roots ${data.workspace_roots.updated ? "已更新" : "未变化"}`,
        `config ${data.changed_config ? "已修改" : "未修改"}`,
        data.skipped_rollout_files.length > 0 ? `跳过 ${data.skipped_rollout_files.length} 个占用/变化文件` : ""
      ].filter(Boolean).join("，")
    );
    if (data.encrypted_content_warning) {
      setError(data.encrypted_content_warning);
    }
    await doScan();
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
    await doScan();
  }

  return (
    <div style={wrap}>
      <h1 style={{ marginBottom: 6 }}>Codex Provider Sync Full</h1>
      <p style={{ color: "#4b5563", marginTop: 0 }}>
        扫描、本地 provider 同步、自动备份、恢复，一套打通。
      </p>

      <Card title="路径与目标">
        <div style={{ display: "grid", gap: 12 }}>
          <input
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="留空自动用 ~/.codex，或手动输入完整路径"
            style={input}
          />
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
            <input
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              placeholder="目标 provider，如 openai / relay"
              style={input}
            />
            <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
              <label><input type="checkbox" checked={updateConfig} onChange={(e) => setUpdateConfig(e.target.checked)} /> 同步改 config.toml</label>
              <label><input type="checkbox" checked={autoBackup} onChange={(e) => setAutoBackup(e.target.checked)} /> 执行前自动备份</label>
            </div>
          </div>
          <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
            <button style={btn} disabled={busy} onClick={doScan}>扫描</button>
            <button style={btn} disabled={busy} onClick={doBackup}>立即备份</button>
            <button style={btnPrimary} disabled={busy} onClick={doSync}>执行同步</button>
          </div>
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
              <th style={th}>路径</th>
              <th style={th}>操作</th>
            </tr>
          </thead>
          <tbody>
            {backups.length === 0 ? (
              <tr><td style={td} colSpan={3}>暂无备份</td></tr>
            ) : backups.map((b) => (
              <tr key={b.id}>
                <td style={td}>{b.id}</td>
                <td style={td}>{b.path}</td>
                <td style={td}>
                  <button style={btn} disabled={busy} onClick={() => doRestore(b.id)}>恢复</button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
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
const btn: React.CSSProperties = { padding: "10px 14px", borderRadius: 8, border: "1px solid #d1d5db", background: "#fff", cursor: "pointer" };
const btnPrimary: React.CSSProperties = { ...btn, background: "#111827", color: "#fff", border: "1px solid #111827" };
const table: React.CSSProperties = { width: "100%", borderCollapse: "collapse", fontSize: 14 };
const th: React.CSSProperties = { textAlign: "left", padding: 10, borderBottom: "1px solid #e5e7eb", background: "#f9fafb" };
const td: React.CSSProperties = { padding: 10, borderBottom: "1px solid #f3f4f6", verticalAlign: "top" };
