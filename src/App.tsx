import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { confirm as confirmDialog, open, save } from "@tauri-apps/plugin-dialog";
import {
  CheckCircle2,
  Download,
  ExternalLink,
  FolderOpen,
  Languages,
  PackagePlus,
  Play,
  RefreshCw,
  RotateCcw,
  Save,
  Square,
  Trash2,
  Undo2,
  Upload,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

type RuntimeInfo = {
  id: string;
  version: string;
  target: string;
  path: string;
  binaryPath: string;
  installedAt: number;
  packageFile: string;
};

type ConfigFileInfo = {
  path: string;
  content: string;
  port?: number;
  managementUrl?: string;
  localManagementKey?: string;
};

type ServiceInfo = {
  running: boolean;
  pid?: number;
  port?: number;
};

type DesktopState = {
  appDataDir: string;
  workspaceDir: string;
  authDir: string;
  activeVersion?: string;
  runtimes: RuntimeInfo[];
  service: ServiceInfo;
  config?: ConfigFileInfo | null;
};

const LANGUAGE_STORAGE_KEY = "cliproxyapi.desktop.language";

const LANGUAGE_OPTIONS = [
  { code: "zh-CN", label: "中文", locale: "zh-CN" },
  { code: "zh-TW", label: "繁體中文（台灣）", locale: "zh-TW" },
  { code: "en", label: "English", locale: "en-US" },
  { code: "ru", label: "Русский", locale: "ru-RU" },
] as const;

type LanguageCode = (typeof LANGUAGE_OPTIONS)[number]["code"];
type BusyKey =
  | "idle"
  | "refresh"
  | "install"
  | "activate"
  | "start"
  | "stop"
  | "shutdown"
  | "open"
  | "openAuth"
  | "exportAuth"
  | "importAuth"
  | "save"
  | "browser"
  | "restore"
  | "delete";

type Translation = {
  appSubtitle: string;
  language: string;
  refresh: string;
  workspace: string;
  importVersionPackage: string;
  configFile: string;
  uninitialized: string;
  reload: string;
  restoreDefault: string;
  restoreDefaultTitle: string;
  save: string;
  webPort: string;
  localManagementKey: string;
  openManagementPage: string;
  noManagementUrl: string;
  service: string;
  notRunning: string;
  running: string;
  stopped: string;
  currentVersion: string;
  targetPlatform: string;
  notInstalled: string;
  start: string;
  stop: string;
  paths: string;
  appData: string;
  workspaceDir: string;
  authFiles: string;
  open: string;
  export: string;
  import: string;
  currentBinary: string;
  installedVersions: string;
  version: string;
  platform: string;
  installedAt: string;
  packageFile: string;
  status: string;
  actions: string;
  current: string;
  setCurrent: string;
  cannotDelete: string;
  delete: string;
  emptyVersions: string;
  cliPackageFilter: string;
  authArchiveFilter: string;
  restoreUnavailable: string;
  restoreStopFirst: string;
  restoreVersionFallback: string;
  restoreFirstTitle: string;
  restoreFirstMessage: string;
  restoreSecondTitle: string;
  restoreSecondMessage: (runtimeLabel: string) => string;
  dialogOpenFailed: (message: string) => string;
  deleteVersionTitle: string;
  deleteVersionMessage: (runtimeLabel: string) => string;
  importAuthTitle: string;
  importAuthMessage: string;
  closeTitle: string;
  closeMessage: string;
  closeSecondTitle: string;
  closeSecondMessage: string;
  closeFailed: (message: string) => string;
  commands: Record<BusyKey, string>;
};

const TRANSLATIONS: Record<LanguageCode, Translation> = {
  "zh-CN": {
    appSubtitle: "版本运行时与本地服务控制台",
    language: "语言",
    refresh: "刷新",
    workspace: "工作区",
    importVersionPackage: "导入版本包",
    configFile: "配置文件",
    uninitialized: "未初始化",
    reload: "重载",
    restoreDefault: "恢复默认",
    restoreDefaultTitle: "恢复当前版本默认配置",
    save: "保存",
    webPort: "Web 端口",
    localManagementKey: "本机管理密钥",
    openManagementPage: "打开管理页",
    noManagementUrl: "暂无管理页地址",
    service: "服务",
    notRunning: "未运行",
    running: "运行中",
    stopped: "已停止",
    currentVersion: "当前版本",
    targetPlatform: "目标平台",
    notInstalled: "未安装",
    start: "启动",
    stop: "停止",
    paths: "路径",
    appData: "应用数据",
    workspaceDir: "运行工作区",
    authFiles: "认证文件",
    open: "打开",
    export: "导出",
    import: "导入",
    currentBinary: "当前二进制",
    installedVersions: "已安装版本",
    version: "版本",
    platform: "平台",
    installedAt: "导入时间",
    packageFile: "包文件",
    status: "状态",
    actions: "操作",
    current: "当前",
    setCurrent: "设为当前",
    cannotDelete: "不可删除",
    delete: "删除",
    emptyVersions: "暂无版本",
    cliPackageFilter: "CLIProxyAPI tar.gz",
    authArchiveFilter: "认证压缩包",
    restoreUnavailable: "还没有可恢复默认配置的当前版本",
    restoreStopFirst: "请先停止服务，再恢复默认配置",
    restoreVersionFallback: "当前版本",
    restoreFirstTitle: "恢复默认配置",
    restoreFirstMessage: "恢复默认配置会覆盖当前工作区的 config.yaml，是否继续？",
    restoreSecondTitle: "再次确认",
    restoreSecondMessage: (runtimeLabel) => `再次确认：将使用 ${runtimeLabel} 的默认配置覆盖当前配置文件。`,
    dialogOpenFailed: (message) => `打开确认弹窗失败: ${message}`,
    deleteVersionTitle: "删除版本",
    deleteVersionMessage: (runtimeLabel) => `删除版本 ${runtimeLabel}？`,
    importAuthTitle: "导入认证文件",
    importAuthMessage: "导入认证压缩包会覆盖同名 JSON 认证文件，是否继续？",
    closeTitle: "关闭应用",
    closeMessage: "关闭应用前会先停止 CLIProxyAPI 服务，是否继续？",
    closeSecondTitle: "再次确认关闭",
    closeSecondMessage: "再次确认：关闭后正在运行的 CLIProxyAPI 服务会被停止。",
    closeFailed: (message) => `关闭前停止服务失败: ${message}`,
    commands: {
      idle: "",
      refresh: "刷新中",
      install: "导入中",
      activate: "切换中",
      start: "启动中",
      stop: "停止中",
      shutdown: "正在关闭服务",
      open: "打开中",
      openAuth: "打开认证目录中",
      exportAuth: "导出认证中",
      importAuth: "导入认证中",
      save: "保存中",
      browser: "打开浏览器中",
      restore: "恢复中",
      delete: "删除中",
    },
  },
  "zh-TW": {
    appSubtitle: "版本執行環境與本機服務控制台",
    language: "語言",
    refresh: "重新整理",
    workspace: "工作區",
    importVersionPackage: "匯入版本包",
    configFile: "設定檔",
    uninitialized: "尚未初始化",
    reload: "重新載入",
    restoreDefault: "還原預設",
    restoreDefaultTitle: "還原目前版本預設設定",
    save: "儲存",
    webPort: "Web 連接埠",
    localManagementKey: "本機管理金鑰",
    openManagementPage: "開啟管理頁",
    noManagementUrl: "尚無管理頁位址",
    service: "服務",
    notRunning: "未執行",
    running: "執行中",
    stopped: "已停止",
    currentVersion: "目前版本",
    targetPlatform: "目標平台",
    notInstalled: "未安裝",
    start: "啟動",
    stop: "停止",
    paths: "路徑",
    appData: "應用資料",
    workspaceDir: "執行工作區",
    authFiles: "認證檔案",
    open: "開啟",
    export: "匯出",
    import: "匯入",
    currentBinary: "目前二進位檔",
    installedVersions: "已安裝版本",
    version: "版本",
    platform: "平台",
    installedAt: "匯入時間",
    packageFile: "包檔案",
    status: "狀態",
    actions: "操作",
    current: "目前",
    setCurrent: "設為目前",
    cannotDelete: "不可刪除",
    delete: "刪除",
    emptyVersions: "尚無版本",
    cliPackageFilter: "CLIProxyAPI tar.gz",
    authArchiveFilter: "認證壓縮包",
    restoreUnavailable: "尚無可還原預設設定的目前版本",
    restoreStopFirst: "請先停止服務，再還原預設設定",
    restoreVersionFallback: "目前版本",
    restoreFirstTitle: "還原預設設定",
    restoreFirstMessage: "還原預設設定會覆蓋目前工作區的 config.yaml，是否繼續？",
    restoreSecondTitle: "再次確認",
    restoreSecondMessage: (runtimeLabel) => `再次確認：將使用 ${runtimeLabel} 的預設設定覆蓋目前設定檔。`,
    dialogOpenFailed: (message) => `開啟確認視窗失敗: ${message}`,
    deleteVersionTitle: "刪除版本",
    deleteVersionMessage: (runtimeLabel) => `刪除版本 ${runtimeLabel}？`,
    importAuthTitle: "匯入認證檔案",
    importAuthMessage: "匯入認證壓縮包會覆蓋同名 JSON 認證檔案，是否繼續？",
    closeTitle: "關閉應用程式",
    closeMessage: "關閉應用程式前會先停止 CLIProxyAPI 服務，是否繼續？",
    closeSecondTitle: "再次確認關閉",
    closeSecondMessage: "再次確認：關閉後正在執行的 CLIProxyAPI 服務會被停止。",
    closeFailed: (message) => `關閉前停止服務失敗: ${message}`,
    commands: {
      idle: "",
      refresh: "重新整理中",
      install: "匯入中",
      activate: "切換中",
      start: "啟動中",
      stop: "停止中",
      shutdown: "正在關閉服務",
      open: "開啟中",
      openAuth: "開啟認證目錄中",
      exportAuth: "匯出認證中",
      importAuth: "匯入認證中",
      save: "儲存中",
      browser: "開啟瀏覽器中",
      restore: "還原中",
      delete: "刪除中",
    },
  },
  en: {
    appSubtitle: "Runtime versions and local service control",
    language: "Language",
    refresh: "Refresh",
    workspace: "Workspace",
    importVersionPackage: "Import Version",
    configFile: "Config File",
    uninitialized: "Not initialized",
    reload: "Reload",
    restoreDefault: "Restore Default",
    restoreDefaultTitle: "Restore the default config for the current version",
    save: "Save",
    webPort: "Web Port",
    localManagementKey: "Local Management Key",
    openManagementPage: "Open management page",
    noManagementUrl: "No management URL",
    service: "Service",
    notRunning: "Not running",
    running: "Running",
    stopped: "Stopped",
    currentVersion: "Current Version",
    targetPlatform: "Target Platform",
    notInstalled: "Not installed",
    start: "Start",
    stop: "Stop",
    paths: "Paths",
    appData: "App Data",
    workspaceDir: "Runtime Workspace",
    authFiles: "Auth Files",
    open: "Open",
    export: "Export",
    import: "Import",
    currentBinary: "Current Binary",
    installedVersions: "Installed Versions",
    version: "Version",
    platform: "Platform",
    installedAt: "Imported At",
    packageFile: "Package",
    status: "Status",
    actions: "Actions",
    current: "Current",
    setCurrent: "Set Current",
    cannotDelete: "Locked",
    delete: "Delete",
    emptyVersions: "No versions installed",
    cliPackageFilter: "CLIProxyAPI tar.gz",
    authArchiveFilter: "Auth archive",
    restoreUnavailable: "There is no current version to restore from",
    restoreStopFirst: "Stop the service before restoring the default config",
    restoreVersionFallback: "Current version",
    restoreFirstTitle: "Restore Default Config",
    restoreFirstMessage: "Restoring defaults will overwrite config.yaml in the workspace. Continue?",
    restoreSecondTitle: "Confirm Again",
    restoreSecondMessage: (runtimeLabel) => `Confirm again: the default config from ${runtimeLabel} will overwrite the current config file.`,
    dialogOpenFailed: (message) => `Failed to open confirmation dialog: ${message}`,
    deleteVersionTitle: "Delete Version",
    deleteVersionMessage: (runtimeLabel) => `Delete version ${runtimeLabel}?`,
    importAuthTitle: "Import Auth Files",
    importAuthMessage: "Importing the auth archive will overwrite JSON auth files with the same names. Continue?",
    closeTitle: "Close App",
    closeMessage: "CLIProxyAPI service will be stopped before the app closes. Continue?",
    closeSecondTitle: "Confirm Close",
    closeSecondMessage: "Confirm again: any running CLIProxyAPI service will be stopped after closing.",
    closeFailed: (message) => `Failed to stop service before closing: ${message}`,
    commands: {
      idle: "",
      refresh: "Refreshing",
      install: "Importing",
      activate: "Switching",
      start: "Starting",
      stop: "Stopping",
      shutdown: "Stopping service",
      open: "Opening",
      openAuth: "Opening auth folder",
      exportAuth: "Exporting auth files",
      importAuth: "Importing auth files",
      save: "Saving",
      browser: "Opening browser",
      restore: "Restoring",
      delete: "Deleting",
    },
  },
  ru: {
    appSubtitle: "Версии среды выполнения и управление локальным сервисом",
    language: "Язык",
    refresh: "Обновить",
    workspace: "Рабочая папка",
    importVersionPackage: "Импорт версии",
    configFile: "Файл конфигурации",
    uninitialized: "Не инициализировано",
    reload: "Перезагрузить",
    restoreDefault: "Сбросить",
    restoreDefaultTitle: "Восстановить конфигурацию текущей версии",
    save: "Сохранить",
    webPort: "Web-порт",
    localManagementKey: "Локальный ключ управления",
    openManagementPage: "Открыть страницу управления",
    noManagementUrl: "Адрес управления отсутствует",
    service: "Сервис",
    notRunning: "Не запущен",
    running: "Запущен",
    stopped: "Остановлен",
    currentVersion: "Текущая версия",
    targetPlatform: "Платформа",
    notInstalled: "Не установлено",
    start: "Запустить",
    stop: "Остановить",
    paths: "Пути",
    appData: "Данные приложения",
    workspaceDir: "Рабочая папка",
    authFiles: "Файлы авторизации",
    open: "Открыть",
    export: "Экспорт",
    import: "Импорт",
    currentBinary: "Текущий бинарный файл",
    installedVersions: "Установленные версии",
    version: "Версия",
    platform: "Платформа",
    installedAt: "Время импорта",
    packageFile: "Пакет",
    status: "Статус",
    actions: "Действия",
    current: "Текущая",
    setCurrent: "Сделать текущей",
    cannotDelete: "Нельзя удалить",
    delete: "Удалить",
    emptyVersions: "Версий пока нет",
    cliPackageFilter: "CLIProxyAPI tar.gz",
    authArchiveFilter: "Архив авторизации",
    restoreUnavailable: "Нет текущей версии для восстановления конфигурации",
    restoreStopFirst: "Сначала остановите сервис, затем восстановите конфигурацию",
    restoreVersionFallback: "Текущая версия",
    restoreFirstTitle: "Сброс конфигурации",
    restoreFirstMessage: "Сброс перезапишет config.yaml в рабочей папке. Продолжить?",
    restoreSecondTitle: "Повторное подтверждение",
    restoreSecondMessage: (runtimeLabel) => `Подтвердите еще раз: конфигурация по умолчанию из ${runtimeLabel} перезапишет текущий файл.`,
    dialogOpenFailed: (message) => `Не удалось открыть окно подтверждения: ${message}`,
    deleteVersionTitle: "Удалить версию",
    deleteVersionMessage: (runtimeLabel) => `Удалить версию ${runtimeLabel}?`,
    importAuthTitle: "Импорт файлов авторизации",
    importAuthMessage: "Импорт архива перезапишет JSON-файлы авторизации с такими же именами. Продолжить?",
    closeTitle: "Закрыть приложение",
    closeMessage: "Перед закрытием приложения сервис CLIProxyAPI будет остановлен. Продолжить?",
    closeSecondTitle: "Подтвердите закрытие",
    closeSecondMessage: "Подтвердите еще раз: после закрытия запущенный сервис CLIProxyAPI будет остановлен.",
    closeFailed: (message) => `Не удалось остановить сервис перед закрытием: ${message}`,
    commands: {
      idle: "",
      refresh: "Обновление",
      install: "Импорт",
      activate: "Переключение",
      start: "Запуск",
      stop: "Остановка",
      shutdown: "Остановка сервиса",
      open: "Открытие",
      openAuth: "Открытие папки авторизации",
      exportAuth: "Экспорт авторизации",
      importAuth: "Импорт авторизации",
      save: "Сохранение",
      browser: "Открытие браузера",
      restore: "Сброс",
      delete: "Удаление",
    },
  },
};

export function App() {
  const [state, setState] = useState<DesktopState | null>(null);
  const [busy, setBusy] = useState<BusyKey>("idle");
  const [error, setError] = useState<string | null>(null);
  const [configDraft, setConfigDraft] = useState("");
  const [portDraft, setPortDraft] = useState("");
  const [managementKeyDraft, setManagementKeyDraft] = useState("");
  const [configDirty, setConfigDirty] = useState(false);
  const [language, setLanguage] = useState<LanguageCode>(readStoredLanguage);
  const closeAllowedRef = useRef(false);
  const closeInProgressRef = useRef(false);

  const t = TRANSLATIONS[language];

  const activeRuntime = useMemo(() => {
    if (!state?.activeVersion) {
      return undefined;
    }
    return state.runtimes.find((runtime) => runtime.id === state.activeVersion);
  }, [state]);

  const runCommand = useCallback(
    async <T,>(key: BusyKey, action: () => Promise<T>) => {
      setBusy(key);
      setError(null);
      try {
        return await action();
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setError(message);
        throw err;
      } finally {
        setBusy("idle");
      }
    },
    [],
  );

  const syncConfigForm = useCallback((content: string) => {
    setConfigDraft(content);
    setPortDraft(readTopLevelScalar(content, "port") ?? "");
  }, []);

  const refresh = useCallback(async () => {
    const nextState = await invoke<DesktopState>("desktop_state");
    setState(nextState);
    setConfigDirty(false);
    syncConfigForm(nextState.config?.content ?? "");
    setManagementKeyDraft(nextState.config?.localManagementKey ?? "");
  }, [syncConfigForm]);

  useEffect(() => {
    void runCommand("refresh", refresh);
  }, [refresh, runCommand]);

  useEffect(() => {
    localStorage.setItem(LANGUAGE_STORAGE_KEY, language);
  }, [language]);

  useEffect(() => {
    const appWindow = getCurrentWindow();
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void appWindow.onCloseRequested(async (event) => {
      if (closeAllowedRef.current) {
        return;
      }

      event.preventDefault();
      if (closeInProgressRef.current) {
        return;
      }

      closeInProgressRef.current = true;
      try {
        const firstConfirmed = await confirmDialog(t.closeMessage, {
          title: t.closeTitle,
          kind: "warning",
        });
        if (!firstConfirmed) {
          closeInProgressRef.current = false;
          return;
        }

        const secondConfirmed = await confirmDialog(t.closeSecondMessage, {
          title: t.closeSecondTitle,
          kind: "warning",
        });
        if (!secondConfirmed) {
          closeInProgressRef.current = false;
          return;
        }

        setBusy("shutdown");
        setError(null);
        await invoke("shutdown_service");
        closeAllowedRef.current = true;
        await appWindow.destroy();
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setError(t.closeFailed(message));
        setBusy("idle");
        closeInProgressRef.current = false;
      }
    }).then((handler) => {
      if (disposed) {
        handler();
        return;
      }
      unlisten = handler;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [t]);

  const installPackage = async () => {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [
        {
          name: t.cliPackageFilter,
          extensions: ["gz", "tgz"],
        },
      ],
    });
    if (typeof selected !== "string") {
      return;
    }
    await runCommand("install", async () => {
      const nextState = await invoke<DesktopState>("install_update_package", {
        path: selected,
      });
      setState(nextState);
      setConfigDirty(false);
      syncConfigForm(nextState.config?.content ?? "");
      setManagementKeyDraft(nextState.config?.localManagementKey ?? "");
    });
  };

  const activateVersion = async (id: string) => {
    await runCommand("activate", async () => {
      const nextState = await invoke<DesktopState>("activate_version", { id });
      setState(nextState);
      setConfigDirty(false);
      syncConfigForm(nextState.config?.content ?? "");
      setManagementKeyDraft(nextState.config?.localManagementKey ?? "");
    });
  };

  const startService = async () => {
    await runCommand("start", async () => {
      const nextState = await invoke<DesktopState>("start_service");
      setState(nextState);
      setConfigDirty(false);
      syncConfigForm(nextState.config?.content ?? "");
      setManagementKeyDraft(nextState.config?.localManagementKey ?? "");
    });
  };

  const stopService = async () => {
    await runCommand("stop", async () => {
      const nextState = await invoke<DesktopState>("stop_service");
      setState(nextState);
      setConfigDirty(false);
      syncConfigForm(nextState.config?.content ?? "");
      setManagementKeyDraft(nextState.config?.localManagementKey ?? "");
    });
  };

  const saveConfig = async () => {
    await runCommand("save", async () => {
      const nextState = await invoke<DesktopState>("save_config_file", {
        content: configDraft,
        managementKey: managementKeyDraft,
      });
      setState(nextState);
      setConfigDirty(false);
      syncConfigForm(nextState.config?.content ?? "");
      setManagementKeyDraft(nextState.config?.localManagementKey ?? "");
    });
  };

  const restoreDefaultConfig = async () => {
    if (!state?.activeVersion) {
      setError(t.restoreUnavailable);
      return;
    }
    if (state.service.running) {
      setError(t.restoreStopFirst);
      return;
    }

    const runtimeLabel = activeRuntime ? `v${activeRuntime.version} (${activeRuntime.target})` : t.restoreVersionFallback;
    let firstConfirmed = false;
    let secondConfirmed = false;
    try {
      firstConfirmed = await confirmDialog(t.restoreFirstMessage, {
        title: t.restoreFirstTitle,
        kind: "warning",
      });
      if (firstConfirmed) {
        secondConfirmed = await confirmDialog(t.restoreSecondMessage(runtimeLabel), {
          title: t.restoreSecondTitle,
          kind: "warning",
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(t.dialogOpenFailed(message));
      return;
    }

    if (!firstConfirmed) {
      return;
    }
    if (!secondConfirmed) {
      return;
    }

    await runCommand("restore", async () => {
      const nextState = await invoke<DesktopState>("restore_default_config");
      setState(nextState);
      setConfigDirty(false);
      syncConfigForm(nextState.config?.content ?? "");
      setManagementKeyDraft(nextState.config?.localManagementKey ?? "");
    });
  };

  const deleteVersion = async (runtime: RuntimeInfo) => {
    const runtimeLabel = `v${runtime.version} (${runtime.target})`;
    const confirmed = await confirmDialog(t.deleteVersionMessage(runtimeLabel), {
      title: t.deleteVersionTitle,
      kind: "warning",
    });
    if (!confirmed) {
      return;
    }

    await runCommand("delete", async () => {
      const nextState = await invoke<DesktopState>("delete_version", { id: runtime.id });
      setState(nextState);
      setConfigDirty(false);
      syncConfigForm(nextState.config?.content ?? "");
      setManagementKeyDraft(nextState.config?.localManagementKey ?? "");
    });
  };

  const openWorkspace = async () => {
    await runCommand("open", async () => {
      await invoke("open_workspace");
    });
  };

  const openAuthDir = async () => {
    await runCommand("openAuth", async () => {
      await invoke("open_auth_dir");
    });
  };

  const exportAuthArchive = async () => {
    const selected = await save({
      defaultPath: defaultAuthArchiveName(),
      filters: [
        {
          name: t.authArchiveFilter,
          extensions: ["gz", "tgz"],
        },
      ],
    });
    if (typeof selected !== "string") {
      return;
    }

    await runCommand("exportAuth", async () => {
      const nextState = await invoke<DesktopState>("export_auth_archive", { path: selected });
      setState(nextState);
    });
  };

  const importAuthArchive = async () => {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [
        {
          name: t.authArchiveFilter,
          extensions: ["gz", "tgz"],
        },
      ],
    });
    if (typeof selected !== "string") {
      return;
    }

    const confirmed = await confirmDialog(t.importAuthMessage, {
      title: t.importAuthTitle,
      kind: "warning",
    });
    if (!confirmed) {
      return;
    }

    await runCommand("importAuth", async () => {
      const nextState = await invoke<DesktopState>("import_auth_archive", { path: selected });
      setState(nextState);
    });
  };

  const openManagementWeb = async () => {
    await runCommand("browser", async () => {
      await invoke("open_management_web");
    });
  };

  const updateConfigDraft = (content: string) => {
    setConfigDraft(content);
    setPortDraft(readTopLevelScalar(content, "port") ?? "");
    setConfigDirty(true);
  };

  const updatePort = (value: string) => {
    const nextValue = value.replace(/[^\d]/g, "").slice(0, 5);
    setPortDraft(nextValue);
    setConfigDraft((current) => upsertTopLevelScalar(current, "port", nextValue || "8317", false));
    setConfigDirty(true);
  };

  const updateManagementKey = (value: string) => {
    setManagementKeyDraft(value);
    setConfigDraft((current) => upsertNestedScalar(current, "remote-management", "secret-key", "", true));
    setConfigDirty(true);
  };

  return (
    <main className="app-shell">
      <section className="topbar">
        <div>
          <h1>CLIProxyAPI Desktop</h1>
          <p>{t.appSubtitle}</p>
        </div>
        <div className="toolbar">
          <label className="language-select" title={t.language}>
            <Languages size={18} />
            <select
              value={language}
              onChange={(event) => {
                if (isLanguageCode(event.target.value)) {
                  setLanguage(event.target.value);
                }
              }}
              aria-label={t.language}
            >
              {LANGUAGE_OPTIONS.map((option) => (
                <option key={option.code} value={option.code}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
          <button className="icon-button secondary" onClick={() => runCommand("refresh", refresh)} disabled={busy !== "idle"} title={t.refresh}>
            <RefreshCw size={18} />
            <span>{t.refresh}</span>
          </button>
          <button className="icon-button secondary" onClick={openWorkspace} disabled={!state || busy !== "idle"} title={t.workspace}>
            <FolderOpen size={18} />
            <span>{t.workspace}</span>
          </button>
          <button className="icon-button primary" onClick={installPackage} disabled={busy !== "idle"} title={t.importVersionPackage}>
            <PackagePlus size={18} />
            <span>{t.importVersionPackage}</span>
          </button>
        </div>
      </section>

      {busy !== "idle" && <div className="status-line">{t.commands[busy]}</div>}
      {error && <div className="error-line">{error}</div>}

      <section className="panel config-panel">
        <div className="panel-heading">
          <div>
            <h2>{t.configFile}</h2>
            <p>{state?.config?.path ?? t.uninitialized}</p>
          </div>
          <div className="toolbar compact">
            <button className="icon-button secondary" onClick={() => runCommand("refresh", refresh)} disabled={!state || busy !== "idle"} title={t.reload}>
              <RotateCcw size={18} />
              <span>{t.reload}</span>
            </button>
            <button className="icon-button warning" onClick={restoreDefaultConfig} disabled={!state?.activeVersion || busy !== "idle"} title={t.restoreDefaultTitle}>
              <Undo2 size={18} />
              <span>{t.restoreDefault}</span>
            </button>
            <button className="icon-button primary" onClick={saveConfig} disabled={!state?.config || !configDirty || busy !== "idle"} title={t.save}>
              <Save size={18} />
              <span>{t.save}</span>
            </button>
          </div>
        </div>

        <div className="quick-config">
          <label className="field">
            <span>{t.webPort}</span>
            <input value={portDraft} onChange={(event) => updatePort(event.target.value)} inputMode="numeric" disabled={!state?.config || busy !== "idle"} />
          </label>
          <label className="field wide">
            <span>{t.localManagementKey}</span>
            <input value={managementKeyDraft} onChange={(event) => updateManagementKey(event.target.value)} disabled={!state?.config || busy !== "idle"} spellCheck={false} autoCapitalize="none" />
          </label>
          <button className="web-link" onClick={openManagementWeb} disabled={!state?.service.running || !state?.config?.managementUrl || busy !== "idle"} title={t.openManagementPage}>
            <ExternalLink size={18} />
            <span>{state?.config?.managementUrl ?? t.noManagementUrl}</span>
          </button>
        </div>

        <textarea className="config-editor" value={configDraft} onChange={(event) => updateConfigDraft(event.target.value)} spellCheck={false} disabled={!state?.config || busy !== "idle"} />
      </section>

      <section className="dashboard">
        <article className="panel service-panel">
          <div className="panel-heading">
            <div>
              <h2>{t.service}</h2>
              <p>{state?.service.running ? `PID ${state.service.pid}` : t.notRunning}</p>
            </div>
            <span className={state?.service.running ? "badge online" : "badge"}>{state?.service.running ? t.running : t.stopped}</span>
          </div>

          <div className="service-grid">
            <div>
              <span className="meta-label">{t.currentVersion}</span>
              <strong>{activeRuntime ? `v${activeRuntime.version}` : t.notInstalled}</strong>
            </div>
            <div>
              <span className="meta-label">{t.targetPlatform}</span>
              <strong>{activeRuntime?.target ?? "--"}</strong>
            </div>
            <div>
              <span className="meta-label">{t.webPort}</span>
              <strong>{state?.service.port ?? "--"}</strong>
            </div>
          </div>

          <div className="service-actions">
            <button className="icon-button primary" onClick={startService} disabled={!state || state.service.running || !state.activeVersion || busy !== "idle"} title={t.start}>
              <Play size={18} />
              <span>{t.start}</span>
            </button>
            <button className="icon-button danger" onClick={stopService} disabled={!state?.service.running || busy !== "idle"} title={t.stop}>
              <Square size={18} />
              <span>{t.stop}</span>
            </button>
          </div>
        </article>

        <article className="panel path-panel">
          <h2>{t.paths}</h2>
          <dl>
            <div>
              <dt>{t.appData}</dt>
              <dd>{state?.appDataDir ?? "--"}</dd>
            </div>
            <div>
              <dt>{t.workspaceDir}</dt>
              <dd>{state?.workspaceDir ?? "--"}</dd>
            </div>
            <div>
              <dt>{t.authFiles}</dt>
              <dd className="path-value-row">
                <span>{state?.authDir ?? "--"}</span>
                <span className="path-actions">
                  <button className="path-action" onClick={openAuthDir} disabled={!state || busy !== "idle"} title={`${t.open} ${t.authFiles}`}>
                    <FolderOpen size={15} />
                    <span>{t.open}</span>
                  </button>
                  <button className="path-action" onClick={exportAuthArchive} disabled={!state || busy !== "idle"} title={t.export}>
                    <Download size={15} />
                    <span>{t.export}</span>
                  </button>
                  <button className="path-action" onClick={importAuthArchive} disabled={!state || busy !== "idle"} title={t.import}>
                    <Upload size={15} />
                    <span>{t.import}</span>
                  </button>
                </span>
              </dd>
            </div>
            <div>
              <dt>{t.currentBinary}</dt>
              <dd>{activeRuntime?.binaryPath ?? "--"}</dd>
            </div>
          </dl>
        </article>
      </section>

      <section className="versions-panel">
        <div className="section-heading">
          <h2>{t.installedVersions}</h2>
          <span>{state?.runtimes.length ?? 0}</span>
        </div>
        <div className="version-table">
          <div className="table-row table-head">
            <span>{t.version}</span>
            <span>{t.platform}</span>
            <span>{t.installedAt}</span>
            <span>{t.packageFile}</span>
            <span>{t.status}</span>
            <span>{t.actions}</span>
          </div>
          {state?.runtimes.map((runtime) => {
            const isActive = state.activeVersion === runtime.id;
            return (
              <div className="table-row" key={runtime.id}>
                <span className="version-cell">v{runtime.version}</span>
                <span>{runtime.target}</span>
                <span>{formatInstalledAt(runtime.installedAt, language)}</span>
                <span className="package-cell">{runtime.packageFile}</span>
                <span>
                  {isActive ? (
                    <span className="active-pill">
                      <CheckCircle2 size={16} />
                      {t.current}
                    </span>
                  ) : (
                    <button className="text-button" onClick={() => activateVersion(runtime.id)} disabled={state.service.running || busy !== "idle"}>
                      {t.setCurrent}
                    </button>
                  )}
                </span>
                <span>
                  {isActive ? (
                    <button className="text-button locked" disabled title={t.cannotDelete}>
                      <Trash2 size={15} />
                      {t.cannotDelete}
                    </button>
                  ) : (
                    <button className="text-button danger" onClick={() => deleteVersion(runtime)} disabled={busy !== "idle"} title={t.delete}>
                      <Trash2 size={15} />
                      {t.delete}
                    </button>
                  )}
                </span>
              </div>
            );
          })}
          {state && state.runtimes.length === 0 && <div className="empty-state">{t.emptyVersions}</div>}
        </div>
      </section>
    </main>
  );
}

function formatInstalledAt(value: number, language: LanguageCode) {
  if (!value) {
    return "--";
  }
  return new Intl.DateTimeFormat(localeForLanguage(language), {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(value * 1000));
}

function readStoredLanguage(): LanguageCode {
  const stored = localStorage.getItem(LANGUAGE_STORAGE_KEY);
  return isLanguageCode(stored) ? stored : "zh-CN";
}

function isLanguageCode(value: string | null): value is LanguageCode {
  return LANGUAGE_OPTIONS.some((option) => option.code === value);
}

function localeForLanguage(language: LanguageCode) {
  return LANGUAGE_OPTIONS.find((option) => option.code === language)?.locale ?? "zh-CN";
}

function defaultAuthArchiveName() {
  const date = new Date();
  const stamp = [
    date.getFullYear(),
    padDatePart(date.getMonth() + 1),
    padDatePart(date.getDate()),
    "_",
    padDatePart(date.getHours()),
    padDatePart(date.getMinutes()),
    padDatePart(date.getSeconds()),
  ].join("");
  return `CLIProxyAPI_auths_${stamp}.tar.gz`;
}

function padDatePart(value: number) {
  return String(value).padStart(2, "0");
}

function readTopLevelScalar(content: string, key: string) {
  const pattern = new RegExp(`^${escapeRegExp(key)}\\s*:\\s*(.*)$`);
  for (const line of content.split(/\r?\n/)) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith("#")) {
      continue;
    }
    const match = line.match(pattern);
    if (match) {
      return cleanScalar(match[1]);
    }
  }
  return undefined;
}

function upsertTopLevelScalar(content: string, key: string, value: string, quoted: boolean) {
  const lines = splitLines(content);
  const pattern = new RegExp(`^${escapeRegExp(key)}\\s*:`);
  const nextLine = `${key}: ${formatScalar(value, quoted)}`;

  for (let index = 0; index < lines.length; index += 1) {
    if (!lines[index].trimStart().startsWith("#") && pattern.test(lines[index])) {
      lines[index] = nextLine;
      return lines.join("\n");
    }
  }

  return [nextLine, ...lines].join("\n");
}

function upsertNestedScalar(content: string, section: string, key: string, value: string, quoted: boolean) {
  const lines = splitLines(content);
  const sectionPattern = new RegExp(`^(\\s*)${escapeRegExp(section)}\\s*:\\s*(?:#.*)?$`);
  const keyPattern = new RegExp(`^\\s+${escapeRegExp(key)}\\s*:`);

  for (let index = 0; index < lines.length; index += 1) {
    const sectionMatch = lines[index].match(sectionPattern);
    if (!sectionMatch || lines[index].trimStart().startsWith("#")) {
      continue;
    }
    const sectionIndent = sectionMatch[1].length;
    const childIndent = `${sectionMatch[1]}  `;
    const nextLine = `${childIndent}${key}: ${formatScalar(value, quoted)}`;

    for (let childIndex = index + 1; childIndex < lines.length; childIndex += 1) {
      const line = lines[childIndex];
      if (line.trim() === "" || line.trimStart().startsWith("#")) {
        continue;
      }
      const indent = line.length - line.trimStart().length;
      if (indent <= sectionIndent) {
        lines.splice(childIndex, 0, nextLine);
        return lines.join("\n");
      }
      if (keyPattern.test(line)) {
        lines[childIndex] = nextLine;
        return lines.join("\n");
      }
    }

    lines.push(nextLine);
    return lines.join("\n");
  }

  const suffix = [`${section}:`, `  ${key}: ${formatScalar(value, quoted)}`];
  return `${content.trimEnd()}\n${suffix.join("\n")}\n`;
}

function splitLines(content: string) {
  return content.length ? content.replace(/\r\n/g, "\n").split("\n") : [""];
}

function formatScalar(value: string, quoted: boolean) {
  return quoted ? JSON.stringify(value) : value;
}

function cleanScalar(value: string) {
  return value.split("#")[0].trim().replace(/^["']|["']$/g, "");
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
