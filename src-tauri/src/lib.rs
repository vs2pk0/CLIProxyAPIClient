use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    net::TcpStream,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tar::{Archive, Builder};
use tauri::{path::BaseDirectory, AppHandle, Emitter, Manager, State};

const BINARY_BASENAME: &str = "cli-proxy-api";
const DEFAULT_CONFIG_CACHE: &str = "default-config.yaml";
const DEFAULT_AUTH_DIR: &str = "~/.cli-proxy-api";
const DEFAULT_RUNTIME_VERSION: &str = "6.9.36";
const USAGE_BACKUP_FILE: &str = "usage-statistics.json";
const CLIPROXYAPI_REPOSITORY_URL: &str = "https://github.com/router-for-me/CLIProxyAPI";
const CLIPROXYAPI_RELEASES_URL: &str = "https://github.com/router-for-me/CLIProxyAPI/releases";
const CLIPROXYAPI_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/router-for-me/CLIProxyAPI/releases/latest";
const GITHUB_USER_AGENT: &str = "CLIProxyAPI-Desktop";
const DOWNLOAD_PROGRESS_EVENT: &str = "cliproxyapi-download-progress";

#[derive(Default)]
struct ProcessState(Mutex<Option<Child>>);

#[derive(Default)]
struct DownloadState(Mutex<DownloadControl>);

#[derive(Default)]
struct DownloadControl {
    active_id: Option<String>,
    cancel_requested: bool,
}

#[derive(Debug, Clone)]
struct PackageInfo {
    id: String,
    version: String,
    target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeMetadata {
    id: String,
    version: String,
    target: String,
    installed_at: u64,
    package_file: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInfo {
    id: String,
    version: String,
    target: String,
    path: String,
    binary_path: String,
    installed_at: u64,
    package_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StoredState {
    active_version: Option<String>,
    management_key: Option<String>,
    managed_pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceInfo {
    running: bool,
    pid: Option<u32>,
    port: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopState {
    app_data_dir: String,
    workspace_dir: String,
    auth_dir: String,
    active_version: Option<String>,
    runtimes: Vec<RuntimeInfo>,
    service: ServiceInfo,
    config: Option<ConfigFileInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFileInfo {
    path: String,
    content: String,
    port: Option<u16>,
    management_url: Option<String>,
    local_management_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateInfo {
    current_version: Option<String>,
    latest_version: String,
    target: String,
    release_url: String,
    download_url: Option<String>,
    asset_name: Option<String>,
    has_update: bool,
    latest_installed: bool,
    latest_active: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgressEvent {
    status: String,
    asset_name: String,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    message: Option<String>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(ProcessState::default())
        .manage(DownloadState::default())
        .setup(|app| {
            if let Err(err) = bootstrap_default_runtime(app.handle()) {
                eprintln!("failed to bootstrap bundled CLIProxyAPI runtime: {err}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            desktop_state,
            install_update_package,
            activate_version,
            delete_version,
            start_service,
            stop_service,
            shutdown_service,
            open_workspace,
            open_auth_dir,
            export_auth_archive,
            import_auth_archive,
            save_config_file,
            restore_default_config,
            open_management_web,
            open_cli_proxy_repository,
            open_cli_proxy_releases,
            check_cli_proxy_update,
            download_cli_proxy_update,
            cancel_cli_proxy_download
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let process = window.state::<ProcessState>();
                let lock_result = process.0.lock();
                if let Ok(mut guard) = lock_result {
                    if let Some(child) = guard.as_mut() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    *guard = None;
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

#[tauri::command]
fn desktop_state(app: AppHandle, process: State<'_, ProcessState>) -> Result<DesktopState, String> {
    build_desktop_state(&app, &process)
}

#[tauri::command]
fn save_config_file(
    app: AppHandle,
    process: State<'_, ProcessState>,
    content: String,
    management_key: String,
) -> Result<DesktopState, String> {
    if content.trim().is_empty() {
        return Err("配置内容不能为空".to_string());
    }

    let dirs = AppDirs::new(&app)?;
    let runtime = active_runtime(&dirs)?;
    let config_path = ensure_workspace_config(&dirs, &runtime)?;
    let config_content = clear_management_secret(&content);
    fs::write(&config_path, config_content).map_err(|err| format!("写入配置文件失败: {err}"))?;

    let mut stored = read_stored_state(&dirs)?;
    stored.management_key =
        non_empty_string(management_key).or_else(|| plain_management_secret_from_content(&content));
    write_stored_state(&dirs, &stored)?;

    build_desktop_state(&app, &process)
}

#[tauri::command]
fn restore_default_config(
    app: AppHandle,
    process: State<'_, ProcessState>,
) -> Result<DesktopState, String> {
    if service_pid_for_state(&app, &process)?.is_some() {
        return Err("请先停止服务，再恢复默认配置".to_string());
    }

    let dirs = AppDirs::new(&app)?;
    let runtime = active_runtime(&dirs)?;
    let default_config = read_default_config(&runtime)?;
    let default_management_key = plain_management_secret_from_content(&default_config);
    let config_content = clear_management_secret(&default_config);
    let config_path = ensure_workspace_dir(&dirs)?.join("config.yaml");
    fs::write(&config_path, config_content).map_err(|err| format!("写入配置文件失败: {err}"))?;

    let mut stored = read_stored_state(&dirs)?;
    stored.management_key = default_management_key;
    write_stored_state(&dirs, &stored)?;

    build_desktop_state(&app, &process)
}

#[tauri::command]
fn install_update_package(
    app: AppHandle,
    process: State<'_, ProcessState>,
    path: String,
) -> Result<DesktopState, String> {
    if service_pid_for_state(&app, &process)?.is_some() {
        return Err("请先停止当前服务，再导入并切换 CLIProxyAPI 版本包".to_string());
    }

    let package_path = PathBuf::from(path);
    install_runtime_package(&app, &package_path, true)?;
    build_desktop_state(&app, &process)
}

#[tauri::command]
fn activate_version(
    app: AppHandle,
    process: State<'_, ProcessState>,
    id: String,
) -> Result<DesktopState, String> {
    if service_pid_for_state(&app, &process)?.is_some() {
        return Err("请先停止当前服务，再切换 CLIProxyAPI 版本".to_string());
    }

    let dirs = AppDirs::new(&app)?;
    let runtime = runtime_by_id(&dirs, &id)?;
    if !runtime.binary_path.exists() {
        return Err(format!("版本 {id} 缺少可执行文件"));
    }

    let mut state = read_stored_state(&dirs)?;
    state.active_version = Some(id);
    write_stored_state(&dirs, &state)?;
    build_desktop_state(&app, &process)
}

#[tauri::command]
fn delete_version(
    app: AppHandle,
    process: State<'_, ProcessState>,
    id: String,
) -> Result<DesktopState, String> {
    let dirs = AppDirs::new(&app)?;
    let stored = read_stored_state(&dirs)?;
    if stored.active_version.as_deref() == Some(id.as_str()) {
        return Err("当前版本不能删除".to_string());
    }

    let runtime = runtime_by_id(&dirs, &id)?;
    if !runtime.path.starts_with(&dirs.runtime_dir) {
        return Err("拒绝删除运行时目录外的文件".to_string());
    }
    fs::remove_dir_all(&runtime.path).map_err(|err| format!("删除版本失败: {err}"))?;

    build_desktop_state(&app, &process)
}

#[tauri::command]
fn start_service(app: AppHandle, process: State<'_, ProcessState>) -> Result<DesktopState, String> {
    if process_pid(&process)?.is_some() {
        return build_desktop_state(&app, &process);
    }

    let dirs = AppDirs::new(&app)?;
    let mut stored = read_stored_state(&dirs)?;
    let active_id = stored
        .active_version
        .clone()
        .ok_or_else(|| "还没有可启动的 CLIProxyAPI 版本".to_string())?;
    let runtime = runtime_by_id(&dirs, &active_id)?;
    let config_path = ensure_workspace_config(&dirs, &runtime)?;
    let management_key = management_key_for_config(&stored, &config_path)?
        .ok_or_else(|| "请先填写并保存本机管理密钥".to_string())?;
    ensure_config_uses_local_management_key(&config_path)?;
    if let Some(pid) = detect_managed_service_pid(&dirs, &stored, &config_path)? {
        stored.managed_pid = Some(pid);
        write_stored_state(&dirs, &stored)?;
        if let Err(err) = restore_usage_statistics(&app) {
            eprintln!("failed to restore CLIProxyAPI usage statistics: {err}");
        }
        return build_desktop_state(&app, &process);
    }
    reject_unmanaged_port_listener(&dirs, &config_path)?;

    let mut command = Command::new(&runtime.binary_path);
    command
        .arg("--config")
        .arg(&config_path)
        .current_dir(&dirs.workspace_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    command.env("MANAGEMENT_PASSWORD", management_key);

    let child = command
        .spawn()
        .map_err(|err| format!("启动 CLIProxyAPI 失败: {err}"))?;
    let child_pid = child.id();

    let mut guard = process
        .0
        .lock()
        .map_err(|_| "服务状态锁已损坏".to_string())?;
    *guard = Some(child);
    drop(guard);

    stored.managed_pid = Some(child_pid);
    write_stored_state(&dirs, &stored)?;

    if let Err(err) = restore_usage_statistics(&app) {
        eprintln!("failed to restore CLIProxyAPI usage statistics: {err}");
    }

    build_desktop_state(&app, &process)
}

#[tauri::command]
fn stop_service(app: AppHandle, process: State<'_, ProcessState>) -> Result<DesktopState, String> {
    if let Err(err) = backup_usage_statistics(&app, &process) {
        eprintln!("failed to backup CLIProxyAPI usage statistics: {err}");
    }

    let mut guard = process
        .0
        .lock()
        .map_err(|_| "服务状态锁已损坏".to_string())?;

    if let Some(child) = guard.as_mut() {
        child
            .kill()
            .map_err(|err| format!("停止 CLIProxyAPI 失败: {err}"))?;
        child
            .wait()
            .map_err(|err| format!("等待 CLIProxyAPI 退出失败: {err}"))?;
    }
    *guard = None;
    drop(guard);

    let dirs = AppDirs::new(&app)?;
    let mut stored = read_stored_state(&dirs)?;
    if let Some(active_id) = stored.active_version.as_deref() {
        let runtime = runtime_by_id(&dirs, active_id)?;
        let config_path = ensure_workspace_config(&dirs, &runtime)?;
        if let Some(pid) = detect_managed_service_pid(&dirs, &stored, &config_path)? {
            terminate_managed_pid(&dirs, pid)?;
        }
    }
    stored.managed_pid = None;
    write_stored_state(&dirs, &stored)?;

    build_desktop_state(&app, &process)
}

#[tauri::command]
fn shutdown_service(app: AppHandle, process: State<'_, ProcessState>) -> Result<(), String> {
    stop_service(app, process).map(|_| ())
}

#[tauri::command]
fn open_workspace(app: AppHandle) -> Result<(), String> {
    let dirs = AppDirs::new(&app)?;
    fs::create_dir_all(&dirs.workspace_dir).map_err(|err| format!("创建工作区失败: {err}"))?;
    tauri_plugin_opener::open_path(&dirs.workspace_dir, None::<&str>)
        .map_err(|err| format!("打开工作区失败: {err}"))
}

#[tauri::command]
fn open_auth_dir(app: AppHandle) -> Result<(), String> {
    let dirs = AppDirs::new(&app)?;
    let auth_dir = current_auth_dir(&dirs)?;
    fs::create_dir_all(&auth_dir).map_err(|err| format!("创建认证文件目录失败: {err}"))?;
    tauri_plugin_opener::open_path(&auth_dir, None::<&str>)
        .map_err(|err| format!("打开认证文件目录失败: {err}"))
}

#[tauri::command]
fn export_auth_archive(
    app: AppHandle,
    process: State<'_, ProcessState>,
    path: String,
) -> Result<DesktopState, String> {
    let dirs = AppDirs::new(&app)?;
    let auth_dir = current_auth_dir(&dirs)?;
    let archive_path = PathBuf::from(path);
    export_auth_archive_file(&auth_dir, &archive_path)?;
    build_desktop_state(&app, &process)
}

#[tauri::command]
fn import_auth_archive(
    app: AppHandle,
    process: State<'_, ProcessState>,
    path: String,
) -> Result<DesktopState, String> {
    let dirs = AppDirs::new(&app)?;
    let auth_dir = current_auth_dir(&dirs)?;
    let archive_path = PathBuf::from(path);
    import_auth_archive_file(&dirs, &archive_path, &auth_dir)?;
    build_desktop_state(&app, &process)
}

#[tauri::command]
fn open_management_web(app: AppHandle) -> Result<(), String> {
    let dirs = AppDirs::new(&app)?;
    let runtime = active_runtime(&dirs)?;
    let config_path = ensure_workspace_config(&dirs, &runtime)?;
    let stored = read_stored_state(&dirs)?;
    if detect_managed_service_pid(&dirs, &stored, &config_path)?.is_none() {
        return Err("服务未运行，启动后再打开管理页".to_string());
    }
    let management_key = management_key_for_config(&stored, &config_path)?;
    let config = config_info(&config_path, management_key)?;
    let url = config
        .management_url
        .ok_or_else(|| "配置文件缺少可访问的 Web 端口".to_string())?;
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|err| format!("打开浏览器失败: {err}"))
}

#[tauri::command]
fn open_cli_proxy_repository() -> Result<(), String> {
    tauri_plugin_opener::open_url(CLIPROXYAPI_REPOSITORY_URL, None::<&str>)
        .map_err(|err| format!("打开 CLIProxyAPI 源码地址失败: {err}"))
}

#[tauri::command]
fn open_cli_proxy_releases() -> Result<(), String> {
    tauri_plugin_opener::open_url(CLIPROXYAPI_RELEASES_URL, None::<&str>)
        .map_err(|err| format!("打开 CLIProxyAPI 下载地址失败: {err}"))
}

#[tauri::command]
fn check_cli_proxy_update(app: AppHandle) -> Result<UpdateInfo, String> {
    let dirs = AppDirs::new(&app)?;
    let release = fetch_latest_release()?;
    update_info_from_release(&dirs, &release)
}

#[tauri::command]
async fn download_cli_proxy_update(app: AppHandle) -> Result<DesktopState, String> {
    let task_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let process = task_app.state::<ProcessState>();
        let download = task_app.state::<DownloadState>();
        download_cli_proxy_update_impl(task_app.clone(), process, download)
    })
    .await
    .map_err(|err| format!("下载任务执行失败: {err}"))?
}

#[tauri::command]
fn cancel_cli_proxy_download(download: State<'_, DownloadState>) -> Result<(), String> {
    let mut guard = download
        .0
        .lock()
        .map_err(|_| "下载状态锁已损坏".to_string())?;
    if guard.active_id.is_some() {
        guard.cancel_requested = true;
    }
    Ok(())
}

fn download_cli_proxy_update_impl(
    app: AppHandle,
    process: State<'_, ProcessState>,
    download: State<'_, DownloadState>,
) -> Result<DesktopState, String> {
    if service_pid_for_state(&app, &process)?.is_some() {
        return Err("请先停止当前服务，再下载并切换 CLIProxyAPI 版本包".to_string());
    }

    let download_id = begin_download(&download)?;
    let result: Result<DesktopState, String> = (|| {
        let dirs = AppDirs::new(&app)?;
        let release = fetch_latest_release()?;
        let update = update_info_from_release(&dirs, &release)?;
        let asset_name = update
            .asset_name
            .ok_or_else(|| format!("最新版本没有匹配 {} 的安装包", update.target))?;
        let download_url = update
            .download_url
            .ok_or_else(|| format!("最新版本没有匹配 {} 的下载地址", update.target))?;
        let package_path = download_release_asset(
            &app,
            &download,
            &download_id,
            &dirs,
            &asset_name,
            &download_url,
        )?;
        emit_download_progress(
            &app,
            "installing",
            &asset_name,
            0,
            None,
            Some("正在自动导入"),
        )?;
        install_runtime_package(&app, &package_path, true)?;
        let next_state = build_desktop_state(&app, &process)?;
        emit_download_progress(&app, "done", &asset_name, 0, None, Some("下载导入完成"))?;
        Ok(next_state)
    })();

    if let Err(message) = &result {
        if !message.contains("下载已取消") {
            let _ = emit_download_progress(&app, "failed", "", 0, None, Some(message));
        }
    }
    clear_download(&download, &download_id);
    result
}

fn fetch_latest_release() -> Result<GitHubRelease, String> {
    let client = github_client()?;
    let response = client
        .get(CLIPROXYAPI_LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|err| format!("检测 CLIProxyAPI 更新失败: {err}"))?;
    let status = response.status();
    if !status.is_success() {
        let message = response.text().unwrap_or_default();
        return Err(format!(
            "检测 CLIProxyAPI 更新失败: HTTP {status} {message}"
        ));
    }

    response
        .json()
        .map_err(|err| format!("解析 CLIProxyAPI 更新信息失败: {err}"))
}

fn github_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(GITHUB_USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|err| format!("初始化下载客户端失败: {err}"))
}

fn update_info_from_release(dirs: &AppDirs, release: &GitHubRelease) -> Result<UpdateInfo, String> {
    let latest_version = normalize_release_version(&release.tag_name);
    let target_aliases = current_package_target_aliases();
    let target = target_aliases.join(" / ");
    let stored = read_stored_state(dirs)?;
    let runtimes = list_runtimes(dirs)?;
    let active_runtime = stored
        .active_version
        .as_deref()
        .and_then(|id| runtimes.iter().find(|runtime| runtime.id == id));
    let current_version = active_runtime.map(|runtime| runtime.version.clone());
    let latest_installed = runtimes.iter().any(|runtime| {
        normalize_release_version(&runtime.version) == latest_version
            && target_aliases
                .iter()
                .any(|target| target == &runtime.target)
    });
    let latest_active = active_runtime.is_some_and(|runtime| {
        normalize_release_version(&runtime.version) == latest_version
            && target_aliases
                .iter()
                .any(|target| target == &runtime.target)
    });
    let asset = release_asset_for_target(release, &latest_version, &target_aliases);
    let has_update = current_version
        .as_deref()
        .map_or(asset.is_some(), |version| {
            asset.is_some() && !latest_installed && is_newer_version(&latest_version, version)
        });

    Ok(UpdateInfo {
        current_version,
        latest_version,
        target,
        release_url: release.html_url.clone(),
        download_url: asset.map(|asset| asset.browser_download_url.clone()),
        asset_name: asset.map(|asset| asset.name.clone()),
        has_update,
        latest_installed,
        latest_active,
    })
}

fn release_asset_for_target<'a>(
    release: &'a GitHubRelease,
    version: &str,
    targets: &[String],
) -> Option<&'a GitHubAsset> {
    let exact_names = targets
        .iter()
        .flat_map(|target| {
            [
                format!("CLIProxyAPI_{version}_{target}.tar.gz"),
                format!("CLIProxyAPI_{version}_{target}.tgz"),
                format!("CLIProxyAPI_{version}_{target}.zip"),
            ]
        })
        .collect::<Vec<_>>();
    release
        .assets
        .iter()
        .find(|asset| exact_names.iter().any(|name| name == &asset.name))
        .or_else(|| {
            release.assets.iter().find(|asset| {
                let name = asset.name.as_str();
                is_runtime_archive_name(name)
                    && name.contains("CLIProxyAPI")
                    && name.contains(version)
                    && targets.iter().any(|target| name.contains(target))
            })
        })
        .or_else(|| {
            release.assets.iter().find(|asset| {
                let name = asset.name.as_str();
                is_runtime_archive_name(name)
                    && name.contains("CLIProxyAPI")
                    && targets.iter().any(|target| name.contains(target))
            })
        })
}

fn is_runtime_archive_name(name: &str) -> bool {
    name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".zip")
}

fn begin_download(download: &State<'_, DownloadState>) -> Result<String, String> {
    let id = format!("{}-{}", std::process::id(), unix_timestamp()?);
    let mut guard = download
        .0
        .lock()
        .map_err(|_| "下载状态锁已损坏".to_string())?;
    if guard.active_id.is_some() {
        return Err("已有下载任务正在进行".to_string());
    }
    guard.active_id = Some(id.clone());
    guard.cancel_requested = false;
    Ok(id)
}

fn clear_download(download: &State<'_, DownloadState>, download_id: &str) {
    if let Ok(mut guard) = download.0.lock() {
        if guard.active_id.as_deref() == Some(download_id) {
            guard.active_id = None;
            guard.cancel_requested = false;
        }
    }
}

fn is_download_cancelled(
    download: &State<'_, DownloadState>,
    download_id: &str,
) -> Result<bool, String> {
    let guard = download
        .0
        .lock()
        .map_err(|_| "下载状态锁已损坏".to_string())?;
    Ok(guard.active_id.as_deref() == Some(download_id) && guard.cancel_requested)
}

fn emit_download_progress(
    app: &AppHandle,
    status: &str,
    asset_name: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    message: Option<&str>,
) -> Result<(), String> {
    app.emit(
        DOWNLOAD_PROGRESS_EVENT,
        DownloadProgressEvent {
            status: status.to_string(),
            asset_name: asset_name.to_string(),
            downloaded_bytes,
            total_bytes,
            message: message.map(ToString::to_string),
        },
    )
    .map_err(|err| format!("发送下载进度失败: {err}"))
}

fn download_release_asset(
    app: &AppHandle,
    download: &State<'_, DownloadState>,
    download_id: &str,
    dirs: &AppDirs,
    asset_name: &str,
    download_url: &str,
) -> Result<PathBuf, String> {
    let file_name = safe_download_file_name(asset_name)?;
    let download_dir = dirs.app_data_dir.join("downloads");
    fs::create_dir_all(&download_dir).map_err(|err| format!("创建下载目录失败: {err}"))?;
    let package_path = download_dir.join(&file_name);
    let temp_path = download_dir.join(format!("{file_name}.download"));

    let result = (|| {
        emit_download_progress(app, "starting", asset_name, 0, None, Some("准备下载"))?;
        let client = github_client()?;
        let mut response = client
            .get(download_url)
            .header("Accept", "application/octet-stream")
            .send()
            .map_err(|err| format!("下载 CLIProxyAPI 安装包失败: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().unwrap_or_default();
            return Err(format!(
                "下载 CLIProxyAPI 安装包失败: HTTP {status} {message}"
            ));
        }
        let total_bytes = response.content_length();
        emit_download_progress(app, "downloading", asset_name, 0, total_bytes, None)?;

        let mut file =
            File::create(&temp_path).map_err(|err| format!("创建下载临时文件失败: {err}"))?;
        let mut downloaded_bytes = 0_u64;
        let mut last_emit_bytes = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if is_download_cancelled(download, download_id)? {
                emit_download_progress(
                    app,
                    "cancelled",
                    asset_name,
                    downloaded_bytes,
                    total_bytes,
                    Some("下载已取消"),
                )?;
                return Err("下载已取消".to_string());
            }

            let read = response
                .read(&mut buffer)
                .map_err(|err| format!("读取安装包失败: {err}"))?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])
                .map_err(|err| format!("写入安装包失败: {err}"))?;
            downloaded_bytes += read as u64;

            if downloaded_bytes == total_bytes.unwrap_or(0)
                || downloaded_bytes.saturating_sub(last_emit_bytes) >= 512 * 1024
            {
                emit_download_progress(
                    app,
                    "downloading",
                    asset_name,
                    downloaded_bytes,
                    total_bytes,
                    None,
                )?;
                last_emit_bytes = downloaded_bytes;
            }
        }
        file.flush()
            .map_err(|err| format!("刷新安装包文件失败: {err}"))?;
        emit_download_progress(
            app,
            "downloading",
            asset_name,
            downloaded_bytes,
            total_bytes,
            None,
        )?;
        fs::rename(&temp_path, &package_path).map_err(|err| format!("保存安装包失败: {err}"))?;
        Ok(package_path)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn safe_download_file_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("下载文件名为空".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("下载文件名包含路径分隔符".to_string());
    }

    let path = Path::new(trimmed);
    let components = path.components().collect::<Vec<_>>();
    if components.len() != 1 || !matches!(components[0], Component::Normal(_)) {
        return Err("下载文件名不安全".to_string());
    }
    Ok(trimmed.to_string())
}

fn normalize_release_version(tag: &str) -> String {
    let trimmed = tag.trim();
    let trimmed = trimmed.trim_start_matches(['v', 'V']);
    if trimmed
        .chars()
        .next()
        .is_some_and(|value| value.is_ascii_digit())
    {
        return trimmed.to_string();
    }

    let Some(start) = trimmed.find(|value: char| value.is_ascii_digit()) else {
        return trimmed.to_string();
    };
    trimmed[start..].to_string()
}

fn is_newer_version(latest: &str, current: &str) -> bool {
    let latest_parts = version_number_parts(latest);
    let current_parts = version_number_parts(current);
    if latest_parts.is_empty() || current_parts.is_empty() {
        return normalize_release_version(latest) != normalize_release_version(current);
    }

    let length = latest_parts.len().max(current_parts.len());
    for index in 0..length {
        let left = latest_parts.get(index).copied().unwrap_or(0);
        let right = current_parts.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn version_number_parts(value: &str) -> Vec<u64> {
    value
        .split(|part: char| !part.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

fn backup_usage_statistics(
    app: &AppHandle,
    process: &State<'_, ProcessState>,
) -> Result<(), String> {
    if service_pid_for_state(app, process)?.is_none() {
        return Ok(());
    }

    let dirs = AppDirs::new(app)?;
    let Some((port, management_key)) = usage_management_context(&dirs)? else {
        return Ok(());
    };
    let body = management_http_request(
        port,
        &management_key,
        "GET",
        "/v0/management/usage/export",
        None,
    )?;

    let backup_path = usage_backup_path(&dirs);
    if !should_write_usage_backup(&backup_path, &body) {
        return Ok(());
    }
    fs::create_dir_all(&dirs.app_data_dir).map_err(|err| format!("创建应用数据目录失败: {err}"))?;
    let temp_path = backup_path.with_extension("json.tmp");
    fs::write(&temp_path, body).map_err(|err| format!("写入使用统计备份失败: {err}"))?;
    fs::rename(&temp_path, &backup_path).map_err(|err| format!("保存使用统计备份失败: {err}"))?;
    Ok(())
}

fn restore_usage_statistics(app: &AppHandle) -> Result<(), String> {
    let dirs = AppDirs::new(app)?;
    let backup_path = usage_backup_path(&dirs);
    if !backup_path.exists() {
        return Ok(());
    }
    let body = fs::read(&backup_path).map_err(|err| format!("读取使用统计备份失败: {err}"))?;
    if body.is_empty() || usage_total_requests(&body) == 0 {
        return Ok(());
    }

    let Some((port, management_key)) = usage_management_context(&dirs)? else {
        return Ok(());
    };

    let mut last_error = None;
    for _ in 0..25 {
        match management_http_request(
            port,
            &management_key,
            "POST",
            "/v0/management/usage/import",
            Some(&body),
        ) {
            Ok(_) => return Ok(()),
            Err(err) => {
                last_error = Some(err);
                thread::sleep(Duration::from_millis(200));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "导入使用统计备份失败".to_string()))
}

fn usage_management_context(dirs: &AppDirs) -> Result<Option<(u16, String)>, String> {
    let stored = read_stored_state(dirs)?;
    let Some(active_id) = stored.active_version.as_deref() else {
        return Ok(None);
    };
    let runtime = runtime_by_id(dirs, active_id)?;
    let config_path = ensure_workspace_config(dirs, &runtime)?;
    let Some(management_key) = management_key_for_config(&stored, &config_path)? else {
        return Ok(None);
    };
    let content = fs::read_to_string(&config_path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => "配置文件还没有初始化".to_string(),
        _ => format!("读取配置文件失败: {err}"),
    })?;
    let Some(port) = read_port_from_content(&content)? else {
        return Ok(None);
    };
    Ok(Some((port, management_key)))
}

fn usage_backup_path(dirs: &AppDirs) -> PathBuf {
    dirs.app_data_dir.join(USAGE_BACKUP_FILE)
}

fn should_write_usage_backup(backup_path: &Path, exported: &[u8]) -> bool {
    usage_total_requests(exported) > 0 || !backup_path.exists()
}

fn usage_total_requests(data: &[u8]) -> i64 {
    serde_json::from_slice::<serde_json::Value>(data)
        .ok()
        .and_then(|value| {
            value
                .pointer("/usage/total_requests")
                .and_then(|count| count.as_i64())
        })
        .unwrap_or(0)
}

fn management_http_request(
    port: u16,
    management_key: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    if management_key.contains('\r') || management_key.contains('\n') {
        return Err("管理密钥包含非法换行字符".to_string());
    }

    let body = body.unwrap_or(&[]);
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {management_key}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");

    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|err| format!("连接管理接口失败: {err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|err| format!("设置读取超时失败: {err}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|err| format!("设置写入超时失败: {err}"))?;
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|err| format!("发送管理请求失败: {err}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|err| format!("读取管理响应失败: {err}"))?;
    parse_http_response(&response)
}

fn parse_http_response(response: &[u8]) -> Result<Vec<u8>, String> {
    let header_end = response
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .ok_or_else(|| "管理接口响应格式无效".to_string())?;
    let header_bytes = &response[..header_end];
    let header_text =
        std::str::from_utf8(header_bytes).map_err(|err| format!("解析响应头失败: {err}"))?;
    let status_line = header_text
        .lines()
        .next()
        .ok_or_else(|| "管理接口缺少响应状态".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "管理接口响应状态无效".to_string())?;
    let body = &response[(header_end + 4)..];
    let decoded_body = if header_text
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked_body(body)?
    } else {
        body.to_vec()
    };

    if (200..300).contains(&status) {
        return Ok(decoded_body);
    }

    let message = String::from_utf8_lossy(&decoded_body);
    Err(format!("管理接口请求失败: HTTP {status} {message}"))
}

fn decode_chunked_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut cursor = 0;

    loop {
        let line_end = body[cursor..]
            .windows(2)
            .position(|part| part == b"\r\n")
            .ok_or_else(|| "分块响应格式无效".to_string())?;
        let size_line = std::str::from_utf8(&body[cursor..(cursor + line_end)])
            .map_err(|err| format!("解析分块长度失败: {err}"))?;
        let size = usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|err| format!("解析分块长度失败: {err}"))?;
        cursor += line_end + 2;
        if size == 0 {
            break;
        }
        if cursor + size > body.len() {
            return Err("分块响应内容不完整".to_string());
        }
        output.extend_from_slice(&body[cursor..(cursor + size)]);
        cursor += size;
        if body.get(cursor..(cursor + 2)) == Some(b"\r\n") {
            cursor += 2;
        }
        if cursor >= body.len() {
            break;
        }
    }

    Ok(output)
}

fn bootstrap_default_runtime(app: &AppHandle) -> Result<(), String> {
    let dirs = AppDirs::new(app)?;
    let runtimes = list_runtimes(&dirs)?;
    if !runtimes.is_empty() {
        let mut stored = read_stored_state(&dirs)?;
        let active_exists = stored
            .active_version
            .as_deref()
            .is_some_and(|id| runtimes.iter().any(|runtime| runtime.id == id));
        if !active_exists {
            stored.active_version = Some(runtimes[0].id.clone());
            write_stored_state(&dirs, &stored)?;
        }
        return Ok(());
    }

    let default_package = default_package_name();
    let package_path = app
        .path()
        .resolve(&default_package, BaseDirectory::Resource)
        .map_err(|err| format!("定位内置版本包失败: {err}"))?;
    install_runtime_package(app, &package_path, true)?;
    Ok(())
}

fn build_desktop_state(
    app: &AppHandle,
    process: &State<'_, ProcessState>,
) -> Result<DesktopState, String> {
    let dirs = AppDirs::new(app)?;
    let stored = read_stored_state(&dirs)?;
    let runtimes = list_runtimes(&dirs)?;
    let active_version = stored.active_version.clone();
    let config = match active_version.as_deref() {
        Some(id) => {
            let runtime = runtime_by_id(&dirs, id)?;
            let config_path = ensure_workspace_config(&dirs, &runtime)?;
            let management_key = management_key_for_config(&stored, &config_path)?;
            Some(config_info(&config_path, management_key)?)
        }
        None => None,
    };
    let port = config.as_ref().and_then(|config| config.port);
    let mut pid = process_pid(process)?;
    if pid.is_none() {
        if let Some(active_id) = active_version.as_deref() {
            let runtime = runtime_by_id(&dirs, active_id)?;
            let config_path = ensure_workspace_config(&dirs, &runtime)?;
            pid = detect_managed_service_pid(&dirs, &stored, &config_path)?;
        }
    }

    Ok(DesktopState {
        app_data_dir: display_path(&dirs.app_data_dir),
        workspace_dir: display_path(&dirs.workspace_dir),
        auth_dir: display_path(&current_auth_dir(&dirs)?),
        active_version,
        runtimes,
        service: ServiceInfo {
            running: pid.is_some(),
            pid,
            port,
        },
        config,
    })
}

fn install_runtime_package(
    app: &AppHandle,
    package_path: &Path,
    activate: bool,
) -> Result<RuntimeInfo, String> {
    if !package_path.exists() {
        return Err(format!("版本包不存在: {}", display_path(package_path)));
    }

    let package = parse_package_info(package_path)?;
    let expected_targets = current_package_target_aliases();
    if !expected_targets
        .iter()
        .any(|target| target == &package.target)
    {
        return Err(format!(
            "版本包平台不匹配: 当前平台需要 {}, 但包是 {}",
            expected_targets.join(" 或 "),
            package.target
        ));
    }

    let dirs = AppDirs::new(app)?;
    fs::create_dir_all(&dirs.runtime_dir).map_err(|err| format!("创建运行时目录失败: {err}"))?;
    fs::create_dir_all(&dirs.staging_dir).map_err(|err| format!("创建临时目录失败: {err}"))?;

    let install_dir = dirs.runtime_dir.join(&package.id);
    let metadata_path = install_dir.join("metadata.json");
    let binary_path = runtime_binary_path(&install_dir);
    if binary_path.exists() {
        ensure_default_config_cache(&install_dir)?;
        let runtime = runtime_from_metadata(&install_dir, &metadata_path)?;
        if activate {
            let mut state = read_stored_state(&dirs)?;
            state.active_version = Some(runtime.id.clone());
            write_stored_state(&dirs, &state)?;
        }
        return Ok(runtime);
    }
    if install_dir.exists() {
        return Err(format!(
            "运行时目录已存在但不完整: {}",
            display_path(&install_dir)
        ));
    }

    let staging_dir = dirs
        .staging_dir
        .join(format!("{}-{}", package.id, unix_timestamp()?));
    fs::create_dir_all(&staging_dir).map_err(|err| format!("创建解包目录失败: {err}"))?;
    unpack_archive(package_path, &staging_dir)?;

    let staging_binary = runtime_binary_path(&staging_dir);
    if !staging_binary.exists() {
        return Err("版本包中缺少 cli-proxy-api 可执行文件".to_string());
    }

    set_executable(&staging_binary)?;
    ensure_default_config_cache(&staging_dir)?;
    let package_file = package_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(default_package_name);
    let metadata = RuntimeMetadata {
        id: package.id.clone(),
        version: package.version,
        target: package.target,
        installed_at: unix_timestamp()?,
        package_file,
    };
    write_json(&staging_dir.join("metadata.json"), &metadata)?;
    fs::rename(&staging_dir, &install_dir).map_err(|err| format!("安装运行时失败: {err}"))?;

    if activate {
        let mut state = read_stored_state(&dirs)?;
        state.active_version = Some(package.id);
        write_stored_state(&dirs, &state)?;
    }

    runtime_from_metadata(&install_dir, &metadata_path)
}

fn unpack_archive(package_path: &Path, target_dir: &Path) -> Result<(), String> {
    let file_name = package_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "版本包文件名无效".to_string())?;
    if file_name.ends_with(".zip") {
        return unpack_zip_archive(package_path, target_dir);
    }
    unpack_tar_gz_archive(package_path, target_dir)
}

fn unpack_tar_gz_archive(package_path: &Path, target_dir: &Path) -> Result<(), String> {
    let file = File::open(package_path).map_err(|err| format!("打开版本包失败: {err}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|err| format!("读取版本包失败: {err}"))?
    {
        let mut entry = entry.map_err(|err| format!("读取版本包条目失败: {err}"))?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err("版本包不能包含符号链接或硬链接".to_string());
        }
        if !entry_type.is_file() && !entry_type.is_dir() {
            continue;
        }

        let entry_path = entry
            .path()
            .map_err(|err| format!("读取版本包路径失败: {err}"))?;
        let relative_path = safe_relative_path(&entry_path)?;
        let output_path = target_dir.join(relative_path);
        if entry_type.is_dir() {
            fs::create_dir_all(&output_path).map_err(|err| format!("创建目录失败: {err}"))?;
        } else {
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).map_err(|err| format!("创建目录失败: {err}"))?;
            }
            entry
                .unpack(&output_path)
                .map_err(|err| format!("解包文件失败: {err}"))?;
        }
    }

    Ok(())
}

fn unpack_zip_archive(package_path: &Path, target_dir: &Path) -> Result<(), String> {
    let file = File::open(package_path).map_err(|err| format!("打开版本包失败: {err}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|err| format!("读取 zip 版本包失败: {err}"))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| format!("读取 zip 版本包条目失败: {err}"))?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("版本包不能包含符号链接".to_string());
        }

        let relative_path = safe_relative_path(Path::new(entry.name()))?;
        let output_path = target_dir.join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(|err| format!("创建目录失败: {err}"))?;
            continue;
        }
        if !entry.is_file() {
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建目录失败: {err}"))?;
        }
        let mut output =
            File::create(&output_path).map_err(|err| format!("创建解包文件失败: {err}"))?;
        io::copy(&mut entry, &mut output).map_err(|err| format!("解包 zip 文件失败: {err}"))?;
    }

    Ok(())
}

fn safe_relative_path(path: &Path) -> Result<PathBuf, String> {
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            _ => return Err(format!("版本包包含不安全路径: {}", display_path(path))),
        }
    }
    if safe.as_os_str().is_empty() {
        return Err("版本包包含空路径".to_string());
    }
    Ok(safe)
}

fn ensure_workspace_config(
    dirs: &AppDirs,
    runtime: &RuntimeInfoInternal,
) -> Result<PathBuf, String> {
    ensure_workspace_dir(dirs)?;
    let config_path = dirs.workspace_dir.join("config.yaml");
    if config_path.exists() {
        return Ok(config_path);
    }

    let source = default_config_path(runtime)?;
    fs::copy(&source, &config_path).map_err(|err| format!("初始化配置文件失败: {err}"))?;
    Ok(config_path)
}

fn ensure_workspace_dir(dirs: &AppDirs) -> Result<&PathBuf, String> {
    fs::create_dir_all(&dirs.workspace_dir).map_err(|err| format!("创建工作区失败: {err}"))?;
    Ok(&dirs.workspace_dir)
}

fn default_config_path(runtime: &RuntimeInfoInternal) -> Result<PathBuf, String> {
    ensure_default_config_cache(&runtime.path)
}

fn ensure_default_config_cache(runtime_path: &Path) -> Result<PathBuf, String> {
    let cache_path = runtime_path.join(DEFAULT_CONFIG_CACHE);
    if cache_path.exists() {
        return Ok(cache_path);
    }

    let source = packaged_default_config_path(runtime_path)?;
    fs::copy(&source, &cache_path).map_err(|err| format!("缓存默认配置失败: {err}"))?;
    Ok(cache_path)
}

fn packaged_default_config_path(runtime_path: &Path) -> Result<PathBuf, String> {
    let config_path = runtime_path.join("config.yaml");
    if config_path.exists() {
        return Ok(config_path);
    }
    let example_path = runtime_path.join("config.example.yaml");
    if example_path.exists() {
        return Ok(example_path);
    }
    Err("当前版本缺少默认配置文件".to_string())
}

fn read_default_config(runtime: &RuntimeInfoInternal) -> Result<String, String> {
    let path = default_config_path(runtime)?;
    fs::read_to_string(&path).map_err(|err| format!("读取默认配置失败: {err}"))
}

fn active_runtime(dirs: &AppDirs) -> Result<RuntimeInfoInternal, String> {
    let stored = read_stored_state(dirs)?;
    let active_id = stored
        .active_version
        .ok_or_else(|| "还没有可用的 CLIProxyAPI 版本".to_string())?;
    runtime_by_id(dirs, &active_id)
}

fn config_info(
    config_path: &Path,
    local_management_key: Option<String>,
) -> Result<ConfigFileInfo, String> {
    let content = fs::read_to_string(config_path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => "配置文件还没有初始化".to_string(),
        _ => format!("读取配置文件失败: {err}"),
    })?;
    let port = read_port_from_content(&content)?;
    let host = read_host_from_content(&content);
    let management_url = port.map(|port| {
        format!(
            "http://{}:{port}/management.html",
            browser_host(host.as_deref())
        )
    });

    Ok(ConfigFileInfo {
        path: display_path(config_path),
        content: clear_management_secret(&content),
        port,
        management_url,
        local_management_key,
    })
}

fn management_key_for_config(
    stored: &StoredState,
    config_path: &Path,
) -> Result<Option<String>, String> {
    if let Some(key) = stored.management_key.as_deref().and_then(non_empty_str) {
        return Ok(Some(key.to_string()));
    }

    let content = fs::read_to_string(config_path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => "配置文件还没有初始化".to_string(),
        _ => format!("读取配置文件失败: {err}"),
    })?;
    Ok(plain_management_secret_from_content(&content))
}

fn ensure_config_uses_local_management_key(config_path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(config_path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => "配置文件还没有初始化".to_string(),
        _ => format!("读取配置文件失败: {err}"),
    })?;
    let cleared = clear_management_secret(&content);
    if cleared != content {
        fs::write(config_path, cleared).map_err(|err| format!("写入配置文件失败: {err}"))?;
    }
    Ok(())
}

fn plain_management_secret_from_content(content: &str) -> Option<String> {
    let value = nested_yaml_scalar_value(content, "remote-management", "secret-key")?;
    if is_bcrypt_hash(&value) {
        return None;
    }
    non_empty_string(value)
}

fn clear_management_secret(content: &str) -> String {
    upsert_nested_yaml_scalar(content, "remote-management", "secret-key", "\"\"")
}

fn nested_yaml_scalar_value(content: &str, section: &str, key: &str) -> Option<String> {
    let lines = content.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        let Some((section_indent, section_key)) = yaml_key_line(line) else {
            continue;
        };
        if section_key != section {
            continue;
        }

        for child in lines.iter().skip(index + 1) {
            if child.trim().is_empty() || child.trim_start().starts_with('#') {
                continue;
            }
            let Some((child_indent, child_key)) = yaml_key_line(child) else {
                continue;
            };
            if child_indent <= section_indent {
                break;
            }
            if child_key == key {
                return yaml_scalar_value(child.trim(), key);
            }
        }
    }
    None
}

fn upsert_nested_yaml_scalar(content: &str, section: &str, key: &str, value: &str) -> String {
    let trailing_newline = content.ends_with('\n');
    let mut lines = content
        .replace("\r\n", "\n")
        .split('\n')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if trailing_newline && lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    for index in 0..lines.len() {
        if lines[index].trim_start().starts_with('#') {
            continue;
        }
        let Some((section_indent, section_key)) = yaml_key_line(&lines[index]) else {
            continue;
        };
        if section_key != section {
            continue;
        }

        let section_prefix = &lines[index][..section_indent];
        let child_prefix = format!("{section_prefix}  ");
        let replacement = format!("{child_prefix}{key}: {value}");

        for child_index in (index + 1)..lines.len() {
            let line = &lines[child_index];
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let Some((child_indent, child_key)) = yaml_key_line(line) else {
                continue;
            };
            if child_indent <= section_indent {
                lines.insert(child_index, replacement);
                return finish_lines(lines, trailing_newline);
            }
            if child_key == key {
                lines[child_index] = replacement;
                return finish_lines(lines, trailing_newline);
            }
        }

        lines.push(replacement);
        return finish_lines(lines, trailing_newline);
    }

    finish_lines(lines, trailing_newline)
}

fn yaml_key_line(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start().len();
    let trimmed = line.trim_start();
    let (key, _) = trimmed.split_once(':')?;
    Some((indent, key.trim()))
}

fn finish_lines(lines: Vec<String>, trailing_newline: bool) -> String {
    let mut content = lines.join("\n");
    if trailing_newline {
        content.push('\n');
    }
    content
}

fn is_bcrypt_hash(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("$2a$")
        || value.starts_with("$2b$")
        || value.starts_with("$2x$")
        || value.starts_with("$2y$")
}

fn non_empty_string(value: String) -> Option<String> {
    non_empty_str(&value).map(ToString::to_string)
}

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn read_port_from_content(content: &str) -> Result<Option<u16>, String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = yaml_scalar_value(trimmed, "port") {
            let port = value
                .trim()
                .parse::<u16>()
                .map_err(|err| format!("解析端口失败: {err}"))?;
            return Ok(Some(port));
        }
    }

    Ok(None)
}

fn read_host_from_content(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = yaml_scalar_value(trimmed, "host") {
            return Some(value);
        }
    }
    None
}

fn read_auth_dir_from_content(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = yaml_scalar_value(trimmed, "auth-dir") {
            return non_empty_string(value);
        }
    }
    None
}

fn yaml_scalar_value(line: &str, key: &str) -> Option<String> {
    let value = line.strip_prefix(&format!("{key}:"))?;
    Some(
        strip_inline_comment(value)
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string(),
    )
}

fn strip_inline_comment(value: &str) -> &str {
    value.split_once('#').map_or(value, |(left, _)| left)
}

fn browser_host(host: Option<&str>) -> String {
    let host = host.unwrap_or("").trim();
    if host.is_empty() || host == "0.0.0.0" || host == "::" || host == "[::]" || host == "::0" {
        return "127.0.0.1".to_string();
    }
    if host.contains(':') && !host.starts_with('[') {
        return format!("[{host}]");
    }
    host.to_string()
}

fn current_auth_dir(dirs: &AppDirs) -> Result<PathBuf, String> {
    let config_path = match active_runtime(dirs) {
        Ok(runtime) => ensure_workspace_config(dirs, &runtime)?,
        Err(_) => dirs.workspace_dir.join("config.yaml"),
    };

    let configured = if config_path.exists() {
        let content = fs::read_to_string(&config_path).map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => "配置文件还没有初始化".to_string(),
            _ => format!("读取配置文件失败: {err}"),
        })?;
        read_auth_dir_from_content(&content).unwrap_or_else(|| DEFAULT_AUTH_DIR.to_string())
    } else {
        DEFAULT_AUTH_DIR.to_string()
    };

    resolve_auth_dir(dirs, &configured)
}

fn resolve_auth_dir(dirs: &AppDirs, auth_dir: &str) -> Result<PathBuf, String> {
    let trimmed = auth_dir.trim();
    let value = if trimmed.is_empty() {
        DEFAULT_AUTH_DIR
    } else {
        trimmed
    };

    if value == "~" || value.starts_with("~/") || value.starts_with("~\\") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "无法定位用户主目录".to_string())?;
        let remainder = value
            .trim_start_matches('~')
            .trim_start_matches(['/', '\\'])
            .replace('\\', "/");
        if remainder.is_empty() {
            return Ok(home);
        }
        return Ok(home.join(Path::new(&remainder)));
    }

    let normalized = value.replace('\\', "/");
    let path = PathBuf::from(normalized);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(dirs.workspace_dir.join(path))
    }
}

fn export_auth_archive_file(auth_dir: &Path, archive_path: &Path) -> Result<(), String> {
    if !auth_dir.exists() {
        return Err("认证文件目录不存在".to_string());
    }
    if !auth_dir.is_dir() {
        return Err("认证文件路径不是目录".to_string());
    }

    let mut files = Vec::new();
    collect_json_files(auth_dir, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err("认证文件目录中没有可导出的 JSON 文件".to_string());
    }

    if let Some(parent) = archive_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| format!("创建导出目录失败: {err}"))?;
        }
    }

    let file = File::create(archive_path).map_err(|err| format!("创建认证压缩包失败: {err}"))?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);

    for file_path in files {
        let relative = file_path
            .strip_prefix(auth_dir)
            .map_err(|err| format!("解析认证文件路径失败: {err}"))?;
        builder
            .append_path_with_name(&file_path, relative)
            .map_err(|err| format!("写入认证压缩包失败: {err}"))?;
    }

    builder
        .finish()
        .map_err(|err| format!("完成认证压缩包失败: {err}"))?;
    let encoder = builder
        .into_inner()
        .map_err(|err| format!("完成认证压缩包失败: {err}"))?;
    encoder
        .finish()
        .map_err(|err| format!("写入认证压缩包失败: {err}"))?;

    Ok(())
}

fn import_auth_archive_file(
    dirs: &AppDirs,
    archive_path: &Path,
    auth_dir: &Path,
) -> Result<(), String> {
    if !archive_path.exists() {
        return Err(format!("认证压缩包不存在: {}", display_path(archive_path)));
    }

    fs::create_dir_all(auth_dir).map_err(|err| format!("创建认证文件目录失败: {err}"))?;
    fs::create_dir_all(&dirs.staging_dir).map_err(|err| format!("创建临时目录失败: {err}"))?;
    let staging_dir = dirs
        .staging_dir
        .join(format!("auth-import-{}", unix_timestamp()?));
    fs::create_dir_all(&staging_dir).map_err(|err| format!("创建导入临时目录失败: {err}"))?;

    let result =
        extract_auth_archive_to_staging(archive_path, &staging_dir).and_then(|relative_paths| {
            copy_imported_auth_files(&staging_dir, auth_dir, relative_paths)
        });
    let _ = fs::remove_dir_all(&staging_dir);
    result
}

fn extract_auth_archive_to_staging(
    archive_path: &Path,
    staging_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let file = File::open(archive_path).map_err(|err| format!("打开认证压缩包失败: {err}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let mut imported = Vec::new();

    for entry in archive
        .entries()
        .map_err(|err| format!("读取认证压缩包失败: {err}"))?
    {
        let mut entry = entry.map_err(|err| format!("读取认证压缩包条目失败: {err}"))?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err("认证压缩包不能包含符号链接或硬链接".to_string());
        }
        if !entry_type.is_file() {
            continue;
        }

        let entry_path = entry
            .path()
            .map_err(|err| format!("读取认证压缩包路径失败: {err}"))?;
        let relative_path = safe_relative_path(&entry_path)?;
        if !is_json_path(&relative_path) {
            continue;
        }

        let output_path = staging_dir.join(&relative_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建认证导入目录失败: {err}"))?;
        }
        entry
            .unpack(&output_path)
            .map_err(|err| format!("解包认证文件失败: {err}"))?;
        imported.push(relative_path);
    }

    if imported.is_empty() {
        return Err("认证压缩包中没有可导入的 JSON 文件".to_string());
    }
    Ok(imported)
}

fn copy_imported_auth_files(
    staging_dir: &Path,
    auth_dir: &Path,
    relative_paths: Vec<PathBuf>,
) -> Result<(), String> {
    for relative_path in relative_paths {
        let source = staging_dir.join(&relative_path);
        let target = auth_dir.join(&relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建认证文件目录失败: {err}"))?;
        }
        fs::copy(&source, &target).map_err(|err| format!("导入认证文件失败: {err}"))?;
        set_private_file_permissions(&target)?;
    }
    Ok(())
}

fn collect_json_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|err| format!("读取认证文件目录失败: {err}"))?
    {
        let entry = entry.map_err(|err| format!("读取认证文件条目失败: {err}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("读取认证文件类型失败: {err}"))?;
        if file_type.is_dir() {
            collect_json_files(&path, files)?;
        } else if file_type.is_file() && is_json_path(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_json_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|err| format!("读取认证文件权限失败: {err}"))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).map_err(|err| format!("设置认证文件权限失败: {err}"))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn reject_unmanaged_port_listener(dirs: &AppDirs, config_path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(config_path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => "配置文件还没有初始化".to_string(),
        _ => format!("读取配置文件失败: {err}"),
    })?;
    let Some(port) = read_port_from_content(&content)? else {
        return Ok(());
    };
    let Some(pid) = listener_pid_on_port(port)? else {
        return Ok(());
    };
    if pid_matches_managed_runtime(dirs, pid) {
        return Ok(());
    }
    Err(format!("端口 {port} 已被其他进程 PID {pid} 占用"))
}

fn detect_managed_service_pid(
    dirs: &AppDirs,
    stored: &StoredState,
    config_path: &Path,
) -> Result<Option<u32>, String> {
    if let Some(pid) = stored.managed_pid {
        if pid_is_running(pid) && pid_matches_managed_runtime(dirs, pid) {
            return Ok(Some(pid));
        }
    }

    let content = fs::read_to_string(config_path).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => "配置文件还没有初始化".to_string(),
        _ => format!("读取配置文件失败: {err}"),
    })?;
    let Some(port) = read_port_from_content(&content)? else {
        return Ok(None);
    };
    let Some(pid) = listener_pid_on_port(port)? else {
        return Ok(None);
    };
    if pid_matches_managed_runtime(dirs, pid) {
        return Ok(Some(pid));
    }
    Ok(None)
}

fn terminate_managed_pid(dirs: &AppDirs, pid: u32) -> Result<(), String> {
    if pid == std::process::id() {
        return Err("拒绝停止桌面应用自身进程".to_string());
    }
    if !pid_is_running(pid) {
        return Ok(());
    }
    if !pid_matches_managed_runtime(dirs, pid) {
        return Err(format!("拒绝停止非本应用托管的进程 PID {pid}"));
    }
    terminate_pid(pid)
}

fn service_pid_for_state(
    app: &AppHandle,
    process: &State<'_, ProcessState>,
) -> Result<Option<u32>, String> {
    if let Some(pid) = process_pid(process)? {
        return Ok(Some(pid));
    }

    let dirs = AppDirs::new(app)?;
    let stored = read_stored_state(&dirs)?;
    let Some(active_id) = stored.active_version.as_deref() else {
        return Ok(None);
    };
    let runtime = runtime_by_id(&dirs, active_id)?;
    let config_path = ensure_workspace_config(&dirs, &runtime)?;
    detect_managed_service_pid(&dirs, &stored, &config_path)
}

#[cfg(unix)]
fn listener_pid_on_port(port: u16) -> Result<Option<u32>, String> {
    let output = Command::new("lsof")
        .arg("-nP")
        .arg(format!("-iTCP:{port}"))
        .arg("-sTCP:LISTEN")
        .arg("-Fp")
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("检查端口监听失败: {err}")),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(pid) = line.strip_prefix('p').and_then(|value| value.parse().ok()) {
            return Ok(Some(pid));
        }
    }
    Ok(None)
}

#[cfg(not(unix))]
fn listener_pid_on_port(_port: u16) -> Result<Option<u32>, String> {
    Ok(None)
}

#[cfg(unix)]
fn pid_command(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("command=")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(not(unix))]
fn pid_command(_pid: u32) -> Option<String> {
    None
}

fn pid_matches_managed_runtime(dirs: &AppDirs, pid: u32) -> bool {
    let Some(command) = pid_command(pid) else {
        return false;
    };
    let runtime_dir = display_path(&dirs.runtime_dir);
    let workspace_config = display_path(&dirs.workspace_dir.join("config.yaml"));
    command.contains(BINARY_BASENAME)
        && (command.contains(&runtime_dir) || command.contains(&workspace_config))
}

#[cfg(unix)]
fn pid_is_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(unix))]
fn pid_is_running(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn terminate_pid(pid: u32) -> Result<(), String> {
    if !pid_is_running(pid) {
        return Ok(());
    }
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .map_err(|err| format!("停止 CLIProxyAPI 失败: {err}"))?;
    if !status.success() {
        return Err(format!("停止 CLIProxyAPI 失败: kill -TERM {pid}"));
    }
    for _ in 0..20 {
        if !pid_is_running(pid) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    let status = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status()
        .map_err(|err| format!("强制停止 CLIProxyAPI 失败: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("强制停止 CLIProxyAPI 失败: kill -KILL {pid}"))
    }
}

#[cfg(not(unix))]
fn terminate_pid(_pid: u32) -> Result<(), String> {
    Ok(())
}

fn process_pid(process: &State<'_, ProcessState>) -> Result<Option<u32>, String> {
    let mut guard = process
        .0
        .lock()
        .map_err(|_| "服务状态锁已损坏".to_string())?;

    let Some(child) = guard.as_mut() else {
        return Ok(None);
    };

    match child
        .try_wait()
        .map_err(|err| format!("读取服务状态失败: {err}"))?
    {
        Some(_) => {
            *guard = None;
            Ok(None)
        }
        None => Ok(Some(child.id())),
    }
}

fn runtime_by_id(dirs: &AppDirs, id: &str) -> Result<RuntimeInfoInternal, String> {
    let path = dirs.runtime_dir.join(id);
    runtime_metadata(&path)?;
    let binary_path = runtime_binary_path(&path);
    Ok(RuntimeInfoInternal { path, binary_path })
}

fn list_runtimes(dirs: &AppDirs) -> Result<Vec<RuntimeInfo>, String> {
    fs::create_dir_all(&dirs.runtime_dir).map_err(|err| format!("创建运行时目录失败: {err}"))?;
    let mut runtimes = Vec::new();

    for entry in
        fs::read_dir(&dirs.runtime_dir).map_err(|err| format!("读取运行时目录失败: {err}"))?
    {
        let entry = entry.map_err(|err| format!("读取运行时条目失败: {err}"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(runtime) = runtime_from_metadata(&path, &path.join("metadata.json")) {
            runtimes.push(runtime);
        }
    }

    runtimes.sort_by(|left, right| right.installed_at.cmp(&left.installed_at));
    Ok(runtimes)
}

fn runtime_from_metadata(path: &Path, metadata_path: &Path) -> Result<RuntimeInfo, String> {
    let metadata = runtime_metadata(path)?;
    let binary_path = runtime_binary_path(path);
    if !binary_path.exists() {
        return Err(format!(
            "运行时缺少可执行文件: {}",
            display_path(&binary_path)
        ));
    }
    if !metadata_path.exists() {
        return Err(format!("运行时缺少元数据: {}", display_path(metadata_path)));
    }

    Ok(RuntimeInfo {
        id: metadata.id,
        version: metadata.version,
        target: metadata.target,
        path: display_path(path),
        binary_path: display_path(&binary_path),
        installed_at: metadata.installed_at,
        package_file: metadata.package_file,
    })
}

fn runtime_metadata(path: &Path) -> Result<RuntimeMetadata, String> {
    let metadata_path = path.join("metadata.json");
    let content =
        fs::read_to_string(&metadata_path).map_err(|err| format!("读取版本元数据失败: {err}"))?;
    serde_json::from_str(&content).map_err(|err| format!("解析版本元数据失败: {err}"))
}

fn read_stored_state(dirs: &AppDirs) -> Result<StoredState, String> {
    let content = match fs::read_to_string(&dirs.state_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(StoredState::default()),
        Err(err) => return Err(format!("读取状态文件失败: {err}")),
    };
    serde_json::from_str(&content).map_err(|err| format!("解析状态文件失败: {err}"))
}

fn write_stored_state(dirs: &AppDirs, state: &StoredState) -> Result<(), String> {
    fs::create_dir_all(&dirs.app_data_dir).map_err(|err| format!("创建数据目录失败: {err}"))?;
    write_json(&dirs.state_path, state)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(value).map_err(|err| format!("序列化 JSON 失败: {err}"))?;
    fs::write(path, content).map_err(|err| format!("写入文件失败: {err}"))
}

fn parse_package_info(path: &Path) -> Result<PackageInfo, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "版本包文件名无效".to_string())?;
    let base_name = file_name
        .strip_suffix(".tar.gz")
        .or_else(|| file_name.strip_suffix(".tgz"))
        .or_else(|| file_name.strip_suffix(".zip"))
        .ok_or_else(|| "版本包必须是 .tar.gz、.tgz 或 .zip 文件".to_string())?;
    let descriptor = base_name
        .strip_prefix("CLIProxyAPI_")
        .ok_or_else(|| "版本包命名需匹配 CLIProxyAPI_<version>_<os>_<arch>".to_string())?;
    let parts = descriptor.split('_').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err("版本包命名需匹配 CLIProxyAPI_<version>_<os>_<arch>".to_string());
    }
    let version = parts[0].trim();
    let target_os = parts[1].trim();
    let target_arch = parts[2].trim();
    if version.is_empty() || target_os.is_empty() || target_arch.is_empty() {
        return Err("版本包文件名包含空版本或平台信息".to_string());
    }

    Ok(PackageInfo {
        id: format!("{version}_{target_os}_{target_arch}"),
        version: version.to_string(),
        target: format!("{target_os}_{target_arch}"),
    })
}

fn binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "cli-proxy-api.exe"
    } else {
        BINARY_BASENAME
    }
}

fn runtime_binary_path(runtime_path: &Path) -> PathBuf {
    runtime_path.join(binary_name())
}

fn default_package_name() -> String {
    format!(
        "CLIProxyAPI_{DEFAULT_RUNTIME_VERSION}_{}.tar.gz",
        current_package_target()
    )
}

fn current_package_target() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    format!("{os}_{arch}")
}

fn current_package_target_aliases() -> Vec<String> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let mut targets = vec![current_package_target()];
    if std::env::consts::ARCH == "aarch64" {
        let aarch64_target = format!("{os}_aarch64");
        if !targets.iter().any(|target| target == &aarch64_target) {
            targets.push(aarch64_target);
        }
    }
    targets
}

fn unix_timestamp() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|err| format!("读取系统时间失败: {err}"))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|err| format!("读取可执行文件权限失败: {err}"))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|err| format!("设置可执行权限失败: {err}"))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

struct AppDirs {
    app_data_dir: PathBuf,
    runtime_dir: PathBuf,
    staging_dir: PathBuf,
    workspace_dir: PathBuf,
    state_path: PathBuf,
}

impl AppDirs {
    fn new(app: &AppHandle) -> Result<Self, String> {
        let app_data_dir = app
            .path()
            .app_data_dir()
            .map_err(|err| format!("定位应用数据目录失败: {err}"))?;
        Ok(Self {
            runtime_dir: app_data_dir.join("runtimes"),
            staging_dir: app_data_dir.join("staging"),
            workspace_dir: app_data_dir.join("workspace"),
            state_path: app_data_dir.join("state.json"),
            app_data_dir,
        })
    }
}

struct RuntimeInfoInternal {
    path: PathBuf,
    binary_path: PathBuf,
}
