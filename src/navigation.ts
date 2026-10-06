import { Activity, ListTree, BarChart3, Gauge, LayoutDashboard, Monitor, Settings2, Trash2 } from 'lucide-react';
export type Page = 'dashboard' | 'monitor' | 'analytics' | 'tuner' | 'cleaner' | 'system' | 'settings' | 'processes';
export const NAV = [
  { id: 'dashboard', label: 'Ringkasan', icon: LayoutDashboard, group: 'Pantau' },
  { id: 'monitor', label: 'Monitor', icon: Activity, group: 'Pantau' },
  { id: 'analytics', label: 'Analitik', icon: BarChart3, group: 'Pantau' },
  { id: 'tuner', label: 'Profil daya', icon: Gauge, group: 'Kelola' },
  { id: 'cleaner', label: 'Pembersihan', icon: Trash2, group: 'Kelola' },
  { id: 'system', label: 'Perangkat', icon: Monitor, group: 'Kelola' },
  { id: 'processes', label: 'Proses', icon: ListTree, group: 'Kelola' },
  { id: 'settings', label: 'Pengaturan', icon: Settings2, group: 'Aplikasi' },
] as const;
