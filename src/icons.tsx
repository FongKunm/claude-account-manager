// 线性图标，统一 24 网格、描边继承文字颜色。
const I = (d: string) => () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><path d={d} /></svg>
);

export const IconLayers = I('M12 2 3 7l9 5 9-5zM3 12l9 5 9-5M3 16.5l9 5 9-5');
export const IconPlus = I('M12 5v14M5 12h14');
export const IconSwap = I('M7 4 3 8l4 4M3 8h14M17 20l4-4-4-4M21 16H7');
export const IconUser = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="8" r="4" /><path d="M4 21a8 8 0 0 1 16 0" /></svg>
);
export const IconKey = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="7.5" cy="15.5" r="4.5" /><path d="m10.7 12.3 9.8-9.8M17 6l3 3M14.5 8.5l2 2" /></svg>
);
export const IconEye = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z" /><circle cx="12" cy="12" r="3" /></svg>
);
export const IconEyeOff = I('M3 3l18 18M10.6 10.6a2 2 0 0 0 2.8 2.8M9.9 5.1A10 10 0 0 1 12 5c6.4 0 10 7 10 7a17 17 0 0 1-3.2 3.9M6.6 6.6C3.9 8.4 2 12 2 12s3.6 7 10 7a9.7 9.7 0 0 0 5.4-1.6');
export const IconAlert = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3 2 20h20L12 3z" /><path d="M12 10v4M12 17v.1" /></svg>
);
export const IconEdit =I('M12 20h9M16.5 3.5a2.1 2.1 0 1 1 3 3L7 19l-4 1 1-4z');
export const IconGlobe = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></svg>
);
export const IconLock = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><rect x="4" y="11" width="16" height="10" rx="2" /><path d="M8 11V7a4 4 0 0 1 8 0v4" /></svg>
);
export const IconRefresh = I('M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7');
export const IconMore = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="5" cy="12" r="1.1" /><circle cx="12" cy="12" r="1.1" /><circle cx="19" cy="12" r="1.1" /></svg>
);
export const IconClose = I('M6 6l12 12M18 6 6 18');
export const IconPlay = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M7 4.5v15l12-7.5z" /></svg>
);
export const IconHelp = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M9.5 9.5a2.5 2.5 0 1 1 3.3 2.4c-.5.2-.8.6-.8 1.1V14M12 17.2v.1" /></svg>
);
export const IconDatabase = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><ellipse cx="12" cy="6" rx="8" ry="3" /><path d="M4 6v6c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6" /></svg>
);
export const IconCopy = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><rect x="9" y="9" width="11" height="11" rx="2" /><path d="M5 15V6a2 2 0 0 1 2-2h9" /></svg>
);
export const IconUserPlus = I('M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8M19 8v6M22 11h-6');
export const IconLogin = I('M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M15 12H3');
export const IconMin = I('M5 12h14');
export const IconChevron = I('M9 6l6 6-6 6');
export const IconMax = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="5" width="14" height="14" rx="1" /></svg>
);
export const IconRestore = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="8" width="11" height="11" rx="1" /><path d="M8 8V6a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-2" /></svg>
);
export const IconSpark = () => (
  <svg className="legacy-icon" viewBox="0 0 24 24" aria-hidden="true">
    <path d="M12 2.2l1.6 6.1 5.8-2.5-3.9 4.9 5.6 2.9-6.3.3 1 6.2L12 15.3l-3.8 4.8 1-6.2-6.3-.3 5.6-2.9-3.9-4.9 5.8 2.5z" />
  </svg>
);
