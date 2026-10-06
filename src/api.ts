import type { GameSelection, GameSettings, GameSnapshot, RegisteredGame, AnalyticsReport, AppSettings, SensorInventory, Capability, CleanerScan, CleaningResult, CleanupProgress, DeviceInfo, HardwareSnapshot, HistoryPoint, PerformanceProfile, PowerPlan, PersistenceStatus, ProcessSnapshot, TuningSession } from './types';

type TauriApi = {
  core: { invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T> };
  event?: { listen: <T>(event: string, handler: (event: { payload: T }) => void) => Promise<() => void> };
};
declare global { interface Window { __TAURI__?: TauriApi } }

export const isDesktop = Boolean(window.__TAURI__?.core?.invoke);
export const onHardwareUpdate = (handler: (sample: HardwareSnapshot) => void) => window.__TAURI__?.event?.listen<HardwareSnapshot>('hardware:update', event => handler(event.payload));
export const onTuningChanged = (handler: () => void) => window.__TAURI__?.event?.listen<TuningSession>('tuning:changed', handler);
export const onDeviceChanged = (handler: () => void) => window.__TAURI__?.event?.listen<DeviceInfo>('device:changed', handler);
export const onAppError = (handler: (message: string) => void) => window.__TAURI__?.event?.listen<{ message: string }>('app:error', event => handler(event.payload.message));
export const onCleanupProgress = (handler: (progress: CleanupProgress) => void) => window.__TAURI__?.event?.listen<CleanupProgress>('cleanup:progress', event => handler(event.payload));
export const onSensorStatus = (handler: (status: SensorInventory) => void) => window.__TAURI__?.event?.listen<SensorInventory>('sensors:status', event => handler(event.payload));
export const onPersistenceStatus = (handler: (status: PersistenceStatus) => void) => window.__TAURI__?.event?.listen<PersistenceStatus>('monitoring:persistence', event => handler(event.payload));

const now = () => new Date().toISOString();
const metric = (value: number | null, unit: string, source = 'Demo') => ({ value, unit, source, status: value === null ? 'unsupported' as const : 'supported' as const, recorded_at: now() });
let tick = 0;
let demoPlan = 'balanced';
const demoProfiles: PerformanceProfile[] = [
  { id: 'balanced', name: 'Balanced', scheme_guid: 'balanced', ac_only: false },
  { id: 'gaming', name: 'Gaming Boost', scheme_guid: 'high-performance', ac_only: true },
  { id: 'saving', name: 'Power Saving', scheme_guid: 'power-saver', ac_only: false },
];
const demoPlans: PowerPlan[] = [
  { guid: 'balanced', name: 'Balanced', active: true },
  { guid: 'high-performance', name: 'High performance', active: false },
  { guid: 'power-saver', name: 'Power saver', active: false },
];

function demoSnapshot(): HardwareSnapshot {
  tick += 1;
  const wave = (base: number, amplitude: number, speed: number) => Math.round((base + Math.sin(tick * speed) * amplitude + Math.sin(tick * speed * 2.7) * amplitude * .32) * 10) / 10;
  return {
    recorded_at: now(), cpu_usage: metric(wave(31, 10, .46), '%'), cpu_temperature: metric(wave(58, 4, .18), '°C'),
    gpu_usage: metric(wave(44, 15, .31), '%'), gpu_temperature: metric(wave(63, 3, .15), '°C'),
    ram_usage: metric(wave(62, 1, .1), '%'), ram_used_gb: metric(10.1, 'GB'), ram_total_gb: metric(16, 'GB'),
    disk_used_gb: metric(338, 'GB'), disk_total_gb: metric(512, 'GB'),
    network_down_kbps: metric(Math.max(0, wave(42, 29, .52)), 'KB/s'), network_up_kbps: metric(Math.max(0, wave(8, 6, .39)), 'KB/s'),
    cpu_name: 'Intel Core i7-12700H', gpu_name: 'NVIDIA GeForce RTX 3060', disk_name: 'Local Disk (C:)', power_source: 'AC power',
  };
}

const demoInfo: DeviceInfo = { device_name: 'DESKTOP-DEMO', operating_system: 'Windows 11 Pro', cpu_model: 'Intel Core i7-12700H', cpu_cores: 14, ram_total_bytes: 16 * 1024 ** 3, gpu_adapters: ['Intel UHD Graphics', 'NVIDIA GeForce RTX 3060'], power_source: 'AC power' };

const demoSettings: AppSettings = { refresh_seconds: 1, theme: 'dark', language: 'id', history_retention_hours: 24, monitor_in_background: false, gpu_adapter_id: null };

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isDesktop) return window.__TAURI__!.core.invoke<T>(command, args);
  switch (command) {
    case 'get_process_snapshot': return { recorded_at: now(), total_count: 3, truncated: false, processes: [{ pid: 1200, started_at: 1, name: 'Core Pulse (demo)', executable: null, cpu_percent: 1.2, memory_bytes: 128 * 1024 ** 2 }, { pid: 2400, started_at: 1, name: 'Browser (demo)', executable: null, cpu_percent: 3.4, memory_bytes: 512 * 1024 ** 2 }, { pid: 3600, started_at: 1, name: 'System (demo)', executable: null, cpu_percent: null, memory_bytes: null }] } as T;
    case 'get_hardware_snapshot': return demoSnapshot() as T;
    case 'get_system_information': case 'detect_hardware': return demoInfo as T;
    case 'get_device_capabilities': return [
      { capability_key: 'cpu_usage', status: 'supported', reason: 'Data simulasi untuk pratinjau', detected_at: now() },
      { capability_key: 'gpu_temperature', status: 'unsupported', reason: 'Sensor nyata hanya tersedia di aplikasi desktop dengan provider', detected_at: now() },
      { capability_key: 'power_plans', status: 'supported', reason: 'Daftar contoh; perubahan tidak diterapkan', detected_at: now() },
    ] as T;
    case 'get_power_plans': return demoPlans.map(p => ({ ...p, active: p.guid === demoPlan })) as T;
    case 'get_performance_profiles': return demoProfiles as T;
    case 'update_profile_mapping': {
      const profile = demoProfiles.find(p => p.id === args?.profileId);
      if (profile) { profile.scheme_guid = String(args?.guid || ''); profile.ac_only = Boolean(args?.acOnly); }
      return profile as T;
    }
    case 'get_active_power_plan': return demoPlans.find(p => p.guid === demoPlan) as T;
    case 'get_settings': return demoSettings as T;
    case 'get_sensor_inventory': return { status: 'demo', message: 'Data simulasi untuk pratinjau', provider: 'Demo', elevated: false, restart_count: 0, updated_at: now(), devices: [{id:'/gpu-nvidia/0',name:'NVIDIA GeForce RTX 3060 (demo)',kind:'gpu',temperature_available:true,load_available:true}] } as T;
    case 'get_monitoring_persistence': return { status: 'demo', message: null, last_saved_at: null, dropped_samples: 0, updated_at: now() } as T;
    case 'update_settings': Object.assign(demoSettings, args?.settings); return demoSettings as T;
    case 'get_hardware_history': return [] as T;
    case 'get_analytics_report': return { from: new Date(Date.now() - Number(args?.minutes) * 60000).toISOString(), to: now(), bucket_seconds: 5, points: [] } as T;
    case 'get_cleaning_history': return [] as T;
    case 'get_unfinished_tuning_sessions': return [] as T;
    case 'get_registered_games': return [] as T;
    case 'get_game_status': return { recorded_at: now(), status: 'demo', games: [] } as T;
    case 'scan_cleanable_files': return { plan_id: 'demo', expires_at: new Date(Date.now() + 300000).toISOString(), category: 'user_temp', estimated_bytes: 0, eligible_count: 0, skipped_count: 0, warnings: ['Pratinjau web tidak memindai file di komputer.'] } as T;
    case 'start_monitoring': case 'stop_monitoring': return undefined as T;
    default: throw new Error('Tindakan ini hanya tersedia di aplikasi desktop.');
  }
}

export const api = {
  pickGame: () => invoke<GameSelection | null>('pick_game_executable'),
  registerGame: (selectionId: string, settings: GameSettings) => invoke<RegisteredGame>('register_game', { selectionId, settings }),
  getGames: () => invoke<RegisteredGame[]>('get_registered_games'),
  updateGame: (gameId: string, settings: GameSettings) => invoke<RegisteredGame>('update_registered_game', { gameId, settings }),
  removeGame: (gameId: string) => invoke<void>('remove_registered_game', { gameId }),
  getGameStatus: () => invoke<GameSnapshot>('get_game_status'),
  getSensors: () => invoke<SensorInventory>('get_sensor_inventory'),
  getProcesses: () => invoke<ProcessSnapshot>('get_process_snapshot'),
  getSnapshot: () => invoke<HardwareSnapshot>('get_hardware_snapshot'),
  getInfo: () => invoke<DeviceInfo>('get_system_information'),
  detect: () => invoke<DeviceInfo>('detect_hardware'),
  getCapabilities: () => invoke<Capability[]>('get_device_capabilities'),
  getPlans: () => invoke<PowerPlan[]>('get_power_plans'),
  getProfiles: () => invoke<PerformanceProfile[]>('get_performance_profiles'),
  updateProfileMapping: (profileId: string, guid: string, acOnly: boolean) => invoke<PerformanceProfile>('update_profile_mapping', { profileId, guid, acOnly }),
  getActivePlan: () => invoke<PowerPlan>('get_active_power_plan'),
  getUnfinishedSessions: () => invoke<TuningSession[]>('get_unfinished_tuning_sessions'),
  activateProfile: (profileId: string) => invoke<TuningSession>('activate_performance_profile', { profileId }),
  restoreProfile: (sessionId: string, force = false) => invoke<TuningSession>('restore_previous_profile', { sessionId, force }),
  scanCleaner: () => invoke<CleanerScan>('scan_cleanable_files', { categories: ['user_temp'] }),
  executeCleaner: (planId: string) => invoke<CleaningResult>('execute_cleanup', { planId, categories: ['user_temp'] }),
  cancelCleaner: () => invoke<void>('cancel_cleanup'),
  getCleaningHistory: () => invoke<CleaningResult[]>('get_cleaning_history'),
  getHistory: (sensorKey: string, hours = 1) => invoke<HistoryPoint[]>('get_hardware_history', { sensorKey, hours }),
  getAnalytics: (minutes: number) => invoke<AnalyticsReport>('get_analytics_report', { minutes }),
  getSettings: () => invoke<AppSettings>('get_settings'),
  getPersistence: () => invoke<PersistenceStatus>('get_monitoring_persistence'),
  updateSettings: (settings: AppSettings) => invoke<AppSettings>('update_settings', { settings }),
  purgeMonitoringHistory: () => invoke<number>('purge_monitoring_history'),
  startMonitoring: () => invoke<void>('start_monitoring'),
  stopMonitoring: () => invoke<void>('stop_monitoring'),
};
