import type { SVGProps } from 'react';

const icons = {
  mark: <><path d="m12 1 1.2 6.9 4.4-5.3-2.4 6.7 6.5-2.7-4.9 4.8 6.7 1.1-7 1.2 5 4.6-6.6-2.3 2.5 6.1-4.5-4.8-1.2 6.1-1-6.4-4.3 4.8 2.2-6.3-6.4 2.5 4.9-4.7-6.7-1.1 6.9-1.1-5-4.6 6.3 2.5-2.5-6.4 4.7 5.2Z" fill="currentColor" stroke="none"/></>,
  grid: <><rect x="3" y="3" width="7" height="7" rx="1.4"/><rect x="14" y="3" width="7" height="7" rx="1.4"/><rect x="3" y="14" width="7" height="7" rx="1.4"/><rect x="14" y="14" width="7" height="7" rx="1.4"/></>,
  list: <><path d="M8 6h13M8 12h13M8 18h13M3 6h.1M3 12h.1M3 18h.1"/></>,
  terminal: <><rect x="2.5" y="4" width="19" height="16" rx="3"/><path d="m6.5 9 3 3-3 3m5 0h4"/></>,
  activity: <><path d="M2 12h5l3-8 4 16 3-8h5"/></>,
  layers: <><path d="m12 3 10 5.5L12 14 2 8.5 12 3Zm-9 9.5 9 5 9-5m-18 4 9 5 9-5"/></>,
  shield: <><path d="m12 3 8 3v5c0 5-3.3 8.3-8 10-4.7-1.7-8-5-8-10V6l8-3Z"/><path d="m8.2 11.8 2.5 2.5 4.9-5"/></>,
  globe: <><circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/></>,
  clock: <><circle cx="12" cy="12" r="9"/><path d="M12 6v6l4 2"/></>,
  refresh: <><path d="M19.4 8a8 8 0 1 0 .4 7M20 3.8v5.4h-5.4"/></>,
  swap: <><path d="M3 8h16m-4-4 4 4-4 4m6 4H5m4 4-4-4 4-4"/></>,
  arrow: <><path d="M4 12h15m-6-6 6 6-6 6"/></>,
  chevron: <><path d="m9 5 7 7-7 7"/></>,
  chevrons: <><path d="m6 9 6 6 6-6"/></>,
  plus: <><path d="M12 5v14M5 12h14"/></>,
  check: <><path d="m4 13 5 5L20 7"/></>,
  alert: <><path d="m10.5 4.8-8 14A1.4 1.4 0 0 0 3.7 21h16.6a1.4 1.4 0 0 0 1.2-2.2l-8-14a1.7 1.7 0 0 0-3 0Z"/><path d="M12 9v5m0 3h.01"/></>,
  key: <><circle cx="8" cy="9" r="4.5"/><path d="m11.5 12.5 7 7h2.7V17h-2.4v-2.2h-2Z"/></>,
  user: <><circle cx="12" cy="8" r="4"/><path d="M4 21v-2c0-3.9 3.6-6 8-6s8 2.1 8 6v2"/></>,
  close: <><path d="m6 6 12 12M18 6 6 18"/></>,
  settings: <><path d="M4 6h16M4 12h16M4 18h16"/><path d="M9 4v4m7 2v4M8 16v4" strokeWidth="3"/></>,
  link: <><path d="m9 15 6-6m-7 3-2 2a3.8 3.8 0 0 0 5.4 5.4l3-3m2-5 2-2a3.8 3.8 0 0 0-5.4-5.4l-3 3"/></>,
  search: <><circle cx="10" cy="10" r="6.5"/><path d="m15 15 5 5"/></>,
  more: <><circle cx="4" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="20" cy="12" r="1"/></>,
  server: <><rect x="3" y="3" width="18" height="7" rx="2"/><rect x="3" y="14" width="18" height="7" rx="2"/><path d="M7 6.5h.1m0 11h.1"/></>,
  filter: <><path d="M3 5h18l-7.2 8.2V19l-3.6 2v-7.8Z"/></>,
  info: <><circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10h.01"/></>,
  star: <><path d="m12 3 2.6 5.6 6.1.7-4.5 4.2 1.2 6.1L12 16.7l-5.4 2.9 1.2-6.1-4.5-4.2 6.1-.7L12 3Z"/></>,
  gauge: <><path d="M3 17a9 9 0 1 1 18 0M12 14l4-5"/></>,
  external: <><path d="M14 3h7v7m-11 4 11-11M10 6H5a2 2 0 0 0-2 2v11a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2v-5"/></>,
  edit: <><path d="m14 5 5 5M3 21l5-1L21 7a2 2 0 0 0 0-3l-1-1a2 2 0 0 0-3 0L4 16l-1 5Z"/></>,
  trash: <><path d="M3 6h18M9 6V4h6v2M6 6l1 14h10l1-14M10 9v8m4-8v8"/></>,
} as const;
const aliases = { spark: 'mark', x: 'close', dots: 'more', sliders: 'settings', sort: 'settings' } as const;
export function QuotaIcon({ name, size = 15, ...props }: SVGProps<SVGSVGElement> & { name: string; size?: number }) {
  const key = Object.prototype.hasOwnProperty.call(aliases, name) ? aliases[name as keyof typeof aliases] : name;
  return <svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.65} strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0 }} {...props}>{icons[key as keyof typeof icons] ?? icons.info}</svg>;
}
