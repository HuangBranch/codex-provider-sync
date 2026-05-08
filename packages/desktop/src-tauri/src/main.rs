#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::Local;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;
use walkdir::WalkDir;

const DB_FILE: &str = "state_5.sqlite";
const GLOBAL_STATE_FILE: &str = ".codex-global-state.json";
const GLOBAL_STATE_BACKUP_FILE: &str = ".codex-global-state.json.bak";
const SESSION_DIRS: [&str; 2] = ["sessions", "archived_sessions"];

#[derive(Serialize, Clone)]
struct ProviderStat {
    provider: String,
    count: u64,
    source: String,
}

#[derive(Default, Clone)]
struct ProviderCounts {
    sessions: BTreeMap<String, u64>,
    archived_sessions: BTreeMap<String, u64>,
}

#[derive(Default)]
struct RolloutCollection {
    changes: Vec<RolloutChange>,
    provider_counts: ProviderCounts,
    encrypted_content_counts: ProviderCounts,
    locked_paths: Vec<String>,
    user_event_thread_ids: HashSet<String>,
    thread_cwd_by_id: HashMap<String, String>,
}

struct RolloutChange {
    path: PathBuf,
    original_first_line: String,
    original_separator: String,
    updated_first_line: String,
}

struct FirstLineRecord {
    first_line: String,
    separator: String,
    rest_start: usize,
}

#[derive(Serialize)]
struct ScanResult {
    codex_home: String,
    exists: bool,
    sessions_count: u64,
    archived_sessions_count: u64,
    state_db_exists: bool,
    config_exists: bool,
    global_state_exists: bool,
    current_provider: String,
    configured_providers: Vec<String>,
    provider_stats: Vec<ProviderStat>,
    encrypted_content_stats: Vec<ProviderStat>,
    locked_rollout_files: Vec<String>,
    user_event_thread_count: u64,
    thread_cwd_count: u64,
    sqlite_repair_stats: Option<SqliteRepairStats>,
    project_visibility: Vec<ProjectVisibility>,
}

#[derive(Serialize)]
struct BackupInfo {
    id: String,
    path: String,
}

#[derive(Serialize)]
struct SyncResult {
    backup_id: Option<String>,
    target_provider: String,
    changed_rollout_files: u64,
    changed_rollout_values: u64,
    skipped_rollout_files: Vec<String>,
    changed_sqlite_rows: u64,
    changed_sqlite_provider_rows: u64,
    changed_sqlite_generic_rows: u64,
    changed_sqlite_user_event_rows: u64,
    changed_sqlite_cwd_rows: u64,
    changed_config: bool,
    workspace_roots: WorkspaceSyncResult,
    encrypted_content_warning: Option<String>,
}

#[derive(Default, Serialize)]
struct SqliteRepairStats {
    user_event_rows_needing_repair: u64,
    cwd_rows_needing_repair: u64,
}

#[derive(Default)]
struct SqliteSyncStats {
    provider_rows: u64,
    generic_provider_rows: u64,
    user_event_rows: u64,
    cwd_rows: u64,
}

impl SqliteSyncStats {
    fn total(&self) -> u64 {
        self.provider_rows + self.generic_provider_rows + self.user_event_rows + self.cwd_rows
    }
}

#[derive(Default, Serialize)]
struct WorkspaceSyncResult {
    present: bool,
    updated: bool,
    updated_workspace_roots: u64,
    saved_workspace_root_count: u64,
}

#[derive(Serialize)]
struct ProjectVisibility {
    root: String,
    interactive_threads: u64,
    first_page_threads: u64,
    exact_cwd_matches: u64,
    verbatim_cwd_rows: u64,
    top_rank: Option<u64>,
    rank_preview: String,
    provider_counts: BTreeMap<String, u64>,
}

#[derive(Clone)]
struct CwdStat {
    cwd: String,
    normalized: String,
    count: u64,
    updated_at_ms: i64,
}

#[derive(Clone)]
struct RankedThreadRow {
    cwd: String,
    desktop_cwd: String,
    normalized_cwd: String,
    provider: String,
    rank: u64,
}

#[tauri::command]
fn scan_codex_home(codex_home: Option<String>) -> Result<ScanResult, String> {
    let home = resolve_codex_home(codex_home)?;
    let sessions_dir = home.join("sessions");
    let archived_dir = home.join("archived_sessions");
    let state_db = home.join(DB_FILE);
    let config_toml = home.join("config.toml");
    let global_state = home.join(GLOBAL_STATE_FILE);

    let rollout = collect_rollout_metadata(&home, None)?;
    let (current_provider, configured_providers) = read_config_info(&config_toml);
    let mut provider_stats = provider_counts_to_stats(&rollout.provider_counts);
    provider_stats.extend(scan_sqlite_provider_stats(&state_db)?);
    let encrypted_content_stats =
        encrypted_counts_to_stats(&rollout.encrypted_content_counts);
    let sqlite_repair_stats =
        read_sqlite_repair_stats(&state_db, &rollout.user_event_thread_ids, &rollout.thread_cwd_by_id)?;
    let project_visibility = read_project_visibility(&home)?;

    Ok(ScanResult {
        codex_home: home.display().to_string(),
        exists: home.exists(),
        sessions_count: count_files_recursive(&sessions_dir),
        archived_sessions_count: count_files_recursive(&archived_dir),
        state_db_exists: state_db.exists(),
        config_exists: config_toml.exists(),
        global_state_exists: global_state.exists(),
        current_provider,
        configured_providers,
        provider_stats,
        encrypted_content_stats,
        locked_rollout_files: rollout.locked_paths,
        user_event_thread_count: rollout.user_event_thread_ids.len() as u64,
        thread_cwd_count: rollout.thread_cwd_by_id.len() as u64,
        sqlite_repair_stats,
        project_visibility,
    })
}

#[tauri::command]
fn create_backup(codex_home: Option<String>) -> Result<BackupInfo, String> {
    let home = resolve_codex_home(codex_home)?;
    create_backup_internal(&home)
}

#[tauri::command]
fn list_backups(codex_home: Option<String>) -> Result<Vec<BackupInfo>, String> {
    let home = resolve_codex_home(codex_home)?;
    let mut out = vec![];
    collect_backup_infos(&backups_root(&home), &mut out)?;
    collect_backup_infos(&legacy_backups_root(&home), &mut out)?;
    out.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(out)
}

#[tauri::command]
fn restore_backup(codex_home: Option<String>, backup_id: String) -> Result<(), String> {
    let home = resolve_codex_home(codex_home)?;
    let backup_dir = find_backup_dir(&home, &backup_id)
        .ok_or_else(|| format!("备份不存在: {}", backup_id))?;

    restore_path(&backup_dir.join("sessions"), &home.join("sessions"))?;
    restore_path(&backup_dir.join("archived_sessions"), &home.join("archived_sessions"))?;
    restore_db_file(&backup_dir, &home, DB_FILE)?;
    restore_db_file(&backup_dir, &home, &format!("{}-wal", DB_FILE))?;
    restore_db_file(&backup_dir, &home, &format!("{}-shm", DB_FILE))?;
    restore_path(&backup_dir.join("config.toml"), &home.join("config.toml"))?;
    restore_path(
        &backup_dir.join(GLOBAL_STATE_FILE),
        &home.join(GLOBAL_STATE_FILE),
    )?;
    restore_path(
        &backup_dir.join(GLOBAL_STATE_BACKUP_FILE),
        &home.join(GLOBAL_STATE_BACKUP_FILE),
    )?;
    Ok(())
}

#[tauri::command]
fn sync_provider(
    codex_home: Option<String>,
    auto_backup: bool,
) -> Result<SyncResult, String> {
    let home = resolve_codex_home(codex_home)?;
    let config_toml = home.join("config.toml");
    if !config_toml.exists() {
        return Err("找不到 config.toml，无法确定当前 provider。请先用 Codex/CCS 配置好账号后再同步。".to_string());
    }
    let (target_provider, _) = read_config_info(&config_toml);
    let target = target_provider.trim();
    if target.is_empty() {
        return Err("config.toml 中没有读取到根级 model_provider，无法确定同步目标。请先用 CCS 切换到目标账号/provider。".to_string());
    }

    let rollout = collect_rollout_metadata(&home, Some(target))?;
    let encrypted_content_warning =
        build_encrypted_content_warning(&rollout.encrypted_content_counts, target);

    let backup = if auto_backup {
        Some(create_backup_internal(&home)?.id)
    } else {
        None
    };

    let apply_result = apply_rollout_changes(&rollout.changes)?;
    let sqlite_stats = sync_sqlite_session_state(
        &home.join(DB_FILE),
        target,
        &rollout.user_event_thread_ids,
        &rollout.thread_cwd_by_id,
    )?;
    let workspace_roots = sync_workspace_roots(&home)?;

    Ok(SyncResult {
        backup_id: backup,
        target_provider: target.to_string(),
        changed_rollout_files: apply_result.applied,
        changed_rollout_values: apply_result.applied,
        skipped_rollout_files: apply_result.skipped_paths,
        changed_sqlite_rows: sqlite_stats.total(),
        changed_sqlite_provider_rows: sqlite_stats.provider_rows,
        changed_sqlite_generic_rows: sqlite_stats.generic_provider_rows,
        changed_sqlite_user_event_rows: sqlite_stats.user_event_rows,
        changed_sqlite_cwd_rows: sqlite_stats.cwd_rows,
        changed_config: false,
        workspace_roots,
        encrypted_content_warning,
    })
}

struct ApplyRolloutResult {
    applied: u64,
    skipped_paths: Vec<String>,
}

fn resolve_codex_home(input: Option<String>) -> Result<PathBuf, String> {
    match input {
        Some(v) if !v.trim().is_empty() => Ok(PathBuf::from(v)),
        _ => dirs::home_dir()
            .map(|p| p.join(".codex"))
            .ok_or("无法自动解析当前用户目录".to_string()),
    }
}

fn backups_root(home: &Path) -> PathBuf {
    home.join("backups_state").join("provider-sync")
}

fn legacy_backups_root(home: &Path) -> PathBuf {
    home.join(".provider-sync-backups")
}

fn collect_backup_infos(root: &Path, out: &mut Vec<BackupInfo>) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(to_err)? {
        let entry = entry.map_err(to_err)?;
        let path = entry.path();
        if path.is_dir() {
            out.push(BackupInfo {
                id: entry.file_name().to_string_lossy().to_string(),
                path: path.display().to_string(),
            });
        }
    }
    Ok(())
}

fn find_backup_dir(home: &Path, id: &str) -> Option<PathBuf> {
    let current = backups_root(home).join(id);
    if current.exists() {
        return Some(current);
    }
    let legacy = legacy_backups_root(home).join(id);
    if legacy.exists() {
        return Some(legacy);
    }
    None
}

fn create_backup_internal(home: &Path) -> Result<BackupInfo, String> {
    fs::create_dir_all(backups_root(home)).map_err(to_err)?;
    let id = Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    let dir = backups_root(home).join(&id);
    fs::create_dir_all(&dir).map_err(to_err)?;

    copy_if_exists(&home.join("sessions"), &dir.join("sessions"))?;
    copy_if_exists(&home.join("archived_sessions"), &dir.join("archived_sessions"))?;
    for file_name in [
        DB_FILE.to_string(),
        format!("{}-wal", DB_FILE),
        format!("{}-shm", DB_FILE),
    ] {
        copy_if_exists(&home.join(&file_name), &dir.join(&file_name))?;
    }
    copy_if_exists(&home.join("config.toml"), &dir.join("config.toml"))?;
    copy_if_exists(
        &home.join(GLOBAL_STATE_FILE),
        &dir.join(GLOBAL_STATE_FILE),
    )?;
    copy_if_exists(
        &home.join(GLOBAL_STATE_BACKUP_FILE),
        &dir.join(GLOBAL_STATE_BACKUP_FILE),
    )?;
    fs::write(
        dir.join("metadata.json"),
        format!(
            "{{\n  \"version\": 1,\n  \"namespace\": \"provider-sync\",\n  \"codexHome\": \"{}\",\n  \"createdAt\": \"{}\"\n}}\n",
            escape_json_string(&home.display().to_string()),
            Local::now().to_rfc3339()
        ),
    )
    .map_err(to_err)?;

    Ok(BackupInfo {
        id,
        path: dir.display().to_string(),
    })
}

fn restore_path(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    if src.is_dir() {
        if dst.exists() {
            fs::remove_dir_all(dst).map_err(to_err)?;
        }
        copy_dir_recursive(src, dst)?;
    } else {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(to_err)?;
        }
        fs::copy(src, dst).map_err(to_err)?;
    }
    Ok(())
}

fn restore_db_file(backup_dir: &Path, home: &Path, file_name: &str) -> Result<(), String> {
    let src = backup_dir.join(file_name);
    let dst = home.join(file_name);
    if src.exists() {
        restore_path(&src, &dst)
    } else {
        if dst.exists() {
            fs::remove_file(dst).map_err(to_err)?;
        }
        Ok(())
    }
}

fn copy_if_exists(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    if src.is_dir() {
        copy_dir_recursive(src, dst)
    } else {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(to_err)?;
        }
        fs::copy(src, dst).map_err(to_err)?;
        Ok(())
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(to_err)?;
    for entry in WalkDir::new(src) {
        let entry = entry.map_err(to_err)?;
        let path = entry.path();
        let rel = path.strip_prefix(src).map_err(to_err)?;
        let target = dst.join(rel);
        if path.is_dir() {
            fs::create_dir_all(&target).map_err(to_err)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(to_err)?;
            }
            fs::copy(path, &target).map_err(to_err)?;
        }
    }
    Ok(())
}

fn count_files_recursive(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .count() as u64
}

fn collect_rollout_metadata(
    home: &Path,
    target_provider: Option<&str>,
) -> Result<RolloutCollection, String> {
    let mut out = RolloutCollection::default();
    for dir_name in SESSION_DIRS {
        let root_dir = home.join(dir_name);
        if !root_dir.exists() {
            continue;
        }
        for entry in WalkDir::new(&root_dir) {
            let entry = entry.map_err(to_err)?;
            if !entry.path().is_file() || !is_rollout_file(entry.path()) {
                continue;
            }
            let content = match fs::read_to_string(entry.path()) {
                Ok(v) => v,
                Err(error) if is_file_busy_error(&error) => {
                    out.locked_paths.push(entry.path().display().to_string());
                    continue;
                }
                Err(error) => return Err(error.to_string()),
            };
            let record = split_first_line(&content);
            let mut parsed = match serde_json::from_str::<Value>(&record.first_line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !is_session_meta_record(&parsed) {
                continue;
            }

            let current_provider = parsed
                .get("payload")
                .and_then(|v| v.get("model_provider"))
                .and_then(|v| v.as_str())
                .unwrap_or("(missing)")
                .to_string();
            increment_provider_count(&mut out.provider_counts, dir_name, &current_provider);

            if let Some((thread_id, cwd)) = session_meta_id_and_cwd(&parsed) {
                out.thread_cwd_by_id
                    .insert(thread_id, to_desktop_workspace_path(&cwd));
            }
            if content.contains("encrypted_content") {
                increment_provider_count(
                    &mut out.encrypted_content_counts,
                    dir_name,
                    &current_provider,
                );
            }
            if let Some(thread_id) = session_meta_id(&parsed) {
                if file_has_user_event(&content) {
                    out.user_event_thread_ids.insert(thread_id);
                }
            }

            if let Some(target) = target_provider {
                if current_provider != target {
                    if let Some(payload) = parsed.get_mut("payload").and_then(|v| v.as_object_mut()) {
                        payload.insert(
                            "model_provider".to_string(),
                            Value::String(target.to_string()),
                        );
                        out.changes.push(RolloutChange {
                            path: entry.path().to_path_buf(),
                            original_first_line: record.first_line,
                            original_separator: record.separator,
                            updated_first_line: serde_json::to_string(&parsed).map_err(to_err)?,
                        });
                    }
                }
            }
        }
    }
    out.locked_paths.sort();
    out.locked_paths.dedup();
    Ok(out)
}

fn is_rollout_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
        return false;
    };
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

fn split_first_line(content: &str) -> FirstLineRecord {
    if let Some(newline_index) = content.find('\n') {
        let bytes = content.as_bytes();
        let crlf = newline_index > 0 && bytes[newline_index - 1] == b'\r';
        let line_end = if crlf { newline_index - 1 } else { newline_index };
        return FirstLineRecord {
            first_line: content[..line_end].to_string(),
            separator: if crlf { "\r\n" } else { "\n" }.to_string(),
            rest_start: newline_index + 1,
        };
    }
    FirstLineRecord {
        first_line: content.to_string(),
        separator: String::new(),
        rest_start: content.len(),
    }
}

fn is_session_meta_record(v: &Value) -> bool {
    v.get("type").and_then(|v| v.as_str()) == Some("session_meta")
        && v.get("payload").and_then(|v| v.as_object()).is_some()
}

fn session_meta_id(v: &Value) -> Option<String> {
    v.get("payload")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.to_string())
}

fn session_meta_id_and_cwd(v: &Value) -> Option<(String, String)> {
    let payload = v.get("payload")?;
    let id = payload.get("id")?.as_str()?.trim();
    let cwd = payload.get("cwd")?.as_str()?.trim();
    if id.is_empty() || cwd.is_empty() {
        return None;
    }
    Some((id.to_string(), cwd.to_string()))
}

fn file_has_user_event(content: &str) -> bool {
    for line in BufReader::new(content.as_bytes()).lines().flatten() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            if record_has_user_event(&value) {
                return true;
            }
        }
    }
    false
}

fn record_has_user_event(v: &Value) -> bool {
    if v.get("type").and_then(|v| v.as_str()) == Some("event_msg")
        && v.get("payload")
            .and_then(|v| v.get("type"))
            .and_then(|v| v.as_str())
            == Some("user_message")
    {
        return true;
    }
    for key in ["payload", "item", "msg"] {
        let Some(value) = v.get(key) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) == Some("message")
            && value.get("role").and_then(|v| v.as_str()) == Some("user")
        {
            return true;
        }
    }
    false
}

fn apply_rollout_changes(changes: &[RolloutChange]) -> Result<ApplyRolloutResult, String> {
    let mut applied = 0;
    let mut skipped_paths = vec![];
    for change in changes {
        let content = match fs::read_to_string(&change.path) {
            Ok(v) => v,
            Err(error) if is_file_busy_error(&error) => {
                skipped_paths.push(change.path.display().to_string());
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        let current = split_first_line(&content);
        if current.first_line != change.original_first_line {
            skipped_paths.push(change.path.display().to_string());
            continue;
        }
        let mut next_content = String::new();
        next_content.push_str(&change.updated_first_line);
        if !change.original_separator.is_empty() {
            next_content.push_str(&change.original_separator);
        }
        next_content.push_str(&content[current.rest_start..]);
        fs::write(&change.path, next_content).map_err(to_err)?;
        applied += 1;
    }
    skipped_paths.sort();
    skipped_paths.dedup();
    Ok(ApplyRolloutResult {
        applied,
        skipped_paths,
    })
}

fn increment_provider_count(counts: &mut ProviderCounts, scope: &str, provider: &str) {
    let bucket = if scope == "archived_sessions" {
        &mut counts.archived_sessions
    } else {
        &mut counts.sessions
    };
    *bucket.entry(provider.to_string()).or_insert(0) += 1;
}

fn provider_counts_to_stats(counts: &ProviderCounts) -> Vec<ProviderStat> {
    let mut out = vec![];
    for (provider, count) in &counts.sessions {
        out.push(ProviderStat {
            provider: provider.clone(),
            count: *count,
            source: "sessions".to_string(),
        });
    }
    for (provider, count) in &counts.archived_sessions {
        out.push(ProviderStat {
            provider: provider.clone(),
            count: *count,
            source: "archived_sessions".to_string(),
        });
    }
    out
}

fn encrypted_counts_to_stats(counts: &ProviderCounts) -> Vec<ProviderStat> {
    let mut out = vec![];
    for (provider, count) in &counts.sessions {
        out.push(ProviderStat {
            provider: provider.clone(),
            count: *count,
            source: "encrypted:sessions".to_string(),
        });
    }
    for (provider, count) in &counts.archived_sessions {
        out.push(ProviderStat {
            provider: provider.clone(),
            count: *count,
            source: "encrypted:archived_sessions".to_string(),
        });
    }
    out
}

fn build_encrypted_content_warning(counts: &ProviderCounts, target_provider: &str) -> Option<String> {
    let mut risky = HashSet::new();
    let mut total = 0u64;
    for (provider, count) in counts.sessions.iter().chain(counts.archived_sessions.iter()) {
        total += *count;
        if provider != target_provider && *count > 0 {
            risky.insert(provider.clone());
        }
    }
    if risky.is_empty() {
        return None;
    }
    let mut providers: Vec<_> = risky.into_iter().collect();
    providers.sort();
    Some(format!(
        "{} 个 rollout 文件含 encrypted_content，来自 provider: {}。可见性可以同步到 {}，但继续对话或 compact 仍可能失败。",
        total,
        providers.join(", "),
        target_provider
    ))
}

fn scan_sqlite_provider_stats(path: &Path) -> Result<Vec<ProviderStat>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let conn = open_sqlite(path)?;
    let mut stats = vec![];
    let tables = sqlite_tables(&conn)?;
    for table in tables {
        let cols = sqlite_provider_columns(&conn, &table)?;
        for col in cols {
            let sql = format!(
                "SELECT CAST({col} AS TEXT), COUNT(*) FROM {table} WHERE {col} IS NOT NULL GROUP BY {col}",
                col = quote_ident(&col),
                table = quote_ident(&table)
            );
            let mut stmt = match conn.prepare(&sql) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let rows = stmt
                .query_map([], |row| {
                    let provider: String = row.get(0)?;
                    let count: i64 = row.get(1)?;
                    Ok((provider, count))
                })
                .map_err(|e| sqlite_err(e, "read SQLite provider stats"))?;
            for row in rows {
                let (provider, count) =
                    row.map_err(|e| sqlite_err(e, "read SQLite provider stats"))?;
                stats.push(ProviderStat {
                    provider,
                    count: count as u64,
                    source: format!("sqlite:{}:{}", table, col),
                });
            }
        }
    }
    Ok(stats)
}

fn sync_sqlite_session_state(
    path: &Path,
    target: &str,
    user_event_thread_ids: &HashSet<String>,
    thread_cwd_by_id: &HashMap<String, String>,
) -> Result<SqliteSyncStats, String> {
    if !path.exists() {
        return Ok(SqliteSyncStats::default());
    }
    let mut conn = open_sqlite(path)?;
    let tx = conn
        .transaction()
        .map_err(|e| sqlite_err(e, "update SQLite session metadata"))?;
    let mut stats = SqliteSyncStats::default();

    if table_exists_tx(&tx, "threads")? {
        let columns = sqlite_columns_tx(&tx, "threads")?;
        if columns.contains("model_provider") {
            let changed: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM threads WHERE COALESCE(model_provider, '') <> ?1",
                    params![target],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if changed > 0 {
                tx.execute(
                    "UPDATE threads SET model_provider = ?1 WHERE COALESCE(model_provider, '') <> ?1",
                    params![target],
                )
                .map_err(|e| sqlite_err(e, "update SQLite session provider"))?;
                stats.provider_rows = changed as u64;
            }
        }
        if columns.contains("has_user_event") {
            let mut updated = 0u64;
            {
                let mut stmt = tx
                    .prepare(
                        "UPDATE threads SET has_user_event = 1 WHERE id = ?1 AND COALESCE(has_user_event, 0) <> 1",
                    )
                    .map_err(|e| sqlite_err(e, "update SQLite user-event flags"))?;
                for thread_id in user_event_thread_ids {
                    updated += stmt
                        .execute(params![thread_id])
                        .map_err(|e| sqlite_err(e, "update SQLite user-event flags"))?
                        as u64;
                }
            }
            stats.user_event_rows = updated;
        }
        if columns.contains("cwd") {
            let mut updated = 0u64;
            {
                let mut stmt = tx
                    .prepare("UPDATE threads SET cwd = ?1 WHERE id = ?2 AND COALESCE(cwd, '') <> ?1")
                    .map_err(|e| sqlite_err(e, "update SQLite cwd paths"))?;
                for (thread_id, cwd) in thread_cwd_by_id {
                    if thread_id.trim().is_empty() || cwd.trim().is_empty() {
                        continue;
                    }
                    updated += stmt
                        .execute(params![cwd, thread_id])
                        .map_err(|e| sqlite_err(e, "update SQLite cwd paths"))?
                        as u64;
                }
            }
            stats.cwd_rows = updated;
        }
    }

    let tables = sqlite_tables_tx(&tx)?;
    for table in tables {
        let cols = sqlite_provider_columns_tx(&tx, &table)?;
        for col in cols {
            if table == "threads" && col == "model_provider" {
                continue;
            }
            let count_sql = format!(
                "SELECT COUNT(*) FROM {table} WHERE {col} IS NOT NULL AND CAST({col} AS TEXT) != ?1",
                table = quote_ident(&table),
                col = quote_ident(&col)
            );
            let changed: i64 = tx
                .query_row(&count_sql, params![target], |row| row.get(0))
                .unwrap_or(0);
            if changed > 0 {
                let update_sql = format!(
                    "UPDATE {table} SET {col} = ?1 WHERE {col} IS NOT NULL AND CAST({col} AS TEXT) != ?1",
                    table = quote_ident(&table),
                    col = quote_ident(&col)
                );
                tx.execute(&update_sql, params![target])
                    .map_err(|e| sqlite_err(e, "update generic SQLite provider columns"))?;
                stats.generic_provider_rows += changed as u64;
            }
        }
    }

    tx.commit()
        .map_err(|e| sqlite_err(e, "commit SQLite session metadata"))?;
    Ok(stats)
}

fn read_sqlite_repair_stats(
    path: &Path,
    user_event_thread_ids: &HashSet<String>,
    thread_cwd_by_id: &HashMap<String, String>,
) -> Result<Option<SqliteRepairStats>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = open_sqlite(path)?;
    if !table_exists(&conn, "threads")? {
        return Ok(None);
    }
    let columns = sqlite_columns(&conn, "threads")?;
    let mut stats = SqliteRepairStats::default();
    if columns.contains("has_user_event") {
        let mut stmt = conn
            .prepare("SELECT has_user_event FROM threads WHERE id = ?1")
            .map_err(|e| sqlite_err(e, "read SQLite user-event diagnostics"))?;
        for thread_id in user_event_thread_ids {
            let value: Option<i64> = stmt
                .query_row(params![thread_id], |row| row.get(0))
                .ok();
            if let Some(v) = value {
                if v != 1 {
                    stats.user_event_rows_needing_repair += 1;
                }
            }
        }
    }
    if columns.contains("cwd") {
        let mut stmt = conn
            .prepare("SELECT cwd FROM threads WHERE id = ?1")
            .map_err(|e| sqlite_err(e, "read SQLite cwd diagnostics"))?;
        for (thread_id, cwd) in thread_cwd_by_id {
            let value: Option<String> = stmt
                .query_row(params![thread_id], |row| row.get(0))
                .ok();
            if let Some(v) = value {
                if v != *cwd {
                    stats.cwd_rows_needing_repair += 1;
                }
            }
        }
    }
    Ok(Some(stats))
}

fn open_sqlite(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| sqlite_err(e, "open SQLite state"))?;
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(|e| sqlite_err(e, "configure SQLite busy timeout"))?;
    Ok(conn)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
            params![table],
            |row| row.get(0),
        )
        .map_err(|e| sqlite_err(e, "inspect SQLite tables"))?;
    Ok(exists > 0)
}

fn table_exists_tx(tx: &rusqlite::Transaction<'_>, table: &str) -> Result<bool, String> {
    let exists: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
            params![table],
            |row| row.get(0),
        )
        .map_err(|e| sqlite_err(e, "inspect SQLite tables"))?;
    Ok(exists > 0)
}

fn sqlite_tables(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|e| sqlite_err(e, "list SQLite tables"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| sqlite_err(e, "list SQLite tables"))?;
    rows.into_iter()
        .map(|r| r.map_err(|e| sqlite_err(e, "list SQLite tables")))
        .collect()
}

fn sqlite_tables_tx(tx: &rusqlite::Transaction<'_>) -> Result<Vec<String>, String> {
    let mut stmt = tx
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|e| sqlite_err(e, "list SQLite tables"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| sqlite_err(e, "list SQLite tables"))?;
    rows.into_iter()
        .map(|r| r.map_err(|e| sqlite_err(e, "list SQLite tables")))
        .collect()
}

fn sqlite_columns(conn: &Connection, table: &str) -> Result<HashSet<String>, String> {
    let sql = format!("PRAGMA table_info({})", quote_ident(table));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| sqlite_err(e, "inspect SQLite columns"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| sqlite_err(e, "inspect SQLite columns"))?;
    rows.into_iter()
        .map(|r| r.map_err(|e| sqlite_err(e, "inspect SQLite columns")))
        .collect()
}

fn sqlite_columns_tx(tx: &rusqlite::Transaction<'_>, table: &str) -> Result<HashSet<String>, String> {
    let sql = format!("PRAGMA table_info({})", quote_ident(table));
    let mut stmt = tx
        .prepare(&sql)
        .map_err(|e| sqlite_err(e, "inspect SQLite columns"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| sqlite_err(e, "inspect SQLite columns"))?;
    rows.into_iter()
        .map(|r| r.map_err(|e| sqlite_err(e, "inspect SQLite columns")))
        .collect()
}

fn sqlite_provider_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    Ok(sqlite_columns(conn, table)?
        .into_iter()
        .filter(|name| name == "provider" || name == "model_provider")
        .collect())
}

fn sqlite_provider_columns_tx(tx: &rusqlite::Transaction<'_>, table: &str) -> Result<Vec<String>, String> {
    Ok(sqlite_columns_tx(tx, table)?
        .into_iter()
        .filter(|name| name == "provider" || name == "model_provider")
        .collect())
}

fn read_config_info(path: &Path) -> (String, Vec<String>) {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut current = String::new();
    let mut configured = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("[model_providers.") {
            if let Some(id) = trimmed
                .strip_prefix("[model_providers.")
                .and_then(|v| v.strip_suffix(']'))
            {
                configured.insert(id.trim_matches('"').to_string());
            }
            continue;
        }
        if trimmed.starts_with('[') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            if key.trim() == "model_provider" {
                current = value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
            }
        }
    }
    let mut configured: Vec<_> = configured.into_iter().collect();
    configured.sort();
    (current, configured)
}

fn read_project_visibility(home: &Path) -> Result<Vec<ProjectVisibility>, String> {
    let global_state_path = home.join(GLOBAL_STATE_FILE);
    if !global_state_path.exists() || !home.join(DB_FILE).exists() {
        return Ok(vec![]);
    }
    let roots = read_workspace_roots(home)?;
    if roots.is_empty() {
        return Ok(vec![]);
    }
    let conn = open_sqlite(&home.join(DB_FILE))?;
    if !table_exists(&conn, "threads")? {
        return Ok(vec![]);
    }
    let columns = sqlite_columns(&conn, "threads")?;
    if !columns.contains("cwd") {
        return Ok(vec![]);
    }
    let archived_filter = if columns.contains("archived") {
        "AND archived = 0"
    } else {
        ""
    };
    let source_filter = if columns.contains("source") {
        "AND source IN ('cli', 'vscode')"
    } else {
        ""
    };
    let first_user_filter = if columns.contains("first_user_message") {
        "AND first_user_message <> ''"
    } else {
        ""
    };
    let provider_expr = if columns.contains("model_provider") {
        "model_provider"
    } else {
        "'' AS model_provider"
    };
    let time_expr = build_time_expression(&columns);
    let sql = format!(
        "SELECT id, cwd, {provider_expr}, {time_expr} AS sort_ts FROM threads \
         WHERE cwd IS NOT NULL AND cwd <> '' {archived_filter} {first_user_filter} {source_filter} \
         ORDER BY sort_ts DESC, id DESC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| sqlite_err(e, "read project visibility diagnostics"))?;
    let rows = stmt
        .query_map([], |row| {
            let cwd: String = row.get(1)?;
            let provider: Option<String> = row.get(2)?;
            Ok((cwd, provider.unwrap_or_else(|| "(missing)".to_string())))
        })
        .map_err(|e| sqlite_err(e, "read project visibility diagnostics"))?;

    let mut ranked_rows = vec![];
    for (idx, row) in rows.enumerate() {
        let (cwd, provider) =
            row.map_err(|e| sqlite_err(e, "read project visibility diagnostics"))?;
        ranked_rows.push(RankedThreadRow {
            desktop_cwd: to_desktop_workspace_path(&cwd),
            normalized_cwd: normalize_comparable_path(&cwd).unwrap_or_default(),
            cwd,
            provider: if provider.is_empty() {
                "(missing)".to_string()
            } else {
                provider
            },
            rank: idx as u64 + 1,
        });
    }

    let mut out = vec![];
    for root in roots {
        let exact_root = to_desktop_workspace_path(&root);
        let Some(normalized_root) = normalize_comparable_path(&exact_root) else {
            continue;
        };
        let matching: Vec<_> = ranked_rows
            .iter()
            .filter(|row| row.normalized_cwd == normalized_root)
            .collect();
        let ranks: Vec<u64> = matching.iter().map(|row| row.rank).collect();
        let mut provider_counts = BTreeMap::new();
        let mut exact_cwd_matches = 0;
        let mut verbatim_cwd_rows = 0;
        for row in &matching {
            *provider_counts.entry(row.provider.clone()).or_insert(0) += 1;
            if row.cwd == exact_root || row.desktop_cwd == exact_root {
                exact_cwd_matches += 1;
            }
            if row.cwd.starts_with(r"\\?\") {
                verbatim_cwd_rows += 1;
            }
        }
        out.push(ProjectVisibility {
            root: exact_root,
            interactive_threads: matching.len() as u64,
            first_page_threads: ranks.iter().filter(|rank| **rank <= 50).count() as u64,
            exact_cwd_matches,
            verbatim_cwd_rows,
            top_rank: ranks.first().copied(),
            rank_preview: format_rank_preview(&ranks),
            provider_counts,
        });
    }
    Ok(out)
}

fn sync_workspace_roots(home: &Path) -> Result<WorkspaceSyncResult, String> {
    let path = home.join(GLOBAL_STATE_FILE);
    let backup_path = home.join(GLOBAL_STATE_BACKUP_FILE);
    if !path.exists() {
        return Ok(WorkspaceSyncResult::default());
    }
    let original_text = fs::read_to_string(&path).map_err(to_err)?;
    let mut state: Value = serde_json::from_str(&original_text).map_err(to_err)?;
    let Some(obj) = state.as_object_mut() else {
        return Ok(WorkspaceSyncResult {
            present: true,
            ..WorkspaceSyncResult::default()
        });
    };

    let cwd_stats = read_thread_cwd_stats(&home.join(DB_FILE))?;
    let existing_saved = path_array(obj.get("electron-saved-workspace-roots"));
    let existing_project_order = path_array(obj.get("project-order"));
    let existing_active = path_array(obj.get("active-workspace-roots"));
    let next_saved = dedupe_paths(
        if existing_project_order.is_empty() {
            existing_saved
                .iter()
                .chain(existing_active.iter())
                .map(|v| resolve_stored_path(v, &cwd_stats))
                .collect()
        } else {
            existing_project_order
                .iter()
                .chain(existing_saved.iter())
                .chain(existing_active.iter())
                .map(|v| resolve_stored_path(v, &cwd_stats))
                .collect()
        },
    );
    let next_project_order = dedupe_paths(
        if existing_project_order.is_empty() {
            next_saved.clone()
        } else {
            existing_project_order
                .iter()
                .chain(existing_saved.iter())
                .map(|v| resolve_stored_path(v, &cwd_stats))
                .collect()
        },
    );
    let next_active = dedupe_paths(
        existing_active
            .iter()
            .map(|v| resolve_stored_path(v, &cwd_stats))
            .collect(),
    );

    let mut changed = false;
    changed |= set_path_array(obj, "electron-saved-workspace-roots", &next_saved);
    changed |= set_path_array(obj, "project-order", &next_project_order);
    changed |= set_active_roots(obj, &next_active);
    changed |= resolve_object_keys(obj, "electron-workspace-root-labels", &cwd_stats);
    if let Some(open_targets) = obj
        .get_mut("open-in-target-preferences")
        .and_then(|v| v.as_object_mut())
    {
        if let Some(per_path) = open_targets.get_mut("perPath") {
            changed |= resolve_value_object_keys(per_path, &cwd_stats);
        }
    }

    let backup_missing = !backup_path.exists();
    let next_text = format!("{}\n", serde_json::to_string_pretty(&state).map_err(to_err)?);
    if changed || backup_missing {
        fs::write(&path, &next_text).map_err(to_err)?;
        fs::write(&backup_path, &next_text).map_err(to_err)?;
    }

    Ok(WorkspaceSyncResult {
        present: true,
        updated: changed || backup_missing,
        updated_workspace_roots: count_path_changes(&existing_saved, &next_saved),
        saved_workspace_root_count: next_saved.len() as u64,
    })
}

fn read_workspace_roots(home: &Path) -> Result<Vec<String>, String> {
    let text = fs::read_to_string(home.join(GLOBAL_STATE_FILE)).map_err(to_err)?;
    let state: Value = serde_json::from_str(&text).map_err(to_err)?;
    let saved = path_array(state.get("electron-saved-workspace-roots"));
    let project_order = path_array(state.get("project-order"));
    let active = path_array(state.get("active-workspace-roots"));
    Ok(dedupe_paths(if project_order.is_empty() {
        saved.into_iter().chain(active).map(|v| to_desktop_workspace_path(&v)).collect()
    } else {
        project_order
            .into_iter()
            .chain(saved)
            .chain(active)
            .map(|v| to_desktop_workspace_path(&v))
            .collect()
    }))
}

fn read_thread_cwd_stats(path: &Path) -> Result<Vec<CwdStat>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let conn = open_sqlite(path)?;
    if !table_exists(&conn, "threads")? {
        return Ok(vec![]);
    }
    let columns = sqlite_columns(&conn, "threads")?;
    if !columns.contains("cwd") {
        return Ok(vec![]);
    }
    let updated_expr = if columns.contains("updated_at_ms") && columns.contains("updated_at") {
        "COALESCE(MAX(updated_at_ms), MAX(updated_at) * 1000, 0)"
    } else if columns.contains("updated_at_ms") {
        "COALESCE(MAX(updated_at_ms), 0)"
    } else if columns.contains("updated_at") {
        "COALESCE(MAX(updated_at) * 1000, 0)"
    } else {
        "0"
    };
    let sql = format!(
        "SELECT cwd, COUNT(*) AS count, {updated_expr} AS updated_at_ms \
         FROM threads WHERE cwd IS NOT NULL AND cwd <> '' \
         GROUP BY cwd ORDER BY count DESC, updated_at_ms DESC, cwd"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| sqlite_err(e, "read SQLite cwd stats"))?;
    let rows = stmt
        .query_map([], |row| {
            let cwd: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let updated_at_ms: i64 = row.get(2)?;
            Ok((cwd, count, updated_at_ms))
        })
        .map_err(|e| sqlite_err(e, "read SQLite cwd stats"))?;
    let mut out = vec![];
    for row in rows {
        let (cwd, count, updated_at_ms) =
            row.map_err(|e| sqlite_err(e, "read SQLite cwd stats"))?;
        if let Some(normalized) = normalize_comparable_path(&cwd) {
            out.push(CwdStat {
                cwd,
                normalized,
                count: count as u64,
                updated_at_ms,
            });
        }
    }
    out.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| b.updated_at_ms.cmp(&a.updated_at_ms))
            .then_with(|| a.cwd.cmp(&b.cwd))
    });
    Ok(out)
}

fn build_time_expression(columns: &HashSet<String>) -> String {
    let mut expressions = vec![];
    if columns.contains("updated_at_ms") {
        expressions.push("updated_at_ms");
    }
    if columns.contains("updated_at") {
        expressions.push("updated_at * 1000");
    }
    if columns.contains("created_at_ms") {
        expressions.push("created_at_ms");
    }
    if columns.contains("created_at") {
        expressions.push("created_at * 1000");
    }
    expressions.push("0");
    format!("COALESCE({})", expressions.join(", "))
}

fn path_array(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.to_string())
            .collect(),
        Some(Value::String(v)) if !v.trim().is_empty() => vec![v.to_string()],
        _ => vec![],
    }
}

fn dedupe_paths(paths: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for path in paths {
        let Some(key) = normalize_comparable_path(&path) else {
            continue;
        };
        if seen.insert(key) {
            out.push(path);
        }
    }
    out
}

fn count_path_changes(previous: &[String], next: &[String]) -> u64 {
    let max_len = previous.len().max(next.len());
    let mut changed = 0;
    for index in 0..max_len {
        if previous.get(index) != next.get(index) {
            changed += 1;
        }
    }
    changed
}

fn set_path_array(
    obj: &mut serde_json::Map<String, Value>,
    key: &str,
    paths: &[String],
) -> bool {
    let next = Value::Array(paths.iter().map(|v| Value::String(v.clone())).collect());
    if obj.get(key) == Some(&next) {
        return false;
    }
    obj.insert(key.to_string(), next);
    true
}

fn set_active_roots(obj: &mut serde_json::Map<String, Value>, paths: &[String]) -> bool {
    let next = if matches!(obj.get("active-workspace-roots"), Some(Value::String(_))) {
        paths
            .first()
            .map(|v| Value::String(v.clone()))
            .unwrap_or(Value::Null)
    } else {
        Value::Array(paths.iter().map(|v| Value::String(v.clone())).collect())
    };
    if obj.get("active-workspace-roots") == Some(&next) {
        return false;
    }
    obj.insert("active-workspace-roots".to_string(), next);
    true
}

fn resolve_object_keys(
    obj: &mut serde_json::Map<String, Value>,
    key: &str,
    cwd_stats: &[CwdStat],
) -> bool {
    if let Some(value) = obj.get_mut(key) {
        return resolve_value_object_keys(value, cwd_stats);
    }
    false
}

fn resolve_value_object_keys(value: &mut Value, cwd_stats: &[CwdStat]) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    let mut next = serde_json::Map::new();
    for (key, value) in map {
        let resolved = resolve_stored_path(key, cwd_stats);
        next.entry(resolved).or_insert_with(|| value.clone());
    }
    let next_value = Value::Object(next);
    if *value == next_value {
        return false;
    }
    *value = next_value;
    true
}

fn resolve_stored_path(value: &str, cwd_stats: &[CwdStat]) -> String {
    let Some(normalized) = normalize_comparable_path(value) else {
        return value.to_string();
    };
    for stat in cwd_stats {
        if stat.normalized == normalized {
            return to_desktop_workspace_path(&stat.cwd);
        }
    }
    to_desktop_workspace_path(value)
}

fn normalize_comparable_path(value: &str) -> Option<String> {
    let mut normalized = value.trim().to_string();
    if normalized.is_empty() {
        return None;
    }
    if let Some(rest) = normalized.strip_prefix(r"\\?\UNC\") {
        normalized = format!(r"\\{}", rest);
    } else if let Some(rest) = normalized.strip_prefix(r"\\?\") {
        normalized = rest.to_string();
    }
    let looks_windows = normalized.starts_with("\\\\")
        || normalized.contains('\\')
        || normalized.as_bytes().get(1) == Some(&b':');
    if looks_windows {
        normalized = normalized.replace('/', "\\");
        while normalized.ends_with('\\') && normalized.len() > 3 {
            normalized.pop();
        }
        return Some(normalized.to_lowercase());
    }
    while normalized.ends_with('/') && normalized.len() > 1 {
        normalized.pop();
    }
    Some(normalized.to_lowercase())
}

fn to_desktop_workspace_path(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return value.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{}", rest).replace('/', "\\");
    }
    if let Some(rest) = trimmed.strip_prefix(r"\\?\") {
        return rest.replace('/', "\\");
    }
    value.to_string()
}

fn format_rank_preview(ranks: &[u64]) -> String {
    let preview: Vec<String> = ranks.iter().take(12).map(|v| v.to_string()).collect();
    let remaining = ranks.len().saturating_sub(preview.len());
    if remaining > 0 {
        format!("{} (+{} more)", preview.join(", "), remaining)
    } else {
        preview.join(", ")
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn is_file_busy_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
    ) || matches!(error.raw_os_error(), Some(32 | 33))
}

fn sqlite_err<E: std::fmt::Display>(e: E, action: &str) -> String {
    let message = e.to_string();
    let lower = message.to_lowercase();
    if lower.contains("database is locked")
        || lower.contains("sqlite_busy")
        || lower.contains("busy")
        || lower.contains("locked")
    {
        return format!(
            "无法{}，因为 state_5.sqlite 正在被占用。请先关闭 Codex / Codex App / app-server 后重试。原始错误：{}",
            action, message
        );
    }
    if lower.contains("malformed")
        || lower.contains("not a database")
        || lower.contains("corrupt")
    {
        return format!(
            "无法{}，因为 state_5.sqlite 损坏或不可读取。请先备份/修复数据库后重试。原始错误：{}",
            action, message
        );
    }
    message
}

fn escape_json_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn to_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            scan_codex_home,
            create_backup,
            list_backups,
            restore_backup,
            sync_provider
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
