import { useEffect, useRef, useState } from 'react';
import { AlertCircle, Check, FolderOpen, Gamepad2, Pencil, Plus, RefreshCw, ShieldCheck, Trash2 } from 'lucide-react';
import { api, isDesktop } from './api';
import { t } from './i18n';
import type { Page } from './navigation';
import type { GameSelection, GameSettings, GameSnapshot, GameStatus, PerformanceProfile, RegisteredGame } from './types';
import { Dialog, EmptyState, PageHeader, SectionHeader, timeText } from './ui';

const STATUS: Record<GameStatus['status'], string> = { ready: 'Siap dipantau', running: 'Sedang berjalan', unavailable: 'Executable perlu ditinjau', unverified: 'Proses belum terverifikasi' };
type Editor = { selection?: GameSelection; game?: RegisteredGame; settings: GameSettings };

export default function Gaming({ profiles, navigate }: { profiles: PerformanceProfile[]; navigate: (page: Page) => void }) {
  const [snapshot, setSnapshot] = useState<GameSnapshot>();
  const [editor, setEditor] = useState<Editor>();
  const [remove, setRemove] = useState<RegisteredGame>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [refresh, setRefresh] = useState(0);
  const generation = useRef(0);
  const mounted = useRef(true);
  const message = useRef<HTMLDivElement>(null);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    let closed = false, timer: number | undefined;
    async function poll() {
      if (document.hidden) { timer = window.setTimeout(poll, 2000); return; }
      const version = generation.current;
      try {
        const next = await api.getGameStatus();
        if (!closed && version === generation.current) setSnapshot(previous => previous && Date.parse(previous.recorded_at) > Date.parse(next.recorded_at) ? previous : next);
      } catch (failure) { if (!closed && version === generation.current) setError(String(failure)); }
      if (!closed) timer = window.setTimeout(poll, 2000);
    }
    void poll();
    return () => { closed = true; clearTimeout(timer); };
  }, [refresh]);
  async function task(action: () => Promise<void>) {
    if (busy) return;
    generation.current++;
    setBusy(true); setError(undefined);
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
  const games = snapshot?.games ?? [];
  const running = games.filter(row => row.status === 'running');
  const setField = <K extends keyof GameSettings>(key: K, value: GameSettings[K]) => setEditor(previous => previous && { ...previous, settings: { ...previous.settings, [key]: value } });
  const closeEditor = () => { if (!busy) setEditor(undefined); };
  const validName = Boolean(editor?.settings.display_name.trim()) && (editor?.settings.display_name.length ?? 0) <= 100;

  return <>
    <PageHeader title="Gaming" description="Daftarkan executable dan lihat game yang sedang berjalan." actions={<><button className="button" disabled={busy} aria-label={t('Perbarui status game')} onClick={() => { generation.current++; setRefresh(previous => previous + 1); }}><RefreshCw size={14} /></button><button className="button button-primary" disabled={!isDesktop || busy || games.length >= 100} onClick={() => void pick()}><Plus size={14} />{t('Daftarkan game')}</button></>} />
    <section className="gaming-overview surface"><div className="gaming-overview-title"><Gamepad2 size={24} strokeWidth={1.4} /><div><strong>{t('Daftar game lokal')}</strong><span>{t('{0} game terdaftar · {1} sedang berjalan', { 0: games.length, 1: running.length })}</span></div></div><div className="gaming-local"><ShieldCheck size={15} /><span>{t('Pendaftaran game tidak mengubah skema daya.')}</span></div></section>
    {error && <div className="gaming-error" role="alert" tabIndex={-1} ref={message}><AlertCircle size={16} /><span>{t(error)}</span></div>}
    {snapshot?.status === 'degraded' && <p className="field-help" role="status">{t('Daftar proses belum lengkap. Status game yang belum terverifikasi tetap ditandai.')}</p>}
    <section className="surface gaming-library"><SectionHeader title="Executable terdaftar" description="Pencocokan memakai lokasi, identitas file, dan proses yang terverifikasi." action={<span className="muted gaming-updated">{t('Diperbarui')} {timeText(snapshot?.recorded_at)}</span>} />
      {!snapshot ? <EmptyState icon={Gamepad2} title="Memuat daftar game" description="Membaca pendaftaran game dari perangkat ini." /> : !games.length ? <EmptyState icon={Gamepad2} title="Belum ada game terdaftar" description="Pilih executable game dari drive lokal. Tidak ada game yang diluncurkan atau ditutup oleh aplikasi." /> : <div className="game-list">{games.map(({ game, status, message, process_count }) => <article className="game-row" key={game.id}>
        <div className="game-icon"><Gamepad2 size={20} /></div><div className="game-details"><div className="game-name"><h3>{game.display_name}</h3><span className={`game-status ${status === 'running' ? 'game-status-running' : ''}`}>{status === 'running' ? <Check size={12} /> : status === 'unavailable' || status === 'unverified' ? <AlertCircle size={12} /> : <ShieldCheck size={12} />}{t(STATUS[status])}</span></div><p className="game-path" title={game.canonical_executable_path}>{game.canonical_executable_path.replace(/^\\\\\?\\/, '')}</p><div className="game-meta"><span>{t('Profil pilihan')}: {t(profiles.find(profile => profile.id === game.profile_id)?.name ?? game.profile_id)}</span><span>{t('Auto Boost nonaktif')}</span>{process_count > 0 && <span>{t('{0} proses terverifikasi', { 0: process_count })}</span>}</div>{message && <p className="game-warning">{t(message)}</p>}</div>
        <div className="game-actions"><button className="icon-button" disabled={busy} aria-label={t('Edit {0}', { 0: game.display_name })} title={t('Edit game')} onClick={() => { setError(undefined); setEditor({ game, settings: { display_name: game.display_name, profile_id: game.profile_id, restore_on_exit: game.restore_on_exit, ac_only: game.ac_only } }); }}><Pencil size={15} /></button><button className="icon-button" disabled={busy} aria-label={t('Hapus {0} dari daftar', { 0: game.display_name })} title={t('Hapus dari daftar')} onClick={() => { setError(undefined); setRemove(game); }}><Trash2 size={15} /></button></div>
      </article>)}</div>}
    </section>
    <div className="gaming-footnote"><span>{t('Executable dengan nama yang sama di lokasi lain tidak dianggap sebagai game terdaftar.')}</span><button className="quiet-button" onClick={() => navigate('tuner')}>{t('Kelola profil daya')}<FolderOpen size={14} /></button></div>
    {editor && <Dialog title={editor.selection ? t('Daftarkan game') : t('Edit game')} description="Pendaftaran ini tersimpan lokal. Auto Boost tidak diaktifkan." onClose={closeEditor} footer={<><button className="button" disabled={busy} onClick={closeEditor}>{t('Batal')}</button><button className="button button-primary" disabled={busy || !validName || (editor.selection && Date.parse(editor.selection.expires_at) < Date.now())} onClick={() => void save()}>{t(busy ? 'Menyimpan…' : 'Simpan game')}</button></>}><div className="game-editor"><label htmlFor="game-name">{t('Nama game')}</label><input id="game-name" data-autofocus autoComplete="off" maxLength={100} disabled={busy} value={editor.settings.display_name} onChange={event => setField('display_name', event.target.value)} /><label htmlFor="game-profile">{t('Profil pilihan')}</label><select id="game-profile" disabled={busy} value={editor.settings.profile_id} onChange={event => setField('profile_id', event.target.value)}>{profiles.map(profile => <option key={profile.id} value={profile.id}>{t(profile.name)}</option>)}</select><p className="game-selected-path">{editor.selection?.canonical_executable_path ?? editor.game?.canonical_executable_path}</p><p className="field-help">{t('File yang berubah atau tidak dapat diverifikasi perlu dipilih ulang.')}</p></div>{error && <p className="field-help" role="alert">{t(error)}</p>}</Dialog>}
    {remove && <Dialog title="Hapus game dari daftar?" description="Executable tidak dihapus dan game tidak dihentikan. Riwayat sesi tetap tersimpan." onClose={() => { if (!busy) setRemove(undefined); }} footer={<><button data-autofocus className="button" disabled={busy} onClick={() => setRemove(undefined)}>{t('Batal')}</button><button className="button button-primary" disabled={busy} onClick={() => void task(async () => { await api.removeGame(remove.id); if (mounted.current) setRemove(undefined); })}>{t('Hapus dari daftar')}</button></>}><p className="game-remove-name">{remove.display_name}</p>{error && <p className="field-help" role="alert">{t(error)}</p>}</Dialog>}
  </>;
}
