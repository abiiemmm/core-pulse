import { useSyncExternalStore } from 'react';
import messages from './locales.json';

export type Language = 'id' | 'en' | 'es';
export const LANGUAGES: { value: Language; label: string }[] = [
  { value: 'id', label: 'Bahasa Indonesia' }, { value: 'en', label: 'English' }, { value: 'es', label: 'Español' },
];
let language: Language = 'id';
try { const saved = localStorage.getItem('core-pulse.language'); if (saved === 'en' || saved === 'es') language = saved; } catch { /* Native settings remain the source of truth. */ }
const subscribers = new Set<() => void>();
export function setLanguage(next: Language) {
  document.documentElement.lang = next;
  try { localStorage.setItem('core-pulse.language', next); } catch { /* Continue using native settings. */ }
  if (next === language) return;
  language = next;
  subscribers.forEach(listener => listener());
}
export const getLocale = () => ({ id: 'id-ID', en: 'en-US', es: 'es-ES' })[language];
export const getLanguage = () => language;
export function useLanguage() {
  return useSyncExternalStore(listener => { subscribers.add(listener); return () => { subscribers.delete(listener); }; }, () => language);
}
export function t(source: string, values: Record<string, string | number> = {}) {
  const entry = (messages as Record<string, string[]>)[source];
  const text = language === 'id' ? source : entry?.[language === 'en' ? 0 : 1] ?? source;
  return text.replace(/\{(\w+)\}/g, (match, key: string) => values[key] == null ? match : String(values[key]));
}
