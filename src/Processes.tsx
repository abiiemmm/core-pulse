import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircle, ChevronLeft, ChevronRight, ListTree, Pause, Play, RefreshCw, Search } from 'lucide-react';
import { api } from './api';
import { t } from './i18n';
import type { ProcessSnapshot } from './types';
import { EmptyState, PageHeader, formatBytes, formatNumber, timeText } from './ui';

const PAGE_SIZE = 50;
type Sort = 'cpu_percent' | 'memory_bytes' | 'name' | 'pid';

export default function Processes() {
  const [snapshot, setSnapshot] = useState<ProcessSnapshot>();
  const [running, setRunning] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<Sort>('memory_bytes');
  const [ascending, setAscending] = useState(false);
  const [page, setPage] = useState(0);
  const consumedRefresh = useRef(0);
  useEffect(() => {
    const manual = consumedRefresh.current !== refresh;
    consumedRefresh.current = refresh;
    if (!running && !manual) { setLoading(false); return; }
    let closed = false, timer: number | undefined;
    async function poll() {
      if (!document.hidden) {
        setLoading(true);
        try { const next = await api.getProcesses(); if (!closed) { setSnapshot(next); setError(false); } }
        catch { if (!closed) setError(true); }
        finally { if (!closed) setLoading(false); }
      }
      if (!closed && running) timer = window.setTimeout(() => void poll(), 2000);
    }
    void poll();
    return () => { closed = true; window.clearTimeout(timer); };
  }, [running, refresh]);
  const rows = useMemo(() => {
    const term = query.trim().toLocaleLowerCase();
    return (snapshot?.processes ?? []).filter(row => !term || `${row.name} ${row.pid} ${row.executable ?? ''}`.toLocaleLowerCase().includes(term)).sort((a, b) => {
      const left = a[sort], right = b[sort];
      if (left == null) return right == null ? a.pid - b.pid : 1;
      if (right == null) return -1;
      const compared = typeof left === 'string' && typeof right === 'string' ? left.localeCompare(right) : Number(left) - Number(right);
      return (ascending ? compared : -compared) || a.pid - b.pid;
    });
  }, [snapshot, query, sort, ascending]);
  const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  const currentPage = Math.min(page, pageCount - 1);
  const visible = rows.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE);
  const order = (key: Sort) => { if (sort === key) setAscending(previous => !previous); else { setSort(key); setAscending(key === 'name' || key === 'pid'); } setPage(0); };
  const column = (key: Sort, label: string) => <th scope="col" aria-sort={sort === key ? ascending ? 'ascending' : 'descending' : 'none'}><button className="process-sort" onClick={() => order(key)}>{label}<span aria-hidden="true">{sort === key ? ascending ? '↑' : '↓' : '↕'}</span></button></th>;
  return <>
    <PageHeader title={t('Proses')} description={t('Lihat penggunaan sumber daya setiap proses yang berjalan.')} actions={<><button className="button" disabled={loading} onClick={() => setRefresh(value => value + 1)}><RefreshCw size={14} className={loading ? 'spin' : ''} />{t('Muat ulang')}</button><button className="button" onClick={() => setRunning(previous => !previous)}>{running ? <Pause size={14} /> : <Play size={14} />}{t(running ? 'Jeda daftar proses' : 'Lanjutkan daftar proses')}</button></>} />
    {error && <div className="inline-note" role="alert"><AlertCircle size={15} /><span>{t('Daftar proses gagal diperbarui. Pembacaan terakhir tetap ditampilkan.')}</span></div>}
    <section className="surface table-surface process-surface">
      <div className="process-toolbar"><label className="process-search"><Search size={15} /><input aria-label={t('Cari proses, PID, atau lokasi')} placeholder={t('Cari proses, PID, atau lokasi')} value={query} onChange={event => { setQuery(event.target.value); setPage(0); }} /></label><span>{snapshot ? t('{0} proses', { 0: snapshot.total_count }) : t('Menghubungkan')}</span></div>
      {visible.length ? <div className="table-scroll"><table className="process-table"><thead><tr>{column('name', t('Nama proses'))}{column('pid', 'PID')}{column('cpu_percent', 'CPU')}{column('memory_bytes', t('Memori'))}</tr></thead><tbody>{visible.map(row => <tr key={`${row.pid}-${row.started_at}`}><td><strong>{row.name}</strong><small title={row.executable ?? undefined}>{row.executable ?? t('Lokasi tidak tersedia')}</small></td><td>{row.pid}</td><td>{row.cpu_percent == null ? '—' : `${formatNumber(row.cpu_percent, 1)}%`}</td><td>{row.memory_bytes == null ? '—' : formatBytes(row.memory_bytes)}</td></tr>)}</tbody></table></div> : <EmptyState icon={ListTree} title={t(snapshot ? 'Tidak ada proses yang cocok' : 'Membaca proses')} description={t(snapshot ? 'Coba nama atau PID lain.' : 'CPU memerlukan dua pembacaan sebelum nilainya tersedia.')} />}
      <div className="process-pagination"><span>{t('{0} hasil', { 0: rows.length })} · {t(running ? 'Diperbarui setiap 2 detik' : 'Dijeda')}{snapshot && ` · ${timeText(snapshot.recorded_at)}`}</span><div><button className="icon-button" aria-label={t('Halaman sebelumnya')} disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}><ChevronLeft size={16} /></button><span>{currentPage + 1} / {pageCount}</span><button className="icon-button" aria-label={t('Halaman berikutnya')} disabled={currentPage + 1 >= pageCount} onClick={() => setPage(currentPage + 1)}><ChevronRight size={16} /></button></div></div>
    </section>
    <div className="inline-note"><ListTree size={15} /><span>{t('CPU dihitung sebagai bagian dari seluruh prosesor. Tanda — berarti menunggu sampel atau akses pembacaan terbatas.')}</span></div>
    {snapshot?.truncated && <div className="inline-note"><AlertCircle size={15} /><span>{t('Daftar dibatasi pada 2.048 proses dengan penggunaan memori tertinggi.')}</span></div>}
  </>;
}
