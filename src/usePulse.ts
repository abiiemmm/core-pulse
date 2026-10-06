import { getLanguage, setLanguage, t } from './i18n';
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, isDesktop, onAppError, onCleanupProgress, onDeviceChanged, onHardwareUpdate, onPersistenceStatus, onSensorStatus, onTuningChanged } from './api';
import type { AppSettings, SensorInventory, Capability, CleanerScan, CleaningResult, CleanupProgress, DeviceInfo, HardwareSnapshot, PerformanceProfile, PowerPlan, PersistenceStatus, TuningSession } from './types';
import { formatBytes } from './ui';

export type Sensor = 'cpu' | 'gpu' | 'ram';
export type Sample = { time: string; cpu: number | null; gpu: number | null; ram: number | null };
export const SENSORS: { value: Sensor; label: string }[] = [{ value: 'cpu', label: 'CPU' }, { value: 'gpu', label: 'GPU' }, { value: 'ram', label: 'Memori' }];
export const sampleAge = (time?: string) => !time ? t("Menunggu sampel") : Math.max(0, Math.floor((Date.now() - Date.parse(time)) / 1000)) < 2 ? t("Baru diperbarui") : t("{0} dtk lalu", { 0: Math.max(0, Math.floor((Date.now() - Date.parse(time)) / 1000)) });
export function summarize(values: (number | null)[]) {
  const valid = values.filter((value): value is number => value != null && Number.isFinite(value));
  return { count: valid.length, average: valid.length ? valid.reduce((sum, value) => sum + value, 0) / valid.length : null, min: valid.length ? Math.min(...valid) : null, max: valid.length ? Math.max(...valid) : null };
}

export function usePulse() {
  const [snapshot, setSnapshot] = useState<HardwareSnapshot>();
  const [info, setInfo] = useState<DeviceInfo>();
  const [capabilities, setCapabilities] = useState<Capability[]>([]);
  const [plans, setPlans] = useState<PowerPlan[]>([]);
  const [profiles, setProfiles] = useState<PerformanceProfile[]>([]);
  const [sessions, setSessions] = useState<TuningSession[]>([]);
  const [settings, setSettings] = useState<AppSettings>({ refresh_seconds: 1, theme: 'dark', language: getLanguage(), history_retention_hours: 24, monitor_in_background: false, gpu_adapter_id: null });
  const [sensorInventory, setSensorInventory] = useState<SensorInventory>({status:isDesktop ? 'waiting' : 'demo',message:'',provider:'LibreHardwareMonitor 0.9.6',elevated:false,restart_count:0,updated_at:new Date(0).toISOString(),devices:[]});
  const gpuSourceKey = settings.gpu_adapter_id ?? [...sensorInventory.devices].filter(device => device.kind === 'gpu').sort((a,b) => (a.id.startsWith('/gpu-nvidia/') ? 0 : a.id.startsWith('/gpu-amd/') ? 1 : 2) - (b.id.startsWith('/gpu-nvidia/') ? 0 : b.id.startsWith('/gpu-amd/') ? 1 : 2) || a.id.localeCompare(b.id))[0]?.id ?? '';
  const gpuIdentity = useRef(gpuSourceKey);
  gpuIdentity.current = gpuSourceKey;
  const [samples, setSamples] = useState<Sample[]>([]);
  const [running, setRunning] = useState(true);
  const [busy, setBusy] = useState('');
  const [message, setMessage] = useState<{ text: string; error: boolean }>();
  const [scan, setScan] = useState<CleanerScan>();
  const [cleanHistory, setCleanHistory] = useState<CleaningResult[]>([]);
  const [persistence, setPersistence] = useState<PersistenceStatus>({ status: isDesktop ? 'waiting' : 'demo', message: null, last_saved_at: null, dropped_samples: 0, updated_at: new Date().toISOString() });
  const [cleanupProgress, setCleanupProgress] = useState<CleanupProgress>();
  const [cancelRequested, setCancelRequested] = useState(false);
  const cleanupPlan = useRef<string | null>(null);
  const [confirmation, setConfirmation] = useState<'clean' | 'purge' | 'restore' | null>(null);
  const [selectedProfile, setSelectedProfile] = useState('gaming');
  const [clock, setClock] = useState(Date.now());
  const [systemLight, setSystemLight] = useState(() => window.matchMedia('(prefers-color-scheme: light)').matches);
  const notify = useCallback((text: string, error = false) => setMessage({ text, error }), []);
  const acceptPersistence = useCallback((status: PersistenceStatus) => setPersistence(previous => Date.parse(previous.updated_at) > Date.parse(status.updated_at) ? previous : status), []);
  const refreshContext = useCallback(async () => {
    const results = await Promise.allSettled([api.getInfo(), api.getCapabilities(), api.getPlans(), api.getProfiles(), api.getUnfinishedSessions(), api.getCleaningHistory(), api.getSettings(), api.getPersistence(), api.getSensors()]);
    if (results[0].status === 'fulfilled') setInfo(results[0].value);
    if (results[1].status === 'fulfilled') setCapabilities(results[1].value);
    if (results[2].status === 'fulfilled') setPlans(results[2].value);
    if (results[3].status === 'fulfilled') setProfiles(results[3].value);
    if (results[4].status === 'fulfilled') setSessions(results[4].value);
    if (results[5].status === 'fulfilled') setCleanHistory(results[5].value);
    if (results[6].status === 'fulfilled') setSettings(results[6].value);
    if (results[7].status === 'fulfilled') acceptPersistence(results[7].value);
    if (results[8].status === 'fulfilled') setSensorInventory(results[8].value);
    const error = results.find(result => result.status === 'rejected');
    if (error?.status === 'rejected') notify(String(error.reason), true);
  }, [notify, acceptPersistence]);
  const acceptSample = useCallback((incoming: HardwareSnapshot) => {
    const matches = !isDesktop || Boolean(gpuIdentity.current && incoming.gpu_usage.source.endsWith(gpuIdentity.current));
    const sample = matches ? incoming : { ...incoming, gpu_name: t('GPU belum tersedia'), gpu_usage:{...incoming.gpu_usage,value:null,status:'unknown' as const},gpu_temperature:{...incoming.gpu_temperature,value:null,status:'unknown' as const} };
    setSnapshot(previous => previous && Date.parse(previous.recorded_at) > Date.parse(sample.recorded_at) ? previous : sample);
    setCapabilities(previous => previous.map(capability => {
      const metric = capability.capability_key === 'gpu_usage' ? sample.gpu_usage : capability.capability_key === 'gpu_temperature' ? sample.gpu_temperature : capability.capability_key === 'cpu_temperature' ? sample.cpu_temperature : null;
      return metric ? {...capability,status:metric.status,reason:capability.capability_key === 'cpu_temperature' ? 'Pembacaan suhu CPU menunggu verifikasi akses driver' : metric.value == null ? 'Sensor belum tersedia dari adapter yang dipilih' : metric.source,detected_at:metric.recorded_at} : capability;
    }));
    setSamples(previous => {
      if (previous.length && Date.parse(previous.at(-1)!.time) > Date.parse(sample.recorded_at)) return previous;
      const point = { time: sample.recorded_at, cpu: sample.cpu_usage.value, gpu: sample.gpu_usage.value, ram: sample.ram_usage.value };
      return (previous.at(-1)?.time === point.time ? [...previous.slice(0, -1), point] : [...previous, point]).slice(-60);
    });
  }, []);
  const poll = useCallback(async () => { try { acceptSample(await api.getSnapshot()); } catch (error) { notify(t(String(error)), true); } }, [acceptSample, notify]);
  useEffect(() => { void refreshContext(); }, [refreshContext]);
  useEffect(() => { setLanguage(settings.language); }, [settings.language]);
  useEffect(() => { void (running ? api.startMonitoring().then(poll) : api.stopMonitoring()).catch(error => notify(t(String(error)), true)); }, [running, poll, notify]);
  useEffect(() => () => { void api.stopMonitoring().catch(() => {}); }, []);
  useEffect(() => { if (!running) return; const id = window.setInterval(() => void poll(), settings.refresh_seconds * 1000); return () => clearInterval(id); }, [poll, running, settings.refresh_seconds]);
  useEffect(() => {
    let unlisten: (() => void) | undefined, closed = false;
    void onHardwareUpdate(sample => { if (running) acceptSample(sample); })?.then(stop => { if (closed) stop(); else unlisten = stop; }).catch(() => {});
    return () => { closed = true; unlisten?.(); };
  }, [acceptSample, running]);
  useEffect(() => {
    let closed = false; const listeners: (() => void)[] = [];
    for (const pending of [onDeviceChanged(() => { void refreshContext(); }), onTuningChanged(() => { void refreshContext(); }), onAppError(text => notify(text, true)), onPersistenceStatus(acceptPersistence), onSensorStatus(status => { setSensorInventory(previous => Date.parse(previous.updated_at) > Date.parse(status.updated_at) ? previous : status); void api.getCapabilities().then(setCapabilities).catch(() => {}); }), onCleanupProgress(progress => {
      if (progress.plan_id !== cleanupPlan.current) return;
      setCleanupProgress(previous => previous && (Date.parse(previous.timestamp) > Date.parse(progress.timestamp) || (previous.status !== 'running' && progress.status === 'running')) ? previous : progress);
    })]) void pending?.then(stop => { if (closed) stop(); else listeners.push(stop); }).catch(() => {});
    return () => { closed = true; listeners.forEach(stop => stop()); };
  }, [refreshContext, notify, acceptPersistence]);
  useEffect(() => {
    if (!isDesktop) return;
    let closed = false;
    // Adapter selection also applies while live monitoring is paused.
    setSnapshot(previous => previous && !previous.gpu_usage.source.endsWith(gpuIdentity.current || '\u0000') ? {...previous,gpu_name:t('GPU belum tersedia'),gpu_usage:{...previous.gpu_usage,value:null,status:'unknown'},gpu_temperature:{...previous.gpu_temperature,value:null,status:'unknown'}} : previous);
    void poll();
    setSamples(previous => previous.map(row => ({...row,gpu:null})));
    void Promise.all([api.getHistory('cpu_usage'), api.getHistory('gpu_usage'), api.getHistory('ram_usage')]).then(([cpu, gpu, ram]) => {
      if (closed) return;
      setSamples(previous => {
        const rows = new Map<string, Sample>();
        for (const [key, values] of [['cpu', cpu], ['gpu', gpu], ['ram', ram]] as const) for (const point of values) {
          const row = rows.get(point.recorded_at) ?? { time: point.recorded_at, cpu: null, gpu: null, ram: null };
          row[key] = point.value; rows.set(point.recorded_at, row);
        }
        previous.forEach(row => { const historic = rows.get(row.time); rows.set(row.time, {...row,gpu:row.gpu ?? historic?.gpu ?? null}); });
        return Array.from(rows.values()).sort((a, b) => a.time.localeCompare(b.time)).slice(-60);
      });
    }).catch(() => {});
    return () => { closed = true; };
  }, [gpuSourceKey, poll]);
  useEffect(() => { const id = window.setInterval(() => setClock(Date.now()), 1000); return () => clearInterval(id); }, []);
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: light)');
    const apply = () => setSystemLight(media.matches);
    media.addEventListener('change', apply); return () => media.removeEventListener('change', apply);
  }, []);
  const resolvedTheme = settings.theme === 'system' ? (systemLight ? 'light' : 'dark') : settings.theme;
  useEffect(() => { document.documentElement.dataset.theme = resolvedTheme; }, [resolvedTheme]);
  useEffect(() => { if (!message || message.error) return; const id = window.setTimeout(() => setMessage(undefined), 4500); return () => clearTimeout(id); }, [message]);

  const activePlan = plans.find(plan => plan.active);
  const activeSession = sessions.find(session => ['active', 'pending', 'restoring', 'conflict'].includes(session.status));
  const selectedMapping = profiles.find(profile => profile.id === selectedProfile);
  const stale = snapshot ? clock - Date.parse(snapshot.recorded_at) > settings.refresh_seconds * 3000 : true;
  const connectionLabel = !running ? t("Dijeda") : !snapshot ? t("Menghubungkan") : stale ? t("Data tertunda") : t("Monitoring aktif");
  const diskPercent = snapshot?.disk_used_gb.value != null && snapshot.disk_total_gb.value != null && snapshot.disk_total_gb.value > 0 ? snapshot.disk_used_gb.value / snapshot.disk_total_gb.value * 100 : null;
  const freeDisk = snapshot?.disk_total_gb.value != null && snapshot.disk_used_gb.value != null ? snapshot.disk_total_gb.value - snapshot.disk_used_gb.value : null;
  const expiredScan = Boolean(scan && Date.parse(scan.expires_at) <= clock);
  const canClean = Boolean(isDesktop && scan?.eligible_count && !expiredScan && !busy);
  async function refreshDetection() { setBusy('detect'); try { setInfo(await api.detect()); await refreshContext(); notify(t("Informasi perangkat diperbarui.")); } catch (error) { notify(t(String(error)), true); } finally { setBusy(''); } }
  async function saveSettings(next: AppSettings) { setBusy('settings'); try { const saved = await api.updateSettings(next); setLanguage(saved.language); setSettings(saved); notify(t("Pengaturan tersimpan.")); } catch (error) { notify(t(String(error)), true); } finally { setBusy(''); } }
  async function mapProfile(guid: string, acOnly: boolean) { setBusy('mapping'); try { await api.updateProfileMapping(selectedProfile, guid, acOnly); setProfiles(await api.getProfiles()); notify(t("Pemetaan profil tersimpan.")); } catch (error) { notify(t(String(error)), true); } finally { setBusy(''); } }
  async function activate() { if (!isDesktop) return; setBusy('tuning'); try { await api.activateProfile(selectedProfile); await refreshContext(); notify(t("Profil daya diterapkan. Skema sebelumnya tersimpan untuk pemulihan.")); } catch (error) { notify(t(String(error)), true); await refreshContext(); } finally { setBusy(''); } }
  async function restore(force = false) { if (!activeSession) return; setConfirmation(null); setBusy('restore'); try { await api.restoreProfile(activeSession.id, force); await refreshContext(); notify(t("Profil daya sebelumnya dipulihkan.")); } catch (error) { notify(t(String(error)), true); await refreshContext(); } finally { setBusy(''); } }
  async function startScan() { setBusy('scan'); setScan(undefined); cleanupPlan.current = null; try { setScan(await api.scanCleaner()); } catch (error) { notify(t(String(error)), true); } finally { setBusy(''); } }
  async function clean() {
    setConfirmation(null); if (!scan || !canClean) return; setBusy('clean');
    cleanupPlan.current = scan.plan_id; setCancelRequested(false); setCleanupProgress(undefined);
    try {
      const result = await api.executeCleaner(scan.plan_id);
      // IPC responses and events can arrive in either order. Use the returned
      // outcome immediately, and allow the matching terminal event to refine it.
      setCleanupProgress(previous => previous && previous.status !== 'running' ? previous : { ...result, plan_id: scan.plan_id, processed_count: result.status === 'cancelled' ? previous?.processed_count ?? 0 : scan.eligible_count, total_count: scan.eligible_count, timestamp: result.finished_at });
      setScan(undefined); setCleanHistory(await api.getCleaningHistory()); notify(t("{0} dibersihkan. {1} file dilewati, {2} gagal.", { 0: formatBytes(result.recovered_bytes), 1: result.skipped_count, 2: result.error_count }), result.error_count > 0);
    }
    catch (error) { notify(t(String(error)), true); } finally { setScan(undefined); setBusy(''); }
  }
  async function cancelClean() { if (cancelRequested) return; setCancelRequested(true); try { await api.cancelCleaner(); notify(t("Permintaan pembatalan dikirim. File yang sudah dihapus tetap terhapus.")); } catch (error) { setCancelRequested(false); notify(t(String(error)), true); } }
  async function purgeHistory() { setConfirmation(null); setBusy('purge'); try { const count = await api.purgeMonitoringHistory(); setSamples([]); notify(t("{0} sampel monitoring dihapus.", { 0: count })); } catch (error) { notify(t(String(error)), true); } finally { setBusy(''); } }
  return { snapshot, info, sensorInventory, gpuSourceKey, capabilities, plans, profiles, settings, resolvedTheme, samples, running, setRunning, busy, message, setMessage, scan, cleanHistory, persistence, cleanupProgress, cancelRequested, confirmation, setConfirmation, selectedProfile, setSelectedProfile, activePlan, activeSession, selectedMapping, stale, connectionLabel, diskPercent, freeDisk, expiredScan, canClean, refreshDetection, refreshContext, saveSettings, mapProfile, activate, restore, startScan, clean, cancelClean, purgeHistory, notify };
}
export type PulseModel = ReturnType<typeof usePulse>;
