import { LANGUAGES, t, useLanguage } from './i18n';
import { useEffect, useRef, useState } from 'react';
import { Activity, AlertCircle, Check, ChevronRight, Database, Info, Monitor, PanelLeft, Pause, Play, RefreshCw, Search, Settings2, ShieldCheck, Sun, Moon, Trash2, X } from 'lucide-react';
import { isDesktop } from './api';
import { NAV } from './navigation';
import type { Page } from './navigation';
import { sampleAge, usePulse } from './usePulse';
import type { Sensor } from './usePulse';
import { DataRow, Dialog, formatBytes } from './ui';
import Views from './views';

export default function App() {
  const m = usePulse();
  const language = useLanguage();
  const isDark = m.resolvedTheme === 'dark';
  const [page, setPage] = useState<Page>('dashboard');
  const [sensor, setSensor] = useState<Sensor>('cpu');
  const [collapsed, setCollapsed] = useState(false);
  const [commandOpen, setCommandOpen] = useState(false);
  const [commandQuery, setCommandQuery] = useState('');
  const [commandIndex, setCommandIndex] = useState(0);
  const viewport = useRef<HTMLElement>(null);
  useEffect(() => { viewport.current?.scrollTo({ top: 0 }); }, [page]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k' && !m.confirmation) { event.preventDefault(); setCommandOpen(previous => !previous); setCommandQuery(''); setCommandIndex(0); }
      if ((event.ctrlKey || event.metaKey) && !commandOpen && !m.confirmation && /^[1-8]$/.test(event.key)) { event.preventDefault(); setPage(NAV[Number(event.key) - 1].id); }
    };
    window.addEventListener('keydown', onKey); return () => window.removeEventListener('keydown', onKey);
  }, [commandOpen, m.confirmation]);
  const openCommands = () => { setCommandOpen(true); setCommandQuery(''); setCommandIndex(0); };
  const commands = [
    ...NAV.map((item, index) => ({ id: item.id, label: t(item.label), icon: item.icon, shortcut: `Ctrl ${index + 1}`, action: () => setPage(item.id) })),
    { id: 'detect', label: t("Deteksi ulang perangkat"), icon: RefreshCw, shortcut: '', action: () => { if (!m.busy) void m.refreshDetection(); } },
    { id: 'pause', label: m.running ? t("Jeda monitoring") : t("Lanjutkan monitoring"), icon: m.running ? Pause : Play, shortcut: '', action: () => m.setRunning(previous => !previous) },
  ].filter(command => command.label.toLocaleLowerCase().includes(commandQuery.toLocaleLowerCase()));
  const runCommand = (index: number) => { const command = commands[index]; if (command) { command.action(); setCommandOpen(false); } };

  return <div className={`app-shell ${collapsed ? 'sidebar-collapsed' : ''}`}>
    <aside className="sidebar">
      <div className="brand"><div className="brand-symbol"><Activity size={22} strokeWidth={1.7} /></div><span>Core Pulse<small>{t("PERANGKAT LOKAL")}</small></span></div>
      <button className="command-launch" onClick={openCommands} title={t("Buka halaman atau tindakan (Ctrl+K)")}><Search size={15} /><span>{t("Buka halaman…")}</span><kbd>Ctrl K</kbd></button>
      <nav aria-label={t("Navigasi utama")}>{['Pantau', 'Kelola'].map(group => <div className="nav-group" key={group}><div className="nav-group-title">{t(group)}</div>{NAV.filter(item => item.group === group).map(item => <button key={item.id} className={`nav-item ${page === item.id ? 'is-current' : ''}`} aria-current={page === item.id ? 'page' : undefined} title={`${t(item.label)} · Ctrl+${NAV.indexOf(item) + 1}`} onClick={() => setPage(item.id)}><item.icon size={17} strokeWidth={1.7} /><span>{t(item.label)}</span>{page === item.id && <i />}</button>)}</div>)}</nav>
      <div className="sidebar-end"><button className={`nav-item ${page === 'settings' ? 'is-current' : ''}`} aria-current={page === 'settings' ? 'page' : undefined} title={t("Pengaturan · Ctrl+8")} onClick={() => setPage('settings')}><Settings2 size={17} /><span>{t("Pengaturan")}</span></button><div className="device-footer"><Monitor size={17} /><div><strong title={m.info?.device_name}>{m.info?.device_name ?? t("Menghubungkan")}</strong><small>{t("Data tersimpan di perangkat")}</small></div></div></div>
    </aside>
    <div className="workspace">
      <header className="workspace-toolbar"><div className="toolbar-left"><button className="icon-button" aria-label={collapsed ? t("Perluas navigasi") : t("Ringkas navigasi")} aria-expanded={!collapsed} onClick={() => setCollapsed(!collapsed)} title={t("Ringkas navigasi")}><PanelLeft size={17} /></button><span className="toolbar-device">{m.info?.device_name ?? t("Perangkat lokal")}</span><ChevronRight size={13} className="muted" /><span>{t(NAV.find(item => item.id === page)?.label ?? '')}</span></div><div className="toolbar-right"><div className="toolbar-preferences"><button className="theme-switch" aria-label={t('Ganti tema')} title={t('Ganti tema')} disabled={Boolean(m.busy)} onClick={() => void m.saveSettings({ ...m.settings, theme: isDark ? 'light' : 'dark' })}>{isDark ? <Moon size={14} /> : <Sun size={14} />}<span>{t(isDark ? 'Gelap' : 'Terang')}</span></button><select className="language-switch" aria-label={t('Bahasa aplikasi')} title={t('Bahasa aplikasi')} value={language} disabled={Boolean(m.busy)} onChange={event => void m.saveSettings({ ...m.settings, language: event.target.value as typeof language })}>{LANGUAGES.map(item => <option key={item.value} value={item.value}>{item.label}</option>)}</select></div><span className={`connection ${!m.running || m.stale ? 'connection-idle' : ''}`}><i />{m.connectionLabel}</span><button className="icon-button" aria-label={t("Deteksi ulang perangkat")} title={t("Deteksi ulang perangkat")} disabled={Boolean(m.busy)} onClick={() => void m.refreshDetection()}><RefreshCw size={15} className={m.busy === 'detect' ? 'spin' : ''} /></button></div></header>
      {!isDesktop && <div className="preview-notice"><Info size={14} /><span>{t("Pratinjau dengan data simulasi. Tindakan sistem tersedia di aplikasi desktop.")}</span></div>}
      {m.persistence.status === 'degraded' && <div className="preview-notice storage-notice" role="status"><Database size={14} /><span>{t(m.persistence.message ?? "Riwayat gagal disimpan. Monitoring tetap berjalan.")}</span><button className="quiet-button" onClick={() => setPage('settings')}>{t("Pengaturan")}</button></div>}
      <main className="workspace-content" ref={viewport} id="main-content"><div className="page-view"><Views page={page} model={m} sensor={sensor} setSensor={setSensor} navigate={setPage} /></div></main>
      <footer className="workspace-statusbar"><span><ShieldCheck size={12} />{isDesktop ? t("Lokal · tanpa cloud") : t("Data simulasi")}<i />Core Pulse 0.1</span><span>{sampleAge(m.snapshot?.recorded_at)}<i />{t('Interval')} {m.settings.refresh_seconds} {t('dtk')}</span></footer>
    </div>
    {commandOpen && <Dialog title={t("Buka halaman atau tindakan")} className="command-dialog" onClose={() => setCommandOpen(false)}><div className="command-input"><Search size={17} /><input data-autofocus aria-label={t("Cari halaman atau tindakan")} placeholder={t("Ketik nama halaman atau tindakan…")} value={commandQuery} onChange={event => { setCommandQuery(event.target.value); setCommandIndex(0); }} onKeyDown={event => { if (event.key === 'ArrowDown') { event.preventDefault(); setCommandIndex(index => Math.max(0, Math.min(commands.length - 1, index + 1))); } if (event.key === 'ArrowUp') { event.preventDefault(); setCommandIndex(index => Math.max(0, index - 1)); } if (event.key === 'Enter') { event.preventDefault(); runCommand(commandIndex); } }} /><kbd>Esc</kbd></div><div className="command-results">{commands.length ? commands.map((command, index) => <button key={command.id} className={index === commandIndex ? 'is-selected' : ''} onMouseEnter={() => setCommandIndex(index)} onClick={() => runCommand(index)}><command.icon size={17} /><span>{command.label}</span>{command.shortcut && <kbd>{command.shortcut}</kbd>}</button>) : <p className="command-empty">{t("Tidak ada hasil untuk “")}{commandQuery}”.</p>}</div><div className="command-footer"><span>{t("↑ ↓ pilih")}</span><span>{t("↵ buka")}</span></div></Dialog>}
    {m.confirmation === 'clean' && <Dialog title={t("Bersihkan file sementara?")} description={t("File yang dihapus mungkin tidak dapat dipulihkan.")} onClose={() => m.setConfirmation(null)} footer={<><button data-autofocus className="button" onClick={() => m.setConfirmation(null)}>{t("Batal")}</button><button className="button button-primary" disabled={!m.canClean} onClick={() => void m.clean()}><Trash2 size={14} />{t("Bersihkan file")}</button></>}><div className="confirmation-detail"><DataRow label={t("Folder")} value={t("Temp akun saat ini")} /><DataRow label={t("File memenuhi syarat")} value={m.scan?.eligible_count ?? 0} /><DataRow label={t("Perkiraan ruang")} value={formatBytes(m.scan?.estimated_bytes ?? 0)} /></div>{m.expiredScan && <p className="field-help">{t("Hasil pemindaian kedaluwarsa. Tutup dialog dan pindai ulang.")}</p>}</Dialog>}
    {m.confirmation === 'purge' && <Dialog title={t("Hapus riwayat monitoring?")} description={t("Seluruh sampel mentah dan agregat lokal akan dihapus permanen. Pengaturan, riwayat pembersihan, dan sesi pemulihan daya tetap tersimpan.")} onClose={() => m.setConfirmation(null)} footer={<><button data-autofocus className="button" onClick={() => m.setConfirmation(null)}>{t("Batal")}</button><button className="button button-primary" onClick={() => void m.purgeHistory()}><Database size={14} />{t("Hapus riwayat")}</button></>} />}
    {m.confirmation === 'restore' && <Dialog title={t("Timpa skema daya saat ini?")} description={t("Windows atau aplikasi lain telah mengubah skema daya. Pemulihan ini akan menggantinya dengan skema yang tersimpan sebelum sesi tuning.")} onClose={() => m.setConfirmation(null)} footer={<><button data-autofocus className="button" onClick={() => m.setConfirmation(null)}>{t("Batal")}</button><button className="button button-primary" onClick={() => void m.restore(true)}>{t("Pulihkan skema sebelumnya")}</button></>} />}
    {m.message && <div className={`toast ${m.message.error ? 'toast-error' : ''}`} role={m.message.error ? 'alert' : 'status'}>{m.message.error ? <AlertCircle size={17} /> : <Check size={17} />}<span>{t(m.message.text)}</span><button className="icon-button" aria-label={t("Tutup pesan")} onClick={() => m.setMessage(undefined)}><X size={15} /></button></div>}
  </div>;
}
