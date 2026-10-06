import { useEffect, useRef, useState } from 'react';
import { AlertCircle, Check, FolderOpen, Gamepad2, Pencil, Plus, RefreshCw, ShieldCheck, Trash2, Zap } from 'lucide-react';
import { api, isDesktop } from './api';
import { t } from './i18n';
import type { Page } from './navigation';
import type { AutoBoostStatus, GameSelection, GameSettings, GameSnapshot, GameStatus, GamingSession, PerformanceProfile, PowerPlan, RegisteredGame } from './types';
import { dateText, Dialog, EmptyState, formatNumber, PageHeader, SectionHeader, timeText, Toggle } from './ui';

const STATUS: Record<GameStatus['status'], string> = { ready: 'Siap dipantau', running: 'Sedang berjalan', unavailable: 'Executable perlu ditinjau', unverified: 'Proses belum terverifikasi' };
const MODES: Record<AutoBoostStatus['mode'], string> = { idle: 'Menunggu game', active: 'Profil otomatis aktif', waiting_for_ac: 'Menunggu daya AC', unmapped: 'Profil perlu dipetakan', recovery_required: 'Pemulihan perlu ditinjau', suspended: 'Menunggu sesi game baru', retained: 'Profil menunggu pemulihan manual', error: 'Auto Boost perlu ditinjau' };
const REASONS: Record<string, string> = {
  manual_restore: 'Profil telah dipulihkan manual. Auto Boost menunggu semua game pada sesi ini selesai.',
  ac_lost: 'Daya AC tidak tersedia. Skema sebelumnya dipulihkan; Auto Boost menunggu sesi game baru.',
  external_change: 'Skema daya berubah di luar aplikasi. Tinjau sesi pemulihan di halaman Profil.',
  activation_failed: 'Aktivasi otomatis gagal. Tinjau profil dan mulai sesi game baru untuk mencoba lagi.',
  cycle_finished: 'Profil otomatis dipulihkan. Sesi ini tidak akan diaktifkan kembali sebelum game selesai.',
  unfinished_tuning_session: 'Tinjau sesi daya sebelumnya sebelum Auto Boost dapat digunakan.',
  manual_restore_required: 'Skema tetap diterapkan sesuai pengaturan. Pulihkan secara manual di halaman Profil.',
  ac_required: 'Game atau profil ini memerlukan daya AC sebelum aktivasi.',
  no_eligible_profile: 'Petakan profil ke skema yang tersedia dan gunakan executable yang terverifikasi.',
  recovery_required: 'Pemulihan otomatis belum selesai. Tinjau sesi daya dan coba pemulihan manual.',
};
const SESSION_STATUS: Record<GamingSession['status'], string> = { active: 'Sedang dicatat', completed: 'Selesai', removed: 'Pendaftaran dihapus', interrupted: 'Terputus', app_closed: 'Aplikasi ditutup' };
type Editor = { selection?: GameSelection; game?: RegisteredGame; settings: GameSettings };
const settingsOf = (game: RegisteredGame): GameSettings => ({ display_name: game.display_name, profile_id: game.profile_id, restore_on_exit: game.restore_on_exit, ac_only: game.ac_only });
const displayPath = (path: string) => path.startsWith(String.fromCharCode(92, 92, 63, 92)) ? path.slice(4) : path;
function duration(session: GamingSession) {
  if (!session.ended_at && session.status !== 'active') return '—';
  const seconds = ((session.ended_at ? Date.parse(session.ended_at) : Date.now()) - Date.parse(session.started_at)) / 1000;
  return !Number.isFinite(seconds) || seconds < 0 ? '—' : seconds < 60 ? t('{0} dtk', { 0: Math.floor(seconds) }) : t('{0} mnt', { 0: formatNumber(seconds / 60, 1) });
}

export default function Gaming({ profiles, plans, navigate }: { profiles: PerformanceProfile[]; plans: PowerPlan[]; navigate: (page: Page) => void }) {
  const [snapshot, setSnapshot] = useState<GameSnapshot>();
  const [automatic, setAutomatic] = useState<AutoBoostStatus>();
  const [history, setHistory] = useState<GamingSession[]>([]);
  const [editor, setEditor] = useState<Editor>();
  const [remove, setRemove] = useState<RegisteredGame>();
  const [enable, setEnable] = useState<RegisteredGame>();
  const [deleteSession, setDeleteSession] = useState<GamingSession>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [refresh, setRefresh] = useState(0);
  const generation = useRef(0), mounted = useRef(true);
  const message = useRef<HTMLDivElement>(null);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    let closed = false, timer: number | undefined;
    async function poll() {
      if (document.hidden) { timer = window.setTimeout(poll, 2000); return; }
      const version = generation.current;
      const results = await Promise.allSettled([api.getGameStatus(), api.getAutoBoost(), api.getGamingSessions()]);
      if (!closed && version === generation.current) {
        if (results[0].status === 'fulfilled') { const next = results[0].value; setSnapshot(previous => previous && Date.parse(previous.recorded_at) > Date.parse(next.recorded_at) ? previous : next); }
        if (results[1].status === 'fulfilled') setAutomatic(results[1].value);
        if (results[2].status === 'fulfilled') setHistory(results[2].value);
        const failed = results.find(result => result.status === 'rejected');
        if (failed?.status === 'rejected') setError(String(failed.reason));
      }
      if (!closed) timer = window.setTimeout(poll, 2000);
    }
    void poll();
    return () => { closed = true; clearTimeout(timer); };
  }, [refresh]);
  async function task(action: () => Promise<void>) {
    if (busy) return;
    generation.current++; setBusy(true); setError(undefined);
    try { await action(); } catch (failure) { if (mounted.current) { setError(String(failure)); window.setTimeout(() => message.current?.focus(), 0); } }
    finally { if (mounted.current) { setBusy(false); setRefresh(previous => previous + 1); } }
  }
  const pick = () => task(async () => {
    const selected = await api.pickGame();
    if (selected && mounted.current) setEditor({ selection: selected, settings: { display_name: selected.suggested_name, profile_id: 'gaming', restore_on_exit: true, ac_only: true } });
  });
  const save = () => task(async () => {
    if (!editor) return;
    if (editor.selection) await api.registerGame(editor.selection.selection_id, editor.settings);
    else if (editor.game) await api.updateGame(editor.game.id, editor.settings);
    if (mounted.current) setEditor(undefined);
  });
  const games = snapshot?.games ?? [], running = games.filter(row => row.status === 'running');
  const setField = <K extends keyof GameSettings>(key: K, value: GameSettings[K]) => setEditor(previous => previous && { ...previous, settings: { ...previous.settings, [key]: value } });
  const closeEditor = () => { if (!busy) setEditor(undefined); };
  const validName = Boolean(editor?.settings.display_name.trim()) && (editor?.settings.display_name.length ?? 0) <= 100;
  const eligible = (game: RegisteredGame) => plans.some(plan => plan.guid === profiles.find(profile => profile.id === game.profile_id)?.scheme_guid);
  const explanation = automatic?.reason ? REASONS[automatic.reason] : automatic?.mode === 'active' ? 'Profil pertama tetap digunakan sampai game terakhir yang memenuhi syarat selesai.' : 'Aktifkan hanya untuk game yang Anda pilih. Pemantauan berjalan saat halaman lain dibuka atau monitoring dijeda.';

  return <>
    <PageHeader title="Gaming" description="Game terverifikasi, profil otomatis, dan riwayat sesi lokal." actions={<><button className="button" disabled={busy} aria-label={t('Perbarui status game')} onClick={() => { generation.current++; setRefresh(previous => previous + 1); }}><RefreshCw size={14} /></button><button className="button button-primary" disabled={!isDesktop || busy || games.length >= 100} onClick={() => void pick()}><Plus size={14} />{t('Daftarkan game')}</button></>} />
    <section className="gaming-overview surface"><div className="gaming-overview-title"><Gamepad2 size={24} strokeWidth={1.4} /><div><strong>{t('Daftar game lokal')}</strong><span>{t('{0} game terdaftar · {1} sedang berjalan', { 0: games.length, 1: running.length })}</span></div></div><div className="gaming-local"><ShieldCheck size={15} /><span>{t('Pendaftaran game tidak mengubah skema daya.')}</span></div></section>
    {error && <div className="gaming-error" role="alert" tabIndex={-1} ref={message}><AlertCircle size={16} /><span>{t(error)}</span></div>}
    <section className="surface game-boost-panel" aria-label={t('Status Auto Boost')}><div className="game-boost-heading"><Zap size={18} /><div><span className="eyebrow">Auto Boost</span><h2>{automatic ? t(MODES[automatic.mode]) : t('Membaca status otomatis')}</h2></div>{automatic?.profile_id && <span className="game-boost-profile">{t(profiles.find(profile => profile.id === automatic.profile_id)?.name ?? automatic.profile_id)}</span>}</div><p>{t(explanation ?? 'Pemulihan perlu ditinjau')}</p>{automatic?.error && <p role="alert" className="game-warning">{t(automatic.error)}</p>}{automatic && automatic.discovery_status !== 'ready' && <p className="game-warning" role="status">{t('Pemeriksaan proses tertunda. Perlindungan AC dan pemulihan manual tetap berjalan.')}</p>}{automatic?.discovery_message && <p className="game-warning">{t(automatic.discovery_message)}</p>}{(automatic?.tuning_session_id || automatic?.mode === 'recovery_required') && <button className="quiet-button" onClick={() => navigate('tuner')}>{t('Tinjau sesi daya')}<FolderOpen size={14} /></button>}</section>
    <section className="surface gaming-library"><SectionHeader title="Executable terdaftar" description="Pencocokan memakai lokasi, identitas file, dan proses yang terverifikasi." action={<span className="muted gaming-updated">{t('Diperbarui')} {timeText(snapshot?.recorded_at)}</span>} />
      {!snapshot ? <EmptyState icon={Gamepad2} title="Memuat daftar game" description="Membaca pendaftaran game dari perangkat ini." /> : !games.length ? <EmptyState icon={Gamepad2} title="Belum ada game terdaftar" description="Pilih executable game dari drive lokal. Tidak ada game yang diluncurkan atau ditutup oleh aplikasi." /> : <div className="game-list">{games.map(({ game, status, message, process_count }) => <article className="game-row" key={game.id}>
        <div className="game-icon"><Gamepad2 size={20} /></div><div className="game-details"><div className="game-name"><h3>{game.display_name}</h3><span className={`game-status ${status === 'running' ? 'game-status-running' : ''}`}>{status === 'running' ? <Check size={12} /> : status === 'unavailable' || status === 'unverified' ? <AlertCircle size={12} /> : <ShieldCheck size={12} />}{t(STATUS[status])}</span></div><p className="game-path" title={game.canonical_executable_path}>{displayPath(game.canonical_executable_path)}</p><div className="game-meta"><span>{t('Profil pilihan')}: {t(profiles.find(profile => profile.id === game.profile_id)?.name ?? game.profile_id)}</span><span>{t(game.auto_boost ? 'Auto Boost aktif' : 'Auto Boost nonaktif')}</span>{process_count > 0 && <span>{t('{0} proses terverifikasi', { 0: process_count })}</span>}</div><div className="game-automation"><span>{t('Auto Boost')}</span><Toggle label={t('Auto Boost untuk {0}', {0: game.display_name})} checked={game.auto_boost} disabled={!isDesktop || busy || (!game.auto_boost && (!eligible(game) || status === 'unavailable'))} onChange={checked => { setError(undefined); if (checked) setEnable(game); else void task(async () => { await api.setAutoBoost(game.id, false, settingsOf(game)); }); }} />{!eligible(game) && !game.auto_boost && <span className="muted">{t('Petakan profil daya terlebih dahulu.')}</span>}</div>{message && <p className="game-warning">{t(message)}</p>}</div>
        <div className="game-actions"><button className="icon-button" disabled={busy} aria-label={t('Edit {0}', { 0: game.display_name })} title={t('Edit game')} onClick={() => { setError(undefined); setEditor({ game, settings: settingsOf(game) }); }}><Pencil size={15} /></button><button className="icon-button" disabled={busy} aria-label={t('Hapus {0} dari daftar', { 0: game.display_name })} title={t('Hapus dari daftar')} onClick={() => { setError(undefined); setRemove(game); }}><Trash2 size={15} /></button></div>
      </article>)}</div>}
    </section>
    <div className="gaming-footnote"><span>{t('Executable dengan nama yang sama di lokasi lain tidak dianggap sebagai game terdaftar.')}</span><button className="quiet-button" onClick={() => navigate('tuner')}>{t('Kelola profil daya')}<FolderOpen size={14} /></button></div>
    <section className="surface game-history"><SectionHeader title="Riwayat sesi game" description="50 sesi terbaru. Ringkasan tersimpan lokal sampai Anda menghapusnya." />{!history.length ? <EmptyState icon={Gamepad2} title="Belum ada sesi game" description="Sesi dicatat ketika executable terdaftar benar-benar terverifikasi sedang berjalan." /> : <div className="game-history-scroll"><table><thead><tr><th>{t('Game')}</th><th>{t('Mulai')}</th><th>{t('Durasi')}</th><th>{t('Profil daya')}</th><th>{t('Status')}</th><th><span className="sr-only">{t('Tindakan')}</span></th></tr></thead><tbody>{history.map(session => <tr key={session.id}><td>{session.game_name}</td><td>{dateText(session.started_at)}</td><td>{duration(session)}</td><td>{session.profile_name ? t(session.profile_name) : t('Tanpa Auto Boost')}</td><td>{t(SESSION_STATUS[session.status] ?? 'Status tidak dikenal')}</td><td><button className="icon-button" disabled={!isDesktop || busy || session.status === 'active'} aria-label={t('Hapus sesi {0}', {0: session.game_name})} onClick={() => { setError(undefined); setDeleteSession(session); }}><Trash2 size={14} /></button></td></tr>)}</tbody></table></div>}</section>
    {editor && <Dialog title={editor.selection ? t('Daftarkan game') : t('Edit game')} description="Atur nama, profil, dan aturan pemulihan game. Pendaftaran baru selalu menonaktifkan Auto Boost." onClose={closeEditor} footer={<><button className="button" disabled={busy} onClick={closeEditor}>{t('Batal')}</button><button className="button button-primary" disabled={busy || !validName || (editor.selection && Date.parse(editor.selection.expires_at) < Date.now())} onClick={() => void save()}>{t(busy ? 'Menyimpan…' : 'Simpan game')}</button></>}><div className="game-editor"><label htmlFor="game-name">{t('Nama game')}</label><input id="game-name" data-autofocus autoComplete="off" maxLength={100} disabled={busy} value={editor.settings.display_name} onChange={event => setField('display_name', event.target.value)} /><label htmlFor="game-profile">{t('Profil pilihan')}</label><select id="game-profile" disabled={busy || editor.game?.auto_boost} value={editor.settings.profile_id} onChange={event => setField('profile_id', event.target.value)}>{profiles.map(profile => <option key={profile.id} value={profile.id}>{t(profile.name)}</option>)}</select><div className="setting-row"><div><strong>{t('Hanya saat daya AC')}</strong><p>{t('Game ini memerlukan listrik sebelum Auto Boost dapat diterapkan.')}</p></div><Toggle label={t('Hanya saat daya AC')} checked={editor.settings.ac_only} disabled={busy || editor.game?.auto_boost} onChange={checked => setField('ac_only', checked)} /></div><div className="setting-row"><div><strong>{t('Pulihkan setelah game selesai')}</strong><p>{t('Jika nonaktif, skema tetap diterapkan sampai Anda memulihkannya manual.')}</p></div><Toggle label={t('Pulihkan setelah game selesai')} checked={editor.settings.restore_on_exit} disabled={busy || editor.game?.auto_boost} onChange={checked => setField('restore_on_exit', checked)} /></div>{editor.game?.auto_boost && <p className="field-help">{t('Nonaktifkan Auto Boost sebelum mengubah profil atau aturan pemulihan.')}</p>}<p className="game-selected-path">{editor.selection?.canonical_executable_path ?? editor.game?.canonical_executable_path}</p><p className="field-help">{t('File yang berubah atau tidak dapat diverifikasi perlu dipilih ulang.')}</p></div>{error && <p className="field-help" role="alert">{t(error)}</p>}</Dialog>}
    {enable && <Dialog title="Aktifkan Auto Boost" description="Saat game terverifikasi berjalan, Core Pulse dapat menerapkan skema daya yang dipetakan. Game yang sudah berjalan juga dapat memulai aktivasi." onClose={() => { if (!busy) setEnable(undefined); }} footer={<><button data-autofocus className="button" disabled={busy} onClick={() => setEnable(undefined)}>{t('Batal')}</button><button className="button button-primary" disabled={busy || !eligible(enable)} onClick={() => void task(async () => { await api.setAutoBoost(enable.id, true, settingsOf(enable)); if (mounted.current) setEnable(undefined); })}>{t('Aktifkan Auto Boost')}</button></>}><div className="game-enable-summary"><strong>{enable.display_name}</strong><p>{t('Profil pilihan')}: {t(profiles.find(profile => profile.id === enable.profile_id)?.name ?? enable.profile_id)}</p><p>{t('Skema daya Windows')}: {plans.find(plan => plan.guid === profiles.find(profile => profile.id === enable.profile_id)?.scheme_guid)?.name ?? '—'}</p><p>{t('Profil pertama tetap digunakan sampai game terakhir yang memenuhi syarat selesai.')}</p><p>{t(enable.restore_on_exit ? 'Skema sebelumnya dipulihkan setelah sesi selesai atau aplikasi ditutup.' : 'Pemulihan otomatis nonaktif. Skema dapat tetap diterapkan setelah game atau aplikasi ditutup.')}</p><p>{t('Kehilangan daya AC tetap memulihkan profil yang dibatasi ke AC. Perubahan skema dari luar tidak ditimpa.')}</p></div>{error && <p className="field-help" role="alert">{t(error)}</p>}</Dialog>}
    {remove && <Dialog title="Hapus game dari daftar?" description="Executable tidak dihapus dan game tidak dihentikan. Riwayat sesi tetap tersimpan." onClose={() => { if (!busy) setRemove(undefined); }} footer={<><button data-autofocus className="button" disabled={busy} onClick={() => setRemove(undefined)}>{t('Batal')}</button><button className="button button-primary" disabled={busy} onClick={() => void task(async () => { await api.removeGame(remove.id); if (mounted.current) setRemove(undefined); })}>{t('Hapus dari daftar')}</button></>}><p className="game-remove-name">{remove.display_name}</p>{error && <p className="field-help" role="alert">{t(error)}</p>}</Dialog>}
    {deleteSession && <Dialog title="Hapus riwayat sesi" description="Ringkasan dan metrik sesi ini dihapus. Data monitoring dan catatan pemulihan daya tetap tersimpan." onClose={() => { if (!busy) setDeleteSession(undefined); }} footer={<><button data-autofocus className="button" disabled={busy} onClick={() => setDeleteSession(undefined)}>{t('Batal')}</button><button className="button button-primary" disabled={busy} onClick={() => void task(async () => { await api.deleteGamingSession(deleteSession.id); if (mounted.current) setDeleteSession(undefined); })}>{t('Hapus riwayat sesi')}</button></>}><p className="game-remove-name">{deleteSession.game_name} · {dateText(deleteSession.started_at)}</p>{error && <p className="field-help" role="alert">{t(error)}</p>}</Dialog>}
  </>;
}
