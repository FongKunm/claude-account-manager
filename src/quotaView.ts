import { isDemo, type Account, type AppState, type NormalLogin, type UsageCheck } from './api';
import { bucketOf, fableOf, IS_WINDOWS, normalPlan, planOf, portOf, relTime, type BucketView } from './util';

export const DEFAULT_LOGIN = '@default';
export interface BoardBucket {
  remaining: number | null;
  used: number | null;
  reset: string;
  tone: 'normal' | 'ok' | 'soon' | 'full' | 'unknown';
  at?: string;
  deadline: number;
}
export interface BoardAccount {
  id: string; name: string; email: string; note: string; plan: string;
  method: '账号登录' | 'Setup Token'; proxy: string; ip: string; port: string;
  short: BoardBucket | null; week: BoardBucket; fable: BoardBucket | null;
  status: string; switchable: boolean; verified: boolean; checking: boolean;
  recommended: boolean; reason: string; updated: string; pairName?: string;
}
export interface Preferences {
  view: 'full' | 'compact'; sort: 'best' | 'reset' | 'remaining';
  filter: 'all' | 'switchable' | 'login' | 'token';
}
export interface BoardState extends Preferences {
  current: BoardAccount | null; items: BoardAccount[]; total: number;
  recommended: BoardAccount | null; terminalCount: number | null;
}
function remainingTime(minutes: number) {
  if (minutes < 60) return minutes + ' 分钟';
  const h = Math.floor(minutes / 60);
  if (h < 24) return h + ' 小时' + (minutes % 60 ? ' ' + minutes % 60 + ' 分' : '');
  return Math.floor(h / 24) + ' 天' + (h % 24 ? ' ' + h % 24 + ' 小时' : '');
}
function displayBucket(bucket: BucketView | null): BoardBucket {
  if (!bucket || !Number.isFinite(bucket.pct)) return { remaining: null, used: null, reset: '等待额度读数', tone: 'unknown', deadline: Infinity };
  const ms = bucket.resetsAt ? Date.parse(bucket.resetsAt) : NaN;
  const at = Number.isFinite(ms) ? new Date(ms).toLocaleString('zh-CN', { hour12: false }) : undefined;
  if (bucket.stale) return { remaining: 100, used: 0, reset: '已重置', tone: 'ok', at, deadline: 0 };
  const minutes = Math.ceil((ms - Date.now()) / 60000);
  const used = bucket.full ? 100 : bucket.pct;
  const reset = at ? remainingTime(Math.max(1, minutes)) + '后重置' : used === 0 ? '使用后开始计时' : '重置时间未知';
  return { remaining: 100 - used, used, reset, at, deadline: at ? ms : Infinity, tone: at && minutes <= 30 ? 'soon' : used === 100 ? 'full' : 'normal' };
}
function readTime(u: UsageCheck | null) {
  if (!u) return '等待首次额度读数';
  const source = u.source === 'native-probe' ? '手动查询' : u.source === 'claude-code' ? 'Claude Code 被动更新' : '额度更新';
  return source + ' · ' + relTime(u.checkedAt);
}
function score(a: BoardAccount): number {
  const week = a.week.remaining;
  if (week === null) return -1;
  if (a.short === null) return week;
  return a.short.remaining === null ? -1 : (2 * a.short.remaining + week) / 3;
}
export function buildBoard(s: AppState, prefs: Preferences, busy: Record<string, { usage?: boolean }>, terminalCount: number | null, windows = IS_WINDOWS): BoardState {
  const ordinary = windows ? s.ordinaryAccounts ?? [] : [];
  type Row = { id: string; name: string; email: string; note: string; plan: string; method: BoardAccount['method']; proxy: string; pairName?: string; network?: Account['lastNetworkCheck']; info: Account | NormalLogin };
  const rows: Row[] = s.accounts.map(a => {
    const twin = ordinary.find(o => o.sourceTokenId === a.id);
    return { id: a.id, name: a.name, email: twin?.email ?? a.email ?? (/^[^\s@]+@[^\s@]+$/.test(a.name) ? a.name : a.tokenTail ? 'sk-…' + a.tokenTail : ''), note: a.note, plan: planOf(a)?.text ?? '', method: 'Setup Token', proxy: a.proxy, pairName: twin?.name, network: a.lastNetworkCheck, info: a };
  });
  for (const a of ordinary) rows.push({ id:a.id, name:a.name, email:a.email ?? '', note:a.note ?? '', plan:normalPlan(a)?.text ?? '', method:'账号登录', proxy:a.proxy ?? '', pairName:s.accounts.find(t => t.id === a.sourceTokenId)?.name, network:a.lastNetworkCheck, info:a });
  if (!isDemo || s.normal.email || s.normal.current || s.normal.org) rows.push({ id:DEFAULT_LOGIN, name:'本机账号登录', email:s.normal.email ?? '', note:'', plan:normalPlan(s.normal)?.text ?? '', method:'账号登录', proxy:s.defaultProxy, info:s.normal });
  const all: BoardAccount[] = rows.map(row => {
    const a = row.info;
    const usage = a.lastUsageCheck;
    const short = row.plan === 'Ultra' ? null : displayBucket(bucketOf(a, '5h'));
    const week = displayBucket(bucketOf(a, '7d'));
    const extra = fableOf(a);
    const verified = !!a.expectedIp && row.network?.ok !== false;
    const invalid = usage?.state === 'invalid';
    const exhausted = short?.remaining === 0 || week.remaining === 0;
    const emptyDefault = row.id === DEFAULT_LOGIN && !s.normal.email && !s.normal.org;
    const status = emptyDefault ? '登录待确认' : invalid ? (row.method === 'Setup Token' ? '令牌失效' : '登录失效') : row.network?.ok === false ? '出口不符' : exhausted ? '额度用完' : !verified ? '出口待核对' : usage?.state === 'expired' ? '登录待续期' : week.remaining === null && short?.remaining == null ? '额度未知' : '可切换';
    return { id:row.id, name:row.name, email:row.email, note:row.note, plan:row.plan, method:row.method, ip:a.expectedIp, proxy:row.proxy, port:String(portOf(row.proxy) ?? ''), pairName:row.pairName, short, week, fable:extra ? displayBucket(extra) : null, status, verified, switchable:!invalid && !exhausted && row.network?.ok !== false, recommended:false, reason:'', updated:readTime(usage), checking:!!busy[row.id]?.usage };
  });
  const current = all.find((_,i) => rows[i].info.current) ?? null;
  const options = all.filter(a => a !== current);
  const sameLogin = new Set<string>();
  if (current) for (const login of ordinary) {
    if (login.id === current.id && login.sourceTokenId) sameLogin.add(login.sourceTokenId);
    if (login.sourceTokenId === current.id) sameLogin.add(login.id);
  }
  const recommended = options.filter(a => !sameLogin.has(a.id) && a.switchable && a.verified && score(a) >= 0).sort((a,b) => score(b)-score(a))[0] ?? null;
  if (recommended) {
    recommended.recommended = true;
    const h = recommended.short;
    recommended.reason = (h ? (h.tone === 'ok' ? '5 小时已重置' : '5 小时剩 ' + h.remaining + '%') + '，' : '') + '周额度余 ' + recommended.week.remaining + '%';
  }
  const items = options.filter(a => prefs.filter === 'all' || (prefs.filter === 'switchable' ? a.switchable : a.method === (prefs.filter === 'login' ? '账号登录' : 'Setup Token')));
  const rank = (a: BoardAccount) => a.recommended ? 0 : a.switchable ? 1 : 2;
  const reset = (a: BoardAccount) => Math.min(a.short?.deadline ?? Infinity, a.week.deadline);
  items.sort((a,b) => (prefs.sort === 'best' ? rank(a)-rank(b) : prefs.sort === 'reset' && reset(a)!==reset(b) ? (reset(a)<reset(b)?-1:1) : 0) || score(b)-score(a));
  return { ...prefs, current, items, recommended, total:options.length, terminalCount };
}
