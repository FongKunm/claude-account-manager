import type { Account, NormalLogin, Plan, UsageBucket } from './api';

/** 有额度记录的号：令牌账号或普通登录 */
export type Usable = Pick<Account, 'lastUsageCheck'> & Partial<Pick<Account, 'lastNetworkCheck'>>;

export const PLANS: { value: Plan; label: string }[] = [
  { value: 'pro', label: 'Pro' },
  { value: 'max5', label: 'Max' },
  { value: 'max20', label: 'Ultra' },
  { value: '', label: '不填' },
];

export function planLabel(plan: Plan | undefined): { cls: string; text: string } | null {
  if (plan === 'pro') return { cls: 'pro', text: 'Pro' };
  if (plan === 'max5') return { cls: 'max', text: 'Max' };
  if (plan === 'max20') return { cls: 'max', text: 'Ultra' };
  return null;
}

const FABLE_KEYS = ['7d_fable', '7d_oi'];

/** 查过用量就能分出 Pro / Max：Max 有「7 天 Fable」额度，Pro 没有。 */
export function detectedTier(a: Usable): 'pro' | 'max' | null {
  const u = a.lastUsageCheck;
  if (!u?.ok || !u.buckets?.length) return null;
  return u.buckets.some((b) => FABLE_KEYS.includes(b.key)) ? 'max' : 'pro';
}

/** 普通登录的套餐（账号资料里读的，不用猜）。 */
export function normalPlan(n: NormalLogin): { cls: string; text: string } | null {
  if (n.plan === 'ultra') return { cls: 'max', text: 'Ultra' };
  if (n.plan === 'max') return { cls: 'max', text: 'Max' };
  return planLabel(n.plan);
}

const SUB_STATUS: Record<string, string> = { canceled: '已取消续订', past_due: '扣款失败', unpaid: '未付款', trialing: '试用中' };

/** 订阅状态里值得提醒的那几种；正常续订返回 null。 */
export function subscriptionNote(n: NormalLogin): string | null {
  const s = n.subscriptionStatus?.toLowerCase();
  if (!s || s === 'active') return null;
  return SUB_STATUS[s] ?? `订阅：${s}`;
}

/** 显示用的套餐：查到的档次为准，手填只用来区分 Max 5× / 20×。 */
export function planOf(a: Account): { cls: string; text: string; auto: boolean } | null {
  const selected = planLabel(a.plan);
  if (selected) return { ...selected, auto: false };
  const tier = detectedTier(a);
  if (tier === 'pro') return { cls: 'pro', text: 'Pro', auto: true };
  if (tier === 'max') {
    const manual = a.plan === 'max5' || a.plan === 'max20' ? planLabel(a.plan) : null;
    return { ...(manual ?? { cls: 'max', text: 'Max' }), auto: true };
  }
  return null;
}

export type Tone = 'ok' | 'warn' | 'bad' | 'info' | '';

// ———— 时间 ————

export function relTime(iso: string | null | undefined): string {
  if (!iso) return '从未';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '—';
  const min = Math.floor(Math.max(0, Date.now() - t) / 60_000);
  if (min < 1) return '刚刚';
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} 小时前`;
  return `${Math.floor(hr / 24)} 天前`;
}

export function clock(iso: string | null | undefined): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 距 iso 还有多久：「2小时14分」「3天9小时」「12分」；已过返回空串。 */
export function until(iso: string | null | undefined): string {
  if (!iso) return '';
  const ms = Date.parse(iso) - Date.now();
  if (Number.isNaN(ms) || ms <= 0) return '';
  const min = Math.ceil(ms / 60_000);
  const d = Math.floor(min / 1440);
  const h = Math.floor((min % 1440) / 60);
  const m = min % 60;
  if (d) return `${d}天${h}小时`;
  if (h) return `${h}小时${m}分`;
  return `${m}分`;
}

export function initial(name: string): string {
  return (Array.from(name.trim())[0] ?? '?').toUpperCase();
}

// ———— 额度 ————

const KNOWN: Record<string, string> = {
  '5h': '5 小时', '7d': '7 天', '7d_fable': '7 天 Fable', '7d_oi': '7 天 Fable', '7d_opus': '7 天 Opus', '7d_sonnet': '7 天 Sonnet',
};

export function bucketLabel(key: string): string {
  if (KNOWN[key]) return KNOWN[key];
  const m = /^(5h|7d)_(.+)$/.exec(key);
  return m ? `${KNOWN[m[1]]}·${m[2]}` : key;
}

const ORDER = ['5h', '7d', '7d_fable', '7d_oi', '7d_opus', '7d_sonnet'];
export function sortBuckets(list: UsageBucket[] | undefined): UsageBucket[] {
  const rank = (k: string) => (ORDER.includes(k) ? ORDER.indexOf(k) : 9);
  return [...(list ?? [])].sort((a, b) => rank(a.key) - rank(b.key) || a.key.localeCompare(b.key));
}

export function usageTone(pct: number): 'ok' | 'warn' | 'bad' {
  if (pct >= 90) return 'bad';
  if (pct >= 75) return 'warn';
  return 'ok';
}

export interface BucketView {
  key: string;
  label: string;
  /** 重置时间已过时按已用 0% 展示，保留原重置时间供界面提示。 */
  stale: boolean;
  pct: number;
  resetsAt: string | null;
  full: boolean;
}

export function bucketView(b: UsageBucket): BucketView {
  const stale = !!b.resetsAt && Date.parse(b.resetsAt) <= Date.now();
  // 与 Claude Code /usage 一致：已用百分比向下取整，剩余值再由 100 - pct 得到。
  const pct = stale ? 0 : Math.max(0, Math.min(100, Math.floor(b.utilization)));
  return { key: b.key, label: bucketLabel(b.key), stale, pct, resetsAt: b.resetsAt, full: !stale && (pct >= 100 || b.status === 'rejected') };
}

export function buckets(a: Usable): BucketView[] {
  return sortBuckets(a.lastUsageCheck?.buckets).map(bucketView);
}

export function bucketOf(a: Usable, key: string): BucketView | null {
  return buckets(a).find((b) => b.key === key) ?? null;
}

export function fableOf(a: Usable): BucketView | null {
  return buckets(a).find((b) => FABLE_KEYS.includes(b.key)) ?? null;
}

/** 5 小时或 7 天用满、且还没到重置：返回最晚恢复时间。 */
export function exhaustedUntil(a: Usable): string | null {
  const full = buckets(a).filter((b) => (b.key === '5h' || b.key === '7d') && b.full && b.resetsAt);
  if (!full.length) return null;
  return full.map((b) => b.resetsAt!).reduce((x, y) => (Date.parse(x) > Date.parse(y) ? x : y));
}

// ———— 状态与切换 ————

export type SwitchState = { ok: true } | { ok: false; reason: string; kind: 'fix' | 'wait' };

export function switchState(a: Account): SwitchState {
  const u = a.lastUsageCheck;
  if (u?.state === 'invalid') return { ok: false, reason: '令牌无效', kind: 'fix' };
  if (a.lastNetworkCheck && !a.lastNetworkCheck.ok) return { ok: false, reason: '出口异常', kind: 'fix' };
  const ex = exhaustedUntil(a);
  if (ex) return { ok: false, reason: `${until(ex)}后恢复`, kind: 'wait' };
  return { ok: true };
}

/** 账号的一句状态，用在标签上。 */
export function statusOf(a: Usable): { tone: Tone; label: string } {
  const u = a.lastUsageCheck;
  if (u?.state === 'invalid') return { tone: 'bad', label: '令牌无效' };
  if (u?.state === 'expired') return { tone: 'warn', label: '登录待续期' };
  if (a.lastNetworkCheck && !a.lastNetworkCheck.ok) return { tone: 'bad', label: '出口异常' };
  if (exhaustedUntil(a)) return { tone: 'bad', label: '已用完' };
  if (u && !u.ok) return { tone: 'warn', label: u.state === 'neterr' ? '网络错误' : '查询出错' };
  if (!u) return { tone: '', label: '未查询' };
  return { tone: 'ok', label: '可用' };
}

function load(a: Account) {
  const five = bucketOf(a, '5h');
  const seven = bucketOf(a, '7d');
  const v = (b: BucketView | null) => (!b ? 50 : b.pct);
  return v(five) + 0.5 * v(seven);
}

/** 推荐：除当前号外、能切、用得最少的那个。只看缓存，不发请求。 */
export function recommend(list: Account[]): Account | null {
  const pool = list.filter((a) => !a.current && switchState(a).ok && a.lastUsageCheck?.ok);
  if (!pool.length) return null;
  return pool.reduce((x, y) => (load(y) < load(x) ? y : x));
}

export function switchOrder(list: Account[], rec: Account | null): Account[] {
  const rank = (a: Account) => {
    if (rec && a.id === rec.id) return 0;
    const s = switchState(a);
    if (s.ok) return 1;
    return s.kind === 'wait' ? 2 : 3;
  };
  return list.filter((a) => !a.current).sort((a, b) => rank(a) - rank(b) || load(a) - load(b));
}

// ———— 本机偏好（存不了也不影响使用） ————

const SKIP_TIP = 'cam.skipSwitchTip';
export function skipSwitchTip(): boolean {
  try {
    return localStorage.getItem(SKIP_TIP) === '1';
  } catch {
    return false;
  }
}
export function setSkipSwitchTip(v: boolean) {
  try {
    if (v) localStorage.setItem(SKIP_TIP, '1');
    else localStorage.removeItem(SKIP_TIP);
  } catch {
    /* 忽略 */
  }
}

/** Windows 走新切换（开着的窗口退出后各自恢复）和多普通账号；Mac 仍是旧流程、不显示新增功能 */
export const IS_WINDOWS = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent);

/** 代理地址的端口（http 省略时是 80）；解析不了为 null */
export function portOf(proxy: string): number | null {
  try {
    const u = new URL(proxy.trim());
    if (u.port) return Number(u.port);
    return u.protocol === 'http:' ? 80 : u.protocol === 'https:' ? 443 : null;
  } catch {
    return null;
  }
}

/** 和后端一样只比端口：本机同一个端口就是同一个出口，127.0.0.1 和 localhost 不算两个 */
export function samePort(a: string, b: string): boolean {
  const p = portOf(a);
  return p !== null && p === portOf(b);
}

/** 比 IP：忽略空格和大小写，::ffff: 开头的按 IPv4 算；空的不算相同 */
export function sameIp(a: string, b: string): boolean {
  const norm = (s: string) => s.trim().toLowerCase().replace(/^::ffff:(?=\d+\.\d+\.\d+\.\d+$)/, '');
  const x = norm(a);
  return x !== '' && x === norm(b);
}
